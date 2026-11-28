//! 配置模块：config.toml 解析（server / auth / audit / resources）。
//!
//! `GatewayConfig::parse` 从 TOML 字符串解析（便于单测，不写临时文件）；
//! `load_from_path` 读文件后复用同一解析入口，文件缺失时返回 None
//! （main 使用默认值 / 环境变量回退）。

use serde::Deserialize;
use std::path::Path;

use crate::policy::PolicyConfig;
use crate::resources::ResourceConfig;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GatewayConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub audit: AuditConfig,
    /// 管理接口配置（ADMIN-AUDIT-20261118：站点后台代理拉取审计用管理密钥）
    #[serde(default)]
    pub admin: AdminConfig,
    /// 策略矩阵（F5-V1：资源 × 角色 × 动作 允许矩阵；缺省 = deny by default、admin 全通）
    #[serde(default)]
    pub policy: PolicyConfig,
    /// F4-V1（R-20261118-012）：TLS 配置（cert_path/key_path 均配置时网关
    /// 直跑 HTTPS/WSS；任一缺失保持 HTTP，由反代终止 TLS）
    #[serde(default)]
    pub tls: TlsConfig,
    /// F4-V1：用户表（[users.<username>] totp_secret = Base32 密钥；配置了
    /// totp_secret 的用户登录要求 OTP 二次认证）。密钥也可用环境变量
    /// EASYRDP_TOTP_SECRET_<USER大写> 提供/覆盖。
    #[serde(default)]
    pub users: std::collections::BTreeMap<String, UserConfig>,
    /// F5-V2（R-20261118-018）：[oidc] 段 —— 真实 IdP 对接（Authorization Code
    /// 流）。未配置或字段不全时 `enabled() == false`，保留占位登录（向后兼容）。
    #[serde(default)]
    pub oidc: OidcConfig,
    /// R-20261122-006：[agent] 段 —— 目标机 Agent 反向隧道通道。
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub resources: Vec<ResourceConfig>,
}

/// R-20261122-006：[agent] 段配置。
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    /// Agent 通道总开关（缺省 false；启用后挂载 GET /agent/connect）。
    #[serde(default)]
    pub enabled: bool,
    /// 单 Agent 连接并发 session 上限（默认 8）。
    #[serde(default)]
    pub max_sessions_per_conn: Option<usize>,
    /// 应用层心跳间隔秒（默认 30；连续 3 次未收到判死）。
    #[serde(default)]
    pub heartbeat_secs: Option<u64>,
    /// DIAL 后等待 OPEN_OK 的超时秒（默认 10）。
    #[serde(default)]
    pub dial_timeout_secs: Option<u64>,
    /// 每用户动态资源配额（默认 3；仅限登记台数）。
    #[serde(default)]
    pub max_resources_per_user: Option<usize>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_sessions_per_conn: None,
            heartbeat_secs: None,
            dial_timeout_secs: None,
            max_resources_per_user: None,
        }
    }
}

impl AgentConfig {
    pub fn max_sessions_per_conn(&self) -> usize {
        self.max_sessions_per_conn.unwrap_or(8).max(1)
    }

    pub fn heartbeat_secs(&self) -> u64 {
        self.heartbeat_secs.unwrap_or(30).max(1)
    }

    pub fn dial_timeout_secs(&self) -> u64 {
        self.dial_timeout_secs.unwrap_or(10).max(1)
    }

    pub fn max_resources_per_user(&self) -> usize {
        self.max_resources_per_user.unwrap_or(3).max(1)
    }

