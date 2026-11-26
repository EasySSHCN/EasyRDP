//! 会话配置模型（SessionProfile）。

use crate::knock::KnockConfig;
use serde::{Deserialize, Serialize};

/// RDP 网关认证方式。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayAuth {
    /// 网关侧一次性密码，以 Cookie 形式下发。
    OtpCookie,
    /// 继承主会话的用户凭据。
    Inherit,
    /// Bearer Token（网关 /api/auth/login 签发的短期 JWT）。
    Token,
    /// 用户名 + 密码：客户端先向网关 /api/auth/login 换取 JWT 再连接。
    UserPassword,
}

/// F4-V2「网关高可用」（R-20261118-016）：单个网关节点端点（备用节点）。
/// 仅含连接定位字段；认证凭据 / resource_id / OTP 与主网关共享
/// （GatewayAuth / token / username / password / otp_code / resource_id
/// 均取自 GatewayConfig 本体，备用节点间不重复存储凭据）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayEndpoint {
    /// 备用网关主机名。
    pub hostname: String,
    /// 备用网关端口，默认 443。
    pub port: u16,
    /// 是否使用 TLS（wss/https），默认 true。
    pub use_tls: bool,
    /// 自签证书放行（wss 场景），默认 false。
    pub allow_self_signed: bool,
}

impl Default for GatewayEndpoint {
    fn default() -> Self {
        Self {
            hostname: String::new(),
            port: 443,
            use_tls: true,
            allow_self_signed: false,
        }
    }
}

impl GatewayEndpoint {
    /// 节点显示标识（host:port），聚合错误 / 健康探测展示用。
    pub fn label(&self) -> String {
        format!("{}:{}", self.hostname.trim(), self.port)
    }
}

/// RD Gateway 网关配置（EasyRDP 云网关 = 自研 WS 隧道协议，
/// 见 easyrdp-gateway/src/tunnel.rs：`GET /tunnel/{resource_id}` + Bearer JWT）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayConfig {
    /// 网关主机名，例如 `gw.example.com`。
    pub hostname: String,
    /// 网关端口（WS/TLS 端口），默认 443。
    pub port: u16,
    /// 是否使用 TLS（wss/https），默认 true。
    pub use_tls: bool,
    pub auth: GatewayAuth,
    /// 网关资源 ID（服务端 resources 注册表条目）；留空 = 以
    /// `host:port` 显式直连（受网关 SSRF 私网白名单约束）。
    pub resource_id: String,
    /// Token 认证的 JWT（GatewayAuth::Token）。
    pub token: Option<String>,
    /// 用户名密码认证（GatewayAuth::UserPassword）。
    pub username: Option<String>,
    pub password: Option<String>,
    /// F4-V1（R-20261118-012）：自签证书放行（wss 场景）。默认 false =
    /// 标准证书校验开启；true = 跳过证书校验（仅内网自签网关，UI 明示勾选）。
    #[serde(default)]
    pub allow_self_signed: bool,
    /// F4-V1（R-20261118-012）：网关 OTP 二次认证码（运行时一次性，登录
    /// /api/auth/login 时作为 mfa_code 提交；不参与凭据 DPAPI 加密）。
    #[serde(default)]
    pub otp_code: Option<String>,
    /// F4-V2（R-20261118-016）：备用网关节点列表（按序故障转移）。
    /// serde default 兼容旧 JSON（缺字段 = 空列表，单网关行为不变）。
    #[serde(default)]
    pub fallbacks: Vec<GatewayEndpoint>,
    /// F4-V2：认证失败是否也继续切换下一节点。默认 false（认证失败即
    /// 停止——凭据错误时逐节点重试只会重复失败）；true 供多租户网关
    /// 分节点发放凭据差异场景使用。serde default 兼容旧 JSON。
    #[serde(default)]
    pub fail_on_auth_fail: bool,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            hostname: String::new(),
            port: 443,
            use_tls: true,
            auth: GatewayAuth::Token,
            resource_id: String::new(),
            token: None,
            username: None,
            password: None,
            allow_self_signed: false,
            otp_code: None,
            fallbacks: Vec::new(),
            fail_on_auth_fail: false,
        }
    }
}

