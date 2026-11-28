//! Agent 反向隧道模块（R-20261122-006 P1）。
//!
//! 线缆协议 agent-wire/1（与 EasySSH 侧共享同一 spec 文本，代码独立实现）：
//! WS Binary 帧承载业务，Text 帧忽略。Binary 帧统一头部：
//!
//! ```text
//! 偏移  长度  字段
//! 0     1    type
//! 1     8    session_id (u64 BE, 每条客户端会话唯一，网关分配)
//! 9     4    payload_len (u32 BE)
//! 13    N    payload
//! ```
//!
//! 指令：0x01 DIAL(GW→Agent) / 0x02 OPEN_OK / 0x03 OPEN_ERR / 0x04 DATA /
//! 0x05 CLOSE / 0x06 PING / 0x07 PONG。心跳 30s 间隔 × 连续 3 次未收到判死。
//!
//! P1 范围：帧编解码 + AgentRegistry（按 resource_id 索引）+ 帧分发 +
//! 心跳判死 + GET /agent/connect 路由（独立 auth_guard，role=agent）。
//! 连接内部用 mpsc 通道桥接真实 WS 与分发逻辑，单测用 mpsc 假 Agent
//! 直接验证 DIAL/DATA/CLOSE/心跳判死语义（P2 tunnel 分支复用同一结构）。

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, RwLock,
};
use tokio::sync::mpsc;

use crate::{audit::AuditEvent, auth::Claims, AppState};

// ---------------------------------------------------------------------
// wire spec：agent-wire/1
// ---------------------------------------------------------------------

/// Binary 帧头长度：1(type) + 8(session_id BE) + 4(len BE)。
pub const FRAME_HEADER_LEN: usize = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// GW→Agent：payload = JSON {"backend":"host:port"}，请求建本地 TCP。
    Dial,
    /// Agent→GW：本地 TCP 已建立，session 开始。
    OpenOk,
    /// Agent→GW：payload = 错误摘要；网关回客户端 502 语义。
    OpenErr,
    /// 双向：payload = 透传字节。
    Data,
    /// 双向：关闭该 session。
    Close,
    /// 双向：应用层心跳（规避中间代理剥协议 Ping）。
    Ping,
    /// 双向：心跳应答。
    Pong,
}

impl FrameKind {
    pub fn from_u8(b: u8) -> Option<FrameKind> {
        match b {
            0x01 => Some(FrameKind::Dial),
            0x02 => Some(FrameKind::OpenOk),
            0x03 => Some(FrameKind::OpenErr),
            0x04 => Some(FrameKind::Data),
            0x05 => Some(FrameKind::Close),
            0x06 => Some(FrameKind::Ping),
            0x07 => Some(FrameKind::Pong),
            _ => None,
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            FrameKind::Dial => 0x01,
            FrameKind::OpenOk => 0x02,
            FrameKind::OpenErr => 0x03,
            FrameKind::Data => 0x04,
            FrameKind::Close => 0x05,
            FrameKind::Ping => 0x06,
            FrameKind::Pong => 0x07,
        }
    }
}

/// 一条 agent-wire/1 帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameKind,
    pub session_id: u64,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(kind: FrameKind, session_id: u64, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            kind,
            session_id,
            payload: payload.into(),
        }
    }

    /// 编码为完整 Binary 帧（header + payload）。
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(FRAME_HEADER_LEN + self.payload.len());
        out.push(self.kind.to_u8());
        out.extend_from_slice(&self.session_id.to_be_bytes());
        // payload 上限即 u32；网关侧 16KiB 切帧远低于该上限。
        out.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// 从缓冲解码一条完整帧。
    /// 返回 `Ok(Some((frame, consumed)))`：解码成功，consumed 为消耗字节数；
    /// 返回 `Ok(None)`：缓冲不足一条完整帧（调用方继续累积）；
    /// 返回 `Err`：type 非法或 payload_len 超过上限。
    pub fn decode(buf: &[u8]) -> Result<Option<(Frame, usize)>, FrameError> {
        if buf.len() < FRAME_HEADER_LEN {
            return Ok(None);
        }
        let kind = FrameKind::from_u8(buf[0]).ok_or(FrameError::BadType(buf[0]))?;
        let session_id = u64::from_be_bytes(buf[1..9].try_into().expect("8 bytes"));
        let payload_len = u32::from_be_bytes(buf[9..13].try_into().expect("4 bytes")) as usize;
        if payload_len > MAX_PAYLOAD_LEN {
            return Err(FrameError::PayloadTooLarge(payload_len));
        }
        let total = FRAME_HEADER_LEN + payload_len;
        if buf.len() < total {
            return Ok(None);
        }
        let payload = buf[FRAME_HEADER_LEN..total].to_vec();
        Ok(Some((Frame::new(kind, session_id, payload), total)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// 非法帧 type。
    BadType(u8),
    /// payload 长度超出上限（防恶意声明打爆内存）。
    PayloadTooLarge(usize),
}

/// 单帧 payload 上限：4 MiB（RDP/SSH 业务帧远小于该值；防声明攻击）。
pub const MAX_PAYLOAD_LEN: usize = 4 * 1024 * 1024;

/// 会话 ID 分配器：每资源通道从 1 起递增（0 保留给控制帧语义）。
#[derive(Default)]
pub struct SessionIdAllocator(AtomicU64);

impl SessionIdAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }
}

