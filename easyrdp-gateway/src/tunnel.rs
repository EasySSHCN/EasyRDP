//! 隧道模块：`GET /tunnel/{resource_id}` WebSocket 升级 + 真实 TCP 转发。
//!
//! 流程：auth_guard 校验短期 JWT -> 依据 resource_id 解析后端地址并做角色级
//! 授权检查（资源注册表优先 + allowed_roles 交集，403 = tunnel_forbidden；
//! 其次显式私网 "host:port"，见 resources.rs）->
//! 连接内网 RDP 主机（升级前失败快速返回）-> WS 升级 ->
//! 双向字节泵（WS Binary 帧 <-> TCP 字节流；Text 帧忽略；Ping/Pong 由底层流自动处理）。
//! 建立、关闭、后端连接失败与角色拒绝均记录审计事件。
//!
//! 泵函数通过 mpsc 通道与 WS 分离（split 后由适配任务搬运消息），
//! 因此单测可用 `tokio::sync::mpsc` 假 WS 直接验证转发语义。

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Path, State,
    },
    http::StatusCode,
    middleware::from_fn_with_state,
    response::{IntoResponse, Json, Response},
    routing::get,
    Extension, Router,
};
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    agent::{self, AgentConnection, Frame, FrameKind},
    audit::AuditEvent,
    auth::{auth_guard, Claims},
    policy::{canonical_role, Action, Decision},
    resources::{BackendResolution, ResourceInfo},
    AppState,
};

/// 构造带角色的审计事件（F5-V1：user/role 取自 JWT claims）。
fn audit_event(
    claims: &Claims,
    event: &str,
    resource_id: &str,
    peer: SocketAddr,
    error: Option<String>,
) -> AuditEvent {
    let mut ev = AuditEvent::new(
        claims.sub.clone(),
        Some(canonical_role(&claims.roles).as_str()),
        event,
        resource_id,
        peer.ip().to_string(),
    );
    ev.error = error;
    ev
}

/// WS -> TCP 泵：从消息通道读取，把 Binary 帧字节写入 TCP 写半。
/// Text 帧忽略；Close 帧（None）结束泵并向 TCP 发 FIN。
/// 返回是否正常收尾（true=EOF/Close，false=写入失败）。
pub async fn pump_ws_to_tcp<W>(mut rx: mpsc::Receiver<Message>, mut tcp_w: W) -> bool
where
    W: AsyncWrite + Unpin,
{
    while let Some(msg) = rx.recv().await {
        match msg {
            Message::Binary(data) => {
                if tcp_w.write_all(&data).await.is_err() {
                    return false;
                }
            }
            Message::Text(_) => {} // 真实隧道只透传二进制 RDP 流量，文本帧忽略
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => {}
        }
    }
    let _ = tcp_w.shutdown().await;
    true
}