    /// 心跳判死静默窗口毫秒（heartbeat × 3）。
    pub fn heartbeat_deadline_ms(&self) -> u64 {
        self.heartbeat_secs().saturating_mul(3) * 1000
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ServerConfig {
    #[serde(default)]
    pub listen_addr: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AuthConfig {
    #[serde(default)]
    pub jwt_secret: Option<String>,
    #[serde(default)]
    pub token_ttl_seconds: Option<u64>,
    /// P0 安全开关（R-20261120-008）：占位登录总闸。设为 false 时**即使未
    /// 配置 [oidc]** 占位登录也强制禁用（生产环境必须 false）。缺省 true =
    /// 向后兼容（未配置 [oidc] 时占位登录保持可用，骨架期行为不变）。
    #[serde(default)]
    pub allow_placeholder_login: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AuditConfig {
    #[serde(default)]
    pub ring_capacity: Option<usize>,
}

/// 管理接口配置。`admin_key` 为受保护管理端点（GET /admin/audit）的共享密钥；
/// 生产环境请使用 >= 32 字节随机值（如 openssl rand -hex 32），绝不入库。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AdminConfig {
    #[serde(default)]
    pub admin_key: Option<String>,
}

/// F4-V1：[tls] 段。cert_path 与 key_path 同时配置 → axum-server rustls
/// 直跑 HTTPS/WSS；未配置（缺省）→ HTTP（Nginx 反代终止 TLS 的既有形态）。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TlsConfig {
    #[serde(default)]
    pub cert_path: Option<String>,
    #[serde(default)]
    pub key_path: Option<String>,
}

impl TlsConfig {
    /// 是否启用网关本体 TLS（两个路径都非空才启用）。
    pub fn enabled(&self) -> bool {
        self.cert_path.as_deref().map_or(false, |s| !s.trim().is_empty())
            && self.key_path.as_deref().map_or(false, |s| !s.trim().is_empty())
    }
}

/// F4-V1：[users.<username>] 条目。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct UserConfig {
    /// Base32（RFC 4648）TOTP 密钥；配置后该用户登录要求 OTP 二次认证。
    #[serde(default)]
    pub totp_secret: Option<String>,
}

/// F5-V2：[oidc] 段 —— OIDC Authorization Code 流对接真实 IdP。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct OidcConfig {
    /// IdP issuer（与 id_token `iss` claim 精确匹配）。
    #[serde(default)]
    pub issuer: Option<String>,
    /// OIDC discovery 文档 URL（缺省 = `{issuer}/.well-known/openid-configuration`）。
    #[serde(default)]
    pub discovery_url: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_secret: Option<String>,
    /// 回调地址，须先在 IdP 侧登记（如 https://gw.example.com/oidc/callback）。
    #[serde(default)]
    pub redirect_uri: Option<String>,
    /// scope 空格分隔（缺省 "openid profile email"）。
    #[serde(default)]
    pub scopes: Option<String>,
    /// id_token 中角色来源 claim 名（缺省 "roles"；为空时回退 "groups"，
    /// 再回退 ["user"]）。
    #[serde(default)]
    pub roles_claim: Option<String>,
    /// 占位登录开关：配置 [oidc] 后占位登录默认禁用；显式设 true 才保留
    /// （仅限联调）。未配置 [oidc] 时占位登录始终可用（向后兼容）。
    #[serde(default)]
    pub allow_placeholder_login: Option<bool>,
}

impl OidcConfig {
    /// 四个必填字段（issuer/client_id/client_secret/redirect_uri）都非空才启用。
    pub fn enabled(&self) -> bool {
        [self.issuer.as_deref(), self.client_id.as_deref(), self.client_secret.as_deref(), self.redirect_uri.as_deref()]
            .iter()
            .all(|f| f.map_or(false, |s| !s.trim().is_empty()))
    }

    pub fn effective_scopes(&self) -> &str {
        self.scopes.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or("openid profile email")
    }

    pub fn effective_roles_claim(&self) -> &str {
        self.roles_claim.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or("roles")
    }
}

impl GatewayConfig {
    pub fn parse(toml_text: &str) -> Result<Self, String> {
        toml::from_str(toml_text).map_err(|e| format!("config.toml 解析失败: {e}"))
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        Self::parse(&text).ok()
    }
}

// ---------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[server]
listen_addr = "0.0.0.0:8443"

[auth]
jwt_secret = "s3cret"
token_ttl_seconds = 300
# P0 安全开关：生产环境强制 false（未配置 [oidc] 也禁用占位登录）
allow_placeholder_login = false

[audit]
ring_capacity = 1024

[admin]
admin_key = "REPLACE_WITH_RANDOM_ADMIN_KEY"

[tls]
# 网关本体 TLS（F4-V1）：同时配置 cert/key 则 axum-server rustls 直跑 HTTPS/WSS；
# 任一留空/缺省 = HTTP（由 Nginx 反代终止 TLS 的既有部署形态）。
cert_path = "/etc/easyrdp/tls/server.crt"
key_path = "/etc/easyrdp/tls/server.key"

[users.alice]
# TOTP 二次认证（F4-V1）：Base32 密钥；配置后 alice 登录必须携带 mfa_code。
totp_secret = "JBSWY3DPEHPK3PXP"

[users.bob]
# 未配置 totp_secret => bob 登录无需 OTP。

[policy]

[[policy.rules]]
resource = "*"
role = "user"
action = "connect"
decision = "allow"

[[policy.rules]]
resource = "*"
role = "readonly"
action = "list"
decision = "allow"

[[resources]]
id = "rdp-win01"
name = "Windows 跳板"
backend = "10.0.0.5:3389"
allowed_roles = ["ops", "admin"]

[[resources]]
id = "rdp-linux01"
name = "Linux 桌面"
backend = "192.168.1.20:3390"

