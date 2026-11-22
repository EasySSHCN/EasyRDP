//! 审计模块：内存环形缓冲审计日志 + 查询接口。
//!
//! 审计事件包含：时间（Unix 秒）、用户、事件、资源、来源 IP。
//! 骨架阶段仅存内存（Mutex + 环形缓冲），生产阶段应落地到
//! 不可篡改的日志后端（如 append-only 文件 / SIEM 推送）。

use axum::{
    extract::{Query, State},
    http::StatusCode,
    middleware::from_fn_with_state,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Extension, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use crate::{auth::auth_guard, AppState};

/// 单条审计事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// 事件时间（Unix 秒）
    pub timestamp: u64,
    /// 用户主体（Claims.sub）
    pub user: String,
    /// 事件名，如 tunnel_open / tunnel_close / auth_login
    pub event: String,
    /// 目标资源 ID（无资源时为 "-"）
    pub resource: String,
    /// 客户端来源 IP
    pub source_ip: String,
    /// 错误摘要（连接失败等场景；成功事件为 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 用户角色（F5-V1：来自 JWT claims 规范化角色 admin/user/readonly；
    /// 兼容旧事件缺省 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

impl AuditEvent {
    /// 便捷构造：填时间/用户/事件/资源/来源 IP/角色，error = None。
    pub fn new(
        user: impl Into<String>,
        role: Option<&str>,
        event: impl Into<String>,
        resource: impl Into<String>,
        source_ip: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: now_unix(),
            user: user.into(),
            role: role.map(str::to_string),
            event: event.into(),
            resource: resource.into(),
            source_ip: source_ip.into(),
            error: None,
        }
    }

    /// 事件结果字符串："ok" 或 "error:<摘要>"（过滤与 CSV 共用）。
    pub fn result(&self) -> String {
        match &self.error {
            None => "ok".to_string(),
            Some(err) => format!("error:{err}"),
        }
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 内存环形缓冲审计日志。
pub struct AuditLog {
    buf: Mutex<RingBuffer>,
}

struct RingBuffer {
    items: Vec<AuditEvent>,
    head: usize, // 下一个写入位置
    count: usize,
}

impl AuditLog {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            buf: Mutex::new(RingBuffer {
                items: Vec::with_capacity(capacity),
                head: 0,
                count: 0,
            }),
        }
    }

    /// 记录一条事件；缓冲满时覆盖最旧记录。
    pub fn record(&self, event: AuditEvent) {
        let mut buf = self.buf.lock().expect("audit mutex poisoned");
        let cap = buf.items.capacity();
        if buf.items.len() < cap {
            buf.items.push(event);
        } else {
            let head = buf.head;
            buf.items[head] = event;
        }
        buf.head = (buf.head + 1) % cap;
        buf.count += 1;
    }

    /// 按写入顺序返回当前缓冲内的事件（旧 -> 新）。
    pub fn snapshot(&self) -> Vec<AuditEvent> {
        let buf = self.buf.lock().expect("audit mutex poisoned");
        let mut out = Vec::with_capacity(buf.items.len());
        if buf.items.len() < buf.items.capacity() {
            out.extend(buf.items.iter().cloned());
        } else {
            let head = buf.head;
            out.extend_from_slice(&buf.items[head..]);
            out.extend_from_slice(&buf.items[..head]);
        }
        out
    }

    /// GW-DEL-20261121：删除指定用户的所有审计事件（用户删除时同步清空其
    /// 网关侧关联审计），返回删除条数。仅 /admin/audit/purge_user 调用。
    pub fn purge_user(&self, user: &str) -> usize {
        let mut buf = self.buf.lock().expect("audit mutex poisoned");
        let before = buf.items.len();
        buf.items.retain(|e| e.user != user);
        // retain 后 items 已压缩为连续紧凑序列（旧->新），环形头归零
        buf.head = 0;
        buf.count = buf.items.len();
        before - buf.items.len()
    }

    /// GW-AUDIT-DEL-20261121：按 (timestamp, user, event) 三元组批量删除
    /// 指定审计事件（官网后台手工删除用），返回删除条数。
    pub fn delete_by_keys(&self, keys: &[(u64, String, String)]) -> usize {
        let mut buf = self.buf.lock().expect("audit mutex poisoned");
        let before = buf.items.len();
        buf.items.retain(|e| {
            !keys.iter()
                .any(|(ts, u, ev)| e.timestamp == *ts && &e.user == u && &e.event == ev)
        });
        buf.head = 0;
        buf.count = buf.items.len();
        before - buf.items.len()
    }
}

/// 查询参数：可选按用户/事件/资源/结果过滤，limit 限制返回条数。
/// 所有参数可选、可任意组合。
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
    /// "ok" 或 "error:<摘要>"（也可只写 "error" 匹配全部错误事件）
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

