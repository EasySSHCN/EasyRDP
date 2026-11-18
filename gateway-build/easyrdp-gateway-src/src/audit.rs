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
    routing::get,
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
}

/// 查询参数：可选按用户/事件过滤，limit 限制返回条数。
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
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
        .filter(|e| q.user.as_ref().map_or(true, |u| &e.user == u))
        .filter(|e| q.event.as_ref().map_or(true, |ev| &e.event == ev))
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

/// 单条对外展示的审计行（时间/事件/来源 IP/资源/结果）。
#[derive(Debug, Serialize)]
pub struct AdminAuditRow {
    /// 事件时间（Unix 秒）
    pub timestamp: u64,
    /// 事件类型
    pub event: String,
    /// 来源 IP
    pub source_ip: String,
    /// 资源
    pub resource: String,
    /// 结果："ok" 或 "error:<摘要>"
    pub result: String,
}

async fn admin_audit(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<AuditQuery>,
) -> Response {
    // 管理密钥未配置 => 端点禁用
    let Some(key) = state.admin_key.as_deref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "admin_key 未配置，管理端点已禁用").into_response();
    };
    // 校验 X-Admin-Key 头（恒定时间比较语义近似：长度 + 内容双重比较）
    let provided = headers
        .get("X-Admin-Key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if provided.is_empty() {
        return (StatusCode::UNAUTHORIZED, "缺少 X-Admin-Key").into_response();
    }
    if !constant_time_eq(provided.as_bytes(), key.as_bytes()) {
        return (StatusCode::UNAUTHORIZED, "X-Admin-Key 无效").into_response();
    }

    let limit = q
        .limit
        .unwrap_or(ADMIN_AUDIT_DEFAULT_LIMIT)
        .clamp(1, ADMIN_AUDIT_MAX_LIMIT);

    let mut events = state.audit.snapshot();
    if events.len() > limit {
        events = events.split_off(events.len() - limit); // 取最新 limit 条
    }
    events.reverse(); // 最新在前，供后台列表直接展示

    let rows: Vec<AdminAuditRow> = events
        .into_iter()
        .map(|e| AdminAuditRow {
            timestamp: e.timestamp,
            event: e.event,
            source_ip: e.source_ip,
            resource: e.resource,
            result: match e.error {
                None => "ok".to_string(),
                Some(err) => format!("error:{err}"),
            },
        })
        .collect();

    Json(serde_json::json!({
        "ok": true,
        "count": rows.len(),
        "events": rows
    }))
    .into_response()
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
pub fn admin_router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/audit", get(admin_audit))
        .with_state(state)
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
            for field in ["timestamp", "event", "source_ip", "resource", "result"] {
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
        assert_eq!(json["events"][0]["user"], serde_json::json!(null)); // 对外行不含 user 字段
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
}