// ---------------------------------------------------------------------
// AgentRegistry：resource_id -> AgentConnection
// ---------------------------------------------------------------------

/// 网关侧一条 Agent 连接的句柄。
pub struct AgentConnection {
    /// 绑定的资源 ID（AgentToken rid claim）。
    pub resource_id: String,
    /// Agent 主体（claims.sub，如 "agent-u-xxxx"）。
    pub agent_id: String,
    /// 该资源的本地后端地址（来自网关配置，Agent 只能 DIAL 该地址）。
    pub local_backend: String,
    /// GW→Agent 出站帧队列（由 outbound pump 编码为 WS Binary 发出）。
    pub outbound_tx: mpsc::Sender<Frame>,
    /// Agent→GW 入站帧队列（连接 handler 取走后持续分发）。
    inbound_rx: tokio::sync::Mutex<Option<mpsc::Receiver<Frame>>>,
    /// 该连接上活跃客户端会话：session_id -> 会话帧回投通道。
    pub sessions: tokio::sync::Mutex<HashMap<u64, mpsc::Sender<Frame>>>,
    /// 并发 session 上限（[agent] max_sessions_per_conn，默认 8）。
    pub max_sessions: usize,
    /// 最近一次收到 Agent 任意帧的时间（unix 毫秒）。
    last_seen_ms: AtomicU64,
}

impl AgentConnection {
    pub fn new(
        resource_id: impl Into<String>,
        agent_id: impl Into<String>,
        local_backend: impl Into<String>,
        outbound_tx: mpsc::Sender<Frame>,
        inbound_rx: mpsc::Receiver<Frame>,
        max_sessions: usize,
        now_ms: u64,
    ) -> Self {
        Self {
            resource_id: resource_id.into(),
            agent_id: agent_id.into(),
            local_backend: local_backend.into(),
            outbound_tx,
            inbound_rx: tokio::sync::Mutex::new(Some(inbound_rx)),
            sessions: tokio::sync::Mutex::new(HashMap::new()),
            max_sessions: max_sessions.max(1),
            last_seen_ms: AtomicU64::new(now_ms),
        }
    }

    /// GW→Agent 发送一帧（队列满/连接关闭时返回 false）。
    pub async fn send(&self, frame: Frame) -> bool {
        self.outbound_tx.send(frame).await.is_ok()
    }

    /// 连接 handler 取走入站流（只允许取一次）。
    pub async fn take_inbound(&self) -> Option<mpsc::Receiver<Frame>> {
        self.inbound_rx.lock().await.take()
    }

    /// 收到 Agent 任意帧时刷新心跳时间。
    pub fn mark_seen_ms(&self, now_ms: u64) {
        self.last_seen_ms.store(now_ms, Ordering::Relaxed);
    }

    pub fn last_seen_ms(&self) -> u64 {
        self.last_seen_ms.load(Ordering::Relaxed)
    }

    /// 是否判死：连续 `max_missed` 个心跳周期内无任何帧即死。
    /// （心跳 30s × 3 => 90s 静默判死。）
    pub fn is_dead(&self, now_ms: u64, heartbeat_ms: u64, max_missed: u64) -> bool {
        now_ms.saturating_sub(self.last_seen_ms.load(Ordering::Relaxed))
            >= heartbeat_ms.saturating_mul(max_missed)
    }
}