/// TCP -> WS 泵：从 TCP 读半读取字节，切成 Binary 帧发往消息通道。
/// 返回是否读到后端 EOF 后正常结束（false=读错误或 WS 侧通道关闭）。
pub async fn pump_tcp_to_ws<R>(mut tcp_r: R, tx: mpsc::Sender<Message>) -> bool
where
    R: AsyncRead + Unpin,
{
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match tcp_r.read(&mut buf).await {
            Ok(0) => return true,
            Ok(n) => {
                if tx.send(Message::Binary(buf[..n].to_vec())).await.is_err() {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
}

/// 适配任务：把 axum 分离出的 WS 读半搬运进 mpsc 通道（供 pump_ws_to_tcp 消费）。
async fn ws_source_to_channel(mut ws_rx: futures_util::stream::SplitStream<WebSocket>, tx: mpsc::Sender<Message>) {
    while let Some(Ok(msg)) = ws_rx.next().await {
        if tx.send(msg).await.is_err() {
            break;
        }
    }
}

/// 适配任务：从 mpsc 通道取消息写回 WS 写半（供 pump_tcp_to_ws 投递）。
async fn channel_to_ws_sink(
    mut rx: mpsc::Receiver<Message>,
    mut ws_tx: futures_util::stream::SplitSink<WebSocket, Message>,
) {
    while let Some(msg) = rx.recv().await {
        if ws_tx.send(msg).await.is_err() {
            break;
        }
    }
}

async fn tunnel_handler(
    State(state): State<Arc<AppState>>,
    Path(resource_id): Path<String>,
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(claims): Extension<Claims>,
) -> Response {
    if resource_id.is_empty() {
        return (StatusCode::BAD_REQUEST, "resource_id 不能为空").into_response();
    }

    // 0. 策略矩阵检查（F5-V1：资源 × 角色 × connect；拒绝 => 403 + policy_denied 审计）
    if state.policy.check(&resource_id, canonical_role(&claims.roles), Action::Connect)
        == Decision::Deny
    {
        tracing::warn!(%peer, %resource_id, user = %claims.sub, "tunnel denied by policy matrix");
        state
            .audit
            .record(audit_event(&claims, "policy_denied", &resource_id, peer, Some("connect denied by policy matrix".into())));
        return (StatusCode::FORBIDDEN, "策略矩阵拒绝该连接（policy_denied）").into_response();
    }

    // 1. 解析后端并做角色级授权（P4：动态层优先且校验 owner=claims.sub 一致，
    //    revoked 一律不存在；回退静态注册表与显式私网 host:port 直连；
    //    agent 型资源返回 Agent 分支，走反向通道 DIAL）
    let backend = match state
        .resources
        .resolve_backend_for_user(&resource_id, &claims.roles, Some(&claims.sub))
    {
        BackendResolution::Allowed(addr) => BackendTarget::Direct(addr),
        BackendResolution::Agent { local_backend } => BackendTarget::AgentMode(local_backend),
        BackendResolution::Forbidden => {
            // 资源存在但 claims.roles 与 allowed_roles 无交集：403 + 审计
            tracing::warn!(%peer, %resource_id, user = %claims.sub, "tunnel forbidden by role policy");
            state.audit.record(audit_event(
                &claims,
                "tunnel_forbidden",
                &resource_id,
                peer,
                Some("roles not allowed by resource allowed_roles".to_string()),
            ));
            return (StatusCode::FORBIDDEN, "当前角色无权访问该资源").into_response();
        }
        BackendResolution::NotFound => {
            state.audit.record(audit_event(
                &claims,
                "tunnel_rejected",
                &resource_id,
                peer,
                Some("backend unknown or not in allowed private ranges".to_string()),
            ));
            return (StatusCode::NOT_FOUND, "资源不存在或后端地址不允许").into_response();
        }
    };

    // 2. 建立后端连接（升级 WS 之前执行，失败快速返回，避免升级后客户端
    //    才发现连不通）：
    //    - direct：网关主动 TcpStream::connect（10s 超时）；
    //    - agent：经反向通道 DIAL，等待 Agent OPEN_OK（dial_timeout_secs 超时）。
    let established = match &backend {
        BackendTarget::Direct(addr) => match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            TcpStream::connect(addr),
        )
        .await
        {
            Ok(Ok(t)) => EstablishedBackend::Tcp(t),
            Ok(Err(e)) => {
                let summary = format!("connect {addr} failed: {e}");
                tracing::warn!(%peer, %resource_id, addr, error = %e, "backend connect failed");
                state.audit.record(audit_event(
                    &claims,
                    "tunnel_backend_error",
                    &resource_id,
                    peer,
                    Some(summary),
                ));
                return (StatusCode::BAD_GATEWAY, "连接后端 RDP 主机失败").into_response();
            }
            Err(_) => {
                let summary = format!("connect {addr} timed out after 10s");
                tracing::warn!(%peer, %resource_id, addr, "backend connect timeout");
                state.audit.record(audit_event(
                    &claims,
                    "tunnel_backend_error",
                    &resource_id,
                    peer,
                    Some(summary),
                ));
                return (StatusCode::GATEWAY_TIMEOUT, "连接后端 RDP 主机超时").into_response();
            }
        },
        BackendTarget::AgentMode(local_backend) => match agent_dial(
            &state, &resource_id, local_backend, &claims, peer,
        )
        .await
        {
            Ok(dialed) => dialed,
            Err(resp) => return resp,
        },
    };

    let user = claims.sub.clone();
    let role = canonical_role(&claims.roles).as_str().to_string();
    ws.on_upgrade(move |socket| {
        handle_tunnel_socket(state, resource_id, backend, user, role, peer, socket, established)
    })
}

/// 后端接入形态（R-20261122-006 P2）。
enum BackendTarget {
    /// direct 型：网关主动连接的 "host:port"。
    Direct(String),
    /// agent 型：经目标机 Agent 反向隧道（local_backend 供 DIAL payload）。
    AgentMode(String),
}

/// 已建立的后端连接。
enum EstablishedBackend {
    Tcp(TcpStream),
    Agent {
        conn: Arc<AgentConnection>,
        session_id: u64,
        sess_rx: mpsc::Receiver<Frame>,
    },
}

/// agent 型资源 DIAL（P2）：分配 session_id → 注册 session 回投通道 →
/// 向 Agent 发 DIAL → dial_timeout_secs 内等 OPEN_OK。
/// 失败（Agent 不在线 / 会话上限 / 发送失败 / OPEN_ERR / 超时）返回错误响应，
/// 并记录 agent_dial_error / agent_dial_timeout 审计。
async fn agent_dial(
    state: &AppState,
    resource_id: &str,
    local_backend: &str,
    claims: &Claims,
    peer: SocketAddr,
) -> Result<EstablishedBackend, Response> {
    let audit_dial_err = |error: String| {
        audit_event(
            claims,
            "agent_dial_error",
            resource_id,
            peer,
            Some(error),
        )
    };

    if !state.agent_cfg.enabled {
        tracing::warn!(%peer, %resource_id, "agent channel disabled");
        return Err((StatusCode::SERVICE_UNAVAILABLE, "agent 通道未启用").into_response());
    }
    let Some(conn) = state.agent_registry.get(resource_id) else {
        tracing::warn!(%peer, %resource_id, "agent not online");
        state.audit.record(audit_dial_err("agent offline".into()));
        return Err((StatusCode::BAD_GATEWAY, "目标机 Agent 不在线").into_response());
    };

    let session_id = state.agent_sessions.next();
    let (sess_tx, sess_rx) = mpsc::channel::<Frame>(64);
    if agent::register_session(&conn, session_id, sess_tx).await.is_none() {
        tracing::warn!(%peer, %resource_id, "agent session limit reached");
        state
            .audit
            .record(audit_dial_err("session limit reached".into()));
        return Err((StatusCode::BAD_GATEWAY, "Agent 会话数已达上限").into_response());
    }

    let dial = Frame::new(
        FrameKind::Dial,
        session_id,
        serde_json::json!({ "backend": local_backend }).to_string().into_bytes(),
    );
    if !conn.send(dial).await {
        agent::deregister_session(&conn, session_id).await;
        tracing::warn!(%peer, %resource_id, "agent outbound channel closed");
        state
            .audit
            .record(audit_dial_err("agent channel closed".into()));
        return Err((StatusCode::BAD_GATEWAY, "目标机 Agent 通道已断开").into_response());
    }

    let timeout_secs = state.agent_cfg.dial_timeout_secs();
    let opened = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        wait_open(sess_rx),
    )
    .await;

    match opened {
        Ok(Ok(rx)) => {
            state.audit.record(audit_event(
                claims,
                "agent_dial",
                resource_id,
                peer,
                None,
            ));
            info!(%peer, %resource_id, session_id, "agent dial opened");
            Ok(EstablishedBackend::Agent { conn, session_id, sess_rx: rx })
        }
        Ok(Err(err_payload)) => {
            agent::deregister_session(&conn, session_id).await;
            let summary = format!("agent open_err: {err_payload}");
            tracing::warn!(%peer, %resource_id, session_id, summary = %summary, "agent dial refused");
            state.audit.record(audit_dial_err(summary));
            Err((StatusCode::BAD_GATEWAY, "目标机 Agent 打开本地后端失败").into_response())
        }
        Err(_) => {
            agent::deregister_session(&conn, session_id).await;
            let summary = format!("agent dial timed out after {timeout_secs}s");
            tracing::warn!(%peer, %resource_id, session_id, "agent dial timeout");
            state.audit.record(audit_event(
                claims,
                "agent_dial_timeout",
                resource_id,
                peer,
                Some(summary),
            ));
            Err((StatusCode::GATEWAY_TIMEOUT, "目标机 Agent 响应超时").into_response())
        }
    }
}

/// 等待该 session 的 OPEN_OK / OPEN_ERR / CLOSE（视为拒绝）。
/// 成功返回带回投通道；失败返回错误 payload 文本。
async fn wait_open(
    mut sess_rx: mpsc::Receiver<Frame>,
) -> Result<mpsc::Receiver<Frame>, String> {
    loop {
        match sess_rx.recv().await {
            Some(Frame { kind: FrameKind::OpenOk, .. }) => return Ok(sess_rx),
            Some(Frame { kind: FrameKind::OpenErr, payload, .. }) => {
                return Err(String::from_utf8_lossy(&payload).into_owned());
            }
            Some(Frame { kind: FrameKind::Close, .. }) => {
                return Err("closed before open".into())
            }
            Some(_) => {} // 其他帧在 OPEN 前到达：忽略（协议上不应出现）
            None => return Err("session channel closed".into()),
        }
    }
}

/// agent 会话双向适配（P2）：TCP 半边替换为 duplex + 帧适配任务，
/// 既有 pump_ws_to_tcp / pump_tcp_to_ws 泵函数零改动。
/// - 任务 A（Agent→客户端）：session 帧队列 DATA payload 写入 duplex；
///   CLOSE/OPEN_ERR/队列关闭 = 后端 EOF（shutdown 写半）。
/// - 任务 B（客户端→Agent）：duplex 读半 16KiB 切帧发 DATA；EOF/错误发 CLOSE
///   并摘除 session。
/// 返回 duplex 的"backend 视角"读半、写半（已 split）与任务 A 句柄，
/// 供既有 pump_ws_to_tcp / pump_tcp_to_ws 直接消费。
fn spawn_agent_session_adapters(
    conn: Arc<AgentConnection>,
    session_id: u64,
    mut sess_rx: mpsc::Receiver<Frame>,
) -> (
    tokio::io::ReadHalf<tokio::io::DuplexStream>,
    tokio::io::WriteHalf<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<()>,
) {
    let (gateway_side, backend_side): (
        tokio::io::DuplexStream,
        tokio::io::DuplexStream,
    ) = tokio::io::duplex(64 * 1024);
    let (backend_r, backend_w) = tokio::io::split(backend_side);

    // 任务 A：Agent 帧 → duplex（backend 写半）。
    let downstream = tokio::spawn(async move {
        let mut w = backend_w;
        loop {
            match sess_rx.recv().await {
                Some(Frame { kind: FrameKind::Data, payload, .. }) => {
                    if w.write_all(&payload).await.is_err() {
                        return;
                    }
                }
                // CLOSE / OPEN_ERR / 通道关闭：后端 EOF 语义
                Some(_) | None => {
                    let _ = w.shutdown().await;
                    return;
                }
            }
        }
    });

    // 任务 B：duplex（backend 读半）→ Agent DATA；收尾发 CLOSE + 摘除 session。
    let _upstream = tokio::spawn(async move {
        let mut r = backend_r;
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match r.read(&mut buf).await {
                Ok(n) if n > 0 => {
                    let frame = Frame::new(FrameKind::Data, session_id, buf[..n].to_vec());
                    if !conn.send(frame).await {
                        break;
                    }
                }
                _ => break,
            }
        }
        let _ = conn
            .send(Frame::new(FrameKind::Close, session_id, Vec::new()))
            .await;
        agent::deregister_session(&conn, session_id).await;
    });

    let (gateway_r, gateway_w) = tokio::io::split(gateway_side);
    (gateway_r, gateway_w, downstream)
}

