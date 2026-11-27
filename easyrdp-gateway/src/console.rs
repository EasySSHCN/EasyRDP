//! Web 控制台（F5-V2 R-20261118-018）：网关内置单页控制台。
//!
//! 路由（全部复用网关自身 axum，无独立前端工程；HTML 单文件内联 CSS）：
//! - GET  `/` `/console`            登录页（OIDC 按钮 + 占位登录表单）
//! - POST `/console/login`          占位登录（CSRF + 可选 TOTP）
//! - GET  `/console/dashboard`      控制台（资源列表 / 策略矩阵只读 / 审计检索 / OTP 状态）
//! - POST `/console/logout`         登出（CSRF + 审计）
//!
//! 会话：JWT（30min 滑动过期，HttpOnly cookie `ezy_session`，SameSite=Lax）。
//! 每次带会话请求校验通过且剩余寿命 < TTL/2 时滚动续签（Set-Cookie 新值）。
//! CSRF：double-submit cookie（`ezy_csrf` cookie 与表单 `csrf_token` 字段
//! 必须一致），所有 POST（登录/登出）强制校验，缺失/不匹配一律 403。
//!
//! 审计：console_login / console_login_failed / console_logout 事件。

use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Form, Router,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::{auth::Claims, AppState};

/// 来源 IP：优先 X-Real-IP / X-Forwarded-For（Nginx 反代终止 TLS 的部署形态
/// 会设置），缺失时回退 "unknown"（本地直连测试场景）。
pub fn client_ip_from_headers(headers: &HeaderMap) -> String {
    for name in ["x-real-ip", "x-forwarded-for"] {
        if let Some(v) = headers.get(name).and_then(|v| v.to_str().ok()) {
            let first = v.split(',').next().unwrap_or("").trim();
            if !first.is_empty() {
                return first.to_string();
            }
        }
    }
    "unknown".to_string()
}

/// 控制台会话 cookie 名。
pub const SESSION_COOKIE: &str = "ezy_session";
/// CSRF double-submit cookie 名。
pub const CSRF_COOKIE: &str = "ezy_csrf";
/// 滑动续签阈值：剩余寿命低于 TTL/2 时滚动 Set-Cookie。
const SLIDE_THRESHOLD: u64 = crate::auth::CONSOLE_SESSION_TTL / 2;

// ---------------------------------------------------------------------
// cookie / CSRF 基础设施
// ---------------------------------------------------------------------

/// 新建 CSRF token 与 Set-Cookie 头（double-submit 用）。
pub fn new_csrf_cookie() -> (String, String) {
    let token = uuid::Uuid::new_v4().to_string();
    let cookie = format!("{CSRF_COOKIE}={token}; Path=/; SameSite=Lax; Max-Age=1800");
    (token, cookie)
}

/// 校验 double-submit CSRF：请求 cookie 中的 `ezy_csrf` 与表单 `csrf_token`
/// 必须同时存在且相等（恒定时间比较）。
pub fn csrf_ok(headers: &HeaderMap, form_token: &str) -> bool {
    let cookie_token = cookie_value(headers, CSRF_COOKIE).unwrap_or_default();
    if form_token.is_empty() || cookie_token.is_empty() {
        return false;
    }
    constant_time_eq(cookie_token.as_bytes(), form_token.as_bytes())
}

/// 从 Cookie 头取指定 cookie 值。
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in raw.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// 会话 Set-Cookie（HttpOnly + SameSite=Lax + 30min Max-Age）。
fn session_cookie(token: &str) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        crate::auth::CONSOLE_SESSION_TTL
    )
}

/// 登录成功响应：302 /console + 会话 cookie。
pub fn session_redirect(token: &str) -> Response {
    (
        StatusCode::FOUND,
        [("Location", "/console/dashboard".to_string()), ("Set-Cookie", session_cookie(token))],
    )
        .into_response()
}

/// 登出响应：302 / + 清除 cookie。
fn logout_response() -> Response {
    (
        StatusCode::FOUND,
        [("Location", "/".to_string())],
        [(
            "Set-Cookie",
            format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
        )],
    )
        .into_response()
}

