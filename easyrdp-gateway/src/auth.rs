//! 认证模块：OIDC 登录占位 + 短期 JWT 签发/校验 + Bearer 认证中间件。
//!
//! 认证流程对齐 RD Gateway 的 GatewayEncryptedAuthCookie 概念：
//! 用户在 IdP 完成 OIDC 登录与 MFA 后，网关签发一个短有效期（默认 10 分钟）
//! 的 JWT 作为"网关加密认证 cookie"，后续隧道/审计请求均凭 Bearer token 鉴权。

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::AppState;

/// 默认 token 有效期：10 分钟。
pub const DEFAULT_TOKEN_TTL: u64 = 600;

/// JWT claims。`exp` 为 Unix 秒；jsonwebtoken 会自动校验过期。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// 用户主体（如 OIDC subject / 工号）
    pub sub: String,
    /// 角色列表，如 ["ops", "admin"] 或 ["contractor"]
    pub roles: Vec<String>,
    /// 签发时间（Unix 秒）
    pub iat: u64,
    /// 过期时间（Unix 秒）
    pub exp: u64,
    /// 签发方
    pub iss: String,
}

/// 认证状态：签名密钥与 token 有效期。
#[derive(Clone)]
pub struct AuthState {
    secret: Arc<String>,
    ttl: Duration,
    issuer: String,
    /// P0 安全闸（R-20261120-008）：占位登录是否允许（[auth]
    /// allow_placeholder_login；缺省 true 向后兼容，生产必须 false）。
    placeholder_allowed: bool,
}

impl AuthState {
    pub fn new(secret: impl Into<String>, ttl_seconds: u64) -> Self {
        Self {
            secret: Arc::new(secret.into()),
            ttl: Duration::from_secs(ttl_seconds.max(1)),
            issuer: "easyrdp-gateway".to_string(),
            placeholder_allowed: true,
        }
    }

    /// 设置占位登录开关（builder 风格，main 启动时按 [auth] 配置注入）。
    pub fn with_placeholder_allowed(mut self, allowed: bool) -> Self {
        self.placeholder_allowed = allowed;
        self
    }

    /// 占位登录是否允许（[oidc] 未启用时的唯一占位闸门）。
    pub fn placeholder_login_allowed(&self) -> bool {
        self.placeholder_allowed
    }

    /// 当前生效的默认 token TTL 秒数（供 /admin/issue 缺省 expires_in 提示）。
    pub fn default_ttl_seconds(&self) -> u64 {
        self.ttl.as_secs()
    }

    /// 签发短期 JWT。
    pub fn issue_token(&self, subject: &str, roles: &[&str]) -> Result<String, AuthError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let claims = Claims {
            sub: subject.to_string(),
            roles: roles.iter().map(|r| r.to_string()).collect(),
            iat: now,
            exp: now + self.ttl.as_secs(),
            iss: self.issuer.clone(),
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret.as_bytes()),
        )
        .map_err(|_| AuthError::IssueFailed)
    }

    /// 校验 JWT：签名、过期、签发方。
    pub fn validate_token(&self, token: &str) -> Result<Claims, AuthError> {
        let mut validation = Validation::default();
        validation.set_issuer(&[&self.issuer]);
        decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.secret.as_bytes()),
            &validation,
        )
        .map(|data| data.claims)
        .map_err(|_| AuthError::InvalidToken)
    }

    /// 按指定 TTL 签发 token（F5-V2 控制台会话用 30min，API token 用默认 10min）。
    pub fn issue_token_with_ttl(
        &self,
        subject: &str,
        roles: &[&str],
        ttl_seconds: u64,
    ) -> Result<String, AuthError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let claims = Claims {
            sub: subject.to_string(),
            roles: roles.iter().map(|r| r.to_string()).collect(),
            iat: now,
            exp: now + ttl_seconds.max(1),
            iss: self.issuer.clone(),
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret.as_bytes()),
        )
        .map_err(|_| AuthError::IssueFailed)
    }
}

#[derive(Debug)]
pub enum AuthError {
    IssueFailed,
    InvalidToken,
}

/// 登录请求体。骨架阶段仅接收 OIDC code，真实接入后由网关向 IdP
/// （oidc_issuer）换 access_token/id_token，并校验 MFA 结论。
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub oidc_code: String,
    /// 可选：MFA 一次性验证码占位
    #[serde(default)]
    pub mfa_code: Option<String>,
}

/// 登录响应：签发的短期 token 与过期秒数。
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub expires_in: u64,
    pub token_type: &'static str,
}

/// POST /api/auth/login —— OIDC code 占位处理。
///
/// 骨架阶段：不真正对接 IdP，而是把 `oidc_code` 里的 `sub:role1,role2`
/// 格式解析为身份（便于联调测试），并签发短期 JWT。
/// 真实实现应：code 换 token -> 校验 id_token 签名/aud -> 读取用户角色 -> 签发 JWT。
async fn login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): axum::extract::Json<LoginRequest>,
) -> Result<Json<LoginResponse>, (StatusCode, String)> {
    // 审计归因：真实来源 IP（X-Real-IP / X-Forwarded-For，nginx 反代注入）。
    // 修复点：此前 auth_placeholder_denied 事件来源 IP 硬编码 "-"，无法归因。
    let src_ip = crate::console::client_ip_from_headers(&headers);
    // F5-V2：配置 [oidc] 后占位登录自动禁用；仅 allow_placeholder_login=true
    // （联调）时可显式保留。未配置 [oidc] 时占位登录保持可用（向后兼容）。
    if let Some(oidc) = &state.oidc {
        if !oidc.allow_placeholder_login() {
            tracing::warn!(src = %src_ip, "placeholder login rejected: OIDC configured");
            let mut ev = crate::audit::AuditEvent::new("-", None, "auth_placeholder_denied", "-", src_ip.clone());
            ev.error = Some("placeholder login disabled by oidc config".into());
            state.audit.record(ev);
            return Err((
                StatusCode::FORBIDDEN,
                "占位登录已禁用（已配置 [oidc]，请走 /oidc/login）".into(),
            ));
        }
    }
    // P0 安全闸（R-20261120-008）：[auth] allow_placeholder_login = false 时，
    // 即使未配置 [oidc] 也强制禁用占位登录（生产环境必须置 false）。
    if state.oidc.is_none() && !state.auth.placeholder_login_allowed() {
        // 归因增强：即使被拒也解析 attempted code 的主体，便于区分"老版本客户端
        // 的真实用户重试"与"匿名扫描探测"（解析失败保持 "-"）。
        let (attempt_sub, _) = parse_placeholder_code(&req.oidc_code);
        let user_field = if attempt_sub.is_empty() { "-".to_string() } else { format!("attempt:{attempt_sub}") };
        tracing::warn!(src = %src_ip, attempt_sub = %attempt_sub, "placeholder login rejected: auth.allow_placeholder_login=false");
        let mut ev = crate::audit::AuditEvent::new(user_field, None, "auth_placeholder_denied", "-", src_ip);
        ev.error = Some("placeholder login disabled by config".into());
        state.audit.record(ev);
        return Err((
            StatusCode::FORBIDDEN,
            "占位登录已禁用（auth.allow_placeholder_login = false）".into(),
        ));
    }
    if req.oidc_code.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "oidc_code 不能为空".into()));
    }

    // TODO(真实接入): 调用 OIDC token endpoint 换取 id_token 并校验 MFA。
    let (subject, roles) = parse_placeholder_code(&req.oidc_code);

    // F4-V1（R-20261118-012）OTP 二次认证：配置了 totp_secret 的用户在
    // OIDC 占位身份解析成功后必须再过一道 TOTP（RFC 6238，30s 步长 ±1
    // 窗口容差）。缺失或校验失败统一返回 401 "otp_required"（客户端据此
    // 弹 OTP 输入并重试）。
    if let Some(secret) = state.totp_secrets.get(&subject) {
        let code = req.mfa_code.as_deref().map(str::trim).unwrap_or("");
        let ok = !code.is_empty()
            && verify_totp(secret, code, 1, SystemTime::now());
        if !ok {
            tracing::info!(%subject, "otp required/invalid for user");
            return Err((StatusCode::UNAUTHORIZED, "otp_required".into()));
        }
    }

    let token = state
        .auth
        .issue_token(&subject, &roles.iter().map(String::as_str).collect::<Vec<_>>())
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "签发 token 失败".into()))?;

    tracing::info!(%subject, "issued short-lived gateway token");
    Ok(Json(LoginResponse {
        token,
        expires_in: DEFAULT_TOKEN_TTL,
        token_type: "Bearer",
    }))
}

