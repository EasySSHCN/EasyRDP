//! Agent 配置：agent.toml（方案 §5）。
//!
//! ```toml
//! [gateway]
//! url = "https://easyrdp.cn"          # 自动解析 wss 443
//! token = "dpapi:AQAAANCM..."         # AgentToken（--enroll 落盘后为 DPAPI 密文）
//!                                     # 也支持 "file:agent.token" 或明文（仅调试）
//! [tunnel]
//! resource_id = "rdp-office-01"
//! local_backend = "127.0.0.1:3389"    # Agent 只 DIAL 该地址（防内网跳板滥用）
//! [options]
//! heartbeat_secs = 30                 # 应用层 PING 周期；3 周期静默判死重连
//! dial_timeout_secs = 5               # 本地后端 TCP connect 超时
//! reconnect_initial_ms = 1000         # 指数退避起点
//! reconnect_cap_ms = 60000            # 指数退避封顶
//! ```

use std::path::Path;
use std::time::Duration;

use crate::dpapi;

pub const DEFAULT_LOCAL_BACKEND: &str = "127.0.0.1:3389";
const DEFAULT_HEARTBEAT_SECS: u64 = 30;
const DEFAULT_DIAL_TIMEOUT_SECS: u64 = 5;
const DEFAULT_RECONNECT_INITIAL_MS: u64 = 1000;
const DEFAULT_RECONNECT_CAP_MS: u64 = 60_000;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewaySection {
    pub url: String,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelSection {
    pub resource_id: String,
    #[serde(default = "default_local_backend")]
    pub local_backend: String,
}

fn default_local_backend() -> String {
    DEFAULT_LOCAL_BACKEND.to_string()
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct OptionsSection {
    pub heartbeat_secs: Option<u64>,
    pub dial_timeout_secs: Option<u64>,
    pub reconnect_initial_ms: Option<u64>,
    pub reconnect_cap_ms: Option<u64>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    pub gateway: GatewaySection,
    pub tunnel: TunnelSection,
    #[serde(default)]
    pub options: OptionsSection,
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    /// token 缺失或格式非法。
    MissingToken,
    /// DPAPI 解密失败。
    TokenDecrypt(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "配置读取失败: {e}"),
            ConfigError::Parse(e) => write!(f, "配置解析失败: {e}"),
            ConfigError::MissingToken => write!(f, "未配置 AgentToken，请先运行 --enroll"),
            ConfigError::TokenDecrypt(e) => write!(f, "AgentToken 解密失败: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl AgentConfig {
    pub fn load_from_path(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
        toml::from_str(&text).map_err(ConfigError::Parse)
    }

    pub fn heartbeat(&self) -> Duration {
        Duration::from_secs(self.options.heartbeat_secs.unwrap_or(DEFAULT_HEARTBEAT_SECS).max(1))
    }

    /// 判死静默窗口 = 心跳周期 × 3（与网关口径一致）。
    pub fn silence_deadline(&self) -> Duration {
        Duration::from_secs(
            self.options.heartbeat_secs.unwrap_or(DEFAULT_HEARTBEAT_SECS).max(1).saturating_mul(3),
        )
    }

    pub fn dial_timeout(&self) -> Duration {
        Duration::from_secs(
            self.options.dial_timeout_secs.unwrap_or(DEFAULT_DIAL_TIMEOUT_SECS).max(1),
        )
    }

    pub fn reconnect_initial(&self) -> Duration {
        Duration::from_millis(
            self.options.reconnect_initial_ms.unwrap_or(DEFAULT_RECONNECT_INITIAL_MS).max(1),
        )
    }

    pub fn reconnect_cap(&self) -> Duration {
        Duration::from_millis(
            self.options.reconnect_cap_ms.unwrap_or(DEFAULT_RECONNECT_CAP_MS).max(1),
        )
    }

    /// 从 token 字段解析出明文 AgentToken：
    /// - `dpapi:<base64>` → Windows DPAPI（CurrentUser scope）解密；
    /// - `file:<path>` → 读取外部文件（明文，调试用）；
    /// - 其余 → 视为明文（仅本地调试）。
    pub fn resolve_token(&self) -> Result<String, ConfigError> {
        let raw = self
            .gateway
            .token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or(ConfigError::MissingToken)?;
        if let Some(blob) = raw.strip_prefix("dpapi:") {
            return dpapi::unprotect(blob).map_err(ConfigError::TokenDecrypt);
        }
        if let Some(path) = raw.strip_prefix("file:") {
            return std::fs::read_to_string(path.trim())
                .map(|s| s.trim().to_string())
                .map_err(|e| ConfigError::TokenDecrypt(format!("读取 token 文件失败: {e}")));
        }
        Ok(raw.to_string())
    }

    /// 把网关 /agent/connect 的 WebSocket URL 拼出来：
    /// `https://gw` → `wss://gw/agent/connect?resource_id=...`；
    /// `http://gw:8443` → `ws://gw:8443/agent/connect?resource_id=...`。
    pub fn connect_ws_url(&self) -> String {
        let base = self.gateway.url.trim().trim_end_matches('/');
        let ws_base = if let Some(rest) = base
            .strip_prefix("https://")
            .or_else(|| base.strip_prefix("wss://"))
        {
            format!("wss://{rest}")
        } else if let Some(rest) = base
            .strip_prefix("http://")
            .or_else(|| base.strip_prefix("ws://"))
        {
            format!("ws://{rest}")
        } else {
            // 无 scheme：按方案口径视为公网地址（443 WSS）。
            format!("wss://{base}")
        };
        format!(
            "{ws_base}/agent/connect?resource_id={}",
            urlencode(&self.tunnel.resource_id)
        )
    }
}

/// 极简 query 编码：仅处理 resource_id 中可能出现的安全字符（ID 为 `u-xxxx` 形态，
/// 字母数字与连字符，其余字符按百分号转义兜底）。
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// --enroll 成功后把 DPAPI 密文写回 agent.toml 的 token 字段（行级替换，
/// 无该字段则在 [gateway] 段内追加），返回写回后的完整文本。
pub fn apply_token_to_toml(toml_text: &str, stored: &str) -> String {
    let token_line = format!("token = \"{stored}\"");
    let mut in_gateway = false;
    let mut replaced = false;
    let mut lines: Vec<String> = toml_text
        .lines()
        .map(|l| {
            let trimmed = l.trim();
            if trimmed.starts_with('[') {
                in_gateway = trimmed == "[gateway]";
            }
            if in_gateway && trimmed.starts_with("token") && trimmed.contains('=') {
                replaced = true;
                token_line.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !replaced {
        // [gateway] 段不存在：追加段 + token 行；存在：插到段头之后。
        if let Some(pos) = lines.iter().position(|l| l.trim() == "[gateway]") {
            lines.insert(pos + 1, token_line);
        } else {
            lines.push(String::new());
            lines.push("[gateway]".to_string());
            lines.push(token_line);
        }
    }
    let mut out = lines.join("\n");
    if !toml_text.ends_with('\n') {
        // 保持无尾换行的原样
    } else {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[gateway]
url = "https://easyrdp.cn"
token = "file:agent.token"

[tunnel]
resource_id = "rdp-office-01"
local_backend = "127.0.0.1:3389"

[options]
heartbeat_secs = 15
"#;

    fn sample() -> AgentConfig {
        toml::from_str(SAMPLE).expect("parse sample")
    }

    #[test]
    fn parse_sample_config_defaults() {
        let cfg = sample();
        assert_eq!(cfg.gateway.url, "https://easyrdp.cn");
        assert_eq!(cfg.tunnel.resource_id, "rdp-office-01");
        assert_eq!(cfg.tunnel.local_backend, DEFAULT_LOCAL_BACKEND);
        assert_eq!(cfg.heartbeat(), Duration::from_secs(15));
        assert_eq!(cfg.silence_deadline(), Duration::from_secs(45));
        assert_eq!(cfg.dial_timeout(), Duration::from_secs(5));
        assert_eq!(cfg.reconnect_initial(), Duration::from_millis(1000));
        assert_eq!(cfg.reconnect_cap(), Duration::from_secs(60));
    }

    #[test]
    fn defaults_when_options_absent() {
        let cfg: AgentConfig = toml::from_str(
            r#"
[gateway]
url = "https://easyrdp.cn"
token = "plain-token"

[tunnel]
resource_id = "r1"
"#,
        )
        .expect("parse minimal");
        assert_eq!(cfg.tunnel.local_backend, DEFAULT_LOCAL_BACKEND);
        assert_eq!(cfg.resolve_token().expect("token"), "plain-token");
    }

    #[test]
    fn missing_token_is_error() {
        let cfg: AgentConfig = toml::from_str(
            r#"
[gateway]
url = "https://easyrdp.cn"

[tunnel]
resource_id = "r1"
"#,
        )
        .expect("parse");
        assert!(matches!(cfg.resolve_token(), Err(ConfigError::MissingToken)));
    }

    #[test]
    fn file_token_resolution() {
        let dir = std::env::temp_dir().join(format!("erd-agent-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let tok_path = dir.join("t.token");
        std::fs::write(&tok_path, "  secret-agent-token \n").expect("write");
        let tok_path_fwd = tok_path.display().to_string().replace('\\', "/");
        let cfg: AgentConfig = toml::from_str(&format!(
            r#"
[gateway]
url = "https://easyrdp.cn"
token = "file:{}"

[tunnel]
resource_id = "r1"
"#,
            tok_path_fwd
        ))
        .expect("parse");
        assert_eq!(cfg.resolve_token().expect("token"), "secret-agent-token");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ws_url_from_https_http_and_bare() {
        let mk = |url: &str| {
            let cfg: AgentConfig = toml::from_str(&format!(
                "[gateway]\nurl = \"{url}\"\ntoken = \"t\"\n\n[tunnel]\nresource_id = \"u-ab12\"\n"
            ))
            .expect("parse");
            cfg
        };
        assert_eq!(
            mk("https://easyrdp.cn").connect_ws_url(),
            "wss://easyrdp.cn/agent/connect?resource_id=u-ab12"
        );
        assert_eq!(
            mk("https://easyrdp.cn/").connect_ws_url(),
            "wss://easyrdp.cn/agent/connect?resource_id=u-ab12"
        );
        assert_eq!(
            mk("http://10.0.0.5:8443").connect_ws_url(),
            "ws://10.0.0.5:8443/agent/connect?resource_id=u-ab12"
        );
        assert_eq!(
            mk("easyrdp.cn").connect_ws_url(),
            "wss://easyrdp.cn/agent/connect?resource_id=u-ab12",
            "无 scheme 默认公网 443 WSS"
        );
        assert_eq!(
            mk("http://gw:8443").connect_ws_url(),
            "ws://gw:8443/agent/connect?resource_id=u-ab12"
        );
    }

    #[test]
    fn urlencode_escapes_reserved() {
        assert_eq!(urlencode("u-ab12"), "u-ab12");
        assert_eq!(urlencode("a b/c?d"), "a%20b%2Fc%3Fd");
    }

    #[test]
    fn apply_token_replaces_existing_line() {
        let out = apply_token_to_toml(SAMPLE, "dpapi:QUJD");
        assert!(out.contains("token = \"dpapi:QUJD\""));
        assert!(!out.contains("file:agent.token"));
        let cfg: AgentConfig = toml::from_str(&out).expect("re-parse");
        assert_eq!(cfg.gateway.token.as_deref(), Some("dpapi:QUJD"));
        // 其余字段保持不变
        assert_eq!(cfg.tunnel.resource_id, "rdp-office-01");
    }

    #[test]
    fn apply_token_inserts_when_absent() {
        let src = "[gateway]\nurl = \"https://easyrdp.cn\"\n\n[tunnel]\nresource_id = \"r1\"\n";
        let out = apply_token_to_toml(src, "dpapi:XYZ");
        let cfg: AgentConfig = toml::from_str(&out).expect("re-parse");
        assert_eq!(cfg.gateway.token.as_deref(), Some("dpapi:XYZ"));
    }

    #[test]
    fn apply_token_appends_gateway_when_section_missing() {
        let src = "[tunnel]\nresource_id = \"r1\"\n";
        let out = apply_token_to_toml(src, "dpapi:XYZ");
        // [gateway] 段与 token 行均已补齐（url 由用户手工补，此处不参与解析）
        assert!(out.contains("[gateway]"));
        assert!(out.contains("token = \"dpapi:XYZ\""));
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_token_inline_roundtrip_via_apply() {
        let stored = dpapi::protect("agent-jwt-token-abc").expect("protect");
        let blob = format!("dpapi:{stored}");
        let out = apply_token_to_toml(SAMPLE, &blob);
        let cfg: AgentConfig = toml::from_str(&out).expect("re-parse");
        assert_eq!(cfg.resolve_token().expect("token"), "agent-jwt-token-abc");
    }
}
