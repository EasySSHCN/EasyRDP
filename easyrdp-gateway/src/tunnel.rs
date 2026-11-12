//! 隧道模块：`GET /tunnel/{resource_id}` WebSocket 升级 + 真实 TCP 转发。
//!
//! 流程：auth_guard 校验短期 JWT -> 依据 resource_id 解析后端地址
//! （资源注册表优先，其次显式私网 "host:port"，见 resources.rs）->
//! 连接内网 RDP 主机（升级前失败快速返回）-> WS 升级 ->
//! 双向字节泵（WS Binary 帧 <-> TCP 字节流；Text 帧忽略；Ping/Pong 由底层流自动处理）。
//! 建立、关闭与后端连接失败均记录审计事件。
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
    audit::AuditEvent,
    auth::{auth_guard, Claims},
    resources::ResourceInfo,
    AppState,
};

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

    // 1. 解析后端地址（注册表优先；其次显式私网 host:port 直连）
    let Some(backend) = state.resources.resolve_backend(&resource_id) else {
        state.audit.record(AuditEvent {
            timestamp: now_unix(),
            user: claims.sub.clone(),
            event: "tunnel_rejected".to_string(),
            resource: resource_id.clone(),
            source_ip: peer.ip().to_string(),
            error: Some("backend unknown or not in allowed private ranges".to_string()),
        });
        return (StatusCode::NOT_FOUND, "资源不存在或后端地址不允许").into_response();
    };

    // 2. 连接内网 RDP 主机（升级 WS 之前执行，失败快速返回 502；
    //    显式 10s 超时，避免不可达主机拖满 OS 默认连接超时）
    let tcp = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        TcpStream::connect(&backend),
    )
    .await
    {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => {
            let summary = format!("connect {backend} failed: {e}");
            tracing::warn!(%peer, %resource_id, %backend, error = %e, "backend connect failed");
            state.audit.record(AuditEvent {
                timestamp: now_unix(),
                user: claims.sub.clone(),
                event: "tunnel_backend_error".to_string(),
                resource: resource_id.clone(),
                source_ip: peer.ip().to_string(),
                error: Some(summary),
            });
            return (StatusCode::BAD_GATEWAY, "连接后端 RDP 主机失败").into_response();
        }
        Err(_) => {
            let summary = format!("connect {backend} timed out after 10s");
            tracing::warn!(%peer, %resource_id, %backend, "backend connect timeout");
            state.audit.record(AuditEvent {
                timestamp: now_unix(),
                user: claims.sub.clone(),
                event: "tunnel_backend_error".to_string(),
                resource: resource_id.clone(),
                source_ip: peer.ip().to_string(),
                error: Some(summary),
            });
            return (StatusCode::GATEWAY_TIMEOUT, "连接后端 RDP 主机超时").into_response();
        }
    };

    let user = claims.sub.clone();
    ws.on_upgrade(move |socket| {
        handle_tunnel_socket(state, resource_id, backend, user, peer, socket, tcp)
    })
}

/// 隧道会话：审计 open -> 双向泵 -> 审计 close。
async fn handle_tunnel_socket(
    state: Arc<AppState>,
    resource_id: String,
    backend: String,
    user: String,
    peer: SocketAddr,
    socket: WebSocket,
    tcp: TcpStream,
) {
    info!(%peer, %resource_id, %backend, "tunnel established");
    state.audit.record(AuditEvent {
        timestamp: now_unix(),
        user: user.clone(),
        event: "tunnel_open".to_string(),
        resource: resource_id.clone(),
        source_ip: peer.ip().to_string(),
        error: None,
    });

    let (ws_tx, ws_rx) = socket.split();
    let (tcp_r, tcp_w) = tokio::io::split(tcp);

    // WS 侧消息经 mpsc 通道中转，使两个泵成为可单测的纯转发单元。
    let (client_tx, client_rx) = mpsc::channel::<Message>(64);
    let (backend_tx, backend_rx) = mpsc::channel::<Message>(64);

    tokio::spawn(ws_source_to_channel(ws_rx, client_tx));
    let sink_task = tokio::spawn(channel_to_ws_sink(backend_rx, ws_tx));

    let to_backend = tokio::spawn(pump_ws_to_tcp(client_rx, tcp_w));
    let to_client = tokio::spawn(pump_tcp_to_ws(tcp_r, backend_tx));
    let _ = tokio::join!(to_backend, to_client);

    // 泵结束后唯一持有 backend_tx 的任务已退出，sink 适配任务随之结束。
    let _ = sink_task.await;

    state.audit.record(AuditEvent {
        timestamp: now_unix(),
        user,
        event: "tunnel_close".to_string(),
        resource: resource_id,
        source_ip: peer.ip().to_string(),
        error: None,
    });
    info!(%peer, "tunnel closed");
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// GET /api/resources —— 资源列表（需认证；只含 id/name，不含后端地址）。
async fn list_resources(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
) -> Response {
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
        }]);
        // 注册表是管理员配置的可信来源，仍可解析；
        // 但 resource_id 显式直连路径受 SSRF 白名单约束。
        assert!(reg.resolve_backend("public-target").is_some());
        assert!(reg.resolve_backend("8.8.8.8:3389").is_none());
    }
}