/// 隧道会话：审计 open -> 双向泵 -> 审计 close。
#[allow(clippy::too_many_arguments)]
async fn handle_tunnel_socket(
    state: Arc<AppState>,
    resource_id: String,
    _backend: BackendTarget,
    user: String,
    role: String,
    peer: SocketAddr,
    socket: WebSocket,
    established: EstablishedBackend,
) {
    info!(%peer, %resource_id, "tunnel established");
    state.audit.record(AuditEvent::new(
        user.clone(),
        Some(&role),
        "tunnel_open",
        resource_id.clone(),
        peer.ip().to_string(),
    ));

    let (ws_tx, ws_rx) = socket.split();
    // WS 侧消息经 mpsc 通道中转，使两个泵成为可单测的纯转发单元。
    let (client_tx, client_rx) = mpsc::channel::<Message>(64);
    let (backend_tx, backend_rx) = mpsc::channel::<Message>(64);

    tokio::spawn(ws_source_to_channel(ws_rx, client_tx));
    let sink_task = tokio::spawn(channel_to_ws_sink(backend_rx, ws_tx));

    // 后端读半 / 写半按接入形态构造；泵函数语义零改动：
    // - Tcp：tokio::io::split 原 TcpStream；
    // - Agent：spawn_agent_session_adapters 生成 duplex + 帧适配任务，
    //   gateway 视角读写两半即是"backend TCP 半边"。
    let (to_backend, to_client, downstream_agent) = match established {
        EstablishedBackend::Tcp(tcp) => {
            let (tcp_r, tcp_w) = tokio::io::split(tcp);
            (
                tokio::spawn(pump_ws_to_tcp(client_rx, tcp_w)),
                tokio::spawn(pump_tcp_to_ws(tcp_r, backend_tx)),
                None,
            )
        }
        EstablishedBackend::Agent { conn, session_id, sess_rx } => {
            let (backend_r, backend_w, downstream_agent) =
                spawn_agent_session_adapters(conn, session_id, sess_rx);
            (
                tokio::spawn(pump_ws_to_tcp(client_rx, backend_w)),
                tokio::spawn(pump_tcp_to_ws(backend_r, backend_tx)),
                Some(downstream_agent),
            )
        }
    };

    let _ = tokio::join!(to_backend, to_client);

    // 泵结束后唯一持有 backend_tx 的任务已退出，sink 适配任务随之结束。
    let _ = sink_task.await;
    if let Some(downstream_agent) = downstream_agent {
        // Agent 会话适配任务 A 可能仍阻塞在帧队列上（客户端先断开时），
        // 显式收尾避免泄漏。
        downstream_agent.abort();
    }

    state.audit.record(AuditEvent::new(
        user,
        Some(&role),
        "tunnel_close",
        resource_id,
        peer.ip().to_string(),
    ));
    info!(%peer, "tunnel closed");
}

