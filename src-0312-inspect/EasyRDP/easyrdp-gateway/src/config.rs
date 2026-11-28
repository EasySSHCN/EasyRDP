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
    #[serde(default)]
    pub resources: Vec<ResourceConfig>,
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
"#;

    #[test]
    fn parse_full_config_with_resources() {
        let cfg = GatewayConfig::parse(SAMPLE).expect("parse");
        assert_eq!(cfg.server.listen_addr.as_deref(), Some("0.0.0.0:8443"));
        assert_eq!(cfg.auth.jwt_secret.as_deref(), Some("s3cret"));
        assert_eq!(cfg.auth.token_ttl_seconds, Some(300));
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
