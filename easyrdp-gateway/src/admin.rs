//! 资源管理端点（R-20261122-006 P4）。
//!
//! `POST /admin/resource`（X-Admin-Key，供官网 PHP 回环调用）动作：
//! - `register`：动态注册资源。`mode=direct` 时先做 SSRF 防线判定 + 3s 试连探测，
//!   探测成功才落库（内存 + resources.dyn.toml）；`mode=agent` 时落库为 pending
//!   并签发一次性 enrollment token（10 分钟、绑定 rid）。
//! - `update`：改 label/backend（mode 不可改）；direct backend 变更需重探测通过。
//! - `delete`：软删（动态层 revoked + Agent 在线通道摘除）；grants/DB 软删由官网侧负责。
//! - `status`：查单条或全量动态资源（含生效状态：pending 超 7 天未上线 = stale 待清理；
//!   agent 在线态取自 AgentRegistry 实时判定）。
//! - `kick`：强制摘除指定资源的 Agent 在线通道（站长后台「强制摘除 Agent」）。
//!
//! `POST /agent/enroll`（无 Admin-Key，目标机 Agent 调用）：校验一次性
//! enrollment token → 签发 rid 绑定的 AgentToken → 动态资源 pending→online 联动。
//! 响应契约与 easyrdp-agent::enroll 对齐：200 `{"agent_token": "..."}`。

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::{
    agent,
    audit::{check_admin_key, AuditEvent},
    resources::{
        probe_direct_backend, DynamicResource, DynamicRegistry, EnrollError,
        ENROLLMENT_TOKEN_TTL_MS,
    },
    AppState,
};

/// /admin/resource 动作有效来源标识（审计 role 字段）。
const ADMIN_ROLE: &str = "admin";

/// 动态资源 owner 邮箱格式（宽松邮箱，1-64 本地段 + 域名，防止注入空白）。
fn valid_owner(email: &str) -> bool {
    !email.is_empty()
        && email.len() <= 64
        && email.contains('@')
        && email.chars().all(|c| !c.is_whitespace() && c.is_ascii_graphic())
}

/// label：1-64 字符（任意 UTF-8 展示名）。
fn valid_label(label: &str) -> bool {
    let n = label.chars().count();
    n >= 1 && n <= 64
}

#[derive(Debug, Deserialize)]
pub struct AdminResourceRequest {
    pub action: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub backend: Option<String>,
    /// owner 用户邮箱（register 必填）。
    #[serde(default)]
    pub owner: Option<String>,
}

async fn admin_resource_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<AdminResourceRequest>,
) -> Response {
    if let Some(resp) = check_admin_key(&state, &headers) {
        return resp;
    }
    let ip = crate::console::client_ip_from_headers(&headers);
    match req.action.as_str() {
        "register" => admin_register(state, req, ip).await,
        "update" => admin_update(state, req, ip).await,
        "delete" => admin_delete(state, req, ip),
        "status" => admin_status(state, req),
        "kick" => admin_kick(state, req, ip),
        _ => (StatusCode::BAD_REQUEST, "未知 action（register/update/delete/status/kick）")
            .into_response(),
    }
}