/// 解析占位 code："zhangsan:ops,admin" -> ("zhangsan", ["ops","admin"])。
pub fn parse_placeholder_code(code: &str) -> (String, Vec<String>) {
    match code.split_once(':') {
        Some((sub, roles)) if !sub.is_empty() => (
            sub.to_string(),
            roles
                .split(',')
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .map(str::to_string)
                .collect(),
        ),
        _ => (code.to_string(), vec!["user".to_string()]),
    }
}

/// auth_guard：校验 `Authorization: Bearer <token>`，将 Claims 注入请求扩展。
pub async fn auth_guard(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let token = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);

    let Some(token) = token else {
        return (StatusCode::UNAUTHORIZED, "缺少 Bearer token").into_response();
    };

    match state.auth.validate_token(token) {
        Ok(claims) => {
            req.extensions_mut().insert(claims);
            next.run(req).await
        }
        Err(AuthError::InvalidToken) => (StatusCode::UNAUTHORIZED, "token 无效或已过期").into_response(),
        Err(AuthError::IssueFailed) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "内部认证错误").into_response()
        }
    }
}

/// 是否拥有 admin 角色（供审计查询等使用）。
pub fn is_admin(claims: &Claims) -> bool {
    claims.roles.iter().any(|r| r == "admin")
}

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let mut r = Router::new().route("/api/auth/login", post(login));
    if state.oidc.is_some() {
        r = r
            .route("/oidc/login", get(oidc_login))
            .route("/oidc/callback", get(oidc_callback))
            .route("/oidc/otp", get(oidc_otp_page).post(oidc_otp_submit));
    }
    r.with_state(state)
}

// ---------------------------------------------------------------------
// F4-V1：TOTP（RFC 6238）手写实现
// ---------------------------------------------------------------------
// 选型说明（三选一）：**手写 TOTP**（RFC 6238 核心算法 + RustCrypto
// `hmac 0.12` + `sha1 0.10` 原语 + 自写 RFC 4648 Base32 解码）。
// 不选 `otp` / `totp-rs`：两者为承载 otpauth URL/issuer/algorithm 等特性
// 引入 base32/url 等额外依赖树；本项目只需「Base32 密钥 + HMAC-SHA1 +
// 30s 步长 ±1 窗口」这一条固定路径，手写约 60 行且完全可单测。

/// TOTP 步长（RFC 6238 默认 30 秒）。
pub const TOTP_STEP: u64 = 30;
/// 默认 6 位数字码长度。
pub const TOTP_DIGITS: u32 = 6;

