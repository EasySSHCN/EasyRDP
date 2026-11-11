//! EasyRDP 云网关入口。
//!
//! 职责：加载配置、初始化审计环形缓冲、挂载路由并启动 axum HTTP/WS 服务。
//! 网关本体默认监听 0.0.0.0:8443，由 Nginx 反代终止 TLS（443 -> 8443）。

mod audit;
mod auth;
mod policy;
mod tunnel;

use std::net::SocketAddr;

use axum::Router;
use tracing_subscriber::EnvFilter;

/// 网关共享状态：认证状态 + 审计缓冲。
pub struct AppState {
    pub auth: auth::AuthState,
    pub audit: audit::AuditLog,
}

#[tokio::main]
async fn main() {
    // 日志：支持 RUST_LOG 环境变量覆盖，默认 info。
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // 配置加载占位：骨架阶段读取环境变量/默认值；后续接入 config.toml 解析（toml crate）。
    let jwt_secret =
        std::env::var("EASYRDP_JWT_SECRET").unwrap_or_else(|_| "dev-secret-do-not-use".to_string());
    let listen_addr = std::env::var("EASYRDP_LISTEN")
        .unwrap_or_else(|_| "0.0.0.0:8443".to_string());

    let state = std::sync::Arc::new(AppState {
        auth: auth::AuthState::new(jwt_secret, 600),
        audit: audit::AuditLog::new(4096),
    });

    let app = Router::new()
        .merge(auth::router(std::sync::Arc::clone(&state)))
        .merge(tunnel::router(std::sync::Arc::clone(&state)))
        .merge(audit::router(std::sync::Arc::clone(&state)));

    let addr: SocketAddr = listen_addr
        .parse()
        .expect("EASYRDP_LISTEN 必须是合法的 SocketAddr，例如 0.0.0.0:8443");

    tracing::info!(%addr, "EasyRDP gateway listening");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("监听端口失败");
    axum::serve(listener, app)
        .await
        .expect("HTTP 服务异常退出");
}
