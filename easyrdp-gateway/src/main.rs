//! EasyRDP 云网关入口。
//!
//! 职责：加载 config.toml（server / auth / audit / [[resources]]，缺失时回退
//! 环境变量与默认值）、初始化资源注册表与审计环形缓冲、挂载路由并启动
//! axum HTTP/WS 服务。网关本体默认监听 0.0.0.0:8443，由 Nginx 反代终止 TLS。

mod audit;
mod auth;
mod config;
mod console;
mod policy;
mod resources;
mod tunnel;
/// F4 端到端集成测试（仅 #[cfg(test)] 编译）：本地 gateway + 客户端桥接
/// 模块（easyrdp-rdp::gateway_bridge）+ TCP echo 后端全链路字节流校验。
#[cfg(test)]
mod tests_e2e;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tracing_subscriber::EnvFilter;

/// 网关共享状态：认证状态 + 审计缓冲 + 资源注册表 + 管理密钥 + 策略矩阵。
pub struct AppState {
    pub auth: auth::AuthState,
    pub audit: audit::AuditLog,
    pub resources: resources::ResourceRegistry,
    /// 管理接口共享密钥（None = 管理端点禁用）
    pub admin_key: Option<Arc<String>>,
    /// 策略矩阵（F5-V1：可热加载；缺省 deny by default、admin 全通）
    pub policy: policy::PolicyHandle,
    /// F4-V1：用户 -> TOTP Base32 密钥（空表 = 无用户要求 OTP 二次认证）
    pub totp_secrets: std::collections::BTreeMap<String, String>,
    /// F5-V2：OIDC 运行态（None = 未配置 [oidc]，占位登录保持可用）
    pub oidc: Option<Arc<auth::OidcState>>,
}