impl AuditQuery {
    /// 统一过滤逻辑（JWT 用户侧与管理侧共用）。
    pub fn matches(&self, e: &AuditEvent) -> bool {
        self.user.as_ref().map_or(true, |u| &e.user == u)
            && self.event.as_ref().map_or(true, |ev| &e.event == ev)
            && self.resource.as_ref().map_or(true, |r| &e.resource == r)
            && self
                .result
                .as_ref()
                .map_or(true, |res| match res.trim() {
                    "error" => e.error.is_some(),
                    other => &e.result() == other,
                })
    }
}

async fn query_audit(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<crate::auth::Claims>,
    Query(q): Query<AuditQuery>,
) -> Response {
    // 双保险：路由已挂 auth_guard，此处再要求 admin 角色。
    if !crate::auth::is_admin(&claims) {
        return (StatusCode::FORBIDDEN, "需要 admin 角色").into_response();
    }

    let mut events: Vec<AuditEvent> = state
        .audit
        .snapshot()
        .into_iter()
        .filter(|e| q.matches(e))
        .collect();

    if let Some(limit) = q.limit {
        let limit = limit.clamp(1, 10_000);
        if events.len() > limit {
            events = events.split_off(events.len() - limit); // 取最新 limit 条
        }
    }

    Json(serde_json::json!({ "events": events })).into_response()
}

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/audit", get(query_audit))
        .route_layer(from_fn_with_state(Arc::clone(&state), auth_guard))
        .with_state(state)
}

// ---------------------------------------------------------------------
// 管理密钥端点（ADMIN-AUDIT-20261118）：GET /admin/audit?limit=N
// 供官网后台 api/gateway/audit.php 以 X-Admin-Key 头代理拉取最近 N 条审计，
// 与 JWT 用户侧 /api/audit 互不干扰。管理密钥未配置时端点保持禁用（503）。
// ---------------------------------------------------------------------

/// 管理端点默认返回条数与上限。
const ADMIN_AUDIT_DEFAULT_LIMIT: usize = 100;
const ADMIN_AUDIT_MAX_LIMIT: usize = 500;

/// 单条对外展示的审计行（时间/用户/角色/事件/来源 IP/资源/结果）。
#[derive(Debug, Serialize)]
pub struct AdminAuditRow {
    /// 事件时间（Unix 秒）
    pub timestamp: u64,
    /// 用户主体（F5-V1 新增）
    pub user: String,
    /// 用户角色（F5-V1 新增；旧事件缺省 "-"）
    pub role: String,
    /// 事件类型
    pub event: String,
    /// 来源 IP
    pub source_ip: String,
    /// 资源
    pub resource: String,
    /// 结果："ok" 或 "error:<摘要>"
    pub result: String,
}

/// 取缓冲内经过滤并截断（最新在前）的事件列表（JSON 与 CSV 端点共用）。
fn filtered_admin_events(state: &AppState, q: &AuditQuery) -> Vec<AuditEvent> {
    let limit = q
        .limit
        .unwrap_or(ADMIN_AUDIT_DEFAULT_LIMIT)
        .clamp(1, ADMIN_AUDIT_MAX_LIMIT);

    let mut events: Vec<AuditEvent> = state
        .audit
        .snapshot()
        .into_iter()
        .filter(|e| q.matches(e))
        .collect();
    if events.len() > limit {
        events = events.split_off(events.len() - limit); // 取最新 limit 条
    }
    events.reverse(); // 最新在前，供后台列表直接展示
    events
}

fn event_to_row(e: AuditEvent) -> AdminAuditRow {
    // 先取 result（借用 e），再逐字段 move，避免部分移动后借用冲突
    let result = e.result();
    AdminAuditRow {
        timestamp: e.timestamp,
        role: e.role.unwrap_or_else(|| "-".to_string()),
        user: e.user,
        event: e.event,
        source_ip: e.source_ip,
        resource: e.resource,
        result,
    }
}

async fn admin_audit(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<AuditQuery>,
) -> Response {
    if let Some(resp) = check_admin_key(&state, &headers) {
        return resp;
    }

    let rows: Vec<AdminAuditRow> = filtered_admin_events(&state, &q)
        .into_iter()
        .map(event_to_row)
        .collect();

    Json(serde_json::json!({
        "ok": true,
        "count": rows.len(),
        "events": rows
    }))
    .into_response()
}