/// RFC 4648 Base32 解码（A-Z/2-7；容忍小写、空格与 `=` 填充）。
/// 非法字符返回 None。
pub fn base32_decode(input: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = Vec::new();
    let mut bits: u32 = 0;
    let mut acc: u32 = 0;
    for ch in input.chars() {
        if ch == '=' || ch == ' ' || ch == '-' {
            continue;
        }
        let upper = ch.to_ascii_uppercase();
        let idx = ALPHABET.iter().position(|&a| a == upper as u8)? as u32;
        acc = (acc << 5) | idx;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

/// HOTP 动态截断（RFC 4226 5.3）：HMAC-SHA1 20 字节 -> 6 位十进制码。
fn hotp_code(key: &[u8], counter: u64) -> String {
    use hmac::{Hmac, Mac};
    use sha1::Sha1;
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0F) as usize;
    let bin = u32::from_be_bytes([
        digest[offset] & 0x7F,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    format!("{:0width$}", bin % 10u32.pow(TOTP_DIGITS), width = TOTP_DIGITS as usize)
}

/// TOTP：给定 Base32 密钥与 Unix 秒时刻，计算该时刻所在 30s 步长的 6 位码。
pub fn totp_code_at(secret_base32: &str, unix_seconds: u64) -> Option<String> {
    let key = base32_decode(secret_base32)?;
    Some(hotp_code(&key, unix_seconds / TOTP_STEP))
}

/// TOTP 校验：当前时刻及 ±`tolerance` 个窗口内任一码匹配即通过
/// （±1 窗口 = 接受前后 30 秒内的时钟漂移）。
pub fn verify_totp(secret_base32: &str, code: &str, tolerance: u64, now: SystemTime) -> bool {
    let Some(key) = base32_decode(secret_base32) else {
        return false;
    };
    let now_secs = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let current = now_secs / TOTP_STEP;
    let code = code.trim();
    (0..=tolerance)
        .flat_map(|d| [current.wrapping_sub(d), current.wrapping_add(d)])
        .any(|step| hotp_code(&key, step) == code)
}

// ---------------------------------------------------------------------
// F5-V2（R-20261118-018）：OIDC Authorization Code 流真实对接
// ---------------------------------------------------------------------
// 端点：GET /oidc/login（302 跳 IdP）→ GET /oidc/callback（state 校验 + code
// 换 token + id_token 签名校验（JWKS/kid）→ claims 映射用户与角色 → 签发
// 网关控制台会话 cookie）。配置了本地 totp_secret 的用户在 OIDC 身份解析
// 成功后追加本地 OTP 步骤（GET /oidc/otp 页 + POST /oidc/otp 校验），保持
// F4-V1「本地 OTP 用户必须二次认证」的既有安全不变量。
//
// IdP 网络交互通过 [`TokenFetcher`] trait 注入：生产实现 [`HttpIdpClient`]
// （reqwest + rustls），单测注入 [`MockIdpClient`]（测试模块内）覆盖
// discovery / token / JWKS 三个动作，全流程不联网。

/// OIDC state 防 CSRF 的有效期（秒）。
pub const OIDC_STATE_TTL: u64 = 600;
/// JWKS 缓存有效期（秒）。
pub const JWKS_CACHE_TTL: u64 = 600;
/// discovery 文档缓存有效期（秒）。
pub const DISCOVERY_CACHE_TTL: u64 = 3600;
/// 控制台会话 cookie TTL（秒）：30 分钟滑动过期。
pub const CONSOLE_SESSION_TTL: u64 = 1800;

/// token 端点响应（MVP 只消费 access_token 与 id_token）。
#[derive(Debug, Clone)]
pub struct TokenResponse {
    /// access_token（当前仅透传保留；网关资源访问凭自家 JWT，不回传给 IdP）
    #[allow(dead_code)]
    pub access_token: String,
    pub id_token: String,
}

impl TokenResponse {
    fn from_json(v: serde_json::Value) -> Result<Self, String> {
        let access_token = v
            .get("access_token")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "token 响应缺 access_token".to_string())?
            .to_string();
        let id_token = v
            .get("id_token")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "token 响应缺 id_token".to_string())?
            .to_string();
        Ok(Self {
            access_token,
            id_token,
        })
    }
}

/// IdP HTTP 动作抽象（对象安全，供单测注入 mock）。
pub trait TokenFetcher: Send + Sync {
    /// POST token 端点（表单编码），返回原始 JSON。
    fn exchange_code<'a>(
        &'a self,
        token_endpoint: &'a str,
        form: &'a [(String, String)],
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, String>> + Send + 'a>>;
    /// GET 任意 URL（discovery / JWKS），返回响应体文本。
    fn fetch_text<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
}

/// 生产实现：reqwest + rustls（webpki-roots），异步原生。
pub struct HttpIdpClient {
    client: reqwest::Client,
}

impl HttpIdpClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client build"),
        }
    }
}

impl Default for HttpIdpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenFetcher for HttpIdpClient {
    fn exchange_code<'a>(
        &'a self,
        token_endpoint: &'a str,
        form: &'a [(String, String)],
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let resp = self
                .client
                .post(token_endpoint)
                .form(form)
                .send()
                .await
                .map_err(|e| format!("token 端点请求失败: {e}"))?;
            let status = resp.status();
            let body = resp
                .text()
                .await
                .map_err(|e| format!("token 响应读取失败: {e}"))?;
            if !status.is_success() {
                return Err(format!("token 端点返回 {status}: {}", truncate(&body, 300)));
            }
            serde_json::from_str(&body).map_err(|e| format!("token 响应非 JSON: {e}"))
        })
    }

    fn fetch_text<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let resp = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|e| format!("请求 {url} 失败: {e}"))?;
            let status = resp.status();
            let body = resp
                .text()
                .await
                .map_err(|e| format!("响应读取失败: {e}"))?;
            if !status.is_success() {
                return Err(format!("{url} 返回 {status}"));
            }
            Ok(body)
        })
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}…", &s[..cut])
    }
}

/// OIDC discovery 文档（MVP 只取 4 个必需字段）。
#[derive(Debug, Clone)]
pub struct DiscoveryDoc {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

impl DiscoveryDoc {
    fn from_json(v: &serde_json::Value) -> Result<Self, String> {
        let get = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .map(str::to_string)
                .ok_or_else(|| format!("discovery 文档缺 {k}"))
        };
        Ok(Self {
            issuer: get("issuer")?,
            authorization_endpoint: get("authorization_endpoint")?,
            token_endpoint: get("token_endpoint")?,
            jwks_uri: get("jwks_uri")?,
        })
    }
}

/// pending state 的用途：Login（防 CSRF 一次性）或 Otp（OIDC 身份已解析、
/// 等待本地 OTP 二次认证）。
#[derive(Debug, Clone)]
enum PendingKind {
    Login,
    Otp { user: String, roles: Vec<String> },
}

/// OIDC 运行态：配置 + 发现/JWKS 缓存 + 一次性 state 表。
pub struct OidcState {
    pub cfg: crate::config::OidcConfig,
    fetcher: Arc<dyn TokenFetcher>,
    discovery: tokio::sync::RwLock<Option<(DiscoveryDoc, Instant)>>,
    jwks: tokio::sync::RwLock<Option<(String, Instant)>>,
    pending: std::sync::Mutex<Vec<(String, PendingKind, u64)>>,
}