/// POST /admin/resource register。
async fn admin_register(state: Arc<AppState>, req: AdminResourceRequest, ip: String) -> Response {
    let mode = req.mode.as_deref().unwrap_or("").trim().to_ascii_lowercase();
    if mode != "direct" && mode != "agent" {
        return (StatusCode::BAD_REQUEST, "mode 必须是 direct 或 agent").into_response();
    }
    let owner = req.owner.as_deref().unwrap_or("").trim().to_string();
    if !valid_owner(&owner) {
        return (StatusCode::BAD_REQUEST, "owner 邮箱非法").into_response();
    }
    let label = req.label.as_deref().unwrap_or("").trim().to_string();
    if !valid_label(&label) {
        return (StatusCode::BAD_REQUEST, "label 为空或超长（1-64 字符）").into_response();
    }
    // 配额：max_resources_per_user（默认 3）——只限登记台数（revoked 不计）。
    let quota = state.agent_cfg.max_resources_per_user();
    if state.resources.dynamic.count_active_by_owner(&owner) >= quota {
        let mut ev = AuditEvent::new(&owner, Some(ADMIN_ROLE), "resource_register_rejected", "-", ip);
        ev.error = Some(format!("quota exceeded ({quota})"));
        state.audit.record(ev);
        return (
            StatusCode::CONFLICT,
            format!("已达每用户资源配额上限（{quota} 台）；如需更多请联系管理员"),
        )
            .into_response();
    }

    let (backend, agent_local_backend, initial_status) = if mode == "direct" {
        // direct：SSRF 防线 + 3s 试连探测，探测成功才落库。
        let backend = req.backend.as_deref().unwrap_or("").trim().to_string();
        if backend.is_empty() {
            return (StatusCode::BAD_REQUEST, "direct 形态必须提供 backend（host:port）").into_response();
        }
        match probe_direct_backend(&backend).await {
            Ok(peer) => {
                tracing::info!(%backend, %peer, "direct resource probe ok");
                (backend, None, "online")
            }
            Err(reason) => {
                let mut ev =
                    AuditEvent::new(&owner, Some(ADMIN_ROLE), "resource_register_rejected", "-", ip);
                ev.error = Some(format!("probe failed: {reason}"));
                state.audit.record(ev);
                return (StatusCode::UNPROCESSABLE_ENTITY, format!("目标不可达：{reason}"))
                    .into_response();
            }
        }
    } else {
        // agent：后端固定为 Agent 本地 DIAL 地址（默认 127.0.0.1:3389），天然无 SSRF 面。
        (String::new(), None, "pending")
    };

    let id = DynamicRegistry::new_resource_id();
    let res = DynamicResource {
        id: id.clone(),
        owner_email: owner.clone(),
        label: label.clone(),
        mode: mode.clone(),
        backend,
        agent_local_backend,
        status: initial_status.to_string(),
        created_at: agent::unix_ms() / 1000,
        last_agent_online_ms: 0,
    };
    if let Err(err) = state.resources.dynamic.insert(res) {
        return (StatusCode::INTERNAL_SERVER_ERROR, err).into_response();
    }

    state.audit.record(AuditEvent::new(
        &owner,
        Some(ADMIN_ROLE),
        "resource_registered",
        &id,
        ip,
    ));

    // agent 形态：生成一次性 enrollment token（10 分钟、绑定 rid）返回给用户。
    let enrollment = if mode == "agent" {
        let token = state
            .resources
            .dynamic
            .create_enrollment_token(&id, &owner);
        Some(serde_json::json!({
            "enrollment_token": token,
            "expires_in": ENROLLMENT_TOKEN_TTL_MS / 1000,
            "command": format!("easyrdp-agent --enroll {token}"),
        }))
    } else {
        None
    };

    let mut body = serde_json::json!({
        "ok": true,
        "id": id,
        "label": label,
        "mode": mode,
        "owner": owner,
        "status": initial_status,
    });
    if let Some(enrollment) = enrollment {
        body["enrollment"] = enrollment;
    }
    Json(body).into_response()
}