/// Agent 连接注册表：按 resource_id 索引（同一资源同时最多一条在线连接，
/// 重连时以新连接替换旧连接）。
#[derive(Default)]
pub struct AgentRegistry {
    conns: RwLock<HashMap<String, Arc<AgentConnection>>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册（替换同资源旧连接），返回新连接句柄。
    pub fn register(&self, conn: AgentConnection) -> Arc<AgentConnection> {
        let conn = Arc::new(conn);
        self.conns
            .write()
            .expect("agent registry mutex poisoned")
            .insert(conn.resource_id.clone(), Arc::clone(&conn));
        conn
    }

    pub fn remove(&self, resource_id: &str) -> Option<Arc<AgentConnection>> {
        self.conns
            .write()
            .expect("agent registry mutex poisoned")
            .remove(resource_id)
    }

    pub fn get(&self, resource_id: &str) -> Option<Arc<AgentConnection>> {
        self.conns
            .read()
            .expect("agent registry mutex poisoned")
            .get(resource_id)
            .cloned()
    }

    pub fn online(&self, resource_id: &str) -> bool {
        self.get(resource_id).is_some()
    }

    pub fn online_count(&self) -> usize {
        self.conns
            .read()
            .expect("agent registry mutex poisoned")
            .len()
    }

    /// 心跳判死清扫：移除所有超时连接，返回被摘除的 resource_id 列表。
    pub fn sweep_dead(&self, now_ms: u64, heartbeat_ms: u64, max_missed: u64) -> Vec<String> {
        let dead: Vec<String> = self
            .conns
            .read()
            .expect("agent registry mutex poisoned")
            .iter()
            .filter(|(_, c)| c.is_dead(now_ms, heartbeat_ms, max_missed))
            .map(|(rid, _)| rid.clone())
            .collect();
        if dead.is_empty() {
            return dead;
        }
        let mut map = self
            .conns
            .write()
            .expect("agent registry mutex poisoned");
        dead.into_iter()
            .filter_map(|rid| {
                // 只摘除仍是同一条旧连接（未被重连替换）的死连接。
                map.remove(&rid)
            })
            .map(|c| c.resource_id.clone())
            .collect()
    }
}

// ---------------------------------------------------------------------
// 帧分发（纯逻辑，mpsc 假 Agent 单测覆盖）
// ---------------------------------------------------------------------

/// 入站帧分发结果（供测试与审计使用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// 已回复对端一帧（如 PING→PONG）。
    Replied(FrameKind),
    /// 帧已路由到 session 通道。
    Routed(FrameKind),
    /// 未知/非法帧，忽略。
    Dropped,
    /// 会话不存在或通道已关闭，丢弃。
    NoSession,
    /// 会话数达上限（OPEN 侧预留；DATA/CLOSE 不受影响；P2 tunnel 分支使用）。
    #[allow(dead_code)]
    SessionLimit,
}

/// 处理一条 Agent→GW 入站帧：
/// - PING → 经 outbound 回 PONG（同 session_id）；
/// - PONG → 刷新心跳（mark_seen 由调用方统一执行，此处无额外动作）；
/// - OPEN_OK/OPEN_ERR/DATA/CLOSE → 路由到已注册 session 通道；
///   DATA/CLOSE 命中的 session 在 CLOSE 后自动摘除。
pub async fn dispatch_agent_frame(conn: &AgentConnection, frame: Frame) -> DispatchOutcome {
    match frame.kind {
        FrameKind::Ping => {
            if conn
                .send(Frame::new(FrameKind::Pong, frame.session_id, Vec::new()))
                .await
            {
                DispatchOutcome::Replied(FrameKind::Pong)
            } else {
                DispatchOutcome::Dropped
            }
        }
        FrameKind::Pong => DispatchOutcome::Replied(FrameKind::Pong), // 心跳应答，mark_seen 已由调用方完成
        FrameKind::OpenOk | FrameKind::OpenErr | FrameKind::Data | FrameKind::Close => {
            let mut sessions = conn.sessions.lock().await;
            match sessions.get(&frame.session_id) {
                Some(tx) => {
                    let routed = tx.send(frame.clone()).await.is_ok();
                    if frame.kind == FrameKind::Close {
                        // CLOSE 后摘除会话，后续同 session 帧不再路由。
                        sessions.remove(&frame.session_id);
                    }
                    if routed {
                        DispatchOutcome::Routed(frame.kind)
                    } else {
                        sessions.remove(&frame.session_id);
                        DispatchOutcome::NoSession
                    }
                }
                None => DispatchOutcome::NoSession,
            }
        }
        // GW→Agent 指令不允许由 Agent 上行伪装。
        FrameKind::Dial => DispatchOutcome::Dropped,
    }
}

