//! 配置模块：config.toml 解析（server / auth / audit / resources）。
//!
//! `GatewayConfig::parse` 从 TOML 字符串解析（便于单测，不写临时文件）；
//! `load_from_path` 读文件后复用同一解析入口，文件缺失时返回 None
//! （main 使用默认值 / 环境变量回退）。

use serde::Deserialize;
use std::path::Path;

use crate::resources::ResourceConfig;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GatewayConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub audit: AuditConfig,
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

[[resources]]
id = "rdp-win01"
name = "Windows 跳板"
backend = "10.0.0.5:3389"

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
        assert_eq!(cfg.resources.len(), 2);
        assert_eq!(cfg.resources[0].id, "rdp-win01");
        assert_eq!(cfg.resources[0].backend, "10.0.0.5:3389");
    }

    #[test]
    fn parse_empty_config_yields_defaults() {
        let cfg = GatewayConfig::parse("").expect("parse empty");
        assert!(cfg.resources.is_empty());
        assert!(cfg.auth.jwt_secret.is_none());
        assert!(cfg.server.listen_addr.is_none());
    }

    #[test]
    fn parse_invalid_toml_is_error() {
        assert!(GatewayConfig::parse("not [valid toml").is_err());
    }
}
