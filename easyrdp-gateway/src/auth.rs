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
}