impl OidcState {
    pub fn new(cfg: crate::config::OidcConfig, fetcher: Arc<dyn TokenFetcher>) -> Self {
        Self {
            cfg,
            fetcher,
            discovery: tokio::sync::RwLock::new(None),
            jwks: tokio::sync::RwLock::new(None),
            pending: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// 占位登录是否显式保留（配置 [oidc] 后默认禁用）。
    pub fn allow_placeholder_login(&self) -> bool {
        self.cfg.allow_placeholder_login.unwrap_or(false)
    }

    fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// 登记一次性 state（顺带清理过期项）。
    fn push_pending(&self, state: &str, kind: PendingKind) {
        let mut pend = self.pending.lock().expect("oidc pending mutex");
        let now = Self::now_unix();
        pend.retain(|(_, _, exp)| *exp > now);
        pend.push((state.to_string(), kind, now + OIDC_STATE_TTL));
    }

    /// 取出并消费一次性 state；不存在或已过期返回 None（防重放）。
    fn pop_pending(&self, state: &str) -> Option<PendingKind> {
        let mut pend = self.pending.lock().expect("oidc pending mutex");
        let now = Self::now_unix();
        if let Some(pos) = pend
            .iter()
            .position(|(s, _, exp)| s == state && *exp > now)
        {
            Some(pend.remove(pos).1)
        } else {
            None
        }
    }

    /// discovery 文档（缓存 1h）。URL：显式 discovery_url 优先，否则
    /// `{issuer 去尾斜杠}/.well-known/openid-configuration`。
    pub async fn discovery(&self) -> Result<DiscoveryDoc, String> {
        {
            let cached = self.discovery.read().await;
            if let Some((doc, at)) = cached.as_ref() {
                if at.elapsed().as_secs() < DISCOVERY_CACHE_TTL {
                    return Ok(doc.clone());
                }
            }
        }
        let url = self.cfg.discovery_url.as_deref().map(str::trim).filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!(
                "{}/.well-known/openid-configuration",
                self.cfg.issuer.as_deref().unwrap_or("").trim_end_matches('/')
            ));
        let text = self.fetcher.fetch_text(&url).await?;
        let doc = DiscoveryDoc::from_json(&serde_json::from_str::<serde_json::Value>(&text).map_err(|e| format!("discovery 响应非 JSON: {e}"))?)?;
        // issuer 精确匹配（RFC：配置了 discovery_url 时仍校验 issuer 一致）
        let cfg_issuer = self.cfg.issuer.as_deref().unwrap_or("").trim_end_matches('/');
        let doc_issuer = doc.issuer.trim_end_matches('/');
        if !cfg_issuer.is_empty() && doc_issuer != cfg_issuer {
            return Err(format!(
                "discovery issuer 不匹配: 文档 {doc_issuer:?} != 配置 {cfg_issuer:?}"
            ));
        }
        *self.discovery.write().await = Some((doc.clone(), Instant::now()));
        Ok(doc)
    }

    /// JWKS 原文（缓存 10min）。
    pub async fn jwks_raw(&self) -> Result<String, String> {
        {
            let cached = self.jwks.read().await;
            if let Some((raw, at)) = cached.as_ref() {
                if at.elapsed().as_secs() < JWKS_CACHE_TTL {
                    return Ok(raw.clone());
                }
            }
        }
        let doc = self.discovery().await?;
        let raw = self.fetcher.fetch_text(&doc.jwks_uri).await?;
        *self.jwks.write().await = Some((raw.clone(), Instant::now()));
        Ok(raw)
    }
}

/// RFC 3986 unreserved 之外的字符百分号编码（authorize URL query 用）。
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// GET /oidc/login —— 302 跳 IdP authorize 端点。
async fn oidc_login(State(state): State<Arc<AppState>>) -> Response {
    let Some(oidc) = state.oidc.as_ref() else {
        return (StatusCode::NOT_FOUND, "OIDC 未配置").into_response();
    };
    let doc = match oidc.discovery().await {
        Ok(d) => d,
        Err(err) => {
            tracing::warn!(%err, "oidc discovery failed");
            return (StatusCode::BAD_GATEWAY, format!("IdP discovery 失败: {err}"))
                .into_response();
        }
    };
    let csrf_state = uuid::Uuid::new_v4().to_string();
    oidc.push_pending(&csrf_state, PendingKind::Login);
    let cfg = &oidc.cfg;
    let url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}",
        doc.authorization_endpoint,
        percent_encode(cfg.client_id.as_deref().unwrap_or("")),
        percent_encode(cfg.redirect_uri.as_deref().unwrap_or("")),
        percent_encode(cfg.effective_scopes()),
        percent_encode(&csrf_state),
    );
    tracing::info!("oidc login redirect issued");
    (
        StatusCode::FOUND,
        [("Location", url)],
    )
        .into_response()
}

/// id_token claims（MVP 字段；roles/groups 由 roles_claim 配置决定，另行提取）。
#[cfg(test)]
#[derive(Debug, Deserialize)]
struct IdTokenMinimal {
    #[allow(dead_code)]
    sub: String,
}

/// 从 claims JSON 中按配置的 roles_claim 提取角色列表。
/// 顺序：roles_claim（默认 "roles"）→ "groups" → ["user"]。
fn extract_roles(claims: &serde_json::Value, roles_claim: &str) -> Vec<String> {
    fn as_string_list(v: &serde_json::Value) -> Option<Vec<String>> {
        match v {
            serde_json::Value::Array(items) => Some(
                items
                    .iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .filter(|s| !s.trim().is_empty())
                    .collect(),
            ),
            serde_json::Value::String(s) if !s.trim().is_empty() => Some(
                s.split(|c: char| c == ',' || c == ' ')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
            ),
            _ => None,
        }
    }
    for key in [roles_claim, "groups"] {
        if let Some(list) = claims.get(key).and_then(as_string_list) {
            if !list.is_empty() {
                return list;
            }
        }
    }
    vec!["user".to_string()]
}

/// 校验 id_token：签名（JWKS kid 匹配）+ iss/aud/exp。返回完整 claims JSON。
pub async fn validate_id_token(
    oidc: &OidcState,
    id_token: &str,
) -> Result<serde_json::Value, String> {
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        return Err("id_token 格式非法（应为 3 段 JWT）".into());
    }
    // header：要求 kid，且 alg 属于支持的 {RS256, ES256}
    let header_json = base64url_decode_json(parts[0])?;
    let kid = header_json
        .get("kid")
        .and_then(|k| k.as_str())
        .ok_or_else(|| "id_token header 缺 kid（本网关要求 JWKS kid 匹配）".to_string())?
        .to_string();
    let alg = header_json.get("alg").and_then(|a| a.as_str()).unwrap_or("");
    let algorithm = match alg {
        "RS256" => Algorithm::RS256,
        "ES256" => Algorithm::ES256,
        other => return Err(format!("不支持的 id_token 算法: {other:?}")),
    };

    let jwks_raw = oidc.jwks_raw().await?;
    let jwk_set: jsonwebtoken::jwk::JwkSet = serde_json::from_str(&jwks_raw)
        .map_err(|e| format!("JWKS 解析失败: {e}"))?;
    let jwk = jwk_set
        .find(&kid)
        .ok_or_else(|| format!("JWKS 中无 kid={kid:?} 的密钥（拒绝）"))?;
    let decoding = DecodingKey::from_jwk(jwk).map_err(|e| format!("JWK 构建失败: {e}"))?;

    let mut validation = Validation::new(algorithm);
    validation.set_issuer(&[oidc.cfg.issuer.as_deref().unwrap_or("")]);
    validation.set_audience(&[oidc.cfg.client_id.as_deref().unwrap_or("")]);
    validation.leeway = 60;
    validation.validate_aud = true;

    let data = decode::<serde_json::Value>(id_token, &decoding, &validation)
        .map_err(|e| format!("id_token 校验失败: {e}"))?;
    Ok(data.claims)
}