impl GatewayConfig {
    /// F4-V2：故障转移节点链 = 主网关在前 + fallbacks 按序在后。
    /// 主网关的定位字段（hostname/port/use_tls/allow_self_signed）原样
    /// 作为链首节点，凭据共享字段不复制（运行时取本体）。
    pub fn endpoint_chain(&self) -> Vec<GatewayEndpoint> {
        let primary = GatewayEndpoint {
            hostname: self.hostname.clone(),
            port: self.port,
            use_tls: self.use_tls,
            allow_self_signed: self.allow_self_signed,
        };
        let mut chain = vec![primary];
        chain.extend(self.fallbacks.iter().cloned());
        chain
    }
    /// WS 隧道 URL（客户端桥接模块连接网关用）。
    /// 例：`wss://gw.example.com:443/tunnel/rdp-win01`。
    pub fn ws_url(&self) -> String {
        let scheme = if self.use_tls { "wss" } else { "ws" };
        format!(
            "{scheme}://{}:{}/tunnel/{}",
            self.hostname.trim(),
            self.port,
            self.resource_id.trim()
        )
    }

    /// HTTP 基地址（登录换 token 用）：`https://gw.example.com:443`。
    pub fn http_base(&self) -> String {
        let scheme = if self.use_tls { "https" } else { "http" };
        format!("{scheme}://{}:{}", self.hostname.trim(), self.port)
    }

    /// 落盘前凭据加密（发布门禁）：token / username / password 经
    /// [`crate::secret::protect`]（Windows DPAPI / 非 Windows AES-256-GCM + 0600 密钥文件）。
    ///
    /// 迁移策略（兼容旧数据）：
    /// - 空 / None：跳过；
    /// - 已是受保护格式（`dpapi:` / `enc:` 前缀）：幂等跳过；
    /// - **明文旧数据**：读到时不改（加载路径不重写磁盘），**下次保存时
    ///   由本函数自动加密**（`is_protected_format` 判定明文 → 加密）。
    pub fn protect_credentials(&mut self) -> Result<(), String> {
        fn enc_field(v: &mut Option<String>) -> Result<(), String> {
            if let Some(s) = v.as_deref() {
                if !s.is_empty() && !crate::secret::is_protected_format(s) {
                    *v = Some(crate::secret::protect(s)?);
                }
            }
            Ok(())
        }
        enc_field(&mut self.token)?;
        enc_field(&mut self.username)?;
        enc_field(&mut self.password)?;
        Ok(())
    }