/// GET /admin/audit.csv —— 同鉴权的 CSV 导出（RFC4180 转义）。
async fn admin_audit_csv(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<AuditQuery>,
) -> Response {
    if let Some(resp) = check_admin_key(&state, &headers) {
        return resp;
    }

    let mut csv = String::from("timestamp,user,role,event,resource,source_ip,result,error\n");
    // 最新在前（与 /admin/audit JSON 列表口径一致）
    let mut events = filtered_admin_events(&state, &q);
    events.reverse();
    for e in events {
        let err = e.error.clone().unwrap_or_default();
        // result 列已含 "error:<摘要>" 语义，error 列保留原始摘要便于表格透视
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            e.timestamp,
            csv_field(&e.user),
            csv_field(e.role.as_deref().unwrap_or("-")),
            csv_field(&e.event),
            csv_field(&e.resource),
            csv_field(&e.source_ip),
            csv_field(&e.result()),
            csv_field(&err),
        ));
    }

    (
        [(axum::http::header::CONTENT_TYPE, "text/csv; charset=utf-8")],
        csv,
    )
        .into_response()
}

/// RFC4180 字段转义：含逗号/引号/换行（CR/LF）时整体加双引号并把
/// 内部引号翻倍，防止 CSV 公式/结构注入。
fn csv_field(s: &str) -> String {
    // 公式注入防护：以 = + - @ 或 TAB 开头的字段同样整体加引号
    // （Excel/LibreOffice 会把这类前缀当公式执行），与结构转义共用引号路径。
    let starts_formula = matches!(s.chars().next(), Some('=' | '+' | '-' | '@' | '\t'));
    if starts_formula || s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r')
    {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// 管理端点鉴权：admin_key 未配置 -> 503；X-Admin-Key 缺失/无效 -> 401。
/// 通过则返回 None。
fn check_admin_key(state: &AppState, headers: &axum::http::HeaderMap) -> Option<Response> {
    let Some(key) = state.admin_key.as_deref() else {
        return Some(
            (StatusCode::SERVICE_UNAVAILABLE, "admin_key 未配置，管理端点已禁用").into_response(),
        );
    };
    let provided = headers
        .get("X-Admin-Key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if provided.is_empty() {
        return Some((StatusCode::UNAUTHORIZED, "缺少 X-Admin-Key").into_response());
    }
    if !constant_time_eq(provided.as_bytes(), key.as_bytes()) {
        return Some((StatusCode::UNAUTHORIZED, "X-Admin-Key 无效").into_response());
    }
    None
}

/// 长度固定时的恒定时间比较（简化实现，防时序侧信道）。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 管理密钥路由：/admin/audit（不走 JWT auth_guard，凭 X-Admin-Key 鉴权）。
/// R-20261120-009：新增 POST /admin/issue —— 官网服务端（issue.php，回环
/// 调用）代用户签发短期 API token；与 /api/auth/login（客户端占位/OIDC）
/// 完全隔离，占位登录禁用后用户 token 由本端点按 grants 审批结果签发。
pub fn admin_router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/audit", get(admin_audit))
        .route("/admin/audit.csv", get(admin_audit_csv))
        .route("/admin/issue", post(admin_issue))
        .route("/admin/audit/purge_user", post(admin_audit_purge_user))
        .route("/admin/audit/delete_batch", post(admin_audit_delete_batch))
        .route("/admin/audit/record", post(admin_audit_record))
        .with_state(state)
}

/// POST /admin/audit/purge_user 请求体：{ user }。
#[derive(Debug, serde::Deserialize)]
pub struct PurgeUserRequest {
    pub user: String,
}

/// GW-DEL-20261121：POST /admin/audit/purge_user —— 清空指定用户的全部
/// 网关审计事件，并留下一条 `user_deleted` 审计记录（官网删除用户时调用）。
async fn admin_audit_purge_user(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<PurgeUserRequest>,
) -> Response {
    if let Some(resp) = check_admin_key(&state, &headers) {
        return resp;
    }
    let user = req.user.trim().to_string();
    if user.is_empty() || user.len() > 64 {
        return (StatusCode::BAD_REQUEST, "user 为空或超长").into_response();
    }
    let removed = state.audit.purge_user(&user);
    let mut ev = crate::audit::AuditEvent::new(
        user.clone(),
        Some("admin"),
        "user_deleted",
        "-",
        crate::console::client_ip_from_headers(&headers),
    );
    ev.error = None;
    state.audit.record(ev);
    tracing::info!(user, removed, "audit purged for deleted user");
    axum::Json(serde_json::json!({
        "ok": true,
        "removed": removed,
        "message": format!("已清除 {removed} 条审计记录并留下 user_deleted 标记")
    }))
    .into_response()
}

/// POST /admin/audit/delete_batch 请求体：{ items: [{timestamp, user, event}] }。
#[derive(Debug, serde::Deserialize)]
pub struct AuditDeleteItem {
    pub timestamp: u64,
    pub user: String,
    pub event: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct AuditDeleteBatchRequest {
    pub items: Vec<AuditDeleteItem>,
}

/// GW-AUDIT-DEL-20261121：POST /admin/audit/delete_batch —— 按
/// (timestamp, user, event) 三元组批量删除指定审计事件（X-Admin-Key 鉴权，
/// 官网后台「网关审计」勾选删除/单条删除代理调用）。
async fn admin_audit_delete_batch(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<AuditDeleteBatchRequest>,
) -> Response {
    if let Some(resp) = check_admin_key(&state, &headers) {
        return resp;
    }
    if req.items.is_empty() || req.items.len() > 500 {
        return (StatusCode::BAD_REQUEST, "items 为空或超限（1-500）").into_response();
    }
    let keys: Vec<(u64, String, String)> = req
        .items
        .iter()
        .map(|it| (it.timestamp, it.user.trim().to_string(), it.event.trim().to_string()))
        .collect();
    let removed = state.audit.delete_by_keys(&keys);
    tracing::info!(requested = keys.len(), removed, "audit delete_batch");
    axum::Json(serde_json::json!({
        "ok": true,
        "removed": removed,
        "message": format!("已删除 {removed} 条审计记录")
    }))
    .into_response()
}

/// POST /admin/audit/record 请求体：{ user, event, resource? }。
#[derive(Debug, serde::Deserialize)]
pub struct AuditRecordRequest {
    pub user: String,
    pub event: String,
    #[serde(default)]
    pub resource: Option<String>,
}

/// GW-SELFDEL-20261121：POST /admin/audit/record —— 官网服务端代用户写入
/// 一条审计事件（如账号注销/自杀删除留痕；X-Admin-Key 鉴权，回环调用）。
async fn admin_audit_record(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<AuditRecordRequest>,
) -> Response {
    if let Some(resp) = check_admin_key(&state, &headers) {
        return resp;
    }
    let user = req.user.trim().to_string();
    let event = req.event.trim().to_string();
    if user.is_empty() || user.len() > 64 || event.is_empty() || event.len() > 64 {
        return (StatusCode::BAD_REQUEST, "user/event 为空或超长（1-64 字符）").into_response();
    }
    let mut ev = crate::audit::AuditEvent::new(
        user,
        Some("user"),
        event,
        req.resource.unwrap_or_else(|| "-".to_string()),
        crate::console::client_ip_from_headers(&headers),
    );
    ev.error = None;
    state.audit.record(ev);
    axum::Json(serde_json::json!({"ok": true, "message": "已记录"})).into_response()
}

/// POST /admin/issue 请求体。
#[derive(Debug, serde::Deserialize)]
pub struct IssueRequest {
    /// 用户主体（如 grants 库用户 id 或邮箱），1–64 字符。
    pub sub: String,
    /// 角色列表（≤8 项，每项 ≤32 字符；空列表按 ["user"] 处理）。
    #[serde(default)]
    pub roles: Vec<String>,
    /// 有效期秒数（可选；缺省 = [auth] token_ttl_seconds；上限 MAX_ISSUE_TTL_SECONDS）。
    /// TTL-MAX-20261121：用户自助签发支持自定义有效期（默认 1h），
    /// 最长约 10 年（31536000×10，即用户可感知的"无限期"）；仍受本常量硬钳制。
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
    /// GW-ISSUERES-20261122：本次签发面向的资源 ID（可选；仅审计展示用途，
    /// 不改变 token 权利范围——token 仍为身份凭证，资源绑定在连接时按 grants 校验）。
    /// 官网侧 issue.php 已先校验 grants 归属；此处仅做长度/字符钳制后入审计。
    #[serde(default)]
    pub resource: Option<String>,
}

/// /admin/issue 允许的最大 token TTL（约 10 年）。历史值 3600 已放宽：
/// 审批关系（grants）由官网侧控制，长期 token 的签发本身受 admin key 保护。
pub const MAX_ISSUE_TTL_SECONDS: u64 = 315_360_000;

/// POST /admin/issue —— X-Admin-Key 鉴权的内部签发端点。
/// 设计边界：仅供官网服务端在**本机回环**调用（网关绑定 127.0.0.1:8443、
/// nginx 不反代 /admin/*）；审批关系（哪个用户可签哪个资源）由官网侧
/// grants 库负责，本端点只做「按需签发 + 审计留痕」。
async fn admin_issue(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<IssueRequest>,
) -> Response {
    if let Some(resp) = check_admin_key(&state, &headers) {
        return resp;
    }
    let sub = req.sub.trim();
    if sub.is_empty() || sub.len() > 64 {
        return (StatusCode::BAD_REQUEST, "sub 为空或超长（1-64 字符）").into_response();
    }
    if req.roles.len() > 8 {
        return (StatusCode::BAD_REQUEST, "roles 数量超限（≤8）").into_response();
    }
    let roles: Vec<String> = req
        .roles
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() <= 32)
        .collect();
    let roles = if roles.is_empty() { vec!["user".to_string()] } else { roles };
    let ttl = req.ttl_seconds.unwrap_or(0).min(MAX_ISSUE_TTL_SECONDS);
    let issued = if ttl == 0 {
        state.auth.issue_token(
            sub,
            &roles.iter().map(String::as_str).collect::<Vec<_>>(),
        )
    } else {
        state.auth.issue_token_with_ttl(
            sub,
            &roles.iter().map(String::as_str).collect::<Vec<_>>(),
            ttl,
        )
    };
    let token = match issued {
        Ok(t) => t,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "签发 token 失败").into_response(),
    };
    let expires_in = if ttl == 0 { state.auth.default_ttl_seconds() } else { ttl };
    /* GW-ISSUERES-20261122：审计记录签发面向的资源（1-128 字符，同 grants resource_id 字符集；
       非法或缺省落 "-"，不影响签发主流程） */
    let audit_resource = req
        .resource
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty() && r.len() <= 128 && r.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'))
        .unwrap_or("-");
    let mut ev = crate::audit::AuditEvent::new(
        sub,
        Some(crate::policy::canonical_role(&roles).as_str()),
        "admin_issue",
        audit_resource,
        crate::console::client_ip_from_headers(&headers),
    );
    ev.error = None;
    state.audit.record(ev);
    tracing::info!(sub, roles = roles.len(), "admin issue token");
    axum::Json(serde_json::json!({
        "ok": true,
        "token": token,
        "expires_in": expires_in,
        "token_type": "Bearer"
    }))
    .into_response()
}