/// base64url 解码（无填充容忍）→ UTF-8 JSON。
fn base64url_decode_json(seg: &str) -> Result<serde_json::Value, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(seg.trim_end_matches('='))
        .map_err(|e| format!("JWT 段 base64url 解码失败: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("JWT 段非 JSON: {e}"))
}

/// 完成登录：签发控制台会话 cookie + 302 到 /console + 审计 console_login。
pub fn finish_session(
    state: &AppState,
    user: &str,
    roles: &[String],
    ip: &str,
) -> Result<axum::response::Response, String> {
    let token = state
        .auth
        .issue_token_with_ttl(
            user,
            &roles.iter().map(String::as_str).collect::<Vec<_>>(),
            CONSOLE_SESSION_TTL,
        )
        .map_err(|_| "签发会话失败".to_string())?;
    state.audit.record(crate::audit::AuditEvent::new(
        user,
        Some(crate::policy::canonical_role(roles).as_str()),
        "console_login",
        "-",
        ip,
    ));
    Ok(crate::console::session_redirect(&token))
}

/// GET /oidc/callback —— state 校验 → code 换 token → id_token 校验 →
/// （可选本地 OTP）→ 控制台会话。
async fn oidc_callback(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let ip = crate::console::client_ip_from_headers(&headers);
    let Some(oidc) = state.oidc.as_ref() else {
        return (StatusCode::NOT_FOUND, "OIDC 未配置").into_response();
    };

    // IdP 侧错误（用户取消等）显式透出
    if let Some(err) = q.get("error") {
        let desc = q.get("error_description").cloned().unwrap_or_default();
        record_oidc_fail(&state, "-", &ip, &format!("IdP 返回 error={err} {desc}"));
        return (StatusCode::UNAUTHORIZED, format!("IdP 错误: {err} {desc}")).into_response();
    }

    let code = q.get("code").map(String::as_str).unwrap_or("");
    let rsp_state = q.get("state").map(String::as_str).unwrap_or("");
    if rsp_state.is_empty() {
        return (StatusCode::BAD_REQUEST, "缺少 state 参数（防 CSRF）").into_response();
    }
    if code.is_empty() {
        return (StatusCode::BAD_REQUEST, "缺少 code 参数").into_response();
    }
    // state：一次性 + 有效期（缺失/过期/重放一律拒绝）
    let Some(PendingKind::Login) = oidc.pop_pending(rsp_state) else {
        record_oidc_fail(&state, "-", &ip, "state 校验失败");
        return (StatusCode::UNAUTHORIZED, "state 无效或已过期（防 CSRF 拒绝）").into_response();
    };

    let doc = match oidc.discovery().await {
        Ok(d) => d,
        Err(err) => {
            record_oidc_fail(&state, "-", &ip, &format!("discovery 失败: {err}"));
            return (StatusCode::BAD_GATEWAY, format!("IdP discovery 失败: {err}")).into_response();
        }
    };
    let form = [
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code.to_string()),
        ("redirect_uri".to_string(), oidc.cfg.redirect_uri.clone().unwrap_or_default()),
        ("client_id".to_string(), oidc.cfg.client_id.clone().unwrap_or_default()),
        ("client_secret".to_string(), oidc.cfg.client_secret.clone().unwrap_or_default()),
    ];
    let token_json = match oidc.fetcher.exchange_code(&doc.token_endpoint, &form).await {
        Ok(v) => v,
        Err(err) => {
            record_oidc_fail(&state, "-", &ip, &format!("code 换 token 失败: {err}"));
            return (StatusCode::UNAUTHORIZED, format!("code 换 token 失败: {err}")).into_response();
        }
    };
    let token = match TokenResponse::from_json(token_json) {
        Ok(t) => t,
        Err(err) => {
            record_oidc_fail(&state, "-", &ip, &err);
            return (StatusCode::UNAUTHORIZED, err).into_response();
        }
    };
    let claims = match validate_id_token(oidc, &token.id_token).await {
        Ok(c) => c,
        Err(err) => {
            record_oidc_fail(&state, "-", &ip, &err);
            return (StatusCode::UNAUTHORIZED, err).into_response();
        }
    };
    let sub = claims
        .get("sub")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    if sub.is_empty() {
        record_oidc_fail(&state, "-", &ip, "id_token 缺 sub");
        return (StatusCode::UNAUTHORIZED, "id_token 缺 sub").into_response();
    }
    let roles = extract_roles(&claims, oidc.cfg.effective_roles_claim());

    // F4-V1 不变量保持：本地配置了 totp_secret 的用户，OIDC 身份解析后仍须
    // 过本地 OTP（IdP MFA 不能替代本地 OTP）。
    if state.totp_secrets.contains_key(&sub) {
        let otp_state = uuid::Uuid::new_v4().to_string();
        oidc.push_pending(&otp_state, PendingKind::Otp { user: sub.clone(), roles: roles.clone() });
        return axum::response::Redirect::to(&format!("/oidc/otp?state={otp_state}"))
            .into_response();
    }

    match finish_session(&state, &sub, &roles, &ip) {
        Ok(resp) => {
            tracing::info!(%sub, "oidc console session issued");
            resp
        }
        Err(err) => {
            record_oidc_fail(&state, &sub, &ip, &err);
            (StatusCode::INTERNAL_SERVER_ERROR, err).into_response()
        }
    }
}

fn record_oidc_fail(state: &AppState, user: &str, ip: &str, err: &str) {
    tracing::warn!(%user, %err, "oidc callback failed");
    let mut ev = crate::audit::AuditEvent::new(user, None, "oidc_callback_failed", "-", ip);
    ev.error = Some(err.to_string());
    state.audit.record(ev);
}

/// GET /oidc/otp —— 本地 OTP 二次认证页（peek pending，不消费）。
async fn oidc_otp_page(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let st = q.get("state").map(String::as_str).unwrap_or("");
    let kind = state.oidc.as_ref().and_then(|o| o.peek_pending(st));
    if !matches!(kind, Some(PendingKind::Otp { user, .. }) if !user.is_empty()) {
        return (StatusCode::UNAUTHORIZED, "OTP 会话无效或已过期").into_response();
    }
    let (csrf, set_cookie) = crate::console::new_csrf_cookie();
    crate::console::page_otp(st, &csrf, set_cookie)
}

/// peek 语义：不消费 pending（供 OTP 页面 GET 使用）。
impl OidcState {
    fn peek_pending(&self, state: &str) -> Option<PendingKind> {
        let pend = self.pending.lock().expect("oidc pending mutex");
        let now = Self::now_unix();
        pend.iter()
            .find(|(s, _, exp)| s == state && *exp > now)
            .map(|(_, k, _)| k.clone())
    }
}

/// POST /oidc/otp —— 校验 OTP + CSRF，签发会话。
#[derive(Debug, serde::Deserialize)]
pub struct OtpForm {
    pub state: String,
    pub otp_code: String,
    pub csrf_token: String,
}