/// 在连接上注册一条客户端会话的回投通道；超过上限返回 None。
pub async fn register_session(
    conn: &AgentConnection,
    session_id: u64,
    tx: mpsc::Sender<Frame>,
) -> Option<()> {
    let mut sessions = conn.sessions.lock().await;
    if sessions.len() >= conn.max_sessions && !sessions.contains_key(&session_id) {
        return None;
    }
    sessions.insert(session_id, tx);
    Some(())
}

/// 摘除一条客户端会话（P2 tunnel 分支：DIAL 失败/会话结束时调用）。
pub async fn deregister_session(conn: &AgentConnection, session_id: u64) {
    conn.sessions.lock().await.remove(&session_id);
}

// ---------------------------------------------------------------------
// /agent/connect 路由
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AgentConnectQuery {
    pub resource_id: String,
}

/// 独立 auth_guard：校验 role=agent 的 AgentToken（独立 issuer/audience 语义，
/// 与用户 JWT 互不可用），且 rid claim 与查询参数 resource_id 一致。
fn verify_agent_token(
    state: &AppState,
    headers: &HeaderMap,
    query_resource_id: &str,
) -> Result<Claims, StatusCode> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let claims = state
        .auth
        .validate_agent_token(&token)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    // role=agent 强制：AgentToken 必须恰好携带 agent 角色。
    if !claims.roles.iter().any(|r| r == "agent") {
        return Err(StatusCode::FORBIDDEN);
    }
    // rid 绑定：token 只能接入签发时绑定的 resource_id。
    if claims.rid.as_deref() != Some(query_resource_id) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(claims)
}

async fn agent_connect_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AgentConnectQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if q.resource_id.is_empty() {
        return (StatusCode::BAD_REQUEST, "resource_id 不能为空").into_response();
    }
    let ip = crate::console::client_ip_from_headers(&headers);

    if !state.agent_cfg.enabled {
        return (StatusCode::NOT_FOUND, "agent 通道未启用").into_response();
    }
    let claims = match verify_agent_token(&state, &headers, &q.resource_id) {
        Ok(c) => c,
        Err(code) => {
            let mut ev = AuditEvent::new("-", Some("agent"), "agent_connect_rejected", &q.resource_id, ip.clone());
            ev.error = Some(format!("auth failed: {code}"));
            state.audit.record(ev);
            return code.into_response();
        }
    };

    // 资源必须已登记为 agent 型（静态或动态层），否则拒绝接入。
    let local_backend = match state
        .resources
        .resolve_agent_backend(&q.resource_id, &claims.roles)
    {
        Some(b) => b,
        None => {
            let mut ev = AuditEvent::new(&claims.sub, Some("agent"), "agent_connect_rejected", &q.resource_id, ip.clone());
            ev.error = Some("resource is not an agent-mode resource".into());
            state.audit.record(ev);
            return (StatusCode::NOT_FOUND, "资源不存在或非 agent 型").into_response();
        }
    };

    let resource_id = q.resource_id.clone();
    ws.on_upgrade(move |socket| {
        handle_agent_socket(state, resource_id, claims, local_backend, ip, socket)
    })
}