    /// 读盘后凭据解密（供运行时/回显使用）。与主密码字段语义一致：
    /// 解密失败（跨用户密文/损坏）保留原值并返回错误，由上层决定降级。
    pub fn unprotect_credentials(&mut self) -> Result<(), String> {
        fn dec_field(v: &mut Option<String>) -> Result<(), String> {
            if let Some(s) = v.as_deref() {
                if !s.is_empty() && crate::secret::is_protected_format(s) {
                    match crate::secret::unprotect(s) {
                        Ok(plain) => *v = Some(plain),
                        Err(e) => return Err(e),
                    }
                }
            }
            Ok(())
        }
        dec_field(&mut self.token)?;
        dec_field(&mut self.username)?;
        dec_field(&mut self.password)?;
        Ok(())
    }
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

impl Resolution {
    /// 解析出像素尺寸（仅作画面缓冲预估/演示帧尺寸）。`FitWindow` 的
    /// **实际请求分辨率**由 easyrdp-rdp [`fit_window_resolution`]（主显示器
    /// 当前分辨率）决定，此处保留 1280x720 作为无窗口环境回退值；
    /// `Fullscreen` 在 profile 层未知实际显示器几何，回退 1920x1080。
    pub fn dims(&self) -> (u32, u32) {
        match self {
            Resolution::FitWindow => (1280, 720),
            Resolution::Fullscreen => (1920, 1080),
            Resolution::Custom { w, h } => (*w, *h),
        }
    }
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
    /// 列表排序偏好（小在前；0 = 未排序，保持追加顺序）。
    /// F1-V1 R-20261118-007：拖拽排序持久化字段，serde default 兼容旧数据。
    pub order: i64,
    /// F3-V1 R-20261118-010 多显示器优化：全屏模式目标显示器序号
    /// （0 起，EnumDisplayMonitors 枚举顺序；None = 主显示器）。
    /// 仅 `Resolution::Fullscreen` 时生效；serde default 兼容旧数据。
    pub monitor: Option<u32>,
    /// F3-V1：全屏模式跨所有显示器（窗口 rect = 虚拟屏 SM_XVIRTUALSCREEN/...）。
    /// true 时忽略 `monitor`；serde default 兼容旧数据。
    pub multi_monitor: bool,
    /// 端口敲门：连接前执行的本地命令（P0 pre_connect_command 钩子）。
    /// 空串 = 不执行。按平台 shell 语义执行（Windows: cmd /S /C，Unix: sh -c）；
    /// 配置导入不做任何字符串拼接（本字段原样存储、原样传给 spawn）。
    /// serde rename 对齐产品 JSON 约定（camelCase）；缺省兼容旧数据。
    #[serde(default, rename = "preConnectCommand")]
    pub pre_connect_command: String,
    /// 端口敲门配置（knockd 序列敲门 / fwknop SPA；enabled=false 保持直连）。
    /// serde default 兼容旧数据；SPA 密钥存储态经 DPAPI 加密。
    #[serde(default)]
    pub knock: KnockConfig,
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
            order: 0,
            // F3-V1：默认主显示器 + 不跨屏（与既有单屏行为完全一致）
            monitor: None,
            multi_monitor: false,
            // 端口敲门（P0-P2）：默认关闭，既有直连行为完全不变。
            pre_connect_command: String::new(),
            knock: KnockConfig::default(),
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
            if gw.port == 0 {
                errors.push(format!(
                    "gateway.port 非法: {} (须为 1-65535)",
                    gw.port
                ));
            }
            match gw.auth {
                GatewayAuth::Token => {
                    if gw.token.as_deref().map(str::trim).unwrap_or("").is_empty()
                        && gw.username.as_deref().map(str::trim).unwrap_or("").is_empty()
                    {
                        errors.push(
                            "网关 Token 认证需填写 Token（或用户名以便在线换取）".to_string(),
                        );
                    }
                }
                GatewayAuth::UserPassword => {
                    if gw.username.as_deref().map(str::trim).unwrap_or("").is_empty() {
                        errors.push("网关用户名密码认证需填写用户名".to_string());
                    }
                }
                GatewayAuth::OtpCookie | GatewayAuth::Inherit => {}
            }
            // F4-V2：备用网关节点校验（每条 hostname 非空 / port 非零）
            for (i, ep) in gw.fallbacks.iter().enumerate() {
                if ep.hostname.trim().is_empty() {
                    errors.push(format!("gateway.fallbacks[{}].hostname 不能为空", i));
                }
                if ep.port == 0 {
                    errors.push(format!(
                        "gateway.fallbacks[{}].port 非法: {} (须为 1-65535)",
                        i, ep.port
                    ));
                }
            }
        }