async fn oidc_otp_submit(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::Form(form): axum::Form<OtpForm>,
) -> Response {
    let ip = crate::console::client_ip_from_headers(&headers);
    let Some(oidc) = state.oidc.as_ref() else {
        return (StatusCode::NOT_FOUND, "OIDC 未配置").into_response();
    };
    if !crate::console::csrf_ok(&headers, &form.csrf_token) {
        return (StatusCode::FORBIDDEN, "CSRF token 缺失或不匹配").into_response();
    }
    let kind = oidc.pop_pending(&form.state);
    let Some(PendingKind::Otp { user, roles }) = kind else {
        return (StatusCode::UNAUTHORIZED, "OTP 会话无效或已过期").into_response();
    };
    if user.is_empty() {
        return (StatusCode::UNAUTHORIZED, "OTP 会话无效").into_response();
    }
    let secret = state.totp_secrets.get(&user).cloned();
    let ok = secret
        .as_deref()
        .map(|s| verify_totp(s, &form.otp_code, 1, SystemTime::now()))
        .unwrap_or(false);
    if !ok {
        let mut ev = crate::audit::AuditEvent::new(&user, None, "console_login_failed", "-", &ip);
        ev.error = Some("otp_invalid".into());
        state.audit.record(ev);
        return (StatusCode::UNAUTHORIZED, "otp_invalid").into_response();
    }
    match finish_session(&state, &user, &roles, &ip) {
        Ok(resp) => resp,
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err).into_response(),
    }
}