/// 从请求头解析并校验控制台会话 JWT（dashboard 等路由共用；保留供后续
/// 中间件化改造）。
#[allow(dead_code)]
fn session_from_headers(state: &AppState, headers: &HeaderMap) -> Option<Claims> {
    let token = cookie_value(headers, SESSION_COOKIE)?;
    state.auth.validate_token(&token).ok()
}

/// 会话中间件语义：校验 + 滑动续签（剩余 < 阈值时补 Set-Cookie）。
/// 返回 (claims, 可选续签 Set-Cookie)。
fn session_with_slide(state: &AppState, headers: &HeaderMap) -> (Option<Claims>, Option<String>) {
    let Some(token) = cookie_value(headers, SESSION_COOKIE) else {
        return (None, None);
    };
    let Ok(claims) = state.auth.validate_token(&token) else {
        return (None, None);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut slide = None;
    if claims.exp.saturating_sub(now) < SLIDE_THRESHOLD {
        if let Ok(new_token) = state.auth.issue_token_with_ttl(
            &claims.sub,
            &claims.roles.iter().map(String::as_str).collect::<Vec<_>>(),
            crate::auth::CONSOLE_SESSION_TTL,
        ) {
            slide = Some(session_cookie(&new_token));
        }
    }
    (Some(claims), slide)
}

/// 恒定时间比较（防时序侧信道）。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---------------------------------------------------------------------
// HTML 渲染（单文件 + 内联 CSS 深色简版）
// ---------------------------------------------------------------------

/// HTML 转义（防 XSS：审计字段/用户名等全部来自外部输入）。
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const STYLE: &str = r#"
body{background:#12161c;color:#d7dce3;font-family:system-ui,"Segoe UI",sans-serif;margin:0}
.wrap{max-width:1100px;margin:40px auto;padding:0 16px}
.card{background:#1b212b;border:1px solid #2a323e;border-radius:8px;padding:16px 20px;margin-bottom:16px}
h1{font-size:20px;margin:8px 0 16px}h2{font-size:15px;color:#9fb0c3;margin:0 0 10px}
table{width:100%;border-collapse:collapse;font-size:13px}
th,td{padding:6px 8px;border-bottom:1px solid #2a323e;text-align:left}
th{color:#9fb0c3;font-weight:600}
button,input,select{background:#232b37;color:#d7dce3;border:1px solid #3a4655;border-radius:4px;padding:6px 10px;font-size:13px}
button{cursor:pointer}button:hover{background:#2c3input{width:180px}
input{width:180px}
a{color:#6ea8fe;text-decoration:none}a:hover{text-decoration:underline}
.ok{color:#7ec699}.deny{color:#e07a7a}
.muted{color:#7a8798;font-size:12px}
.err{color:#e07a7a;margin-top:10px}
nav{display:flex;gap:14px;margin-bottom:16px;font-size:14px}
.badge{display:inline-block;background:#232b37;border:1px solid #3a4655;border-radius:10px;padding:1px 8px;font-size:12px}
"#;

fn html_page(title: &str, body: String) -> Response {
    let html = format!(
        "<!DOCTYPE html><html lang=\"zh\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>{}</title><style>{}</style></head><body><div class=\"wrap\">{}</div></body></html>",
        esc(title),
        STYLE,
        body
    );
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}

/// 登录页。
fn page_login(state: &AppState, csrf: &str, csrf_cookie: &str, error: Option<&str>) -> Response {
    let oidc_btn = if state.oidc.is_some() {
        format!("<p><a class=\"badge\" href=\"/oidc/login\">通过 OIDC (SSO) 登录</a></p>")
    } else {
        String::new()
    };
    // 占位登录：未配置 [oidc] 时可用（向后兼容）；配置后仅
    // allow_placeholder_login=true 显式保留。
    let placeholder_allowed = state
        .oidc
        .as_ref()
        .map(|o| o.allow_placeholder_login())
        .unwrap_or(true);
    let placeholder_form = if placeholder_allowed {
        let hint = if state.oidc.is_some() {
            "（占位登录已显式开启，仅限联调）"
        } else {
            "（未配置 [oidc]，占位登录可用）"
        };
        format!(
            "<form method=\"post\" action=\"/console/login\">\
<input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
<p>占位登录 {hint}</p>\
<p><input name=\"oidc_code\" placeholder=\"user:roles（如 zhangsan:ops,admin）\"></p>\
<p><input name=\"mfa_code\" placeholder=\"OTP（如启用）\"></p>\
<p><button type=\"submit\">登录</button></p></form>"
        )
    } else {
        "<p class=\"muted\">占位登录已禁用（已配置 [oidc]）</p>".to_string()
    };
    let err_html = error.map(|e| format!("<p class=\"err\">{}</p>", esc(e))).unwrap_or_default();
    let body = format!(
        "<h1>EasyRDP 网关控制台</h1><div class=\"card\">{oidc_btn}{placeholder_form}{err_html}</div>\
<p class=\"muted\">EasyRDP Gateway · F5-V2</p>"
    );
    let mut resp = html_page("EasyRDP 控制台登录", body);
    resp.headers_mut()
        .append(header::SET_COOKIE, HeaderValue::from_str(csrf_cookie).expect("csrf cookie"));
    resp
}

/// 本地 OTP 二次认证页（OIDC 身份已解析）。
pub fn page_otp(otp_state: &str, csrf: &str, csrf_cookie: String) -> Response {
    let body = format!(
        "<h1>二次认证 (OTP)</h1><div class=\"card\">\
<p class=\"muted\">OIDC 身份已解析，该账号要求本地 OTP 二次认证。</p>\
<form method=\"post\" action=\"/oidc/otp\">\
<input type=\"hidden\" name=\"state\" value=\"{}\">\
<input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
<p><input name=\"otp_code\" placeholder=\"6 位验证码\"></p>\
<p><button type=\"submit\">提交</button></p></form></div>",
        esc(otp_state)
    );
    let mut resp = html_page("EasyRDP OTP", body);
    resp.headers_mut()
        .append(header::SET_COOKIE, HeaderValue::from_str(&csrf_cookie).expect("csrf cookie"));
    resp
}

/// 审计表 HTML（过滤 + 分页后的行）。
fn audit_table_html(rows: &[crate::audit::AuditEvent]) -> String {
    if rows.is_empty() {
        return "<p class=\"muted\">（无匹配审计记录）</p>".to_string();
    }
    let mut t = String::from(
        "<table><tr><th>时间</th><th>用户</th><th>角色</th><th>事件</th><th>资源</th>\
<th>来源 IP</th><th>结果</th></tr>",
    );
    for e in rows {
        let result = e.result();
        let cls = if result == "ok" { "ok" } else { "deny" };
        t.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td>\
<td class=\"{cls}\">{}</td></tr>",
            e.timestamp,
            esc(&e.user),
            esc(e.role.as_deref().unwrap_or("-")),
            esc(&e.event),
            esc(&e.resource),
            esc(&e.source_ip),
            esc(&result),
        ));
    }
    t.push_str("</table>");
    t
}

/// 控制台主页面。
fn page_dashboard(
    state: &AppState,
    claims: &Claims,
    q: &DashQuery,
    slide_cookie: Option<String>,
) -> Response {
    let is_admin = crate::auth::is_admin(claims);
    let mut body = String::from("<h1>EasyRDP 控制台</h1><nav>");
    body.push_str(&format!(
        "<a href=\"/console/dashboard\">概览</a> · <span class=\"badge\">{}</span> · {}",
        esc(&claims.sub),
        if is_admin {
            "<a href=\"/console/dashboard?tab=audit\">审计</a>"
        } else {
            ""
        }
    ));
    body.push_str(&format!(
        "<form method=\"post\" action=\"/console/logout\" style=\"margin-left:auto\">\
<input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><button type=\"submit\">登出</button></form></nav>",
        esc(&q.csrf_for_nav.clone().unwrap_or_default())
    ));

    // 资源列表（所有已登录角色可见）
    body.push_str("<div class=\"card\"><h2>资源列表</h2><table><tr><th>ID</th><th>名称</th></tr>");
    let resources = state.resources.list();
    if resources.is_empty() {
        body.push_str("<tr><td colspan=\"2\" class=\"muted\">（无注册资源）</td></tr>");
    }
    for r in &resources {
        body.push_str(&format!(
            "<tr><td>{}</td><td>{}</td></tr>",
            esc(&r.id),
            esc(&r.name)
        ));
    }
    body.push_str("</table></div>");

    // 策略矩阵只读视图（仅 admin）
    if is_admin {
        body.push_str("<div class=\"card\"><h2>策略矩阵（只读）</h2><table>\
<tr><th>资源</th><th>角色</th><th>动作</th><th>结论</th></tr>");
        let rules = state.policy.describe();
        if rules.is_empty() {
            body.push_str("<tr><td colspan=\"4\" class=\"muted\">（无显式规则：默认矩阵 deny by default、admin 全通）</td></tr>");
        }
        for (res, role, action, allow) in &rules {
            body.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td class=\"{}\">{}</td></tr>",
                esc(res),
                esc(role),
                esc(action),
                if *allow { "ok" } else { "deny" },
                if *allow { "Allow" } else { "Deny" },
            ));
        }
        body.push_str("</table></div>");
    }

    // 审计检索表（仅 admin）
    if is_admin {
        let audit_q = crate::audit::AuditQuery {
            user: q.user.clone().filter(|s| !s.is_empty()),
            event: None,
            resource: q.resource.clone().filter(|s| !s.is_empty()),
            result: q.result.clone().filter(|s| !s.is_empty()),
            limit: None,
        };
        let all: Vec<_> = state
            .audit
            .snapshot()
            .into_iter()
            .filter(|e| audit_q.matches(e))
            .collect();
        let total = all.len();
        let page_size = 20usize.max(q.page_size.unwrap_or(20).clamp(5, 100));
        let pages = total.div_ceil(page_size).max(1);
        let page = q.page.unwrap_or(1).clamp(1, pages);
        let start = (page - 1) * page_size;
        let end = (start + page_size).min(total);
        let slice: Vec<_> = all[start..end].to_vec();

        body.push_str(&format!(
            "<div class=\"card\"><h2>审计检索（共 {total} 条）</h2>\
<form method=\"get\" action=\"/console/dashboard\">\
用户 <input name=\"user\" value=\"{}\"> \
资源 <input name=\"resource\" value=\"{}\"> \
结果 <select name=\"result\">{}</select> \
<input type=\"hidden\" name=\"page_size\" value=\"{page_size}\">\
<button type=\"submit\">筛选</button></form><br>",
            esc(q.user.as_deref().unwrap_or("")),
            esc(q.resource.as_deref().unwrap_or("")),
            ["", "ok", "error"]
                .iter()
                .map(|v| format!(
                    "<option value=\"{v}\"{}>{}</option>",
                    if q.result.as_deref() == Some(*v) { " selected" } else { "" },
                    if v.is_empty() { "全部" } else { *v }
                ))
                .collect::<String>()
        ));
        body.push_str(&audit_table_html(&slice));
        // 分页条
        body.push_str("<p>");
        if page > 1 {
            body.push_str(&format!(
                "<a href=\"/console/dashboard?user={}&amp;resource={}&amp;result={}&amp;page={}&amp;page_size={}\">上一页</a> ",
                esc(q.user.as_deref().unwrap_or("")),
                esc(q.resource.as_deref().unwrap_or("")),
                esc(q.result.as_deref().unwrap_or("")),
                page - 1,
                page_size
            ));
        }
        body.push_str(&format!(
            "<span class=\"muted\">第 {page}/{pages} 页</span> "
        ));
        if page < pages {
            body.push_str(&format!(
                "<a href=\"/console/dashboard?user={}&amp;resource={}&amp;result={}&amp;page={}&amp;page_size={}\">下一页</a>",
                esc(q.user.as_deref().unwrap_or("")),
                esc(q.resource.as_deref().unwrap_or("")),
                esc(q.result.as_deref().unwrap_or("")),
                page + 1,
                page_size
            ));
        }
        body.push_str("</p></div>");
    }

    // OTP 绑定状态（当前用户）
    let otp_bound = state.totp_secrets.contains_key(&claims.sub);
    body.push_str(&format!(
        "<div class=\"card\"><h2>OTP 绑定状态</h2><p>{}</p></div>",
        if otp_bound {
            "<span class=\"ok\">已绑定本地 TOTP（登录要求 OTP 二次认证）</span>"
        } else {
            "<span class=\"muted\">未绑定本地 TOTP</span>"
        }
    ));

    let mut resp = html_page("EasyRDP 控制台", body);
    if let Some(cookie) = slide_cookie {
        if let Ok(v) = HeaderValue::from_str(&cookie) {
            resp.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    resp
}

/// GET / 、GET /console —— 登录页；已登录则 302 到 dashboard。
async fn login_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let (claims, _) = session_with_slide(&state, &headers);
    if claims.is_some() {
        return axum::response::Redirect::to("/console/dashboard").into_response();
    }
    let (csrf, cookie) = new_csrf_cookie();
    page_login(&state, &csrf, &cookie, None)
}

/// GET /console/dashboard —— 需会话；admin 才渲染策略/审计区块。
#[derive(Debug, Deserialize, Default)]
pub struct DashQuery {
    pub user: Option<String>,
    pub resource: Option<String>,
    pub result: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
    /// 导航表单所需的 CSRF（来自请求 cookie 回填）。
    #[serde(default)]
    pub csrf_for_nav: Option<String>,
}

async fn dashboard(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<DashQuery>,
    headers: HeaderMap,
) -> Response {
    let (claims, slide) = session_with_slide(&state, &headers);
    let Some(claims) = claims else {
        return axum::response::Redirect::to("/").into_response();
    };
    // 导航登出表单的 CSRF 取当前请求 cookie 值（double-submit 闭环）
    let mut q = q;
    q.csrf_for_nav = cookie_value(&headers, CSRF_COOKIE);
    page_dashboard(&state, &claims, &q, slide)
}

/// POST /console/login 表单（csrf_token 允许缺失以便统一走 CSRF 校验拒绝）。
#[derive(Debug, Deserialize)]
pub struct ConsoleLoginForm {
    pub csrf_token: Option<String>,
    pub oidc_code: String,
    #[serde(default)]
    pub mfa_code: Option<String>,
}

/// POST /console/login —— 占位登录（CSRF 强制；[oidc] 后门控同 API 登录）。
async fn console_login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<ConsoleLoginForm>,
) -> Response {
    let ip = client_ip_from_headers(&headers);
    if !csrf_ok(&headers, form.csrf_token.as_deref().unwrap_or("")) {
        return (StatusCode::FORBIDDEN, "CSRF token 缺失或不匹配").into_response();
    }
    // 占位登录门控（与 /api/auth/login 同口径）
    if let Some(oidc) = &state.oidc {
        if !oidc.allow_placeholder_login() {
            let mut ev = crate::audit::AuditEvent::new("-", None, "console_login_failed", "-", &ip);
            ev.error = Some("placeholder_disabled".into());
            state.audit.record(ev);
            let (csrf, cookie) = new_csrf_cookie();
            return page_login(&state, &csrf, &cookie, Some("占位登录已禁用，请通过 OIDC 登录"));
        }
    }
    let (subject, roles) = crate::auth::parse_placeholder_code(&form.oidc_code);
    // TOTP（F4-V1 不变量）
    if let Some(secret) = state.totp_secrets.get(&subject) {
        let code = form.mfa_code.as_deref().map(str::trim).unwrap_or("");
        let ok = !code.is_empty()
            && crate::auth::verify_totp(secret, code, 1, std::time::SystemTime::now());
        if !ok {
            let mut ev =
                crate::audit::AuditEvent::new(&subject, None, "console_login_failed", "-", &ip);
            ev.error = Some("otp_required_or_invalid".into());
            state.audit.record(ev);
            let (csrf, cookie) = new_csrf_cookie();
            return page_login(&state, &csrf, &cookie, Some("需要有效的 OTP 验证码"));
        }
    }
    match crate::auth::finish_session(&state, &subject, &roles, &ip) {
        Ok(resp) => resp,
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err).into_response(),
    }
}

/// POST /console/logout 表单（csrf_token 允许缺失以便统一走 CSRF 校验拒绝）。
#[derive(Debug, Deserialize)]
pub struct LogoutForm {
    pub csrf_token: Option<String>,
}

async fn console_logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<LogoutForm>,
) -> Response {
    let ip = client_ip_from_headers(&headers);
    if !csrf_ok(&headers, form.csrf_token.as_deref().unwrap_or("")) {
        return (StatusCode::FORBIDDEN, "CSRF token 缺失或不匹配").into_response();
    }
    let (claims, _) = session_with_slide(&state, &headers);
    if let Some(c) = claims {
        state.audit.record(crate::audit::AuditEvent::new(
            &c.sub,
            Some(crate::policy::canonical_role(&c.roles).as_str()),
            "console_logout",
            "-",
            &ip,
        ));
    }
    logout_response()
}

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(login_page))
        .route("/console", get(login_page))
        .route("/console/dashboard", get(dashboard))
        .route("/console/login", post(console_login))
        .route("/console/logout", post(console_logout))
        .with_state(state)
}

// ---------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthState;
    use crate::resources::ResourceRegistry;
    use axum::body::Body;
    use axum::http::{Request, StatusCode as Code};
    use tower::ServiceExt;

    fn test_state(oidc: Option<Arc<crate::auth::OidcState>>) -> Arc<AppState> {
        let pc = crate::policy::PolicyConfig {
            rules: vec![crate::policy::PolicyRuleConfig {
                resource: "*".into(),
                role: "user".into(),
                action: "connect".into(),
                decision: "allow".into(),
            }],
        };
        Arc::new(AppState {
            auth: AuthState::new("test-secret", crate::auth::DEFAULT_TOKEN_TTL),
            audit: crate::audit::AuditLog::new(64),
            resources: ResourceRegistry::from_configs(&[crate::resources::ResourceConfig {
                id: "rdp-win01".into(),
                name: "Windows 跳板".into(),
                backend: "10.0.0.5:3389".into(),
                allowed_roles: None,
                mode: None,
                agent_local_backend: None,
            }]),
            admin_key: None,
            policy: crate::policy::PolicyHandle::from_matrix(
                crate::policy::PolicyMatrix::from_config(&pc).expect("matrix"),
            ),
            totp_secrets: {
                let mut m = std::collections::BTreeMap::new();
                m.insert("otpuser".to_string(), "JBSWY3DPEHPK3PXP".to_string());
                m
            },
            oidc,
            agent_registry: crate::agent::AgentRegistry::new(),
            agent_cfg: Default::default(),
            agent_sessions: crate::agent::SessionIdAllocator::new(),
        })
    }

    fn console_app(state: &Arc<AppState>) -> axum::Router {
        router(Arc::clone(state)).with_state(Arc::clone(state))
    }

    async fn get_login_page(state: &Arc<AppState>) -> (Code, String, String) {
        let resp = console_app(state)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let code = resp.status();
        let set_cookie = resp
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (code, String::from_utf8_lossy(&body).to_string(), set_cookie)
    }

    /// 从登录页响应提取 CSRF token（cookie + 表单 hidden 一致）。
    fn extract_csrf(cookie: &str, body: &str) -> String {
        let tok = cookie
            .split('=')
            .nth(1)
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        assert!(
            body.contains(&format!("value=\"{tok}\"")),
            "表单 hidden csrf 应与 cookie 一致"
        );
        tok
    }

    async fn post_form(
        state: &Arc<AppState>,
        uri: &str,
        body: &str,
        cookies: &[(&str, &str)],
    ) -> (Code, String) {
        let mut req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(body.to_string()))
            .unwrap();
        if !cookies.is_empty() {
            let joined: String = cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ");
            req.headers_mut()
                .insert(header::COOKIE, HeaderValue::from_str(&joined).unwrap());
        }
        let resp = console_app(state).oneshot(req).await.unwrap();
        let code = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (code, String::from_utf8_lossy(&body).to_string())
    }

    #[tokio::test]
    async fn login_page_contains_csrf_and_placeholder_form() {
        let state = test_state(None);
        let (code, body, cookie) = get_login_page(&state).await;
        assert_eq!(code, Code::OK);
        assert!(cookie.starts_with(&format!("{CSRF_COOKIE}=")));
        assert!(body.contains("/console/login"), "应含占位登录表单");
        assert!(!body.contains("/oidc/login"), "未配置 OIDC 不应出现 SSO 按钮");
        let _ = extract_csrf(&cookie, &body);
    }

    #[tokio::test]
    async fn login_page_shows_oidc_button_when_configured() {
        let state = test_state(Some(Arc::new(crate::auth::OidcState::new(
            crate::config::OidcConfig {
                issuer: Some("https://sso.example.com".into()),
                client_id: Some("gw".into()),
                client_secret: Some("s".into()),
                redirect_uri: Some("https://gw.example.com/oidc/callback".into()),
                ..Default::default()
            },
            Arc::new(MockNoNet),
        ))));
        let (_, body, _) = get_login_page(&state).await;
        assert!(body.contains("/oidc/login"), "配置 OIDC 后应有 SSO 按钮");
        assert!(
            body.contains("占位登录已禁用"),
            "配置 OIDC 且未显式开启时应禁用占位登录: {body}"
        );
    }

    /// 占位（无网络）fetcher：任何请求都失败（页面渲染用，不触发）。
    struct MockNoNet;
    impl crate::auth::TokenFetcher for MockNoNet {
        fn exchange_code<'a>(
            &'a self,
            _: &'a str,
            _: &'a [(String, String)],
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send + 'a>,
        > {
            Box::pin(async { Err("no network in unit test".into()) })
        }
        fn fetch_text<'a>(
            &'a self,
            _: &'a str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>,
        > {
            Box::pin(async { Err("no network in unit test".into()) })
        }
    }

    // ---------------- CSRF ----------------

    #[tokio::test]
    async fn logout_requires_csrf_missing_is_403() {
        let state = test_state(None);
        let token = state.auth.issue_token("alice", &["user"]).unwrap();
        // 无 csrf_token 字段 → 403
        let (code, _) = post_form(
            &state,
            "/console/logout",
            "",
            &[(SESSION_COOKIE, &token), (CSRF_COOKIE, "t123")],
        )
        .await;
        assert_eq!(code, Code::FORBIDDEN, "缺失 CSRF 必须拒绝");
        // 不匹配 → 403
        let (code, _) = post_form(
            &state,
            "/console/logout",
            "csrf_token=wrong",
            &[(SESSION_COOKIE, &token), (CSRF_COOKIE, "t123")],
        )
        .await;
        assert_eq!(code, Code::FORBIDDEN);
        // 匹配 → 302 登出
        let (code, _) = post_form(
            &state,
            "/console/logout",
            "csrf_token=t123",
            &[(SESSION_COOKIE, &token), (CSRF_COOKIE, "t123")],
        )
        .await;
        assert_eq!(code, Code::FOUND);
        // 审计已记录 console_logout
        let evs = state.audit.snapshot();
        assert!(evs.iter().any(|e| e.event == "console_logout" && e.user == "alice"));
    }

    #[tokio::test]
    async fn console_login_requires_csrf_and_records_audit() {
        let state = test_state(None);
        let (code, _) = post_form(
            &state,
            "/console/login",
            "oidc_code=alice:user",
            &[(CSRF_COOKIE, "t1")],
        )
        .await;
        assert_eq!(code, Code::FORBIDDEN, "登录缺 CSRF 拒绝");

        // 正常路径：拿登录页 CSRF → 提交 → 302 + 会话 cookie + 审计
        let (_, body, cookie) = get_login_page(&state).await;
        let csrf = extract_csrf(&cookie, &body);
        let body = format!("csrf_token={csrf}&oidc_code=alice:user");
        let mut req = Request::builder()
            .method("POST")
            .uri("/console/login")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(header::COOKIE, cookie.as_str())
            .body(Body::from(body))
            .unwrap();
        // 需要 ConnectInfo：axum oneshot 不注入 ConnectInfo 时 handler 提取会失败
        // —— console_login 使用 Option 兜底见实现说明；此处直接断言成功路径。
        let resp = console_app(&state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), Code::FOUND, "成功登录应 302");
        let set_cookie = resp
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(set_cookie.contains(&format!("{SESSION_COOKIE}=")));
        assert!(set_cookie.contains("HttpOnly"));
        let evs = state.audit.snapshot();
        assert!(evs.iter().any(|e| e.event == "console_login" && e.user == "alice"));
        let _ = post_form; // 保留 helper
    }

    // ---------------- 角色 → 控制台可见性 ----------------

    async fn dashboard_for(state: &Arc<AppState>, roles: &[&str]) -> String {
        let token = state.auth.issue_token("viewer", roles).unwrap();
        let resp = console_app(state)
            .oneshot(
                Request::builder()
                    .uri("/console/dashboard")
                    .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), Code::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        String::from_utf8_lossy(&body).to_string()
    }

    #[tokio::test]
    async fn admin_sees_policy_and_audit_user_does_not() {
        let state = test_state(None);
        let admin_body = dashboard_for(&state, &["admin"]).await;
        assert!(admin_body.contains("策略矩阵"), "admin 应见策略页区块");
        assert!(admin_body.contains("审计检索"), "admin 应见审计区块");
        assert!(admin_body.contains("rdp-win01"), "资源列表对所有角色可见");

        let user_body = dashboard_for(&state, &["user"]).await;
        assert!(!user_body.contains("策略矩阵"), "非 admin 不应见策略区块");
        assert!(!user_body.contains("审计检索"), "非 admin 不应见审计区块");
        assert!(user_body.contains("rdp-win01"), "资源列表可见");
        assert!(user_body.contains("OTP 绑定状态"));
    }

    #[tokio::test]
    async fn dashboard_without_session_redirects_to_login() {
        let state = test_state(None);
        let resp = console_app(&state)
            .oneshot(
                Request::builder()
                    .uri("/console/dashboard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), Code::SEE_OTHER);
        assert_eq!(
            resp.headers().get("location").and_then(|v| v.to_str().ok()),
            Some("/")
        );
    }

    #[tokio::test]
    async fn invalid_session_cookie_rejected() {
        let state = test_state(None);
        let resp = console_app(&state)
            .oneshot(
                Request::builder()
                    .uri("/console/dashboard")
                    .header(header::COOKIE, format!("{SESSION_COOKIE}=garbage.jwt"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), Code::SEE_OTHER, "无效会话 cookie 应回落到登录页");
    }

    // ---------------- 会话滑动过期 ----------------

    #[test]
    fn session_cookie_flags() {
        let cookie = session_cookie("tok123");
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains(&format!("Max-Age={}", crate::auth::CONSOLE_SESSION_TTL)));
    }

    #[test]
    fn csrf_constant_time_eq_and_cookie_parse() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=x; ezy_csrf=tok; more=y"),
        );
        assert_eq!(cookie_value(&headers, CSRF_COOKIE).as_deref(), Some("tok"));
        assert_eq!(cookie_value(&headers, SESSION_COOKIE), None);
    }

    // ---------------- OTP 绑定状态展示 ----------------

    #[tokio::test]
    async fn otp_binding_status_shown_per_user() {
        let state = test_state(None);
        // "viewer" 未绑定 → 未绑定文案；"otpuser" 已绑定
        let token = state.auth.issue_token("otpuser", &["user"]).unwrap();
        let resp = console_app(&state)
            .oneshot(
                Request::builder()
                    .uri("/console/dashboard")
                    .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body = String::from_utf8_lossy(&body).to_string();
        assert!(body.contains("已绑定本地 TOTP"));
    }
}