/// 策略热加载轮询间隔（Windows 无 SIGHUP，统一用 mtime 轮询）。
const POLICY_RELOAD_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// 后台热加载任务：每 30s 检查 config.toml mtime，变更时重载 [policy] 段。
/// 成功记录 policy_reload 审计；失败保留旧策略并记录 warn 审计事件。
async fn policy_reload_task(config_path: String, state: Arc<AppState>) {
    let mut last_mtime: Option<std::time::SystemTime> =
        std::fs::metadata(&config_path).ok().and_then(|m| m.modified().ok());
    loop {
        tokio::time::sleep(POLICY_RELOAD_INTERVAL).await;
        let mtime = std::fs::metadata(&config_path)
            .ok()
            .and_then(|m| m.modified().ok());
        if mtime == last_mtime {
            continue;
        }
        last_mtime = mtime;
        match state.policy.reload_from_file(&config_path) {
            Ok(_) => {
                tracing::info!("policy matrix hot-reloaded");
                state.audit.record(audit::AuditEvent::new(
                    "-",
                    None,
                    "policy_reload",
                    "config",
                    "-",
                ));
            }
            Err(err) => {
                tracing::warn!(%err, "policy hot-reload failed, keeping previous matrix");
                let mut ev = audit::AuditEvent::new("-", None, "policy_reload_failed", "config", "-");
                ev.error = Some(err);
                state.audit.record(ev);
            }
        }
    }
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
    // 管理密钥：config.toml [admin].admin_key 优先，缺失时回退环境变量；
    // 均未配置则 /admin/* 管理端点保持禁用（返回 503）。
    let admin_key = cfg
        .admin
        .admin_key
        .or_else(|| std::env::var("EASYRDP_ADMIN_KEY").ok())
        .filter(|k| !k.trim().is_empty())
        .map(|k| std::sync::Arc::new(k));

    let registry =
        resources::ResourceRegistry::from_configs(&cfg.resources);
    tracing::info!(count = cfg.resources.len(), "resource registry loaded");

    // 策略矩阵：config.toml [policy] 段编译；解析失败回退默认矩阵
    // （deny by default、admin 全通）并记录 warn 审计事件。
    let policy = match policy::PolicyMatrix::from_config(&cfg.policy) {
        Ok(m) => policy::PolicyHandle::from_matrix(m),
        Err(err) => {
            // 启动期坏 [policy] 段：回退默认矩阵（deny by default、admin 全通），
            // 记 warn 日志；审计缓冲尚未构造，启动后由热加载路径补记审计。
            tracing::warn!(%err, "policy matrix parse failed, using default matrix");
            policy::PolicyHandle::default()
        }
    };

    // F4-V1：TOTP 用户表。config.toml [users.<user>].totp_secret 优先，
    // 环境变量 EASYRDP_TOTP_SECRET_<USER大写>（非字母数字 -> _）补充/覆盖。
    let mut totp_secrets = std::collections::BTreeMap::new();
    for (user, uc) in &cfg.users {
        if let Some(s) = uc.totp_secret.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            totp_secrets.insert(user.clone(), s.to_string());
        }
    }
    for (key, val) in std::env::vars() {
        if let Some(user) = key
            .strip_prefix("EASYRDP_TOTP_SECRET_")
            .filter(|k| !k.trim().is_empty())
        {
            totp_secrets.insert(user.to_string(), val);
        }
    }
    tracing::info!(users = totp_secrets.len(), "totp second-factor users loaded");

    // F5-V2：[oidc] 段 —— 配置齐全才启用真实 OIDC（Authorization Code 流），
    // 否则保持占位登录向后兼容。
    let oidc = if cfg.oidc.enabled() {
        tracing::info!(
            issuer = cfg.oidc.issuer.as_deref().unwrap_or(""),
            "OIDC enabled (authorization code flow); placeholder login {}",
            if cfg.oidc.allow_placeholder_login.unwrap_or(false) {
                "explicitly allowed"
            } else {
                "disabled"
            }
        );
        Some(Arc::new(auth::OidcState::new(
            cfg.oidc.clone(),
            Arc::new(auth::HttpIdpClient::new()),
        )))
    } else {
        tracing::info!("OIDC not configured, placeholder login stays available");
        None
    };

    let state = std::sync::Arc::new(AppState {
        auth: auth::AuthState::new(jwt_secret, ttl)
            // P0 安全闸（R-20261120-008）：[auth] allow_placeholder_login，
            // 缺省 true 向后兼容；生产环境配置文件必须置 false。
            .with_placeholder_allowed(cfg.auth.allow_placeholder_login.unwrap_or(true)),
        audit: audit::AuditLog::new(capacity),
        resources: registry,
        admin_key,
        policy,
        totp_secrets,
        oidc,
    });

    let app = Router::new()
        .merge(auth::router(std::sync::Arc::clone(&state)))
        .merge(console::router(std::sync::Arc::clone(&state)))
        .merge(tunnel::router(std::sync::Arc::clone(&state)))
        .merge(audit::router(std::sync::Arc::clone(&state)))
        .merge(audit::admin_router(std::sync::Arc::clone(&state)))
        .with_state(state.clone());

    // 策略热加载轮询任务（F5-V1）
    tokio::spawn(policy_reload_task(config_path, state));

    let addr: SocketAddr = listen_addr
        .parse()
        .expect("listen_addr 必须是合法的 SocketAddr，例如 0.0.0.0:8443");

    tracing::info!(%addr, "EasyRDP gateway listening");
    // 注入 ConnectInfo<SocketAddr>，隧道 handler 需要客户端来源 IP 用于审计。
    // F4-V1：[tls] 配置 cert_path/key_path → axum-server rustls 直跑 HTTPS/WSS；
    // 未配置 → 保持 HTTP（Nginx 反代终止 TLS 的既有部署形态）。
    let make_svc = app.into_make_service_with_connect_info::<SocketAddr>();
    if cfg.tls.enabled() {
        let cert_path = cfg.tls.cert_path.clone().unwrap();
        let key_path = cfg.tls.key_path.clone().unwrap();
        // ring provider（见 Cargo.toml 注释：tls-rustls-no-provider + 显式安装）
        let _ = rustls::crypto::ring::default_provider().install_default();
        let tls_cfg = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path)
            .await
            .unwrap_or_else(|e| {
                panic!("TLS 证书加载失败 (cert={cert_path}, key={key_path}): {e}")
            });
        tracing::info!("TLS enabled (HTTPS/WSS), cert={cert_path}");
        axum_server::bind_rustls(addr, tls_cfg)
            .serve(make_svc)
            .await
            .expect("HTTPS 服务异常退出");
    } else {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("监听端口失败");
        axum::serve(listener, make_svc)
            .await
            .expect("HTTP 服务异常退出");
    }
}