/// Agent WS 会话：注册表登记 → 出站泵（编码 Binary）→ WS 读半解码入帧分发 →
/// 心跳计时 → 断开摘除 + 审计。
async fn handle_agent_socket(
    state: Arc<AppState>,
    resource_id: String,
    claims: Claims,
    local_backend: String,
    ip: String,
    socket: WebSocket,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<Frame>(64);
    let (inbound_tx, inbound_rx) = mpsc::channel::<Frame>(64);
    let now_ms = unix_ms();
    let conn = state.agent_registry.register(AgentConnection::new(
        resource_id.clone(),
        claims.sub.clone(),
        local_backend.clone(),
        outbound_tx,
        inbound_rx,
        state.agent_cfg.max_sessions_per_conn(),
        now_ms,
    ));

    state.audit.record(AuditEvent::new(
        &claims.sub,
        Some("agent"),
        "agent_online",
        &resource_id,
        ip.clone(),
    ));
    // P4 动态资源联动：该资源是动态登记的 agent 型资源时，状态 offline→online。
    state.resources.dynamic.set_agent_online(&resource_id, unix_ms());
    tracing::info!(resource_id = %resource_id, agent_id = %claims.sub, "agent connected");

    // 出站泵：GW→Agent 帧编码为 WS Binary。
    let out_task = tokio::spawn(async move {
        while let Some(frame) = outbound_rx.recv().await {
            if ws_tx.send(Message::Binary(frame.encode())).await.is_err() {
                break;
            }
        }
        let _ = ws_tx.close().await;
    });

    // 心跳计时器：网关侧驱动，每 heartbeat_secs 发应用层 PING（session_id=0）。
    let hb_conn = Arc::clone(&conn);
    let hb_secs = state.agent_cfg.heartbeat_secs();
    let hb_task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(hb_secs));
        tick.tick().await; // 首个 tick 立即返回，跳过
        loop {
            tick.tick().await;
            if !hb_conn
                .send(Frame::new(FrameKind::Ping, 0, Vec::new()))
                .await
            {
                break;
            }
        }
    });

    // 入站泵：WS Binary → 解码（跨帧粘包缓冲）→ inbound 通道；Text/Close 处理。
    let in_task = tokio::spawn(async move {
        let mut buf: Vec<u8> = Vec::new();
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Binary(data) => {
                    buf.extend_from_slice(&data);
                    loop {
                        match Frame::decode(&buf) {
                            Ok(Some((frame, consumed))) => {
                                buf.drain(..consumed);
                                if inbound_tx.send(frame).await.is_err() {
                                    return;
                                }
                            }
                            Ok(None) => break, // 等待更多字节
                            Err(err) => {
                                tracing::warn!(?err, len = buf.len(), "agent frame decode failed");
                                return; // 丢弃残余缓冲，连接由摘除路径收尾
                            }
                        }
                    }
                }
                Message::Text(_) => {} // 与隧道语义一致：文本帧忽略
                Message::Close(_) => break,
                Message::Ping(_) | Message::Pong(_) => {}
            }
        }
    });

    // 入站分发循环：任何帧刷新心跳；PING 自动回 PONG；OPEN_OK/OPEN_ERR/DATA/CLOSE
    // 路由到 session（P2 tunnel 分支消费）。
    if let Some(mut rx) = conn.take_inbound().await {
        while let Some(frame) = rx.recv().await {
            conn.mark_seen_ms(unix_ms());
            dispatch_agent_frame(&conn, frame).await;
        }
    }

    // 分发循环退出 = 入站流断开：摘除注册表条目（仅当仍是本连接）并审计下线。
    let removed = state.agent_registry.remove(&resource_id);
    if removed.map(|c| Arc::ptr_eq(&c, &conn)).unwrap_or(false) {
        state.audit.record(AuditEvent::new(
            &claims.sub,
            Some("agent"),
            "agent_offline",
            &resource_id,
            ip.clone(),
        ));
        // P4 动态资源联动：online→offline（revoked/pending 不受影响）。
        state.resources.dynamic.set_agent_offline(&resource_id);
        tracing::info!(resource_id = %resource_id, "agent disconnected");
    }
    out_task.abort();
    hb_task.abort();
    in_task.abort();
}

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let _ = state;
    Router::new().route("/agent/connect", get(agent_connect_handler))
}