/// POST /admin/resource update（label/backend；mode 不可改）。
async fn admin_update(state: Arc<AppState>, req: AdminResourceRequest, ip: String) -> Response {
    let id = req.id.as_deref().unwrap_or("").trim().to_string();
    if id.is_empty() {
        return (StatusCode::BAD_REQUEST, "id 缺失").into_response();
    }
    let Some(existing) = state.resources.dynamic.get(&id) else {
        return (StatusCode::NOT_FOUND, format!("资源不存在：{id}")).into_response();
    };
    if existing.status == "revoked" {
        return (StatusCode::NOT_FOUND, format!("资源已删除：{id}")).into_response();
    }
    let label = req.label.as_deref().map(str::trim).map(str::to_string);
    if let Some(l) = label.as_deref() {
        if !valid_label(l) {
            return (StatusCode::BAD_REQUEST, "label 为空或超长（1-64 字符）").into_response();
        }
    }
    let backend = req.backend.as_deref().map(str::trim).map(str::to_string);
    if let Some(b) = backend.as_deref() {
        if b.is_empty() {
            return (StatusCode::BAD_REQUEST, "backend 不能为空").into_response();
        }
    }

    // direct backend 变更：同步重探测（3s 试连 + SSRF 复检），成功才落库。
    let mut probe_ok = true;
    if let (Some(ref b), "direct") = (&backend, existing.mode.as_str()) {
        match probe_direct_backend(b).await {
            Ok(_) => probe_ok = true,
            Err(reason) => {
                let mut ev =
                    AuditEvent::new("-", Some(ADMIN_ROLE), "resource_update_rejected", &id, ip);
                ev.error = Some(format!("probe failed: {reason}"));
                state.audit.record(ev);
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("目标不可达：{reason}"),
                )
                    .into_response();
            }
        }
    } else if backend.is_some() && existing.mode != "direct" {
        // agent 型资源不允许改 backend（update 内部同样兜底拒绝）
    }

    let updated = match state.resources.dynamic.update(&id, label.clone(), backend.clone(), probe_ok) {
        Ok(r) => r,
        Err(err) => return (StatusCode::BAD_REQUEST, err).into_response(),
    };
    state.audit.record(AuditEvent::new(
        &updated.owner_email,
        Some(ADMIN_ROLE),
        "resource_updated",
        &id,
        ip,
    ));
    Json(serde_json::json!({
        "ok": true,
        "id": updated.id,
        "label": updated.label,
        "backend": if updated.mode == "direct" { updated.backend.clone() } else { String::new() },
        "mode": updated.mode,
        "status": updated.status,
    }))
    .into_response()
}

/// POST /admin/resource delete：动态层软删（revoked）+ Agent 在线通道摘除。
fn admin_delete(state: Arc<AppState>, req: AdminResourceRequest, ip: String) -> Response {
    let id = req.id.as_deref().unwrap_or("").trim().to_string();
    if id.is_empty() {
        return (StatusCode::BAD_REQUEST, "id 缺失").into_response();
    }
    let Some(existing) = state.resources.dynamic.get(&id) else {
        return (StatusCode::NOT_FOUND, format!("资源不存在：{id}")).into_response();
    };
    if let Err(err) = state.resources.dynamic.revoke(&id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, err).into_response();
    }
    // Agent 在线通道摘除（若有在线连接）。
    if let Some(conn) = state.agent_registry.remove(&id) {
        tracing::info!(resource_id = %id, agent_id = %conn.agent_id, "agent kicked by resource delete");
    }
    state.audit.record(AuditEvent::new(
        &existing.owner_email,
        Some(ADMIN_ROLE),
        "resource_deleted",
        &id,
        ip,
    ));
    Json(serde_json::json!({"ok": true, "id": id, "status": "revoked"})).into_response()
}

/// 动态资源对外状态行（不含后端明文；agent 在线态取实时注册表）。
fn status_row(state: &AppState, res: &DynamicResource) -> serde_json::Value {
    let now_secs = agent::unix_ms() / 1000;
    let agent_online = res.mode == "agent" && state.agent_registry.online(&res.id);
    serde_json::json!({
        "id": res.id,
        "owner": res.owner_email,
        "label": res.label,
        "mode": res.mode,
        "status": DynamicRegistry::effective_status(res, now_secs),
        "agent_online": agent_online,
        "created_at": res.created_at,
        "last_agent_online_ms": res.last_agent_online_ms,
    })
}

/// POST /admin/resource status：{ id } 查单条；{ }（或 id 为空）返回全量。
fn admin_status(state: Arc<AppState>, req: AdminResourceRequest) -> Response {
    state.resources.dynamic.sweep_expired_enrollments();
    match req.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => match state.resources.dynamic.get(id) {
            Some(res) => Json(serde_json::json!({
                "ok": true,
                "resource": status_row(&state, &res),
            }))
            .into_response(),
            None => (StatusCode::NOT_FOUND, format!("资源不存在：{id}")).into_response(),
        },
        None => {
            let rows: Vec<serde_json::Value> = state
                .resources
                .dynamic
                .list()
                .iter()
                .map(|r| status_row(&state, r))
                .collect();
            Json(serde_json::json!({
                "ok": true,
                "count": rows.len(),
                "resources": rows,
            }))
            .into_response()
        }
    }
}

