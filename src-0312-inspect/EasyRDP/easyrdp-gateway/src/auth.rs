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
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
}

impl AuthState {
    pub fn new(secret: impl Into<String>, ttl_seconds: u64) -> Self {
        Self {
            secret: Arc::new(secret.into()),
            ttl: Duration::from_secs(ttl_seconds.max(1)),
            issuer: "easyrdp-gateway".to_string(),
        }
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
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, (StatusCode, String)> {
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
fn parse_placeholder_code(code: &str) -> (String, Vec<String>) {
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
    Router::new().route("/api/auth/login", post(login)).with_state(state)
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
}