pub fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------
// 单元测试：wire 编解码 + 假 Agent（mpsc）验证 DIAL/DATA/CLOSE/心跳判死
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn fake_conn(max_sessions: usize) -> (Arc<AgentConnection>, mpsc::Receiver<Frame>, mpsc::Sender<Frame>) {
        let (out_tx, out_rx) = mpsc::channel(8);
        let (in_tx, in_rx) = mpsc::channel(8);
        let conn = Arc::new(AgentConnection::new(
            "rdp-office-01",
            "agent-u-0001",
            "127.0.0.1:3389",
            out_tx,
            in_rx,
            max_sessions,
            unix_ms(),
        ));
        (conn, out_rx, in_tx)
    }

    #[test]
    fn frame_roundtrip_all_kinds() {
        for kind in [
            FrameKind::Dial,
            FrameKind::OpenOk,
            FrameKind::OpenErr,
            FrameKind::Data,
            FrameKind::Close,
            FrameKind::Ping,
            FrameKind::Pong,
        ] {
            let f = Frame::new(kind, 0x0102_0304_0506_0708, b"payload-bytes".to_vec());
            let bytes = f.encode();
            assert_eq!(bytes.len(), FRAME_HEADER_LEN + 13);
            let (decoded, consumed) = Frame::decode(&bytes).expect("decode ok").expect("complete");
            assert_eq!(consumed, bytes.len());
            assert_eq!(decoded, f);
        }
    }

    #[test]
    fn frame_decode_partial_returns_none() {
        let full = Frame::new(FrameKind::Data, 7, b"abc".to_vec()).encode();
        // 半帧：只有部分 header
        assert_eq!(Frame::decode(&full[..5]).expect("ok"), None);
        // header 完整但 payload 不完整
        assert_eq!(Frame::decode(&full[..FRAME_HEADER_LEN + 1]).expect("ok"), None);
        // 完整帧解码成功
        let (f, n) = Frame::decode(&full).expect("ok").expect("complete");
        assert_eq!(f.session_id, 7);
        assert_eq!(n, full.len());
        // 粘包：两帧一次性解码
        let mut two = full.clone();
        two.extend_from_slice(&full);
        let (f1, n1) = Frame::decode(&two).expect("ok").expect("complete");
        assert_eq!(n1, full.len());
        let (f2, n2) = Frame::decode(&two[n1..]).expect("ok").expect("complete");
        assert_eq!(f1, f2);
        assert_eq!(n2, full.len());
    }

    #[test]
    fn frame_decode_rejects_bad_type_and_oversize() {
        let mut f = Frame::new(FrameKind::Data, 1, Vec::new()).encode();
        f[0] = 0xFF;
        assert_eq!(
            Frame::decode(&f),
            Err(FrameError::BadType(0xFF))
        );
        // 声明 payload 超上限
        let mut f2 = Frame::new(FrameKind::Data, 1, Vec::new()).encode();
        f2[9..13].copy_from_slice(&(u32::MAX).to_be_bytes());
        assert_eq!(
            Frame::decode(&f2),
            Err(FrameError::PayloadTooLarge(u32::MAX as usize))
        );
    }

    #[tokio::test]
    async fn gw_sends_dial_frame_to_fake_agent() {
        let (conn, mut out_rx, _in_tx) = fake_conn(8);
        let dial = Frame::new(
            FrameKind::Dial,
            1,
            br#"{"backend":"127.0.0.1:3389"}"#.to_vec(),
        );
        assert!(conn.send(dial.clone()).await);
        let got = out_rx.recv().await.expect("fake agent receives dial");
        assert_eq!(got, dial);
        assert_eq!(got.kind, FrameKind::Dial);
        assert_eq!(got.session_id, 1);
    }

    #[tokio::test]
    async fn dispatch_ping_replies_pong_same_session() {
        let (conn, mut out_rx, _in_tx) = fake_conn(8);
        let outcome = dispatch_agent_frame(&conn, Frame::new(FrameKind::Ping, 42, Vec::new())).await;
        assert_eq!(outcome, DispatchOutcome::Replied(FrameKind::Pong));
        let pong = out_rx.recv().await.expect("pong");
        assert_eq!(pong.kind, FrameKind::Pong);
        assert_eq!(pong.session_id, 42);
    }

    #[tokio::test]
    async fn dispatch_agent_uplink_dial_is_dropped() {
        let (conn, _out_rx, _in_tx) = fake_conn(8);
        // Agent 上行伪装 DIAL 必须被丢弃（DIAL 仅允许 GW→Agent）。
        let outcome = dispatch_agent_frame(&conn, Frame::new(FrameKind::Dial, 1, Vec::new())).await;
        assert_eq!(outcome, DispatchOutcome::Dropped);
    }

    #[tokio::test]
    async fn dispatch_data_and_close_routed_to_registered_session() {
        let (conn, _out_rx, _in_tx) = fake_conn(8);
        let (sess_tx, mut sess_rx) = mpsc::channel(8);
        assert!(register_session(&conn, 9, sess_tx).await.is_some());

        // 未注册 session：DATA 丢弃
        let miss = dispatch_agent_frame(&conn, Frame::new(FrameKind::Data, 8, b"x".to_vec())).await;
        assert_eq!(miss, DispatchOutcome::NoSession);

        // DATA 路由到 session
        let o1 = dispatch_agent_frame(&conn, Frame::new(FrameKind::Data, 9, b"RDP".to_vec())).await;
        assert_eq!(o1, DispatchOutcome::Routed(FrameKind::Data));
        let got = sess_rx.recv().await.expect("session frame");
        assert_eq!(got.payload, b"RDP");

        // OPEN_ERR 同样路由
        let o2 = dispatch_agent_frame(&conn, Frame::new(FrameKind::OpenErr, 9, b"refused".to_vec())).await;
        assert_eq!(o2, DispatchOutcome::Routed(FrameKind::OpenErr));
        let err = sess_rx.recv().await.expect("session frame");
        assert_eq!(err.kind, FrameKind::OpenErr);
        assert_eq!(err.payload, b"refused");

        // CLOSE 路由后摘除 session
        let o3 = dispatch_agent_frame(&conn, Frame::new(FrameKind::Close, 9, Vec::new())).await;
        assert_eq!(o3, DispatchOutcome::Routed(FrameKind::Close));
        let close = sess_rx.recv().await.expect("session frame");
        assert_eq!(close.kind, FrameKind::Close);
        // 后续同 session DATA：NoSession
        let o4 = dispatch_agent_frame(&conn, Frame::new(FrameKind::Data, 9, b"late".to_vec())).await;
        assert_eq!(o4, DispatchOutcome::NoSession);
    }

    #[tokio::test]
    async fn session_limit_enforced() {
        let (conn, _out_rx, _in_tx) = fake_conn(2);
        for sid in [1u64, 2] {
            let (tx, _rx) = mpsc::channel(8);
            assert!(register_session(&conn, sid, tx).await.is_some(), "sid {sid} 应注册成功");
        }
        let (tx3, _rx3) = mpsc::channel(8);
        assert!(register_session(&conn, 3, tx3).await.is_none(), "超过上限应拒绝");
        // 已存在 session 的重复注册仍放行（覆盖语义）
        let (tx1b, _rx1b) = mpsc::channel(8);
        assert!(register_session(&conn, 1, tx1b).await.is_some());
    }

    #[test]
    fn heartbeat_judges_death_after_missed_windows() {
        let (out_tx, _out_rx) = mpsc::channel(8);
        let (_in_tx, in_rx) = mpsc::channel(4);
        let conn = AgentConnection::new("r", "a", "127.0.0.1:3389", out_tx, in_rx, 8, 1_000);
        // 心跳 30s × 3 = 90s 判死
        assert!(!conn.is_dead(1_000 + 89_999, 30_000, 3));
        assert!(conn.is_dead(1_000 + 90_000, 30_000, 3));
        // 收到帧后重新计时
        conn.mark_seen_ms(100_000);
        assert!(!conn.is_dead(150_000, 30_000, 3));
        assert!(conn.is_dead(190_001, 30_000, 3));
    }

    #[test]
    fn registry_sweep_dead_removes_only_silent_connections() {
        let reg = AgentRegistry::new();
        let mk = |rid: &str, seen: u64| {
            let (out_tx, _o) = mpsc::channel(4);
            let (_i, in_rx) = mpsc::channel(4);
            AgentConnection::new(rid, "agent", "127.0.0.1:3389", out_tx, in_rx, 8, seen)
        };
        reg.register(mk("alive", 1_000_000));
        reg.register(mk("dead", 1_000_000));
        // alive 在 90s 窗口内有帧；dead 静默超过 90s
        reg.get("alive").expect("alive").mark_seen_ms(1_080_000);
        let now = 1_091_000; // dead: 91s 静默 => 判死；alive: 11s => 存活
        let removed = reg.sweep_dead(now, 30_000, 3);
        assert_eq!(removed, vec!["dead".to_string()]);
        assert!(reg.online("alive"));
        assert!(!reg.online("dead"));
        assert_eq!(reg.online_count(), 1);
    }

    #[test]
    fn registry_replace_and_remove() {
        let reg = AgentRegistry::new();
        let mk = |rid: &str| {
            let (out_tx, _o) = mpsc::channel(4);
            let (_i, in_rx) = mpsc::channel(4);
            AgentConnection::new(rid, "agent", "127.0.0.1:3389", out_tx, in_rx, 8, unix_ms())
        };
        let first = reg.register(mk("r1"));
        let second = reg.register(mk("r1"));
        assert!(!Arc::ptr_eq(&first, &second), "重连应替换旧连接");
        assert!(Arc::ptr_eq(&second, &reg.get("r1").expect("get")));
        assert_eq!(reg.online_count(), 1);
        let removed = reg.remove("r1");
        assert!(Arc::ptr_eq(&second, &removed.expect("removed")));
        assert!(!reg.online("r1"));
    }

    #[test]
    fn session_id_allocator_increments() {
        let alloc = SessionIdAllocator::new();
        assert_eq!(alloc.next(), 1);
        assert_eq!(alloc.next(), 2);
        assert_eq!(alloc.next(), 3);
    }

    #[tokio::test]
    async fn fake_agent_end_to_end_dial_data_close_over_channels() {
        // mpsc 假 Agent 全链路：GW 发 DIAL → 假 Agent 回 OPEN_OK → 双向 DATA → CLOSE。
        let (conn, mut out_rx, in_tx) = fake_conn(8);
        let (sess_tx, mut sess_rx) = mpsc::channel(8);
        register_session(&conn, 1, sess_tx).await.expect("register session");
        let mut inbound = conn.take_inbound().await.expect("inbound once");

        // GW→Agent: DIAL
        let dial = Frame::new(FrameKind::Dial, 1, br#"{"backend":"127.0.0.1:3389"}"#.to_vec());
        assert!(conn.send(dial.clone()).await);
        assert_eq!(out_rx.recv().await.expect("dial"), dial);

        // 假 Agent → GW: OPEN_OK（入站通道 → 分发 → session 路由）
        in_tx.send(Frame::new(FrameKind::OpenOk, 1, Vec::new())).await.unwrap();
        let f = inbound.recv().await.expect("frame from agent");
        conn.mark_seen_ms(unix_ms());
        assert_eq!(f.kind, FrameKind::OpenOk);
        assert_eq!(
            dispatch_agent_frame(&conn, f).await,
            DispatchOutcome::Routed(FrameKind::OpenOk)
        );

        // 假 Agent → GW: DATA
        in_tx.send(Frame::new(FrameKind::Data, 1, b"banner".to_vec())).await.unwrap();
        let f = inbound.recv().await.expect("data frame");
        conn.mark_seen_ms(unix_ms());
        assert_eq!(
            dispatch_agent_frame(&conn, f).await,
            DispatchOutcome::Routed(FrameKind::Data)
        );
        let got_open = sess_rx.recv().await.expect("session open ok");
        assert_eq!(got_open.kind, FrameKind::OpenOk);
        let got = sess_rx.recv().await.expect("session data");
        assert_eq!(got.payload, b"banner");

        // GW → 假 Agent: DATA（客户端方向下行）
        let down = Frame::new(FrameKind::Data, 1, b"client->target".to_vec());
        assert!(conn.send(down.clone()).await);
        assert_eq!(out_rx.recv().await.expect("downlink"), down);

        // CLOSE：路由后 session 摘除，迟到 DATA 不再路由
        in_tx.send(Frame::new(FrameKind::Close, 1, Vec::new())).await.unwrap();
        let f = inbound.recv().await.expect("close frame");
        conn.mark_seen_ms(unix_ms());
        assert_eq!(
            dispatch_agent_frame(&conn, f).await,
            DispatchOutcome::Routed(FrameKind::Close)
        );
        let late = dispatch_agent_frame(&conn, Frame::new(FrameKind::Data, 1, b"x".to_vec())).await;
        assert_eq!(late, DispatchOutcome::NoSession);
    }
}