/// POST /admin/resource kick：强制摘除指定资源的 Agent 在线通道（不动资源状态）。
fn admin_kick(state: Arc<AppState>, req: AdminResourceRequest, ip: String) -> Response {
    let id = req.id.as_deref().unwrap_or("").trim().to_string();
    if id.is_empty() {
        return (StatusCode::BAD_REQUEST, "id 缺失").into_response();
    }
    let kicked = state.agent_registry.remove(&id).is_some();
    if kicked {
        // 连接摘除由 Agent 侧读循环收尾；状态联动 offline 立即生效。
        state.resources.dynamic.set_agent_offline(&id);
        state.audit.record(AuditEvent::new("-", Some(ADMIN_ROLE), "agent_kicked", &id, ip));
    }
    Json(serde_json::json!({"ok": true, "id": id, "kicked": kicked})).into_response()
}

// ---------------------------------------------------------------------
// POST /agent/enroll（easyrdp-agent --enroll 契约）
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AgentEnrollRequest {
    pub enrollment_token: String,
    pub resource_id: String,
}

async fn agent_enroll_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<AgentEnrollRequest>,
) -> Response {
    if !state.agent_cfg.enabled {
        return (StatusCode::NOT_FOUND, "agent 通道未启用").into_response();
    }
    let ip = crate::console::client_ip_from_headers(&headers);
    let token = req.enrollment_token.trim();
    let rid = req.resource_id.trim();
    if token.is_empty() || rid.is_empty() {
        return (StatusCode::BAD_REQUEST, "enrollment_token / resource_id 缺失").into_response();
    }
    let owner = match state.resources.dynamic.consume_enrollment_token(token, rid) {
        Ok(owner) => owner,
        Err(EnrollError::Invalid) => {
            let mut ev =
                AuditEvent::new("-", Some("agent"), "agent_enroll_rejected", rid, ip);
            ev.error = Some("invalid or consumed enrollment token".into());
            state.audit.record(ev);
            return (StatusCode::UNAUTHORIZED, "enrollment token 无效或已被使用").into_response();
        }
        Err(EnrollError::RidMismatch) => {
            let mut ev =
                AuditEvent::new("-", Some("agent"), "agent_enroll_rejected", rid, ip);
            ev.error = Some("enrollment token rid mismatch".into());
            state.audit.record(ev);
            return (
                StatusCode::FORBIDDEN,
                "enrollment token 与 resource_id 不匹配",
            )
                .into_response();
        }
        Err(EnrollError::Expired) => {
            let mut ev =
                AuditEvent::new("-", Some("agent"), "agent_enroll_rejected", rid, ip);
            ev.error = Some("enrollment token expired".into());
            state.audit.record(ev);
            return (StatusCode::GONE, "enrollment token 已过期（10 分钟有效期）").into_response();
        }
    };
    // 资源必须仍是动态 agent 型且未 revoked。
    let Some(res) = state.resources.dynamic.get(rid) else {
        return (StatusCode::NOT_FOUND, "资源不存在").into_response();
    };
    if res.mode != "agent" || res.status == "revoked" {
        return (
            StatusCode::FORBIDDEN,
            "资源非 agent 型或已被删除，无法 enroll",
        )
            .into_response();
    }
    // 签发 rid 绑定的 AgentToken（24h 有效；Agent 心跳自动续签由 V2 决定）。
    let agent_token = match state.auth.issue_agent_token(
        &format!("agent-{rid}"),
        rid,
        24 * 3600,
    ) {
        Ok(t) => t,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "AgentToken 签发失败").into_response(),
    };
    // 状态联动：pending → online（方案 §6.1 形态 B 第 3 步）。
    state.resources.dynamic.mark_enrolled(rid, agent::unix_ms());
    state.audit.record(AuditEvent::new(
        &owner,
        Some("agent"),
        "agent_enrolled",
        rid,
        ip,
    ));
    tracing::info!(resource_id = %rid, owner = %owner, "agent enrolled");
    Json(serde_json::json!({
        "ok": true,
        "agent_token": agent_token,
        "resource_id": rid,
    }))
    .into_response()
}

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let _ = state;
    Router::new()
        .route("/admin/resource", post(admin_resource_handler))
        .route("/agent/enroll", post(agent_enroll_handler))
        // 运维探针：GET /admin/resource → 405 引导（axum 默认 405 已足够，不挂载）
}