// ---------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn event(user: &str, event: &str) -> AuditEvent {
        AuditEvent {
            timestamp: 1_700_000_000,
            user: user.to_string(),
            role: Some("user".to_string()),
            event: event.to_string(),
            resource: "rdp-win01".to_string(),
            source_ip: "10.0.0.5".to_string(),
            error: None,
        }
    }

    #[test]
    fn ring_buffer_overwrites_oldest_when_full() {
        let log = AuditLog::new(3);
        for i in 0..5 {
            log.record(event(&format!("u{i}"), "tunnel_open"));
        }
        let snap = log.snapshot();
        // 容量 3，只保留最新 3 条，且顺序为旧 -> 新
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].user, "u2");
        assert_eq!(snap[2].user, "u4");
    }

    #[test]
    fn snapshot_preserves_order_and_count() {
        let log = AuditLog::new(16);
        log.record(event("alice", "auth_login"));
        log.record(event("bob", "tunnel_open"));
        let snap = log.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].event, "auth_login");
        assert_eq!(snap[1].event, "tunnel_open");
    }

    // ---------------- 管理密钥端点测试 ----------------

    use crate::resources::ResourceRegistry;
    use axum::http::{Request, StatusCode as HttpCode};
    use tower::ServiceExt; // oneshot

    fn admin_state(admin_key: Option<&str>) -> Arc<AppState> {
        Arc::new(AppState {
            auth: crate::auth::AuthState::new("test-secret", 600),
            audit: AuditLog::new(64),
            resources: ResourceRegistry::from_configs(&[]),
            admin_key: admin_key.map(|k| Arc::new(k.to_string())),
            policy: crate::policy::PolicyHandle::default(),
            totp_secrets: Default::default(),
            oidc: None,
        })
    }

    async fn call_admin_audit(state: &Arc<AppState>, key: Option<&str>, limit: Option<usize>) -> (HttpCode, serde_json::Value) {
        let mut uri = "/admin/audit".to_string();
        if let Some(l) = limit {
            uri.push_str(&format!("?limit={l}"));
        }
        let mut req = Request::builder()
            .method("GET")
            .uri(uri)
            .body(axum::body::Body::empty())
            .expect("build request");
        if let Some(k) = key {
            req.headers_mut()
                .insert("X-Admin-Key", k.parse().expect("header value"));
        }
        let app = admin_router(Arc::clone(state)).with_state(Arc::clone(state));
        let resp = app.oneshot(req).await.expect("oneshot");
        let code = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .expect("body");
        let json = if body.is_empty() {
            serde_json::json!(null)
        } else {
            serde_json::from_slice(&body).unwrap_or(serde_json::json!(null))
        };
        (code, json)
    }

    #[tokio::test]
    async fn admin_audit_rejects_missing_and_wrong_key() {
        let state = admin_state(Some("correct-key"));
        // 无头
        let (code, _) = call_admin_audit(&state, None, None).await;
        assert_eq!(code, HttpCode::UNAUTHORIZED);
        // 错误密钥
        let (code, _) = call_admin_audit(&state, Some("wrong-key"), None).await;
        assert_eq!(code, HttpCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_audit_disabled_without_key() {
        let state = admin_state(None);
        let (code, _) = call_admin_audit(&state, Some("anything"), None).await;
        assert_eq!(code, HttpCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn admin_audit_success_and_row_shape() {
        let state = admin_state(Some("k1"));
        state.audit.record(event("alice", "tunnel_open"));
        let mut ev = event("bob", "tunnel_close");
        ev.error = Some("backend unreachable".to_string());
        state.audit.record(ev);
        let (code, json) = call_admin_audit(&state, Some("k1"), None).await;
        assert_eq!(code, HttpCode::OK);
        assert_eq!(json["ok"], serde_json::json!(true));
        assert_eq!(json["count"], serde_json::json!(2));
        let events = json["events"].as_array().expect("events array");
        assert_eq!(events.len(), 2);
        // 最新在前
        assert_eq!(events[0]["event"], "tunnel_close");
        assert_eq!(events[0]["result"], "error:backend unreachable");
        assert_eq!(events[1]["event"], "tunnel_open");
        assert_eq!(events[1]["result"], "ok");
        for row in events {
            for field in ["timestamp", "user", "role", "event", "source_ip", "resource", "result"] {
                assert!(row.get(field).is_some(), "row missing field {field}");
            }
        }
    }

    #[tokio::test]
    async fn admin_audit_limit_boundaries() {
        let state = admin_state(Some("k1"));
        for i in 0..5 {
            state.audit.record(event(&format!("u{i}"), "tunnel_open"));
        }
        // limit=2 取最新 2 条
        let (code, json) = call_admin_audit(&state, Some("k1"), Some(2)).await;
        assert_eq!(code, HttpCode::OK);
        assert_eq!(json["count"], serde_json::json!(2));
        assert_eq!(json["events"][0]["user"], serde_json::json!("u4")); // F5-V1 起对外行含 user
        assert_eq!(json["events"][0]["event"], "tunnel_open");
        // limit=0 钳制到 1
        let (_, json) = call_admin_audit(&state, Some("k1"), Some(0)).await;
        assert_eq!(json["count"], serde_json::json!(1));
        // limit 超上限钳制到 500，且不超过缓冲实际条数
        let (_, json) = call_admin_audit(&state, Some("k1"), Some(999_999)).await;
        assert_eq!(json["count"], serde_json::json!(5));
        // 缺省 limit = 100
        let (_, json) = call_admin_audit(&state, Some("k1"), None).await;
        assert_eq!(json["count"], serde_json::json!(5));
    }

    // ---------------- R-20261120-009：/admin/issue 内部签发 ----------------

    async fn call_admin_issue(
        state: &Arc<AppState>,
        key: Option<&str>,
        body: serde_json::Value,
    ) -> (HttpCode, serde_json::Value) {
        let mut req = Request::builder()
            .method("POST")
            .uri("/admin/issue")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .expect("build request");
        if let Some(k) = key {
            req.headers_mut()
                .insert("X-Admin-Key", k.parse().expect("header value"));
        }
        let app = admin_router(Arc::clone(state)).with_state(Arc::clone(state));
        let resp = app.oneshot(req).await.expect("oneshot");
        let code = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.expect("body");
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::json!(null));
        (code, json)
    }

    #[tokio::test]
    async fn admin_issue_requires_admin_key() {
        let state = admin_state(Some("k1"));
        let body = serde_json::json!({"sub": "u1"});
        let (code, _) = call_admin_issue(&state, None, body.clone()).await;
        assert_eq!(code, HttpCode::UNAUTHORIZED, "缺 X-Admin-Key 必须 401");
        let (code, _) = call_admin_issue(&state, Some("wrong"), body).await;
        assert_eq!(code, HttpCode::UNAUTHORIZED, "错密钥必须 401");
        // admin_key 未配置 → 503（端点整体禁用）
        let none = admin_state(None);
        let (code, _) = call_admin_issue(&none, Some("k"), serde_json::json!({"sub":"u1"})).await;
        assert_eq!(code, HttpCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn admin_issue_issues_verifiable_short_token() {
        let state = admin_state(Some("k1"));
        let (code, json) = call_admin_issue(
            &state,
            Some("k1"),
            serde_json::json!({"sub": "u7", "roles": ["user"], "ttl_seconds": 300}),
        )
        .await;
        assert_eq!(code, HttpCode::OK);
        assert_eq!(json["ok"], serde_json::json!(true));
        assert_eq!(json["expires_in"], serde_json::json!(300));
        let claims = state
            .auth
            .validate_token(json["token"].as_str().expect("token"))
            .expect("issued token must validate");
        assert_eq!(claims.sub, "u7");
        assert_eq!(claims.roles, vec!["user".to_string()]);
        assert_eq!(claims.exp - claims.iat, 300);
        // 审计留痕 admin_issue
        assert!(state
            .audit
            .snapshot()
            .iter()
            .any(|e| e.event == "admin_issue" && e.user == "u7"));
    }

    #[tokio::test]
    async fn admin_issue_rejects_bad_sub_and_roles() {
        let state = admin_state(Some("k1"));
        // 空 sub
        let (code, _) = call_admin_issue(&state, Some("k1"), serde_json::json!({"sub": "  "})).await;
        assert_eq!(code, HttpCode::BAD_REQUEST);
        // roles 超数量
        let many: Vec<String> = (0..9).map(|i| format!("r{i}")).collect();
        let (code, _) = call_admin_issue(
            &state,
            Some("k1"),
            serde_json::json!({"sub": "u1", "roles": many}),
        )
        .await;
        assert_eq!(code, HttpCode::BAD_REQUEST);
        // 空角色列表回退 ["user"]
        let (code, json) = call_admin_issue(&state, Some("k1"), serde_json::json!({"sub": "u2", "roles": []})).await;
        assert_eq!(code, HttpCode::OK);
        let claims = state.auth.validate_token(json["token"].as_str().unwrap()).unwrap();
        assert_eq!(claims.roles, vec!["user".to_string()]);
    }

    #[tokio::test]
    async fn admin_issue_ttl_clamped_to_max() {
        let state = admin_state(Some("k1"));
        let (code, json) = call_admin_issue(
            &state,
            Some("k1"),
            serde_json::json!({"sub": "u9", "ttl_seconds": u64::MAX - 1}),
        )
        .await;
        assert_eq!(code, HttpCode::OK);
        let claims = state.auth.validate_token(json["token"].as_str().unwrap()).unwrap();
        assert!(
            claims.exp - claims.iat <= MAX_ISSUE_TTL_SECONDS,
            "TTL 必须钳制到 ≤MAX_ISSUE_TTL_SECONDS"
        );
        // 自定义 TTL（默认 1h 档）应原样生效
        let (_code, json2) = call_admin_issue(
            &state,
            Some("k1"),
            serde_json::json!({"sub": "u9b", "ttl_seconds": 86_400}),
        )
        .await;
        let claims2 = state.auth.validate_token(json2["token"].as_str().unwrap()).unwrap();
        assert_eq!(claims2.exp - claims2.iat, 86_400);
    }

    // ---------------- F5-V1：过滤查询 + CSV 导出 ----------------

    /// GET /admin/audit.csv 原始文本（校验 Content-Type 与逐行内容）。
    async fn call_admin_csv(
        state: &Arc<AppState>,
        key: Option<&str>,
        query: &str,
    ) -> (HttpCode, String, String) {
        let uri = format!("/admin/audit.csv{query}");
        let mut req = Request::builder()
            .method("GET")
            .uri(uri)
            .body(axum::body::Body::empty())
            .expect("build request");
        if let Some(k) = key {
            req.headers_mut()
                .insert("X-Admin-Key", k.parse().expect("header value"));
        }
        let app = admin_router(Arc::clone(state)).with_state(Arc::clone(state));
        let resp = app.oneshot(req).await.expect("oneshot");
        let code = resp.status();
        let ctype = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .expect("body");
        (code, ctype, String::from_utf8_lossy(&body).to_string())
    }

    #[test]
    fn csv_field_escapes_comma_quote_and_newlines() {
        // 普通字段原样
        assert_eq!(csv_field("alice"), "alice");
        assert_eq!(csv_field("10.0.0.5"), "10.0.0.5");
        // 逗号 / 引号 / 换行（LF）/ 回车换行（CRLF）注入全部触发整体引号包裹
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("line1\nline2"), "\"line1\nline2\"");
        assert_eq!(csv_field("line1\r\nline2"), "\"line1\r\nline2\"");
        // 公式注入载荷同样被引号包裹（不再破坏 CSV 结构）
        assert_eq!(csv_field("=cmd|' /C calc'!A0"), "\"=cmd|' /C calc'!A0\"");
        // 组合注入
        let nasty = "x\",\"y\n\"z";
        assert_eq!(csv_field(nasty), "\"x\"\",\"\"y\n\"\"z\"");
    }

    #[test]
    fn query_filter_combinations() {
        let q = |user: Option<&str>, resource: Option<&str>, result: Option<&str>| AuditQuery {
            user: user.map(str::to_string),
            event: None,
            resource: resource.map(str::to_string),
            result: result.map(str::to_string),
            limit: None,
        };
        let mut ok_ev = event("alice", "tunnel_open");
        ok_ev.role = Some("user".to_string());
        let mut err_ev = event("bob", "policy_denied");
        err_ev.role = Some("readonly".to_string());
        err_ev.resource = "rdp-linux01".to_string();
        err_ev.error = Some("matrix deny".to_string());
        let events = [ok_ev, err_ev];

        let hit = |qq: &AuditQuery| events.iter().filter(|e| qq.matches(e)).count();

        // 无过滤全命中
        assert_eq!(hit(&q(None, None, None)), 2);
        // 单条件
        assert_eq!(hit(&q(Some("alice"), None, None)), 1);
        assert_eq!(hit(&q(None, Some("rdp-linux01"), None)), 1);
        assert_eq!(hit(&q(None, None, Some("ok"))), 1);
        assert_eq!(hit(&q(None, None, Some("error"))), 1); // 裸 "error" 匹配全部错误事件
        // 组合：bob 的错误事件
        assert_eq!(hit(&q(Some("bob"), Some("rdp-linux01"), Some("error"))), 1);
        // 组合无命中
        assert_eq!(hit(&q(Some("alice"), None, Some("error"))), 0);
        assert_eq!(hit(&q(None, Some("nope"), None)), 0);
        // 精确 result 前缀
        assert_eq!(hit(&q(None, None, Some("error:matrix deny"))), 1);
    }

    #[tokio::test]
    async fn csv_export_requires_admin_key() {
        let state = admin_state(Some("k1"));
        let (code, _, _) = call_admin_csv(&state, None, "").await;
        assert_eq!(code, HttpCode::UNAUTHORIZED);
        let (code, _, _) = call_admin_csv(&state, Some("wrong"), "").await;
        assert_eq!(code, HttpCode::UNAUTHORIZED);
        let state = admin_state(None);
        let (code, _, _) = call_admin_csv(&state, Some("k"), "").await;
        assert_eq!(code, HttpCode::SERVICE_UNAVAILABLE);
    }

    /// 极简 RFC4180 解析器（仅测试用）：按引号包裹解析记录，
    /// 用于验证字段内换行不会伪造出额外记录。
    fn csv_records(body: &str) -> Vec<Vec<String>> {
        let mut records = Vec::new();
        let mut record = Vec::new();
        let mut field = String::new();
        let mut in_quotes = false;
        let mut chars = body.chars().peekable();
        while let Some(c) = chars.next() {
            if in_quotes {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        in_quotes = false;
                    }
                } else {
                    field.push(c);
                }
            } else {
                match c {
                    '"' => in_quotes = true,
                    ',' => {
                        record.push(std::mem::take(&mut field));
                    }
                    '\n' => {
                        record.push(std::mem::take(&mut field));
                        records.push(std::mem::take(&mut record));
                    }
                    '\r' => {}
                    _ => field.push(c),
                }
            }
        }
        if !field.is_empty() || !record.is_empty() {
            record.push(field);
            records.push(record);
        }
        records
    }

    #[tokio::test]
    async fn csv_export_content_and_injection_safety() {
        let state = admin_state(Some("k1"));
        // 注入载荷：逗号 + 引号 + 换行同时出现在 user 与 error 字段
        let mut ev = event("evil,usr \"x\"\nline2", "tunnel_open");
        ev.error = Some("boom,\"quote\"\r\nnewrow".to_string());
        state.audit.record(ev);
        state.audit.record(event("plain", "tunnel_open"));

        let (code, ctype, body) = call_admin_csv(&state, Some("k1"), "").await;
        assert_eq!(code, HttpCode::OK);
        assert!(ctype.starts_with("text/csv"), "content-type: {ctype}");

        let records = csv_records(&body);
        assert_eq!(records.len(), 3, "RFC4180 解析后记录数: {body:?}"); // header + 2 事件
        assert_eq!(
            records[0],
            vec![
                "timestamp",
                "user",
                "role",
                "event",
                "resource",
                "source_ip",
                "result",
                "error"
            ]
        );
        // 注入事件（最新在前）：字段值原样还原，注入内容未破坏结构
        assert_eq!(records[1][1], "evil,usr \"x\"\nline2");
        assert_eq!(records[1][6], "error:boom,\"quote\"\r\nnewrow");
        // 普通事件行原样
        assert_eq!(
            records[2],
            vec![
                "1700000000",
                "plain",
                "user",
                "tunnel_open",
                "rdp-win01",
                "10.0.0.5",
                "ok",
                ""
            ]
        );
    }

    #[tokio::test]
    async fn csv_export_respects_filters() {
        let state = admin_state(Some("k1"));
        state.audit.record(event("alice", "tunnel_open"));
        let mut ev = event("bob", "tunnel_close");
        ev.error = Some("x".to_string());
        state.audit.record(ev);

        let (_, _, body) = call_admin_csv(&state, Some("k1"), "?user=alice").await;
        assert!(body.contains("alice"));
        assert!(!body.contains("bob"));

        let (_, _, body) = call_admin_csv(&state, Some("k1"), "?result=error").await;
        assert!(body.contains("bob"));
        assert!(!body.contains(",alice,"));
    }
}