// ---------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AuthState {
        AuthState::new("test-secret", DEFAULT_TOKEN_TTL)
    }

    #[test]
    fn issue_token_contains_subject_and_roles() {
        let s = state();
        let token = s.issue_token("alice", &["ops", "admin"]).expect("issue");
        let claims = s.validate_token(&token).expect("validate");
        assert_eq!(claims.sub, "alice");
        assert_eq!(claims.roles, vec!["ops".to_string(), "admin".to_string()]);
        assert_eq!(claims.iss, "easyrdp-gateway");
        // 短期语义：exp - iat == 600s
        assert_eq!(claims.exp - claims.iat, DEFAULT_TOKEN_TTL);
    }

    #[test]
    fn valid_token_passes_validation() {
        let s = state();
        let token = s.issue_token("bob", &["user"]).expect("issue");
        // 正常签发后立刻校验应通过（未过期、签名正确、issuer 匹配）
        assert!(s.validate_token(&token).is_ok());
        // 错误密钥应失败
        let other = AuthState::new("wrong-secret", DEFAULT_TOKEN_TTL);
        assert!(matches!(
            other.validate_token(&token),
            Err(AuthError::InvalidToken)
        ));
    }

    #[test]
    fn expired_token_is_rejected() {
        // 直接手工编码一个 exp 已过期 10 分钟的 claims（默认 leeway 60s，
        // 过期 10 分钟可确定性触发过期路径）。
        let s = state();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let claims = Claims {
            sub: "carol".to_string(),
            roles: vec!["user".to_string()],
            iat: now - 1_200,
            exp: now - 600,
            iss: "easyrdp-gateway".to_string(),
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(b"test-secret"),
        )
        .expect("encode expired claims");
        assert!(matches!(
            s.validate_token(&token),
            Err(AuthError::InvalidToken)
        ));
    }

    // -----------------------------------------------------------------
    // F4-V1：TOTP（生成 / 校验 / 窗口容差 / 拒绝）
    // -----------------------------------------------------------------

    /// RFC 6238 附录 B 参考向量（HMAC-SHA1，密钥 "12345678901234567890" 的
    /// Base32 = GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ）：T=59 → 94287082 截取
    /// 6 位 = 287082。
    #[test]
    fn totp_matches_rfc6238_reference_vector() {
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        let code = totp_code_at(secret, 59).expect("decode");
        assert_eq!(code, "287082");
    }

    /// base32 解码：大小写 / 填充 / 空格容忍；非法字符拒绝。
    #[test]
    fn base32_decode_variants() {
        assert_eq!(base32_decode("JBSWY3DP"), Some(b"Hello".to_vec()));
        assert_eq!(base32_decode("jbswy3dp"), base32_decode("JBSWY3DP"));
        assert_eq!(base32_decode("JBSWY3DP==="), base32_decode("JBSWY3DP"));
        assert_eq!(base32_decode("JB SW-Y3DP"), base32_decode("JBSWY3DP"));
        assert_eq!(base32_decode("JBSWY3DP1"), None, "1 非法 base32 字符");
    }

    /// 同一时刻生成 == 校验通过；换窗口（±31s → 相邻步长）在 ±1 容差内通过。
    #[test]
    fn verify_totp_window_tolerance() {
        let secret = "JBSWY3DPEHPK3PXP";
        let t0 = UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        let code = totp_code_at(secret, 1_000_000).unwrap();
        assert!(verify_totp(secret, &code, 1, t0), "当前窗口应通过");
        // 前一窗口（-30s）与后一窗口（+30s）：±1 容差内应通过
        let prev = totp_code_at(secret, 1_000_000 - TOTP_STEP).unwrap();
        let next = totp_code_at(secret, 1_000_000 + TOTP_STEP).unwrap();
        assert!(verify_totp(secret, &prev, 1, t0));
        assert!(verify_totp(secret, &next, 1, t0));
        // ±2 窗口（-60s/+60s）：±1 容差应拒绝
        let prev2 = totp_code_at(secret, 1_000_000 - 2 * TOTP_STEP).unwrap();
        let next2 = totp_code_at(secret, 1_000_000 + 2 * TOTP_STEP).unwrap();
        assert!(!verify_totp(secret, &prev2, 1, t0));
        assert!(!verify_totp(secret, &next2, 1, t0));
        // 容差 0 时相邻窗口拒绝
        assert!(!verify_totp(secret, &prev, 0, t0));
    }

    /// 拒绝路径：错码 / 空码 / 非法密钥 / 非数字串。
    #[test]
    fn verify_totp_rejects_bad_input() {
        let secret = "JBSWY3DPEHPK3PXP";
        let t0 = UNIX_EPOCH + std::time::Duration::from_secs(2_000_000);
        let code = totp_code_at(secret, 2_000_000).unwrap();
        // 篡改一位数字
        let bad: String = if code.starts_with('0') {
            format!("1{}", &code[1..])
        } else {
            format!("0{}", &code[1..])
        };
        assert_ne!(bad, code);
        assert!(!verify_totp(secret, &bad, 1, t0));
        assert!(!verify_totp(secret, "", 1, t0), "空码拒绝");
        assert!(!verify_totp("NOT@VALID32!", &code, 1, t0), "非法密钥拒绝");
        assert!(!verify_totp(secret, "abcdef", 1, t0));
    }

    // -----------------------------------------------------------------
    // F5-V2：OIDC Authorization Code 流（mock IdP：discovery/token/JWKS 全内联，
    // ES256 测试密钥由 rcgen 运行时生成，不联网）
    // -----------------------------------------------------------------

    use axum::body::Body;
    use axum::http::{Request, StatusCode as HttpCode};
    use tower::ServiceExt;

    /// 运行时生成 ES256 (P-256) 测试密钥，返回 (PEM 私钥, JWKS JSON)。
    fn es256_key_and_jwks(kid: &str) -> (String, String) {
        let kp = rcgen::KeyPair::generate().expect("generate P-256 key");
        let pem = kp.serialize_pem();
        let spki = kp.public_key_der(); // 26 字节头部 + 65 字节未压缩点
        assert_eq!(spki.len(), 91, "P-256 SPKI 应为 91 字节");
        let point = &spki[26..];
        assert_eq!(point[0], 0x04);
        let x = &point[1..33];
        let y = &point[33..65];
        let jwks = format!(
            r#"{{"keys":[{{"kty":"EC","crv":"P-256","kid":"{kid}","use":"sig","x":"{}","y":"{}"}}]}}"#,
            b64url(x),
            b64url(y)
        );
        (pem, jwks)
    }

    fn b64url(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    /// 用 ES256 私钥签发 id_token（header 带 kid）。
    fn sign_id_token(key_pem: &str, claims: serde_json::Value, kid: &str) -> String {
        let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
        header.kid = Some(kid.to_string());
        jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_ec_pem(key_pem.as_bytes()).expect("ec pem"),
        )
        .expect("sign id_token")
    }

    struct MockIdp {
        discovery: String,
        jwks: String,
        id_token: std::sync::Mutex<Option<String>>,
    }

    impl TokenFetcher for MockIdp {
        fn exchange_code<'a>(
            &'a self,
            _token_endpoint: &'a str,
            _form: &'a [(String, String)],
        ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, String>> + Send + 'a>> {
            let id_token = self
                .id_token
                .lock()
                .unwrap()
                .clone()
                .expect("测试需预置 id_token");
            Box::pin(async move {
                Ok(serde_json::json!({
                    "access_token": "at-123",
                    "token_type": "Bearer",
                    "id_token": id_token,
                }))
            })
        }

        fn fetch_text<'a>(
            &'a self,
            url: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
            let body = if url.ends_with("/jwks") {
                self.jwks.clone()
            } else {
                self.discovery.clone()
            };
            Box::pin(async move { Ok(body) })
        }
    }

    const ISSUER: &str = "https://sso.test/realms/m";
    const CLIENT_ID: &str = "easyrdp-gateway";

    fn mock_oidc_state(id_token: String, jwks: String, allow_placeholder: bool) -> OidcState {
        OidcState::new(
            crate::config::OidcConfig {
                issuer: Some(ISSUER.to_string()),
                discovery_url: None,
                client_id: Some(CLIENT_ID.to_string()),
                client_secret: Some("topsecret".to_string()),
                redirect_uri: Some("https://gw.test/oidc/callback".to_string()),
                scopes: Some("openid profile roles".to_string()),
                roles_claim: None, // 缺省 "roles"
                allow_placeholder_login: Some(allow_placeholder),
            },
            Arc::new(MockIdp {
                discovery: format!(
                    r#"{{"issuer":"{ISSUER}","authorization_endpoint":"https://sso.test/auth","token_endpoint":"https://sso.test/token","jwks_uri":"https://sso.test/jwks"}}"#
                ),
                jwks,
                id_token: std::sync::Mutex::new(Some(id_token)),
            }),
        )
    }

    fn valid_claims(sub: &str, roles: &[&str]) -> serde_json::Value {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        serde_json::json!({
            "iss": ISSUER,
            "aud": CLIENT_ID,
            "sub": sub,
            "exp": now + 300,
            "iat": now,
            "roles": roles,
        })
    }

    /// 构建 (AppState, router)；预置一个已签好的 id_token。
    fn oidc_app(
        key_pem: &str,
        claims: serde_json::Value,
        kid: &str,
        jwks: &str,
        allow_placeholder: bool,
    ) -> (Arc<AppState>, axum::Router) {
        let id_token = sign_id_token(key_pem, claims, kid);
        let oidc = Arc::new(mock_oidc_state(id_token, jwks.to_string(), allow_placeholder));
        let state = Arc::new(AppState {
            auth: AuthState::new("test-secret", DEFAULT_TOKEN_TTL),
            audit: crate::audit::AuditLog::new(64),
            resources: crate::resources::ResourceRegistry::from_configs(&[]),
            admin_key: None,
            policy: crate::policy::PolicyHandle::default(),
            totp_secrets: Default::default(),
            oidc: Some(oidc),
        });
        let app = axum::Router::new()
            .merge(router(Arc::clone(&state)))
            .with_state(Arc::clone(&state));
        (state, app)
    }

    /// 走 /oidc/login 拿到含合法 state 的 callback URI。
    async fn start_login(app: &axum::Router) -> String {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/oidc/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), HttpCode::FOUND);
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .expect("Location header")
            .to_string();
        assert!(location.starts_with("https://sso.test/auth?"));
        assert!(location.contains(&format!("client_id={}", percent_encode(CLIENT_ID))));
        // 从 Location 提取 state（供 callback 回传）
        let state_param = location
            .split('&')
            .find(|kv| kv.starts_with("state="))
            .map(|kv| kv.trim_start_matches("state=").to_string())
            .expect("state in authorize url");
        format!("/oidc/callback?code=auth-code-1&state={state_param}")
    }

    #[tokio::test]
    async fn oidc_callback_success_issues_console_session() {
        let (key_pem, jwks) = es256_key_and_jwks("test-key");
        let (state, app) = oidc_app(&key_pem, valid_claims("zhangsan", &["ops", "admin"]), "test-key", &jwks, false);
        let uri = start_login(&app).await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), HttpCode::FOUND);
        assert_eq!(
            resp.headers().get("location").and_then(|v| v.to_str().ok()),
            Some("/console/dashboard")
        );
        let cookie = resp
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(cookie.starts_with("ezy_session="));
        assert!(cookie.contains("HttpOnly"));
        // cookie 里的 JWT 可被 AuthState 校验且角色/主体映射正确
        let token = cookie.split(';').next().unwrap().split('=').nth(1).unwrap();
        let claims = state.auth.validate_token(token).expect("session jwt");
        assert_eq!(claims.sub, "zhangsan");
        assert!(claims.roles.contains(&"admin".to_string()));
        // 审计 console_login 已记录
        assert!(state
            .audit
            .snapshot()
            .iter()
            .any(|e| e.event == "console_login" && e.user == "zhangsan"));
    }

    #[tokio::test]
    async fn oidc_callback_rejects_missing_state() {
        let (key_pem, jwks) = es256_key_and_jwks("test-key");
        let (_state, app) = oidc_app(&key_pem, valid_claims("a", &["user"]), "test-key", &jwks, false);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/oidc/callback?code=x")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), HttpCode::BAD_REQUEST, "缺 state 必须 400");
    }

    #[tokio::test]
    async fn oidc_callback_rejects_wrong_and_replayed_state() {
        let (key_pem, jwks) = es256_key_and_jwks("test-key");
        let (_state, app) = oidc_app(&key_pem, valid_claims("a", &["user"]), "test-key", &jwks, false);
        // 伪造 state → 401
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/oidc/callback?code=x&state=forged")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), HttpCode::UNAUTHORIZED);
        // 合法 state 用一次后重放 → 401（一次性）
        let uri = start_login(&app).await;
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), HttpCode::FOUND);
        let resp = app
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), HttpCode::UNAUTHORIZED, "state 重放必须拒绝");
    }

    #[tokio::test]
    async fn oidc_callback_rejects_unknown_kid() {
        let (key_pem, jwks) = es256_key_and_jwks("test-key");
        let (_state, app) = oidc_app(&key_pem, valid_claims("a", &["user"]), "rotated-key", &jwks, false);
        let uri = start_login(&app).await;
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), HttpCode::UNAUTHORIZED, "JWKS 无此 kid 必须拒绝");
        // 审计 oidc_callback_failed 已记录
        assert!(_state.audit.snapshot().iter().any(|e| e.event == "oidc_callback_failed"));
    }

    #[tokio::test]
    async fn oidc_callback_rejects_bad_signature() {
        // JWKS 发布 key1，但 id_token 用 key2 签 → 签名校验必须失败
        let (key1, jwks) = es256_key_and_jwks("test-key");
        let (key2, _) = es256_key_and_jwks("other");
        let (_state, app) = oidc_app(&key2, valid_claims("a", &["user"]), "test-key", &jwks, false);
        let _ = key1;
        let uri = start_login(&app).await;
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), HttpCode::UNAUTHORIZED, "坏签名必须拒绝");
    }

    #[tokio::test]
    async fn oidc_callback_rejects_expired_token() {
        let (key_pem, jwks) = es256_key_and_jwks("test-key");
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let claims = serde_json::json!({
            "iss": ISSUER, "aud": CLIENT_ID, "sub": "a",
            "exp": now - 600, "iat": now - 1200, "roles": ["user"],
        });
        let (_state, app) = oidc_app(&key_pem, claims, "test-key", &jwks, false);
        let uri = start_login(&app).await;
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), HttpCode::UNAUTHORIZED, "过期 id_token 必须拒绝");
    }

    #[tokio::test]
    async fn oidc_callback_rejects_aud_and_iss_mismatch() {
        let (key_pem, jwks) = es256_key_and_jwks("test-key");
        let mut claims = valid_claims("a", &["user"]);
        claims["aud"] = serde_json::json!("other-client");
        let (_state, app) = oidc_app(&key_pem, claims, "test-key", &jwks, false);
        let uri = start_login(&app).await;
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), HttpCode::UNAUTHORIZED, "aud 不匹配必须拒绝");

        // iss 不匹配
        let mut claims = valid_claims("a", &["user"]);
        claims["iss"] = serde_json::json!("https://evil.test");
        let (_state, app) = oidc_app(&key_pem, claims, "test-key", &jwks, false);
        let uri = start_login(&app).await;
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), HttpCode::UNAUTHORIZED, "iss 不匹配必须拒绝");
    }

    #[tokio::test]
    async fn placeholder_login_disabled_when_oidc_configured() {
        let (key_pem, jwks) = es256_key_and_jwks("test-key");
        let (_state, app) = oidc_app(&key_pem, valid_claims("a", &["user"]), "test-key", &jwks, false);
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(r#"{"oidc_code":"a:user"}"#.to_string())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), HttpCode::FORBIDDEN, "[oidc] 后占位登录必须禁用");

        // allow_placeholder_login=true（联调）→ 显式恢复可用
        let (_state, app) = oidc_app(&key_pem, valid_claims("a", &["user"]), "test-key", &jwks, true);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(r#"{"oidc_code":"a:user"}"#.to_string())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), HttpCode::OK, "显式开启后占位登录可用");
    }

    #[test]
    fn extract_roles_prefers_roles_claim_then_groups() {
        assert_eq!(
            extract_roles(&serde_json::json!({"roles": ["ops", "admin"]}), "roles"),
            vec!["ops".to_string(), "admin".to_string()]
        );
        // roles 缺失 → groups 回退
        assert_eq!(
            extract_roles(&serde_json::json!({"groups": ["easyrdp-admin"]}), "roles"),
            vec!["easyrdp-admin".to_string()]
        );
        // 都缺失 → ["user"]
        assert_eq!(
            extract_roles(&serde_json::json!({"sub": "x"}), "roles"),
            vec!["user".to_string()]
        );
        // 字符串型 claim（逗号分隔）
        assert_eq!(
            extract_roles(&serde_json::json!({"roles": "ops, admin"}), "roles"),
            vec!["ops".to_string(), "".to_string(), "admin".to_string()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn oidc_otp_second_factor_required_for_local_totp_user() {
        let (key_pem, jwks) = es256_key_and_jwks("test-key");
        let id_token = sign_id_token(&key_pem, valid_claims("otpuser", &["user"]), "test-key");
        let oidc = Arc::new(mock_oidc_state(id_token, jwks, false));
        let mut totp = std::collections::BTreeMap::new();
        totp.insert("otpuser".to_string(), "JBSWY3DPEHPK3PXP".to_string());
        let state = Arc::new(AppState {
            auth: AuthState::new("test-secret", DEFAULT_TOKEN_TTL),
            audit: crate::audit::AuditLog::new(64),
            resources: crate::resources::ResourceRegistry::from_configs(&[]),
            admin_key: None,
            policy: crate::policy::PolicyHandle::default(),
            totp_secrets: totp,
            oidc: Some(oidc),
        });
        let app = axum::Router::new()
            .merge(router(Arc::clone(&state)))
            .with_state(Arc::clone(&state));

        let uri = start_login(&app).await;
        // 身份解析后 → 303 /oidc/otp（不直接发会话）
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), HttpCode::SEE_OTHER);
        let location = resp.headers().get("location").and_then(|v| v.to_str().ok()).unwrap().to_string();
        assert!(location.starts_with("/oidc/otp?state="), "TOTP 用户应跳 OTP 页: {location}");
        // 未带 OTP 提交 → 无效
        let otp_state = location.trim_start_matches("/oidc/otp?state=");
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oidc/otp")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(format!("state={otp_state}&otp_code=000000&csrf_token=x"))
                    .unwrap(),
            )
            .await
            .unwrap();
        // CSRF 缺失（cookie 未带）→ 403
        assert_eq!(resp.status(), HttpCode::FORBIDDEN);
    }
}
