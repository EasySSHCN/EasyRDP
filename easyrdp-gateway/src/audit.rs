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
            buf.items[buf.head] = event;
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
}
