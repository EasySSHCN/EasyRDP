//! EasyRDP 云网关入口。
//!
//! 职责：加载 config.toml（server / auth / audit / [[resources]]，缺失时回退
//! 环境变量与默认值）、初始化资源注册表与审计环形缓冲、挂载路由并启动
//! axum HTTP/WS 服务。网关本体默认监听 0.0.0.0:8443，由 Nginx 反代终止 TLS。

mod audit;
mod auth;
mod config;
mod policy;
mod resources;
mod tunnel;

use std::net::SocketAddr;

use axum::Router;
use tracing_subscriber::EnvFilter;

/// 网关共享状态：认证状态 + 审计缓冲 + 资源注册表。
pub struct AppState {
    pub auth: auth::AuthState,
    pub audit: audit::AuditLog,
    pub resources: resources::ResourceRegistry,
}

#[tokio::main]
async fn main() {
    // 日志：支持 RUST_LOG 环境变量覆盖，默认 info。
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // 配置：优先 config.toml（EASYRDP_CONFIG 指定路径，缺省 ./config.toml），
    // 文件不存在则使用环境变量/默认值（保持与骨架阶段兼容）。
    let config_path =
        std::env::var("EASYRDP_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
    let cfg = config::GatewayConfig::load_from_path(&config_path).unwrap_or_default();

    let jwt_secret = cfg
        .auth
        .jwt_secret
        .or_else(|| std::env::var("EASYRDP_JWT_SECRET").ok())
        .unwrap_or_else(|| "dev-secret-do-not-use".to_string());
    let listen_addr = cfg
        .server
        .listen_addr
        .or_else(|| std::env::var("EASYRDP_LISTEN").ok())
        .unwrap_or_else(|| "0.0.0.0:8443".to_string());
    let ttl = cfg.auth.token_ttl_seconds.unwrap_or(auth::DEFAULT_TOKEN_TTL);
    let capacity = cfg.audit.ring_capacity.unwrap_or(4096).max(1);

    let registry =
        resources::ResourceRegistry::from_configs(&cfg.resources);
    tracing::info!(count = cfg.resources.len(), "resource registry loaded");

    let state = std::sync::Arc::new(AppState {
        auth: auth::AuthState::new(jwt_secret, ttl),
        audit: audit::AuditLog::new(capacity),
        resources: registry,
    });

    let app = Router::new()
        .merge(auth::router(std::sync::Arc::clone(&state)))
        .merge(tunnel::router(std::sync::Arc::clone(&state)))
        .merge(audit::router(std::sync::Arc::clone(&state)))
        .with_state(state);

    let addr: SocketAddr = listen_addr
        .parse()
        .expect("listen_addr 必须是合法的 SocketAddr，例如 0.0.0.0:8443");

    tracing::info!(%addr, "EasyRDP gateway listening");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("监听端口失败");
    // 注入 ConnectInfo<SocketAddr>，隧道 handler 需要客户端来源 IP 用于审计。
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("HTTP 服务异常退出");
}