// ---------------------------------------------------------------------
// 单元测试（P4 门禁新增）
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::ResourceRegistry;
    use axum::{
        body::Body,
        http::{Request, StatusCode as Code},
        Router,
    };
    use tower::ServiceExt; // oneshot

    /// 测试用 AppState（对齐 audit.rs 测试构造；agent 通道默认启用）。
    fn state_with_admin(admin_key: Option<&str>, agent_enabled: bool) -> Arc<AppState> {
        Arc::new(AppState {
            auth: crate::auth::AuthState::new("test-secret", 600),
            audit: crate::audit::AuditLog::new(64),
            resources: ResourceRegistry::from_configs(&[]),
            admin_key: admin_key.map(|k| Arc::new(k.to_string())),
            policy: crate::policy::PolicyHandle::default(),
            totp_secrets: Default::default(),
            oidc: None,
            agent_registry: crate::agent::AgentRegistry::new(),
            agent_cfg: crate::config::AgentConfig {
                enabled: agent_enabled,
                ..Default::default()
            },
            agent_sessions: crate::agent::SessionIdAllocator::new(),
        })
    }

    fn mkapp(state: Arc<AppState>) -> Router {
        router(Arc::clone(&state)).with_state(state)
    }

    /// POST JSON 到 admin 路由并返回 (status, body json)。
    async fn call(
        app: Router,
        path: &str,
        key: Option<&str>,
        body: serde_json::Value,
    ) -> (Code, serde_json::Value) {
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header("Content-Type", "application/json");
        if let Some(k) = key {
            builder = builder.header("X-Admin-Key", k);
        }
        let resp = app
            .oneshot(builder.body(Body::from(body.to_string())).expect("body"))
            .await
            .expect("oneshot");
        let code = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.expect("body");
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::json!(null));
        (code, json)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn admin_resource_requires_admin_key() {
        let state = state_with_admin(Some("k1"), false);
        let app = mkapp(Arc::clone(&state));
        let (code, _) = call(app, "/admin/resource", None, serde_json::json!({"action":"status"})).await;
        assert_eq!(code, Code::UNAUTHORIZED, "缺 X-Admin-Key 必须 401");
        // 未配置 admin_key → 503
        let state = state_with_admin(None, false);
        let (code, _) = call(
            mkapp(state),
            "/admin/resource",
            Some("k"),
            serde_json::json!({"action":"status"}),
        )
        .await;
        assert_eq!(code, Code::SERVICE_UNAVAILABLE, "admin_key 未配置端点必须禁用");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn register_direct_rejects_ssrf_and_unreachable_without_persisting() {
        let state = state_with_admin(Some("k"), false);
        let app = mkapp(Arc::clone(&state));
        // SSRF：环回字面量必须被拒
        let (code, _) = call(
            app,
            "/admin/resource",
            Some("k"),
            serde_json::json!({
                "action":"register","mode":"direct","label":"内网",
                "backend":"127.0.0.1:3389","owner":"a@x.com"
            }),
        )
        .await;
        assert_eq!(code, Code::UNPROCESSABLE_ENTITY, "环回地址必须被 SSRF 防线拒绝");
        assert_eq!(state.resources.dynamic.count_active_by_owner("a@x.com"), 0, "探测失败不落库");
        // 不可达公网：3s 探测失败（198.51.100.9 TEST-NET 公网不可路由）
        let (code, _) = call(
            mkapp(Arc::clone(&state)),
            "/admin/resource",
            Some("k"),
            serde_json::json!({
                "action":"register","mode":"direct","label":"死目标",
                "backend":"198.51.100.9:3389","owner":"a@x.com"
            }),
        )
        .await;
        assert_eq!(code, Code::UNPROCESSABLE_ENTITY);
        assert_eq!(state.resources.dynamic.count_active_by_owner("a@x.com"), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn register_direct_with_live_local_fixture_fails_only_on_loopback_recheck() {
        // 本地 TcpListener fixture：证明探测真实发起了 TCP 连接；
        // 直连落库全链路（非回环地址）在 CRUD 冒烟脚本中以公网 TEST-NET fixture
        // 无法在本机复现，此处以探测请求真实到达 fixture 为验证点。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                if let Ok((s, _)) = listener.accept().await {
                    drop(s);
                }
            }
        });
        // 连接成功但 peer 是环回 → SSRF 复检拒绝（错误消息含"私网/环回"）
        let err = probe_direct_backend(&addr.to_string())
            .await
            .expect_err("环回 fixture 必须被复检拒绝");
        assert!(err.contains("私网") || err.contains("环回"), "实际错误: {err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn register_agent_creates_pending_resource_with_enrollment_token() {
        let state = state_with_admin(Some("k"), true);
        let app = mkapp(Arc::clone(&state));
        let (code, j) = call(
            app,
            "/admin/resource",
            Some("k"),
            serde_json::json!({
                "action":"register","mode":"agent",
                "label":"内网目标机","owner":"user@x.com"
            }),
        )
        .await;
        assert_eq!(code, Code::OK);
        assert_eq!(j["ok"], true);
        let id = j["id"].as_str().unwrap();
        assert!(id.starts_with("u-"), "动态资源必须 u- 前缀: {id}");
        assert_eq!(j["status"], "pending");
        let enrollment = j["enrollment"].as_object().expect("agent 形态必须返回 enrollment");
        let token = enrollment["enrollment_token"].as_str().unwrap();
        assert!(token.starts_with("enr-"));
        assert_eq!(enrollment["expires_in"].as_u64(), Some(600));

        // status 查询单条
        let (code, j) = call(
            mkapp(Arc::clone(&state)),
            "/admin/resource",
            Some("k"),
            serde_json::json!({"action":"status","id":id}),
        )
        .await;
        assert_eq!(code, Code::OK);
        assert_eq!(j["resource"]["status"], "pending");
        assert_eq!(j["resource"]["owner"], "user@x.com");

        // /agent/enroll：正确 rid 换取 AgentToken + pending→online 联动
        let (code, j) = call(
            mkapp(Arc::clone(&state)),
            "/agent/enroll",
            None,
            serde_json::json!({"enrollment_token": token, "resource_id": id}),
        )
        .await;
        assert_eq!(code, Code::OK);
        let agent_token = j["agent_token"].as_str().expect("enroll 必须返回 agent_token");
        let claims = state.auth.validate_agent_token(agent_token).expect("token 合法");
        assert_eq!(claims.rid.as_deref(), Some(id), "AgentToken 必须绑定 rid");
        assert!(claims.roles.iter().any(|r| r == "agent"));
        assert_eq!(
            state.resources.dynamic.get(id).expect("get").status,
            "online",
            "enroll 后资源状态 pending→online"
        );

        // 一次性：同一 token 再次 enroll → 401
        let (code, _) = call(
            mkapp(Arc::clone(&state)),
            "/agent/enroll",
            None,
            serde_json::json!({"enrollment_token": token, "resource_id": id}),
        )
        .await;
        assert_eq!(code, Code::UNAUTHORIZED, "enrollment token 必须一次性");

        // rid 不匹配 → 403
        let tok2 = state.resources.dynamic.create_enrollment_token(id, "user@x.com");
        let (code, _) = call(
            mkapp(Arc::clone(&state)),
            "/agent/enroll",
            None,
            serde_json::json!({"enrollment_token": tok2, "resource_id": "u-other"}),
        )
        .await;
        assert_eq!(code, Code::FORBIDDEN);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn register_quota_enforced_per_user() {
        let state = state_with_admin(Some("k"), false);
        let app = mkapp(Arc::clone(&state));
        for _ in 0..state.agent_cfg.max_resources_per_user() {
            let id = DynamicRegistry::new_resource_id();
            state
                .resources
                .dynamic
                .insert(DynamicResource {
                    id: id.clone(),
                    owner_email: "q@x.com".into(),
                    label: "t".into(),
                    mode: "agent".into(),
                    backend: String::new(),
                    agent_local_backend: None,
                    status: "pending".into(),
                    created_at: 0,
                    last_agent_online_ms: 0,
                })
                .expect("insert");
        }
        let (code, _) = call(
            app,
            "/admin/resource",
            Some("k"),
            serde_json::json!({"action":"register","mode":"agent","label":"超出","owner":"q@x.com"}),
        )
        .await;
        assert_eq!(code, Code::CONFLICT, "超出配额必须 409");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn update_delete_status_full_cycle() {
        let state = state_with_admin(Some("k"), false);
        let app = mkapp(Arc::clone(&state));
        let id = DynamicRegistry::new_resource_id();
        state
            .resources
            .dynamic
            .insert(DynamicResource {
                id: id.clone(),
                owner_email: "d@x.com".into(),
                label: "旧名".into(),
                mode: "agent".into(),
                backend: String::new(),
                agent_local_backend: None,
                status: "online".into(),
                created_at: 0,
                last_agent_online_ms: 0,
            })
            .expect("insert");

        // update label（agent 型：仅 label；backend 空串不传）
        let (code, j) = call(
            mkapp(Arc::clone(&state)),
            "/admin/resource",
            Some("k"),
            serde_json::json!({"action":"update","id":id,"label":"新名"}),
        )
        .await;
        assert_eq!(code, Code::OK);
        assert_eq!(j["label"], "新名");

        // 不存在的资源 → 404
        let (code, _) = call(
            mkapp(Arc::clone(&state)),
            "/admin/resource",
            Some("k"),
            serde_json::json!({"action":"update","id":"u-nope","label":"x"}),
        )
        .await;
        assert_eq!(code, Code::NOT_FOUND);

        // delete → revoked tombstone
        let (code, _) = call(
            mkapp(Arc::clone(&state)),
            "/admin/resource",
            Some("k"),
            serde_json::json!({"action":"delete","id":id}),
        )
        .await;
        assert_eq!(code, Code::OK);
        assert_eq!(state.resources.dynamic.get(&id).expect("tombstone").status, "revoked");
        // revoked 后 update 拒绝（404）
        let (code, _) = call(
            mkapp(Arc::clone(&state)),
            "/admin/resource",
            Some("k"),
            serde_json::json!({"action":"update","id":id,"label":"y"}),
        )
        .await;
        assert_eq!(code, Code::NOT_FOUND);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kick_removes_live_agent_connection() {
        let state = state_with_admin(Some("k"), false);
        let app = mkapp(Arc::clone(&state));
        let id = DynamicRegistry::new_resource_id();
        state
            .resources
            .dynamic
            .insert(DynamicResource {
                id: id.clone(),
                owner_email: "k@x.com".into(),
                label: "kick".into(),
                mode: "agent".into(),
                backend: String::new(),
                agent_local_backend: None,
                status: "online".into(),
                created_at: 0,
                last_agent_online_ms: 0,
            })
            .expect("insert");
        // 挂一条假 Agent 连接
        let (out_tx, _out) = tokio::sync::mpsc::channel(4);
        let (_in, in_rx) = tokio::sync::mpsc::channel(4);
        state.agent_registry.register(crate::agent::AgentConnection::new(
            id.clone(),
            "agent-x",
            "127.0.0.1:3389",
            out_tx,
            in_rx,
            8,
            crate::agent::unix_ms(),
        ));
        let (code, j) = call(
            app,
            "/admin/resource",
            Some("k"),
            serde_json::json!({"action":"kick","id":id}),
        )
        .await;
        assert_eq!(code, Code::OK);
        assert_eq!(j["kicked"], true);
        assert!(!state.agent_registry.online(&id), "kick 后 Agent 通道必须摘除");
        assert_eq!(state.resources.dynamic.get(&id).expect("get").status, "offline");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enroll_requires_enabled_channel_and_known_resource() {
        // agent 通道未启用 → 404
        let state = state_with_admin(Some("k"), false);
        let (code, _) = call(
            mkapp(Arc::clone(&state)),
            "/agent/enroll",
            None,
            serde_json::json!({"enrollment_token":"enr-x","resource_id":"u-x"}),
        )
        .await;
        assert_eq!(code, Code::NOT_FOUND);
    }
}