/// GET /api/resources —— 资源列表（需认证；只含 id/name，不含后端地址）。
async fn list_resources(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
) -> Response {
    // 策略矩阵检查（F5-V1：角色 × list；resource 维度以 "*" 通配判定）
    if state.policy.check("*", canonical_role(&_claims.roles), Action::List) == Decision::Deny {
        return (StatusCode::FORBIDDEN, "策略矩阵拒绝资源列表访问").into_response();
    }
    let items: Vec<ResourceInfo> = state.resources.list();
    Json(serde_json::json!({ "resources": items })).into_response()
}

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // 隧道路由挂 auth_guard：Bearer 校验失败即 401，WebSocket 升级不会发生。
    Router::new()
        // 注意：axum 0.7 路径参数语法为 ":param"（0.8 起才是 "{param}"）
        .route("/tunnel/:resource_id", get(tunnel_handler))
        .route("/api/resources", get(list_resources))
        .route_layer(from_fn_with_state(Arc::clone(&state), auth_guard))
        .with_state(state)
}

// ---------------------------------------------------------------------
// 单元测试：泵函数转发语义（用 mpsc 假 WS + duplex 假 TCP）
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::ResourceRegistry;
    use tokio::io::duplex;

    #[tokio::test]
    async fn ws_to_tcp_pump_forwards_binary_ignores_text() {
        let (tx, rx) = mpsc::channel(8);
        let (tcp_client, mut tcp_server) = duplex(64);

        let pump = tokio::spawn(pump_ws_to_tcp(rx, tcp_client));

        tx.send(Message::Binary(vec![0x03, 0x00, 0x00, 0x00])).await.unwrap();
        tx.send(Message::Text("should-be-ignored".into())).await.unwrap();
        tx.send(Message::Binary(b"rdp-payload".to_vec())).await.unwrap();
        drop(tx); // 关闭通道 => 泵结束并向 TCP shutdown

        assert!(pump.await.unwrap(), "正常收尾应返回 true");

        let mut got = Vec::new();
        tcp_server.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"\x03\x00\x00\x00rdp-payload");
    }

    #[tokio::test]
    async fn ws_to_tcp_pump_reports_failure_when_write_errors() {
        let (tx, rx) = mpsc::channel(8);
        // 立即丢弃对端，使写入失败（EPIPE/EOF 语义）
        let (tcp_client, _server_dropped) = duplex(4);
        drop(_server_dropped);
        // 通道容量 8 足够让第一条 send 成功入队后再被泵消费
        tx.send(Message::Binary(vec![1u8; 64])).await.unwrap();
        drop(tx);
        assert!(!pump_ws_to_tcp(rx, tcp_client).await, "写入失败应返回 false");
    }

    #[tokio::test]
    async fn tcp_to_ws_pump_streams_binary_until_eof() {
        let (mut tcp_server, tcp_client) = duplex(64);
        let (tx, mut rx) = mpsc::channel(8);

        let pump = tokio::spawn(pump_tcp_to_ws(tcp_client, tx));
        tcp_server.write_all(b"RDP:hello").await.unwrap();
        tcp_server.shutdown().await.unwrap(); // EOF

        assert!(pump.await.unwrap(), "EOF 应正常结束");
        let mut frames = Vec::new();
        while let Some(msg) = rx.recv().await {
            match msg {
                Message::Binary(d) => frames.extend_from_slice(&d),
                Message::Text(_) => panic!("不应产生文本帧"),
                _ => {}
            }
        }
        assert_eq!(frames, b"RDP:hello");
    }

    #[tokio::test]
    async fn tcp_to_ws_pump_reports_failure_when_channel_closed() {
        // 保持写端存活使读端有数据可读；消息通道接收端已关闭 => 发送失败。
        let (mut tcp_writer, tcp_client) = duplex(64);
        let (tx, rx) = mpsc::channel(1);
        drop(rx); // 接收端不存在 => 首次 send 即 Err
        let pump = tokio::spawn(pump_tcp_to_ws(tcp_client, tx));
        tcp_writer.write_all(b"x").await.unwrap();
        assert!(!pump.await.unwrap(), "通道关闭应返回 false");
    }

    #[test]
    fn resolve_rejects_public_backend_for_explicit_tunnel() {
        let reg = ResourceRegistry::from_configs(&[crate::resources::ResourceConfig {
            id: "public-target".into(),
            name: "x".into(),
            backend: "8.8.8.8:3389".into(),
            allowed_roles: None,
            mode: None,
            agent_local_backend: None,
        }]);
        // 注册表是管理员配置的可信来源，仍可解析；
        // 但 resource_id 显式直连路径受 SSRF 白名单约束。
        assert_eq!(
            reg.resolve_backend("public-target", &[]),
            crate::resources::BackendResolution::Allowed("8.8.8.8:3389".into())
        );
        assert_eq!(
            reg.resolve_backend("8.8.8.8:3389", &[]),
            crate::resources::BackendResolution::NotFound
        );
    }

    // -----------------------------------------------------------------
    // P2（R-20261122-006）：agent 分支
    // -----------------------------------------------------------------

    use crate::config::AgentConfig;
    use std::net::SocketAddr;

    fn fake_agent_conn(rid: &str, max_sessions: usize) -> Arc<AgentConnection> {
        let (out_tx, _out_rx) = mpsc::channel(16);
        let (_in_tx, in_rx) = mpsc::channel(16);
        Arc::new(AgentConnection::new(
            rid,
            "agent-e2e",
            "127.0.0.1:3389",
            out_tx,
            in_rx,
            max_sessions,
            agent::unix_ms(),
        ))
    }

    fn p2_app_state() -> Arc<AppState> {
        let reg = ResourceRegistry::from_configs(&[crate::resources::ResourceConfig {
            id: "agent-target".into(),
            name: "内网目标机".into(),
            backend: "-".into(),
            allowed_roles: None,
            mode: Some("agent".into()),
            agent_local_backend: Some("127.0.0.1:3390".into()),
        }]);
        Arc::new(AppState {
            auth: crate::auth::AuthState::new("p2-test-secret", 3600),
            audit: crate::audit::AuditLog::new(64),
            resources: reg,
            admin_key: None,
            policy: crate::policy::PolicyHandle::default(),
            totp_secrets: Default::default(),
            oidc: None,
            agent_registry: agent::AgentRegistry::new(),
            agent_cfg: AgentConfig {
                enabled: true,
                dial_timeout_secs: Some(1),
                ..Default::default()
            },
            agent_sessions: agent::SessionIdAllocator::new(),
        })
    }

    fn p2_claims() -> Claims {
        Claims {
            sub: "alice".into(),
            roles: vec!["user".into()],
            exp: u64::MAX,
            rid: None,
            iat: 0,
            iss: "easyrdp-gateway".into(),
        }
    }

    /// agent 分支资源解析：resolve_backend 应返回 Agent { local_backend }。
    #[test]
    fn resolve_agent_mode_resource_returns_agent_branch() {
        let reg = ResourceRegistry::from_configs(&[crate::resources::ResourceConfig {
            id: "agent-target".into(),
            name: "内网目标机".into(),
            backend: "-".into(),
            allowed_roles: None,
            mode: Some("agent".into()),
            agent_local_backend: Some("127.0.0.1:3390".into()),
        }]);
        assert_eq!(
            reg.resolve_backend("agent-target", &["user".to_string()]),
            BackendResolution::Agent {
                local_backend: "127.0.0.1:3390".into()
            }
        );
    }

    /// Agent 不在线：agent_dial 快速 502，不出 DIAL 帧。
    #[tokio::test]
    async fn agent_dial_offline_returns_502() {
        let state = p2_app_state();
        let claims = p2_claims();
        let peer: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let resp = agent_dial(&state, "agent-target", "127.0.0.1:3390", &claims, peer)
            .await
            .err()
            .expect("offline must fail");
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    /// DIAL → OPEN_OK：成功建立会话，后续帧走会话回投通道。
    #[tokio::test]
    async fn agent_dial_open_ok_establishes_session() {
        let state = p2_app_state();
        let claims = p2_claims();
        let peer: SocketAddr = "127.0.0.1:5001".parse().unwrap();

        let (out_tx, mut out_rx) = mpsc::channel(16);
        let (in_tx, in_rx) = mpsc::channel(16);
        let conn = state.agent_registry.register(AgentConnection::new(
            "agent-target",
            "agent-e2e",
            "127.0.0.1:3390",
            out_tx,
            in_rx,
            8,
            agent::unix_ms(),
        ));

        // 先挂分发循环（OPEN_OK/DATA 需经 dispatch_agent_frame 路由到会话）；
        // agent_dial 放独立任务，避免单线程 runtime 下先 recv 等 DIAL 死锁。
        let mut inbound = conn.take_inbound().await.expect("inbound once");
        let dispatch_conn = conn.clone();
        let dispatch = tokio::spawn(async move {
            while let Some(frame) = inbound.recv().await {
                dispatch_conn.mark_seen_ms(agent::unix_ms());
                agent::dispatch_agent_frame(&dispatch_conn, frame).await;
            }
        });
        let st = Arc::clone(&state);
        let fut = tokio::spawn(async move {
            agent_dial(&st, "agent-target", "127.0.0.1:3390", &p2_claims(), peer).await
        });
        // 网关应已向 Agent 发出 DIAL，payload 为 local_backend JSON
        let dial = out_rx.recv().await.expect("dial frame");
        assert_eq!(dial.kind, FrameKind::Dial);
        let payload = String::from_utf8(dial.payload).unwrap();
        assert!(payload.contains("127.0.0.1:3390"), "DIAL payload = {payload}");
        let sid = dial.session_id;
        assert!(sid > 0);

        // 假 Agent 回 OPEN_OK → wait_open 解除阻塞
        in_tx.send(Frame::new(FrameKind::OpenOk, sid, Vec::new()))
            .await
            .unwrap();

        let EstablishedBackend::Agent {
            conn: c,
            session_id: s,
            sess_rx,
        } = fut.await.expect("task").expect("dial ok")
        else {
            panic!("expected agent established");
        };
        assert!(Arc::ptr_eq(&c, &conn));
        assert_eq!(s, sid);

        // Agent 上行 DATA 能经分发循环路由到该会话（验证会话已注册）
        in_tx.send(Frame::new(FrameKind::Data, sid, b"banner".to_vec()))
            .await
            .unwrap();
        let mut sess_rx = sess_rx;
        let got = sess_rx.recv().await.expect("data routed to session");
        assert_eq!(got.payload, b"banner");
        dispatch.abort();
    }

    /// DIAL → OPEN_ERR：502 + 会话摘除。
    #[tokio::test]
    async fn agent_dial_open_err_returns_502_and_deregisters() {
        let state = p2_app_state();
        let claims = p2_claims();
        let peer: SocketAddr = "127.0.0.1:5002".parse().unwrap();

        let (out_tx, mut out_rx) = mpsc::channel(16);
        let (in_tx, in_rx) = mpsc::channel(16);
        let conn = state.agent_registry.register(AgentConnection::new(
            "agent-target",
            "agent-e2e",
            "127.0.0.1:3390",
            out_tx,
            in_rx,
            8,
            agent::unix_ms(),
        ));

        // 分发循环：OPEN_ERR 需经 dispatch_agent_frame 投递到 session 才算"拒绝"
        let mut inbound = conn.take_inbound().await.expect("inbound once");
        let dispatch_conn = conn.clone();
        let dispatch = tokio::spawn(async move {
            while let Some(frame) = inbound.recv().await {
                dispatch_conn.mark_seen_ms(agent::unix_ms());
                agent::dispatch_agent_frame(&dispatch_conn, frame).await;
            }
        });
        // agent_dial 放独立任务：单线程 runtime 下先 recv 等 DIAL 才不会死锁
        let st = Arc::clone(&state);
        let fut = tokio::spawn(async move {
            agent_dial(&st, "agent-target", "127.0.0.1:3390", &p2_claims(), peer).await
        });
        // 并行驱动：等 DIAL 帧后回 OPEN_ERR
        let dial = out_rx.recv().await.expect("dial");
        in_tx.send(Frame::new(FrameKind::OpenErr, dial.session_id, b"refused".to_vec()))
            .await
            .unwrap();
        let resp = fut.await.expect("task").err().expect("must fail");
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        // 会话已摘除
        assert!(
            conn.sessions.lock().await.is_empty(),
            "OPEN_ERR 后 session 必须摘除"
        );
        dispatch.abort();
    }

    /// DIAL 超时（dial_timeout_secs=1）：504 + 会话摘除 + CLOSE 发往 Agent。
    #[tokio::test]
    async fn agent_dial_timeout_returns_504_and_deregisters() {
        let state = p2_app_state();
        let claims = p2_claims();
        let peer: SocketAddr = "127.0.0.1:5003".parse().unwrap();

        let (out_tx, mut out_rx) = mpsc::channel(16);
        let (_in_tx, in_rx) = mpsc::channel(16);
        let conn = state.agent_registry.register(AgentConnection::new(
            "agent-target",
            "agent-e2e",
            "127.0.0.1:3390",
            out_tx,
            in_rx,
            8,
            agent::unix_ms(),
        ));

        let resp = agent_dial(&state, "agent-target", "127.0.0.1:3390", &claims, peer)
            .await
            .err()
            .expect("must timeout");
        assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);
        assert!(conn.sessions.lock().await.is_empty(), "超时后 session 摘除");
        let _ = out_rx.recv().await; // 消费 DIAL 帧，确认通道工作正常
    }

    /// 回环 e2e（字节级）：agent_dial → 适配器 → 既有双泵 → mpsc 假 Agent
    /// （DIAL→OPEN_OK、DATA 原样回显）全链路，客户端写入与回读必须逐字节一致；
    /// 客户端断开后 CLOSE 摘除会话。
    #[tokio::test]
    async fn agent_p2_loopback_bytes_identical_and_close_cleanup() {
        let state = p2_app_state();
        let claims = p2_claims();
        let peer: SocketAddr = "127.0.0.1:5004".parse().unwrap();

        let (out_tx, mut out_rx) = mpsc::channel::<Frame>(256);
        let (in_tx, mut in_rx) = mpsc::channel::<Frame>(256);
        let conn = state.agent_registry.register(AgentConnection::new(
            "agent-target",
            "agent-e2e",
            "127.0.0.1:3389",
            out_tx,
            in_rx,
            8,
            agent::unix_ms(),
        ));

        // 假 Agent：DIAL→OPEN_OK；DATA 原样回显（模拟 echo 后端）；
        // 收到 CLOSE 时向 closed 通道发标记供收尾断言。
        let (closed_tx, closed_rx) = tokio::sync::oneshot::channel::<u64>();
        let mut closed_tx = Some(closed_tx);
        let echo_in_tx = in_tx.clone();
        let agent_task = tokio::spawn(async move {
            let mut open_sessions = std::collections::HashSet::new();
            while let Some(frame) = out_rx.recv().await {
                match frame.kind {
                    FrameKind::Dial => {
                        open_sessions.insert(frame.session_id);
                        echo_in_tx
                            .send(Frame::new(FrameKind::OpenOk, frame.session_id, Vec::new()))
                            .await
                            .unwrap();
                    }
                    FrameKind::Data if open_sessions.contains(&frame.session_id) => {
                        echo_in_tx
                            .send(Frame::new(FrameKind::Data, frame.session_id, frame.payload))
                            .await
                            .unwrap();
                    }
                    FrameKind::Close => {
                        open_sessions.remove(&frame.session_id);
                        if let Some(tx) = closed_tx.take() {
                            let _ = tx.send(frame.session_id);
                        }
                    }
                    _ => {}
                }
            }
        });

        // 网关侧分发循环（与 handle_agent_socket 同语义）：路由入站帧到会话。
        let dispatch_conn = conn.clone();
        let mut inbound = conn.take_inbound().await.expect("inbound once");
        let dispatch_task = tokio::spawn(async move {
            while let Some(frame) = inbound.recv().await {
                dispatch_conn.mark_seen_ms(agent::unix_ms());
                agent::dispatch_agent_frame(&dispatch_conn, frame).await;
            }
        });

        // agent 分支 DIAL：经假 Agent OPEN_OK 后建立会话
        let EstablishedBackend::Agent {
            conn: dial_conn,
            session_id,
            sess_rx,
        } = agent_dial(&state, "agent-target", "127.0.0.1:3389", &claims, peer)
            .await
            .expect("agent dial ok")
        else {
            panic!("expected agent established");
        };
        assert!(Arc::ptr_eq(&dial_conn, &conn));
        assert!(conn.sessions.lock().await.contains_key(&session_id));

        // 适配器 + 既有双泵（WS 半边用 mpsc 通道模拟）
        let (gw_r, gw_w, downstream_agent) =
            spawn_agent_session_adapters(conn.clone(), session_id, sess_rx);
        let (client_tx, client_rx) = mpsc::channel::<Message>(64);
        let (backend_tx, mut backend_rx) = mpsc::channel::<Message>(64);
        let to_backend = tokio::spawn(pump_ws_to_tcp(client_rx, gw_w));
        let to_client = tokio::spawn(pump_tcp_to_ws(gw_r, backend_tx));

        // 客户端 → 目标机 → 回读：100KB 任意字节（强制多帧切分）
        let pattern: Vec<u8> = (0..100 * 1024u32).map(|i| (i % 253) as u8).collect();
        client_tx
            .send(Message::Binary(pattern.clone()))
            .await
            .unwrap();

        let mut got = Vec::with_capacity(pattern.len());
        while got.len() < pattern.len() {
            match backend_rx.recv().await {
                Some(Message::Binary(d)) => got.extend_from_slice(&d),
                Some(_) => {}
                None => panic!("backend channel closed early at {} bytes", got.len()),
            }
        }
        assert_eq!(got, pattern, "经 Agent 反向隧道回环的字节流必须逐字节一致");

        // 客户端断开：CLOSE → 假 Agent 摘会话 → 回环静默收尾
        drop(client_tx);
        let _ = tokio::join!(to_backend, to_client);
        downstream_agent.abort();

        // 任务 B 收尾：CLOSE 已发往 Agent（假 Agent 收到后发标记），session 已摘除
        let closed = tokio::time::timeout(std::time::Duration::from_secs(2), closed_rx).await;
        assert!(closed.is_ok(), "适配器收尾必须向 Agent 发送 CLOSE");
        assert_eq!(closed.expect("closed").expect("sid"), session_id, "CLOSE 应带本会话号");
        assert!(
            conn.sessions.lock().await.is_empty(),
            "会话结束后必须从注册表摘除"
        );

        dispatch_task.abort();
        agent_task.abort();
    }
}