[oidc]
issuer = "https://sso.example.com/realms/easyrdp"
client_id = "easyrdp-gateway"
client_secret = "topsecret"
redirect_uri = "https://gw.example.com/oidc/callback"
scopes = "openid profile email roles"
roles_claim = "roles"
allow_placeholder_login = false
"#;

    #[test]
    fn parse_full_config_with_resources() {
        let cfg = GatewayConfig::parse(SAMPLE).expect("parse");
        assert_eq!(cfg.server.listen_addr.as_deref(), Some("0.0.0.0:8443"));
        assert_eq!(cfg.auth.jwt_secret.as_deref(), Some("s3cret"));
        assert_eq!(cfg.auth.token_ttl_seconds, Some(300));
        // R-20261120-008：占位登录总闸解析
        assert_eq!(cfg.auth.allow_placeholder_login, Some(false));
        assert_eq!(cfg.audit.ring_capacity, Some(1024));
        assert_eq!(cfg.admin.admin_key.as_deref(), Some("REPLACE_WITH_RANDOM_ADMIN_KEY"));
        // F4-V1：[tls] 段与 [users.<user>].totp_secret 解析
        assert!(cfg.tls.enabled());
        assert_eq!(cfg.tls.cert_path.as_deref(), Some("/etc/easyrdp/tls/server.crt"));
        assert_eq!(cfg.users["alice"].totp_secret.as_deref(), Some("JBSWY3DPEHPK3PXP"));
        assert_eq!(cfg.users["bob"].totp_secret, None);
        // [policy] 段：两条规则按序解析
        assert_eq!(cfg.policy.rules.len(), 2);
        assert_eq!(cfg.policy.rules[0].role, "user");
        assert_eq!(cfg.policy.rules[0].decision, "allow");
        assert_eq!(cfg.policy.rules[1].role, "readonly");
        assert_eq!(cfg.resources.len(), 2);
        assert_eq!(cfg.resources[0].id, "rdp-win01");
        assert_eq!(cfg.resources[0].backend, "10.0.0.5:3389");
        // allowed_roles：显式数组按原样解析；缺省字段为 None（= 所有角色可用）
        assert_eq!(
            cfg.resources[0].allowed_roles.as_deref(),
            Some(["ops".to_string(), "admin".to_string()].as_slice())
        );
        assert_eq!(cfg.resources[1].allowed_roles, None);
        // F5-V2：[oidc] 段解析与 enabled() 判定
        assert!(cfg.oidc.enabled());
        assert_eq!(cfg.oidc.issuer.as_deref(), Some("https://sso.example.com/realms/easyrdp"));
        assert_eq!(cfg.oidc.effective_scopes(), "openid profile email roles");
        assert_eq!(cfg.oidc.effective_roles_claim(), "roles");
        assert_eq!(cfg.oidc.allow_placeholder_login, Some(false));
    }

    /// [oidc] 缺省：未配置 → 占位登录保持可用（向后兼容）。
    #[test]
    fn parse_oidc_absent_keeps_placeholder_login() {
        let cfg = GatewayConfig::parse("").expect("parse empty");
        assert!(!cfg.oidc.enabled());
        assert_eq!(cfg.oidc.effective_scopes(), "openid profile email");
        assert_eq!(cfg.oidc.effective_roles_claim(), "roles");
        assert_eq!(cfg.oidc.allow_placeholder_login, None);
        // 占位登录可用 = oidc 未启用（main 据此挂占位登录开关）
        assert!(!cfg.oidc.enabled());
    }

    /// [oidc] 字段不全（缺 client_secret）：不启用，退回占位登录形态。
    #[test]
    fn parse_oidc_partial_config_not_enabled() {
        let cfg = GatewayConfig::parse(
            "[oidc]\nissuer = \"https://sso.example.com\"\nclient_id = \"gw\"\n",
        )
        .expect("parse partial oidc");
        assert!(!cfg.oidc.enabled());
    }

    #[test]
    fn parse_empty_config_yields_defaults() {
        let cfg = GatewayConfig::parse("").expect("parse empty");
        assert!(cfg.resources.is_empty());
        assert!(cfg.auth.jwt_secret.is_none());
        assert!(cfg.server.listen_addr.is_none());
        // F4-V1：[tls]/[users] 缺省 = 网关本体 TLS 关闭、无 OTP 用户
        assert!(!cfg.tls.enabled());
        assert!(cfg.users.is_empty());
        // [policy] 缺省 = 空规则（默认矩阵 deny by default、admin 全通）
        assert!(cfg.policy.rules.is_empty());
    }

    #[test]
    fn parse_invalid_toml_is_error() {
        assert!(GatewayConfig::parse("not [valid toml").is_err());
    }
}
