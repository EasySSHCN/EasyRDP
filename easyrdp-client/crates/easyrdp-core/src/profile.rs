//! 会话配置模型（SessionProfile）。

use serde::{Deserialize, Serialize};

/// RDP 网关认证方式。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayAuth {
    /// 网关侧一次性密码，以 Cookie 形式下发。
    OtpCookie,
    /// 继承主会话的用户凭据。
    Inherit,
}

/// RD Gateway 网关配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// 网关主机名，例如 `gw.example.com`。
    pub hostname: String,
    pub auth: GatewayAuth,
}

/// 认证方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    Password,
    Sso,
    SmartCard,
}

/// 窗口分辨率策略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// 跟随客户端窗口大小自适应。
    FitWindow,
    Fullscreen,
    Custom { w: u32, h: u32 },
}

/// 重定向开关集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Redirect {
    pub clipboard: bool,
    pub printers: bool,
    pub drives: bool,
    pub camera: bool,
}

impl Default for Redirect {
    fn default() -> Self {
        Self {
            clipboard: true,
            printers: false,
            drives: false,
            camera: false,
        }
    }
}

/// 一次远程桌面会话的完整配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionProfile {
    /// 配置显示名。
    pub name: String,
    /// 目标主机（IP 或域名，不含端口）。
    pub host: String,
    /// RDP 端口，默认 3389。
    pub port: u16,
    pub username: String,
    pub auth_method: AuthMethod,
    pub resolution: Resolution,
    /// 颜色深度（bit），常用 16 / 24 / 32。
    pub color_depth: u32,
    pub redirect: Redirect,
    /// RD Gateway 网关配置，None 表示直连。
    pub gateway: Option<GatewayConfig>,
}

impl Default for SessionProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            port: 3389,
            username: String::new(),
            auth_method: AuthMethod::Password,
            resolution: Resolution::FitWindow,
            color_depth: 32,
            redirect: Redirect::default(),
            gateway: None,
        }
    }
}

impl SessionProfile {
    /// 校验配置合法性：主机非空、端口在 1-65535 之间。
    /// 返回全部错误信息（便于 UI 一次性展示）；为空表示通过。
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.host.trim().is_empty() {
            errors.push("host 不能为空".to_string());
        }
        if self.port == 0 {
            errors.push(format!("port 非法: {} (须为 1-65535)", self.port));
        }
        if let Some(gw) = &self.gateway {
            if gw.hostname.trim().is_empty() {
                errors.push("gateway.hostname 不能为空".to_string());
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_is_valid_and_has_default_port() {
        let mut p = SessionProfile::default();
        p.host = "10.0.0.5".into();
        p.name = "office".into();
        assert_eq!(p.port, 3389);
        assert!(p.validate().is_ok());
    }

    #[test]
    fn empty_host_fails_validation() {
        let p = SessionProfile::default();
        let errs = p.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("host")));
    }

    #[test]
    fn zero_port_fails_validation() {
        let mut p = SessionProfile::default();
        p.host = "h".into();
        p.port = 0;
        assert!(p.validate().is_err());
    }

    #[test]
    fn serde_roundtrip_with_gateway() {
        let mut p = SessionProfile::default();
        p.host = "srv.local".into();
        p.name = "gw test".into();
        p.gateway = Some(GatewayConfig {
            hostname: "gw.example.com".into(),
            auth: GatewayAuth::OtpCookie,
        });
        let json = serde_json::to_string(&p).unwrap();
        let back: SessionProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