        // 端口敲门配置校验（P1/P2）
        if let Err(errs) = self.knock.validate() {
            errors.extend(errs);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// 端口敲门 SPA 密钥落盘前加密（与网关凭据同一 DPAPI secret 层；幂等）。
    pub fn protect_knock_secrets(&mut self) -> Result<(), String> {
        self.knock.protect_secrets()
    }

    /// 端口敲门 SPA 密钥读盘后解密（运行时/回显路径）。
    pub fn unprotect_knock_secrets(&mut self) -> Result<(), String> {
        self.knock.unprotect_secrets()
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
        assert_eq!(p.order, 0);
        assert!(p.validate().is_ok());
    }

    /// F1-V1 兼容：旧 Profile JSON（无 order 字段）serde default 补 0。
    #[test]
    fn serde_backward_compat_old_json_without_order() {
        let old = r#"{ "name": "legacy", "host": "10.0.0.5" }"#;
        let p: SessionProfile = serde_json::from_str(old).expect("旧 JSON 应可解析");
        assert_eq!(p.order, 0);
        assert!(p.validate().is_ok());
        // 带 order 的 roundtrip
        let mut p2 = SessionProfile::default();
        p2.host = "h".into();
        p2.order = -7;
        let back: SessionProfile = serde_json::from_str(&serde_json::to_string(&p2).unwrap()).unwrap();
        assert_eq!(back.order, -7);
    }

    /// F3-V1 R-20261118-010 兼容：旧 Profile JSON（无 monitor/multi_monitor
    /// 字段）serde default 补 None/false——既有单屏行为不变。
    #[test]
    fn serde_backward_compat_old_json_without_monitor_fields() {
        let old = r#"{ "name": "legacy", "host": "10.0.0.5", "resolution": "fullscreen" }"#;
        let p: SessionProfile = serde_json::from_str(old).expect("旧 JSON 应可解析");
        assert_eq!(p.monitor, None, "缺省 monitor = 主显示器");
        assert!(!p.multi_monitor, "缺省不跨屏");
        // 新字段 roundtrip：指定显示器 + 跨屏
        let mut p2 = SessionProfile::default();
        p2.host = "h".into();
        p2.resolution = Resolution::Fullscreen;
        p2.monitor = Some(1);
        p2.multi_monitor = true;
        let back: SessionProfile =
            serde_json::from_str(&serde_json::to_string(&p2).unwrap()).unwrap();
        assert_eq!(back.monitor, Some(1));
        assert!(back.multi_monitor);
        // 部分新字段的 JSON（只有 monitor）：multi_monitor 缺省 false
        let mid = r#"{ "name": "m", "host": "10.0.0.6", "monitor": 2 }"#;
        let p3: SessionProfile = serde_json::from_str(mid).unwrap();
        assert_eq!(p3.monitor, Some(2));
        assert!(!p3.multi_monitor);
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
            port: 8443,
            use_tls: false,
            auth: GatewayAuth::Token,
            resource_id: "rdp-win01".into(),
            token: Some("jwt-abc".into()),
            username: None,
            password: None,
            allow_self_signed: false,
            otp_code: None,
            ..GatewayConfig::default()
        });
        let json = serde_json::to_string(&p).unwrap();
        let back: SessionProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
        let gw = back.gateway.unwrap();
        assert_eq!(gw.ws_url(), "ws://gw.example.com:8443/tunnel/rdp-win01");
        assert_eq!(gw.http_base(), "http://gw.example.com:8443");
    }

    /// 旧版 connections.json 兼容：gateway 只有 hostname + auth 两个字段，
    /// 新字段（port/use_tls/resource_id/token/...）由 serde default 补齐。
    #[test]
    fn serde_backward_compat_old_gateway_json() {
        let old = r#"{
            "name": "legacy",
            "host": "10.0.0.5",
            "gateway": { "hostname": "gw.example.com", "auth": { "type": "otp_cookie" } }
        }"#;
        let p: SessionProfile = serde_json::from_str(old).expect("旧 JSON 应可解析");
        let gw = p.gateway.as_ref().expect("gateway");
        assert_eq!(gw.hostname, "gw.example.com");
        assert_eq!(gw.port, 443, "默认端口 443");
        assert!(gw.use_tls, "默认 TLS 开启");
        assert!(!gw.allow_self_signed, "默认自签证书放行关闭（F4-V1）");
        assert_eq!(gw.otp_code, None, "默认无 OTP 码（F4-V1）");
        assert_eq!(gw.auth, GatewayAuth::OtpCookie);
        assert!(p.validate().is_ok());
    }

    #[test]
    fn gateway_token_auth_without_token_fails_validation() {
        let mut p = SessionProfile::default();
        p.host = "10.0.0.5".into();
        p.gateway = Some(GatewayConfig {
            hostname: "gw.example.com".into(),
            auth: GatewayAuth::Token,
            ..GatewayConfig::default()
        });
        let errs = p.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("Token")), "{errs:?}");

        // 填 token 后通过
        p.gateway.as_mut().unwrap().token = Some("jwt".into());
        assert!(p.validate().is_ok());
        // 只填用户名（在线换取路径）也通过
        p.gateway.as_mut().unwrap().token = None;
        p.gateway.as_mut().unwrap().username = Some("zhangsan".into());
        assert!(p.validate().is_ok());
    }

    #[test]
    fn gateway_user_password_requires_username() {
        let mut p = SessionProfile::default();
        p.host = "10.0.0.5".into();
        p.gateway = Some(GatewayConfig {
            hostname: "gw.example.com".into(),
            auth: GatewayAuth::UserPassword,
            password: Some("pw".into()),
            ..GatewayConfig::default()
        });
        let errs = p.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("用户名")), "{errs:?}");
        p.gateway.as_mut().unwrap().username = Some("ops".into());
        assert!(p.validate().is_ok());
    }

    #[test]
    fn gateway_invalid_port_fails_validation() {
        let mut p = SessionProfile::default();
        p.host = "10.0.0.5".into();
        p.gateway = Some(GatewayConfig {
            hostname: "gw.example.com".into(),
            port: 0,
            auth: GatewayAuth::Token,
            token: Some("jwt".into()),
            ..GatewayConfig::default()
        });
        let errs = p.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("gateway.port")), "{errs:?}");
    }

    // -----------------------------------------------------------------
    // 网关凭据 DPAPI 加密（发布门禁）
    // -----------------------------------------------------------------

    /// 加密→解密回环：三个凭据字段密文落盘、解密还原；密文不含明文。
    #[test]
    fn gateway_credentials_protect_unprotect_roundtrip() {
        let mut gw = GatewayConfig {
            hostname: "gw.example.com".into(),
            auth: GatewayAuth::UserPassword,
            resource_id: "rdp-win01".into(),
            token: Some("jwt-plain-token".into()),
            username: Some("zhangsan".into()),
            password: Some("P@ss 中文 🔐".into()),
            ..GatewayConfig::default()
        };
        gw.protect_credentials().expect("protect");
        // 密文形态断言（Windows=dpapi: 前缀；非 Windows=enc: AES-GCM 前缀）
        let stored = serde_json::to_string(&gw).unwrap();
        assert!(!stored.contains("jwt-plain-token"), "落盘串不得含明文 token");
        assert!(!stored.contains("P@ss 中文"), "落盘串不得含明文密码");
        // 解密还原
        gw.unprotect_credentials().expect("unprotect");
        assert_eq!(gw.token.as_deref(), Some("jwt-plain-token"));
        assert_eq!(gw.username.as_deref(), Some("zhangsan"));
        assert_eq!(gw.password.as_deref(), Some("P@ss 中文 🔐"));
    }

    /// None / 空串字段：加解密全程不动。
    #[test]
    fn gateway_credentials_empty_and_none_untouched() {
        let mut gw = GatewayConfig {
            hostname: "gw".into(),
            auth: GatewayAuth::Token,
            token: Some(String::new()),
            ..GatewayConfig::default()
        };
        gw.protect_credentials().unwrap();
        assert_eq!(gw.token, Some(String::new()));
        assert!(gw.username.is_none());
        assert!(gw.password.is_none());
        gw.unprotect_credentials().unwrap();
        assert_eq!(gw.token, Some(String::new()));
    }

    /// 已加密格式幂等：重复 protect 不双重加密。
    #[test]
    fn gateway_credentials_protect_is_idempotent() {
        let mut gw = GatewayConfig {
            hostname: "gw".into(),
            auth: GatewayAuth::Token,
            token: Some("raw-jwt".into()),
            ..GatewayConfig::default()
        };
        gw.protect_credentials().unwrap();
        let once = gw.token.clone().unwrap();
        assert!(crate::secret::is_protected_format(&once) && once != "raw-jwt");
        gw.protect_credentials().unwrap();
        assert_eq!(gw.token.as_deref(), Some(once.as_str()), "二次 protect 必须幂等");
    }

    /// 旧 profile JSON 兼容 + 迁移：明文凭据加载成功（校验通过），落盘前
    /// protect 自动加密（"读到明文 → 首次保存自动加密" 策略）。
    #[test]
    fn gateway_legacy_plaintext_json_loads_then_encrypts_on_save() {
        let old = r#"{
            "name": "legacy-gw",
            "host": "10.0.0.5",
            "gateway": {
                "hostname": "gw.example.com",
                "auth": { "type": "token" },
                "resource_id": "rdp-win01",
                "token": "old-plaintext-jwt",
                "username": "zhangsan",
                "password": "old-plaintext-pw"
            }
        }"#;
        let mut p: SessionProfile = serde_json::from_str(old).expect("旧 JSON（明文凭据）应可解析");
        assert!(p.validate().is_ok(), "明文旧数据加载后校验通过");
        let gw = p.gateway.as_mut().unwrap();
        // 模拟保存路径：protect_credentials（连接存储层落盘前统一调用）
        gw.protect_credentials().expect("迁移加密");
        let stored = serde_json::to_string(&p).unwrap();
        assert!(!stored.contains("old-plaintext-jwt"));
        assert!(!stored.contains("old-plaintext-pw"));
        // 重新读入（模拟 connections.json）→ 解密 → 明文还原
        let mut reloaded: SessionProfile = serde_json::from_str(&stored).unwrap();
        reloaded.gateway.as_mut().unwrap().unprotect_credentials().unwrap();
        assert_eq!(reloaded.gateway.as_ref().unwrap().token.as_deref(), Some("old-plaintext-jwt"));
        assert_eq!(reloaded.gateway.as_ref().unwrap().password.as_deref(), Some("old-plaintext-pw"));
    }

    // -----------------------------------------------------------------
    // F4-V2「网关高可用」（R-20261118-016）：多网关模型 / serde 兼容
    // -----------------------------------------------------------------

    /// serde 兼容：旧网关 JSON（无 fallbacks / fail_on_auth_fail 字段）
    /// 解析后补默认值——空列表 + false，单网关行为完全不变。
    #[test]
    fn f4v2_serde_backward_compat_old_gateway_without_fallbacks() {
        let old = r#"{
            "name": "legacy-ha",
            "host": "10.0.0.5",
            "gateway": {
                "hostname": "gw.example.com",
                "auth": { "type": "token" },
                "token": "jwt"
            }
        }"#;
        let p: SessionProfile = serde_json::from_str(old).expect("旧 JSON 应可解析");
        let gw = p.gateway.as_ref().unwrap();
        assert!(gw.fallbacks.is_empty(), "缺省 fallbacks = 空列表");
        assert!(!gw.fail_on_auth_fail, "缺省 fail_on_auth_fail = false");
        assert!(p.validate().is_ok());
    }

    /// serde roundtrip：带 fallbacks 的新 JSON 写出再读入字段不丢。
    #[test]
    fn f4v2_serde_roundtrip_with_fallbacks() {
        let mut p = SessionProfile::default();
        p.host = "10.0.0.5".into();
        p.gateway = Some(GatewayConfig {
            hostname: "gw-a.example.com".into(),
            port: 443,
            auth: GatewayAuth::Token,
            token: Some("jwt".into()),
            resource_id: "rdp-win01".into(),
            fail_on_auth_fail: true,
            fallbacks: vec![
                GatewayEndpoint {
                    hostname: "gw-b.example.com".into(),
                    port: 8443,
                    use_tls: false,
                    allow_self_signed: true,
                },
                GatewayEndpoint {
                    hostname: "gw-c.example.com".into(),
                    ..GatewayEndpoint::default()
                },
            ],
            ..GatewayConfig::default()
        });
        let json = serde_json::to_string(&p).unwrap();
        let back: SessionProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back, "带 fallbacks 的 roundtrip 应无损");
        let gw = back.gateway.unwrap();
        assert_eq!(gw.fallbacks.len(), 2);
        assert_eq!(gw.fallbacks[0].hostname, "gw-b.example.com");
        assert!(!gw.fallbacks[0].use_tls);
        assert!(gw.fallbacks[0].allow_self_signed);
        assert_eq!(gw.fallbacks[1].port, 443);
        assert!(gw.fail_on_auth_fail);
    }

    /// endpoint_chain：主网关在前、fallbacks 按序在后，凭据共享字段不进链。
    #[test]
    fn f4v2_endpoint_chain_primary_then_fallbacks() {
        let gw = GatewayConfig {
            hostname: "gw-a".into(),
            port: 443,
            use_tls: true,
            allow_self_signed: false,
            auth: GatewayAuth::Token,
            token: Some("jwt".into()),
            fallbacks: vec![
                GatewayEndpoint { hostname: "gw-b".into(), port: 8443, use_tls: false, allow_self_signed: true },
                GatewayEndpoint { hostname: "gw-c".into(), ..GatewayEndpoint::default() },
            ],
            ..GatewayConfig::default()
        };
        let chain = gw.endpoint_chain();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].hostname, "gw-a");
        assert_eq!(chain[0].port, 443);
        assert_eq!(chain[1].hostname, "gw-b");
        assert!(!chain[1].use_tls);
        assert_eq!(chain[2].hostname, "gw-c");
        assert_eq!(chain[1].label(), "gw-b:8443");
        // 无 fallback：链长恒 1（与旧单网关一致）
        let single = GatewayConfig { hostname: "only".into(), ..GatewayConfig::default() };
        assert_eq!(single.endpoint_chain().len(), 1);
    }

    /// 校验：fallback 条目 hostname 空 / port 0 报错；合法条目通过。
    #[test]
    fn f4v2_validate_fallback_entries() {
        let mut p = SessionProfile::default();
        p.host = "10.0.0.5".into();
        p.gateway = Some(GatewayConfig {
            hostname: "gw-a".into(),
            auth: GatewayAuth::Token,
            token: Some("jwt".into()),
            fallbacks: vec![
                GatewayEndpoint { hostname: String::new(), ..GatewayEndpoint::default() },
                GatewayEndpoint { hostname: "gw-b".into(), port: 0, ..GatewayEndpoint::default() },
            ],
            ..GatewayConfig::default()
        });
        let errs = p.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("fallbacks[0].hostname")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("fallbacks[1].port")), "{errs:?}");
        // 合法 fallback 后通过
        p.gateway.as_mut().unwrap().fallbacks =
            vec![GatewayEndpoint { hostname: "gw-b".into(), ..GatewayEndpoint::default() }];
        assert!(p.validate().is_ok());
    }

    // -----------------------------------------------------------------
    // 端口敲门（P0-P2）：preConnectCommand + knock 字段兼容与校验
    // -----------------------------------------------------------------

    /// serde 兼容：旧 Profile JSON（无 preConnectCommand/knock）默认关闭、空命令。
    #[test]
    fn knock_serde_backward_compat_old_json_without_knock() {
        let old = r#"{ "name": "legacy", "host": "10.0.0.5" }"#;
        let p: SessionProfile = serde_json::from_str(old).unwrap();
        assert_eq!(p.pre_connect_command, "", "缺省 preConnectCommand = 空");
        assert!(!p.knock.enabled, "缺省敲门关闭");
        assert!(p.validate().is_ok());
    }

    /// 新字段 roundtrip：camelCase 命名（preConnectCommand/seqTimeout/delayMs）。
    #[test]
    fn knock_serde_roundtrip_camel_case() {
        let json = r#"{
            "name": "k",
            "host": "10.0.0.5",
            "preConnectCommand": "systemctl start myvpn",
            "knock": {
                "enabled": true,
                "type": "sequence",
                "sequence": [7000, 8000, 9000],
                "protocol": "udp",
                "seqTimeout": 10,
                "delayMs": 150
            }
        }"#;
        let p: SessionProfile = serde_json::from_str(json).unwrap();
        assert_eq!(p.pre_connect_command, "systemctl start myvpn");
        assert!(p.knock.enabled);
        assert_eq!(p.knock.sequence, vec![7000, 8000, 9000]);
        assert!(p.validate().is_ok());
        let back: SessionProfile =
            serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(p, back);
    }

    /// 校验：敲门启用但序列为空 → 报错；SPA 缺 key → 报错；关闭时全部放行。
    #[test]
    fn knock_validate_cases() {
        let mut p = SessionProfile::default();
        p.host = "10.0.0.5".into();
        p.knock = crate::knock::KnockConfig {
            enabled: true,
            knock_type: crate::knock::KnockType::Sequence,
            sequence: vec![],
            ..crate::knock::KnockConfig::default()
        };
        let errs = p.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("sequence")), "{errs:?}");

        p.knock.knock_type = crate::knock::KnockType::Spa;
        let errs = p.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("SPA")), "{errs:?}");

        // 合法 SPA 配置通过
        p.knock.key = "k".into();
        assert!(p.validate().is_ok());
        // 关闭后即使配置不完整也放行
        p.knock.enabled = false;
        p.knock.key = String::new();
        assert!(p.validate().is_ok());
    }
}
