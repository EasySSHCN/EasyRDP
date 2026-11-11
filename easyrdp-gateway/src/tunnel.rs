//! 隧道模块：`GET /tunnel/{resource_id}` WebSocket 升级 handler 骨架。
//!
//! 流程：auth_guard 校验短期 JWT -> 记录审计事件（隧道建立/关闭）->
//! 升级为 WebSocket -> 进入占位双向循环（echo）。
//! TODO(后续阶段)：将 echo 占位替换为到内网 RDP 主机的 TCP 转发
//! （tokio::net::TcpStream + fork 循环，二进制帧透传）。

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Path, State,
    },
    http::StatusCode,
    middleware::from_fn_with_state,
    response::IntoResponse,
    routing::get,
    Router,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::info;

use crate::{audit::AuditEvent, auth::auth_guard, AppState};

async fn tunnel_handler(
    State(state): State<Arc<AppState>>,
    Path(resource_id): Path<String>,
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    // token 已由 auth_guard 校验并通过扩展注入；这里读取用于审计与策略。
    // 注意：axum 在 from_fn 中注入的 extensions 会随请求传入本 handler。
    // 真实实现应在此处继续执行 policy::check_redirect 决定是否放行目标资源。
    // （骨架阶段：记录审计并直接升级。）

    if resource_id.is_empty() {
        return (StatusCode::BAD_REQUEST, "resource_id 不能为空").into_response();
    }

    ws.on_upgrade(move |socket| {
        handle_tunnel_socket(state, resource_id, peer, socket)
    })
}

/// 占位双向循环：echo。
/// TODO(后续阶段)：替换为
///   1. 依据 resource_id 解析内网 RDP 主机地址（来自策略/资源目录）；
///   2. tokio::net::TcpStream::connect；
///   3. WebSocket 与 TCP 之间 fork 双向字节转发。
async fn handle_tunnel_socket(
    state: Arc<AppState>,
    resource_id: String,
    peer: SocketAddr,
    mut socket: WebSocket,
) {
    info!(%peer, %resource_id, "tunnel established (placeholder echo)");

    state.audit.record(AuditEvent {
        timestamp: now_unix(),
        user: "tunnel-client".to_string(), // TODO: 从 auth_guard 注入的 Claims.sub 取值
        event: "tunnel_open".to_string(),
        resource: resource_id.clone(),
        source_ip: peer.ip().to_string(),
    });

    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(text) => {
                // 占位：文本帧回显；真实隧道只透传二进制 RDP 流量。
                if socket.send(Message::Text(format!("echo: {text}"))).await.is_err() {
                    break;
                }
            }
            Message::Binary(data) => {
                // TODO: 转发到内网 RDP 主机；占位阶段原样回传。
                if socket.send(Message::Binary(data)).await.is_err() {
                    break;
                }
            }
            Message::Ping(p) => {
                let _ = socket.send(Message::Pong(p)).await;
            }
            Message::Close(_) => break,
            Message::Pong(_) => {}
        }
    }

    state.audit.record(AuditEvent {
        timestamp: now_unix(),
        user: "tunnel-client".to_string(),
        event: "tunnel_close".to_string(),
        resource: resource_id,
        source_ip: peer.ip().to_string(),
    });
    info!(%peer, "tunnel closed");
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // 隧道路由挂 auth_guard：Bearer 校验失败即 401，WebSocket 升级不会发生。
    Router::new()
        .route(
            "/tunnel/{resource_id}",
            get(tunnel_handler),
        )
        .route_layer(from_fn_with_state(Arc::clone(&state), auth_guard))
        .with_state(state)
}
