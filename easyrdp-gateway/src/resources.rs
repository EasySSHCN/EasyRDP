//! 资源注册表模块：内网 RDP 资源目录 + 后端地址解析 + 角色级授权 + SSRF 防护。
//!
//! 后端地址解析优先级：
//! 1. `config.toml` 的 `[[resources]]` 表：`resource_id -> "host:port"`，
//!    并按该资源可选的 `allowed_roles` 做 JWT claims.roles 交集授权
//!    （缺省 = 所有角色可用；无交集返回 `BackendResolution::Forbidden`）；
//! 2. resource_id 本身是显式 `"host:port"` 格式，且主机通过
//!    `is_allowed_backend` 私网白名单校验（防止 SSRF 任意探测公网/DNS 目标）。
//!
//! 白名单：10.0.0.0/8、172.16.0.0/12、192.168.0.0/16、
//! localhost 与 127.0.0.0/8（IPv4 回环）、::1（IPv6 回环）。
//! DNS 主机名一律拒绝：网关不代查域名，显式 IP 才允许。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::RwLock;

/// config.toml `[[resources]]` 条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceConfig {
    /// 资源 ID（客户端 /tunnel/{resource_id} 使用的标识）
    pub id: String,
    /// 展示名称
    pub name: String,
    /// 后端地址 "host:port"（仅注册表内部可见，不通过 API 暴露；
    /// agent 型资源缺省使用 agent_local_backend，此字段可留空占位）
    #[serde(default)]
    pub backend: String,
    /// 允许访问该资源的角色列表（与 JWT claims.roles 求交集）；
    /// 缺省（None 或空数组）= 所有角色可用。
    #[serde(default)]
    pub allowed_roles: Option<Vec<String>>,
    /// 接入形态（R-20261122-006）：缺省 "direct" = 网关主动连后端；
    /// "agent" = 经目标机 Agent 反向隧道。
    #[serde(default)]
    pub mode: Option<String>,
    /// agent 型资源的本地后端（Agent 在目标机本地 DIAL 的地址），
    /// 缺省 "127.0.0.1:3389"。
    #[serde(default)]
    pub agent_local_backend: Option<String>,
}

/// 资源接入形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceMode {
    /// 公网直连：网关主动 TcpStream::connect(backend)。
    Direct,
    /// 内网反向：网关经 Agent 通道发 DIAL，Agent 在目标机本地建连。
    Agent,
}

impl ResourceMode {
    pub fn from_str(s: &str) -> Option<ResourceMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "direct" => Some(ResourceMode::Direct),
            "agent" => Some(ResourceMode::Agent),
            _ => None,
        }
    }

    #[allow(dead_code)] // P4 /admin/resource 与 UI 列表将使用
    pub fn as_str(self) -> &'static str {
        match self {
            ResourceMode::Direct => "direct",
            ResourceMode::Agent => "agent",
        }
    }
}

/// agent 型资源默认本地后端。
pub const DEFAULT_AGENT_LOCAL_BACKEND: &str = "127.0.0.1:3389";

/// 注册表内部条目：展示名 + 后端地址 + 角色授权 + 接入形态。
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    pub name: String,
    pub backend: String,
    pub allowed_roles: Option<Vec<String>>,
    pub mode: ResourceMode,
    /// agent 型：Agent 本地 DIAL 的后端地址。
    pub agent_local_backend: Option<String>,
}

/// resolve_backend 的授权判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendResolution {
    /// 授权通过，返回后端地址 "host:port"（direct 型：网关主动连接）。
    Allowed(String),
    /// 授权通过，agent 型资源：返回 Agent 本地 DIAL 的后端地址；
    /// 隧道层经反向通道发 DIAL，由目标机 Agent 建连（P2）。
    Agent {
        local_backend: String,
    },
    /// 资源存在但当前角色不被 allowed_roles 允许（映射 403 tunnel_forbidden）。
    Forbidden,
    /// 资源未注册 / 后端地址非法 / 显式直连未过 SSRF 白名单（映射 404）。
    NotFound,
}

/// 对外资源条目（GET /api/resources）：只含 id/name，不含后端地址。
#[derive(Debug, Clone, Serialize)]
pub struct ResourceInfo {
    pub id: String,
    pub name: String,
}

/// 资源注册表：id -> ResourceEntry。
/// 骨架阶段启动时由 config.toml 一次性加载；用 RwLock 便于后续动态注册。
/// P4（R-20261122-006）：并列持有动态资源注册表（用户自助登记层），
/// resolve/list/agent 通道解析均合并静态+动态两层。
#[derive(Default)]
pub struct ResourceRegistry {
    map: RwLock<HashMap<String, ResourceEntry>>,
    /// 动态资源层（内存 + 落盘 resources.dyn.toml；main.rs 启动时 load_from_path）。
    pub dynamic: DynamicRegistry,
}

impl ResourceRegistry {
    pub fn from_configs(configs: &[ResourceConfig]) -> Self {
        let map = configs
            .iter()
            .map(|r| {
                let mode = r
                    .mode
                    .as_deref()
                    .and_then(ResourceMode::from_str)
                    .unwrap_or(ResourceMode::Direct);
                (
                    r.id.clone(),
                    ResourceEntry {
                        name: r.name.clone(),
                        backend: r.backend.clone(),
                        allowed_roles: r.allowed_roles.clone(),
                        mode,
                        agent_local_backend: r.agent_local_backend.clone(),
                    },
                )
            })
            .collect();
        Self {
            map: RwLock::new(map),
            dynamic: DynamicRegistry::default(),
        }
    }

    /// 简易注册（骨架/测试用）：展示名回退为 id，角色授权为缺省全放行。
    pub fn register(&self, id: impl Into<String>, backend: impl Into<String>) {
        let id = id.into();
        self.map
            .write()
            .expect("registry mutex poisoned")
            .insert(
                id.clone(),
                ResourceEntry {
                    name: id,
                    backend: backend.into(),
                    allowed_roles: None,
                    mode: ResourceMode::Direct,
                    agent_local_backend: None,
                },
            );
    }

    /// 完整注册：展示名 + 角色授权。
    pub fn register_full(
        &self,
        id: impl Into<String>,
        name: impl Into<String>,
        backend: impl Into<String>,
        allowed_roles: Option<Vec<String>>,
    ) {
        self.map
            .write()
            .expect("registry mutex poisoned")
            .insert(
                id.into(),
                ResourceEntry {
                    name: name.into(),
                    backend: backend.into(),
                    allowed_roles,
                    mode: ResourceMode::Direct,
                    agent_local_backend: None,
                },
            );
    }

    /// 列出对外资源信息（id/name，不含后端地址与角色授权）。
    /// P4：合并静态 + 动态两层（动态 revoked 条目不展示）。
    pub fn list(&self) -> Vec<ResourceInfo> {
        let mut out: Vec<ResourceInfo> = self
            .map
            .read()
            .expect("registry mutex poisoned")
            .iter()
            .map(|(id, e)| ResourceInfo {
                id: id.clone(),
                name: e.name.clone(),
            })
            .collect();
        for r in self.dynamic.list() {
            if r.status != "revoked" {
                out.push(ResourceInfo {
                    id: r.id,
                    name: r.label,
                });
            }
        }
        out
    }

    /// 判断 claims.roles 是否被 allowed_roles 放行：
    /// allowed_roles 为 None 或空 = 所有角色可用；否则需与角色列表有交集。
    fn roles_allowed(allowed_roles: Option<&[String]>, roles: &[String]) -> bool {
        match allowed_roles {
            None => true,
            Some(list) if list.is_empty() => true,
            Some(list) => roles.iter().any(|r| list.contains(r)),
        }
    }

    /// 依据 resource_id 解析后端 "host:port" 并做角色级授权检查：
    /// 先查注册表（匹配 allowed_roles 才放行）；否则接受显式 "host:port"
    /// 且主机在私网白名单内（显式直连不做角色限制）。
    pub fn resolve_backend(&self, resource_id: &str, roles: &[String]) -> BackendResolution {
        Self::resolve_static(self, resource_id, roles)
    }

    /// P4 用户侧解析（owner 绑定）：动态层优先（owner 一致才放行，
    /// revoked 一律不存在），未命中回退静态层与显式直连。
    /// `sub` = 用户 JWT claims.sub（官网签发口径 = 用户邮箱）。
    pub fn resolve_backend_for_user(
        &self,
        resource_id: &str,
        roles: &[String],
        sub: Option<&str>,
    ) -> BackendResolution {
        if let Some(res) = self.dynamic.resolve_for_user(resource_id, sub) {
            return res;
        }
        self.resolve_backend(resource_id, roles)
    }

    /// 静态层解析（原 resolve_backend 主体）。
    fn resolve_static(&self, resource_id: &str, roles: &[String]) -> BackendResolution {
        let lookup = self
            .map
            .read()
            .expect("registry mutex poisoned")
            .get(resource_id)
            .cloned();

        if let Some(entry) = lookup {
            if !Self::roles_allowed(entry.allowed_roles.as_deref(), roles) {
                return BackendResolution::Forbidden;
            }
            // agent 型资源（R-20261122-006 P2）：不要求 backend 是合法
            // host:port（配置里可以是 "-" 占位），改为返回 Agent 分支，
            // 由隧道层经 Agent 反向通道 DIAL。
            if entry.mode == ResourceMode::Agent {
                return BackendResolution::Agent {
                    local_backend: entry
                        .agent_local_backend
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| DEFAULT_AGENT_LOCAL_BACKEND.to_string()),
                };
            }
            return if parse_host_port(&entry.backend).is_some() {
                BackendResolution::Allowed(entry.backend)
            } else {
                BackendResolution::NotFound
            };
        }

        // 显式 "host:port" 直连：必须格式合法且主机在私网白名单内。
        let Some((host, port)) = parse_host_port(resource_id) else {
            return BackendResolution::NotFound;
        };
        if !is_allowed_backend(host) {
            return BackendResolution::NotFound;
        }
        BackendResolution::Allowed(format!("{host}:{port}"))
    }

    /// Agent 通道接入解析（R-20261122-006）：resource_id 必须已登记为
    /// agent 型资源，且 AgentToken claims（roles 含 agent）通过
    /// allowed_roles 检查（agent 型资源缺省全放行）。返回 Agent 本地
    /// DIAL 的后端地址（缺省 127.0.0.1:3389）；非 agent 型/未登记/角色
    /// 不符一律 None。
    pub fn resolve_agent_backend(
        &self,
        resource_id: &str,
        roles: &[String],
    ) -> Option<String> {
        // P4：动态层优先（revoked 拒绝接入；动态层无 allowed_roles，owner 已在
        // AgentToken 签发链路绑定）。
        if let Some(b) = self.dynamic.resolve_agent_backend_merged(resource_id, roles) {
            return Some(b);
        }
        let lookup = self
            .map
            .read()
            .expect("registry mutex poisoned")
            .get(resource_id)
            .cloned()?;
        if lookup.mode != ResourceMode::Agent {
            return None;
        }
        if !Self::roles_allowed(lookup.allowed_roles.as_deref(), roles) {
            return None;
        }
        Some(
            lookup
                .agent_local_backend
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_AGENT_LOCAL_BACKEND.to_string()),
        )
    }
}

/// 动态资源 ID 前缀（R-20261122-006 P4）：与静态 [[resources]] 命名空间隔离。
pub const DYN_ID_PREFIX: &str = "u-";
/// enrollment token 有效期（毫秒）：一次性、10 分钟。
pub const ENROLLMENT_TOKEN_TTL_MS: u64 = 10 * 60 * 1000;
/// agent 形态资源 7 天无 Agent 上线 → 生效状态标记「待清理」（stale）。
pub const STALE_PENDING_SECS: u64 = 7 * 24 * 3600;

// =====================================================================
// 动态资源注册表（R-20261122-006 P4）：内存 + 落盘 resources.dyn.toml
// =====================================================================

/// 动态（用户自助登记）资源条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicResource {
    /// 资源 ID（`u-` 前缀短随机，防与静态资源冲突/覆盖）。
    pub id: String,
    /// owner 用户邮箱（grants 校验 owner 一致才放行 issue/tunnel）。
    pub owner_email: String,
    /// 展示名称。
    pub label: String,
    /// 接入形态："direct" | "agent"（登记后不可改）。
    pub mode: String,
    /// direct 型后端 "host:port"（探测成功才落库）。
    #[serde(default)]
    pub backend: String,
    /// agent 型本地后端（缺省 127.0.0.1:3389）。
    #[serde(default)]
    pub agent_local_backend: Option<String>,
    /// 存储态：pending | online | offline | revoked；
    /// 生效态（effective_status）在 pending 超 7 天未上线时呈现为 stale（待清理）。
    pub status: String,
    /// 创建时间（unix 秒）。
    pub created_at: u64,
    /// 最近一次 Agent 上线时间（unix 毫秒，0 = 从未上线）。
    #[serde(default)]
    pub last_agent_online_ms: u64,
}

/// 落盘文件结构：resources.dyn.toml。
#[derive(Debug, Default, Serialize, Deserialize)]
struct DynResourcesFile {
    #[serde(default)]
    resources: Vec<DynamicResource>,
}

/// 动态资源注册表：与静态 [[resources]] 并列的运行时资源层。
/// 网关重启自动从落盘文件恢复；每次变更原子写回（tmp + rename）。
#[derive(Default)]
pub struct DynamicRegistry {
    map: RwLock<HashMap<String, DynamicResource>>,
    /// 落盘路径（None = 纯内存，测试用；不落盘）。
    path: RwLock<Option<PathBuf>>,
    /// 一次性 enrollment token 池：token -> (resource_id, owner, expires_unix_ms)。
    enrollments: RwLock<HashMap<String, (String, String, u64)>>,
}

impl DynamicRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从落盘文件恢复（文件缺失 = 空表；解析失败保留空表并告警）。
    pub fn load_from_path(&self, path: &std::path::Path) {
        *self.path.write().expect("dyn path mutex poisoned") = Some(path.to_path_buf());
        match std::fs::read(path) {
            Ok(bytes) => match toml::from_str::<DynResourcesFile>(&String::from_utf8_lossy(&bytes)) {
                Ok(file) => {
                    let mut map = self.map.write().expect("dyn registry mutex poisoned");
                    map.clear();
                    for r in file.resources {
                        map.insert(r.id.clone(), r);
                    }
                    tracing::info!(count = map.len(), "dynamic resources restored");
                }
                Err(err) => {
                    tracing::warn!(%err, "resources.dyn.toml parse failed, starting empty");
                }
            },
            Err(_) => {
                tracing::info!("no existing resources.dyn.toml, dynamic registry starts empty");
            }
        }
    }

    /// 原子落盘（tmp + rename）；未配置路径或写失败仅告警，不影响内存态。
    fn persist(&self) {
        let path_guard = self.path.read().expect("dyn path mutex poisoned");
        let Some(path) = path_guard.as_ref() else { return };
        let file = DynResourcesFile {
            resources: {
                let map = self.map.read().expect("dyn registry mutex poisoned");
                let mut v: Vec<DynamicResource> = map.values().cloned().collect();
                v.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
                v
            },
        };
        let body = match toml::to_string_pretty(&file) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(%err, "dynamic resources serialize failed");
                return;
            }
        };
        let tmp = path.with_extension("toml.tmp");
        if let Err(err) = std::fs::write(&tmp, body.as_bytes())
            .and_then(|_| std::fs::rename(&tmp, path))
        {
            tracing::warn!(%err, path = %path.display(), "dynamic resources persist failed");
        }
    }

    /// 生成新动态资源 ID：`u-` + 8 位 hex。
    pub fn new_resource_id() -> String {
        let hex = uuid::Uuid::new_v4().simple().to_string();
        format!("{}{}", DYN_ID_PREFIX, &hex[..8])
    }

    /// 注册新动态资源（direct 已探测成功 / agent 已生成 enrollment token 由调用方保证）。
    /// ID 冲突返回 Err（防御性：u- 随机 8 位几乎不可能撞）。
    pub fn insert(&self, res: DynamicResource) -> Result<(), String> {
        let mut map = self.map.write().expect("dyn registry mutex poisoned");
        if map.contains_key(&res.id) {
            return Err(format!("动态资源 ID 冲突：{}", res.id));
        }
        map.insert(res.id.clone(), res);
        drop(map);
        self.persist();
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<DynamicResource> {
        self.map
            .read()
            .expect("dyn registry mutex poisoned")
            .get(id)
            .cloned()
    }

    /// 更新 label / backend（mode 不可改——改接入方式应删除重登记）。
    /// direct backend 变更要求 `probe_ok=true`（调用方已重探测成功）；
    /// agent 型资源仅允许改 label（无 backend 面）。
    pub fn update(&self, id: &str, label: Option<String>, backend: Option<String>, probe_ok: bool) -> Result<DynamicResource, String> {
        let mut map = self.map.write().expect("dyn registry mutex poisoned");
        let res = map.get_mut(id).ok_or_else(|| format!("资源不存在：{id}"))?;
        if backend.is_some() {
            if res.mode != "direct" {
                return Err("agent 型资源无 backend 可改；如需调整请删除后重新登记".into());
            }
            if !probe_ok {
                return Err("backend 变更必须先探测成功".into());
            }
            res.backend = backend.unwrap();
            res.status = "online".to_string(); // 探测通过即视为可达
        }
        if let Some(l) = label {
            res.label = l;
        }
        let out = res.clone();
        drop(map);
        self.persist();
        Ok(out)
    }

    /// 摘除（软删除的网关侧动作）：标记 revoked 并保留 tombstone，
    /// 防止同 ID 复用与配额旁路；revoked 不参与 resolve/list/quota。
    pub fn revoke(&self, id: &str) -> Result<DynamicResource, String> {
        let mut map = self.map.write().expect("dyn registry mutex poisoned");
        let res = map.get_mut(id).ok_or_else(|| format!("资源不存在：{id}"))?;
        res.status = "revoked".to_string();
        let out = res.clone();
        drop(map);
        self.persist();
        Ok(out)
    }

    /// 非 revoked 资源数（配额口径：只限登记台数）。
    pub fn count_active_by_owner(&self, owner_email: &str) -> usize {
        self.map
            .read()
            .expect("dyn registry mutex poisoned")
            .values()
            .filter(|r| r.status != "revoked" && r.owner_email.eq_ignore_ascii_case(owner_email))
            .count()
    }

    /// 全量列表（含 revoked tombstone，供管理端审计展示）。
    pub fn list(&self) -> Vec<DynamicResource> {
        let mut v: Vec<DynamicResource> = self
            .map
            .read()
            .expect("dyn registry mutex poisoned")
            .values()
            .cloned()
            .collect();
        v.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        v
    }

    /// 生效状态：pending 超 7 天未上线 → "stale"（待清理）；其余透传存储态。
    pub fn effective_status(res: &DynamicResource, now_unix_secs: u64) -> &'static str {
        if res.status == "pending" && now_unix_secs.saturating_sub(res.created_at) >= STALE_PENDING_SECS {
            "stale"
        } else {
            match res.status.as_str() {
                "pending" => "pending",
                "online" => "online",
                "offline" => "offline",
                "revoked" => "revoked",
                _ => "unknown",
            }
        }
    }

    /// Agent 上线联动：status → online 并记录最近上线时间（revoked 除外）。
    pub fn set_agent_online(&self, id: &str, now_unix_ms: u64) {
        {
            let mut map = self.map.write().expect("dyn registry mutex poisoned");
            if let Some(res) = map.get_mut(id) {
                if res.status == "revoked" || res.status == "pending" {
                    return; // pending 需走 enroll 才翻 online
                }
                res.status = "online".to_string();
                res.last_agent_online_ms = now_unix_ms;
            } else {
                return; // 非动态资源（静态 [[resources]]）无动态状态
            }
        }
        self.persist();
    }

    /// enrollment 完成联动（/agent/enroll）：pending → online（方案 §6.1 形态 B 第 3 步）。
    pub fn mark_enrolled(&self, id: &str, now_unix_ms: u64) -> bool {
        let changed = {
            let mut map = self.map.write().expect("dyn registry mutex poisoned");
            if let Some(res) = map.get_mut(id) {
                if res.status == "pending" && res.mode == "agent" {
                    res.status = "online".to_string();
                    res.last_agent_online_ms = now_unix_ms;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if changed {
            self.persist();
        }
        changed
    }

    /// Agent 断开联动：online → offline。
    pub fn set_agent_offline(&self, id: &str) {
        {
            let mut map = self.map.write().expect("dyn registry mutex poisoned");
            if let Some(res) = map.get_mut(id) {
                if res.status == "online" {
                    res.status = "offline".to_string();
                } else {
                    return;
                }
            } else {
                return;
            }
        }
        self.persist();
    }

    /// 动态层隧道解析（P4）：命中动态资源时按 owner + 状态裁决；
    /// 未命中返回 None（由调用方回退静态层）。revoked 一律 NotFound，
    /// owner 不一致 Forbidden（用户 A 不得隧道用户 B 登记的资源）。
    pub fn resolve_for_user(
        &self,
        resource_id: &str,
        sub: Option<&str>,
    ) -> Option<BackendResolution> {
        let res = self.get(resource_id)?;
        if res.status == "revoked" {
            return Some(BackendResolution::NotFound);
        }
        let owner_ok = sub
            .map(|s| s.eq_ignore_ascii_case(&res.owner_email))
            .unwrap_or(false);
        if !owner_ok {
            return Some(BackendResolution::Forbidden);
        }
        match ResourceMode::from_str(&res.mode).unwrap_or(ResourceMode::Direct) {
            ResourceMode::Agent => Some(BackendResolution::Agent {
                local_backend: res
                    .agent_local_backend
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| DEFAULT_AGENT_LOCAL_BACKEND.to_string()),
            }),
            ResourceMode::Direct => {
                if parse_host_port(&res.backend).is_some() {
                    Some(BackendResolution::Allowed(res.backend))
                } else {
                    Some(BackendResolution::NotFound)
                }
            }
        }
    }

    /// Agent 通道接入解析（P4）：动态 agent 型资源（revoked 拒绝）优先，回退静态层。
    pub fn resolve_agent_backend_merged(&self, resource_id: &str, roles: &[String]) -> Option<String> {
        if let Some(res) = self.get(resource_id) {
            if res.status == "revoked" {
                return None;
            }
            if res.mode != "agent" {
                return None;
            }
            return Some(
                res.agent_local_backend
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| DEFAULT_AGENT_LOCAL_BACKEND.to_string()),
            );
        }
        let _ = roles; // 动态层无 allowed_roles（owner 绑定已足够）；保留参数兼容调用方
        None
    }

    // -----------------------------------------------------------------
    // enrollment token 池（一次性、10 分钟、绑定 rid）
    // -----------------------------------------------------------------

    /// 生成一次性 enrollment token（绑定 resource_id + owner，10 分钟有效）。
    pub fn create_enrollment_token(&self, resource_id: &str, owner_email: &str) -> String {
        let token = format!("enr-{}", uuid::Uuid::new_v4().simple());
        self.enrollments
            .write()
            .expect("enrollment mutex poisoned")
            .insert(token.clone(), (resource_id.to_string(), owner_email.to_string(), unix_secs_ms() + ENROLLMENT_TOKEN_TTL_MS));
        token
    }

    /// 校验并消费一次性 enrollment token：rid 匹配 + 未过期才放行（一次性）。
    pub fn consume_enrollment_token(&self, token: &str, resource_id: &str) -> Result<String, EnrollError> {
        let mut pool = self.enrollments.write().expect("enrollment mutex poisoned");
        match pool.get(token) {
            None => Err(EnrollError::Invalid),
            Some((rid, owner, exp)) => {
                if rid != resource_id {
                    return Err(EnrollError::RidMismatch);
                }
                if unix_secs_ms() > *exp {
                    pool.remove(token);
                    return Err(EnrollError::Expired);
                }
                let owner = owner.clone();
                pool.remove(token); // 一次性：校验通过即消费
                Ok(owner)
            }
        }
    }

    /// 清理过期的 enrollment token（管理端 status / 周期任务可调用；返回清理数）。
    pub fn sweep_expired_enrollments(&self) -> usize {
        let now = unix_secs_ms();
        let mut pool = self.enrollments.write().expect("enrollment mutex poisoned");
        let before = pool.len();
        pool.retain(|_, (_, _, exp)| *exp > now);
        before - pool.len()
    }
}

/// enrollment 校验失败分类（映射 HTTP 状态码：401/403/410）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollError {
    /// token 不存在（无效或已消费）。
    Invalid,
    /// token 与 resource_id 不匹配。
    RidMismatch,
    /// token 已过 10 分钟有效期。
    Expired,
}

fn unix_secs_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// direct 形态 SSRF 防线（P4）：注册探测的主机白名单反向判定——
/// 拒绝私网/环回/链路本地/CGNAT/未指定/组播地址；IPv6 拒绝回环、
/// 唯一本地（fc00::/7）与链路本地（fe80::/10）。公网单播地址放行，
/// DNS 域名允许（真实连通性由 3s 试连探测 + 探测后 peer_addr 复检保证，
/// 域名解析到私网地址时同样被拒）。
pub fn is_forbidden_probe_host(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, _, _] = v4.octets();
            v4.is_loopback()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || (a == 10)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
                || (a == 127)
                || (a == 169 && b == 254)      // 链路本地 169.254/16
                || (a == 100 && (64..=127).contains(&b)) // CGNAT 100.64/10
                || (a == 0)                    // 0.0.0.0/8 未指定段
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
        }
    }
}

/// 解析注册 backend 主机：IP 字面量直接判定；DNS 域名返回 None 交由探测后复检。
pub fn probe_host_forbidden_literal(host: &str) -> Option<bool> {
    let host = host.trim();
    match host.parse::<IpAddr>() {
        Ok(ip) => Some(is_forbidden_probe_host(ip)),
        Err(_) => None, // 非 IP 字面量（域名）：由探测 peer_addr 复检
    }
}

/// direct 注册探测（方案 §6.1 形态 A 第 2 步）：3s 试连，成功且
/// peer_addr 非私网/环回/链路本地才放行。返回 Err 携带面向用户的失败原因。
pub async fn probe_direct_backend(addr: &str) -> Result<std::net::SocketAddr, String> {
    let Some((host, port)) = parse_host_port(addr) else {
        return Err("目标地址格式非法（应为 host:port）".into());
    };
    if probe_host_forbidden_literal(host) == Some(true) {
        return Err("目标地址属于私网/环回/链路本地，禁止登记为直连资源".into());
    }
    let connect = tokio::net::TcpStream::connect((host, port));
    let stream = tokio::time::timeout(std::time::Duration::from_secs(3), connect)
        .await
        .map_err(|_| format!("目标 {host}:{port} 3 秒内不可达"))?
        .map_err(|e| format!("目标 {host}:{port} 连接失败：{e}"))?;
    let peer = stream.peer_addr().map_err(|e| format!("探测连接异常：{e}"))?;
    if is_forbidden_probe_host(peer.ip()) {
        return Err("目标解析到私网/环回/链路本地地址，禁止登记为直连资源".into());
    }
    Ok(peer)
}

/// 解析 "host:port"，返回 (host, port)。host 可为 IP 字面量或 localhost。
pub fn parse_host_port(addr: &str) -> Option<(&str, u16)> {
    let (host, port) = addr.trim().rsplit_once(':')?;
    let host = host.trim();
    if host.is_empty() {
        return None;
    }
    let port = port.parse::<u16>().ok()?;
    if port == 0 {
        return None;
    }
    Some((host, port))
}

/// SSRF 防护白名单：仅允许私网 IPv4 段与回环（localhost / 127.0.0.0/8 / ::1）。
///
/// 规则：
/// - "localhost"（忽略大小写）允许；
/// - 含任何字母的主机串（即 DNS 域名）一律拒绝；
/// - IP 字面量：IPv4 须落在 10/8、172.16/12、192.168/16 或回环 127/8；
///   IPv6 仅允许回环 ::1。
pub fn is_allowed_backend(host: &str) -> bool {
    let host = host.trim();
    if host.is_empty() {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // 拒绝主机名/DNS 域名：仅允许 IP 字面量（IPv6 字面量同样含冒号之外的字母时被拒，
    // 仅十六进制形式的 IPv6 地址可继续走解析）。
    if host.chars().any(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    let ip: IpAddr = match host.parse() {
        Ok(ip) => ip,
        Err(_) => return false,
    };
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, _, _] = v4.octets();
            a == 10 || (a == 172 && (16..=31).contains(&b)) || (a == 192 && b == 168) || a == 127
        }
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

// ---------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_backends_include_private_ranges() {
        for host in [
            "10.0.0.5",
            "10.255.255.255",
            "192.168.1.10",
            "192.168.0.0",
            "172.16.0.1",
            "172.31.255.254",
            "localhost",
            "LOCALHOST",
            "127.0.0.1",
        ] {
            assert!(is_allowed_backend(host), "{host} 应被允许");
        }
    }

    #[test]
    fn disallowed_backends_reject_public_and_dns() {
        for host in [
            "8.8.8.8",
            "1.1.1.1",
            "172.32.0.1",     // 恰好超出 172.16-31
            "172.15.0.1",     // 低于 172.16
            "example.com",    // DNS 域名
            "rdp.internal",   // 内网域名也不放行（不代查 DNS）
            "",               // 空
            "10.0.0.5:3389",  // 带端口的串不是主机
            "[::1]:3389",     // 带端口/括号不是主机
            "0.0.0.0",
            "169.254.1.1",
            "::1",            // IPv6 回环按字面量解析允许
            "2001:db8::1",    // 含字母（db8）且非回环 -> 拒绝
        ] {
            assert_eq!(
                is_allowed_backend(host),
                host == "::1",
                "{host} 判定不符合预期"
            );
        }
    }

    #[test]
    fn parse_host_port_requires_explicit_format() {
        assert_eq!(parse_host_port("10.0.0.5:3389"), Some(("10.0.0.5", 3389)));
        assert_eq!(parse_host_port(" localhost :3389 "), Some(("localhost", 3389)));
        assert_eq!(parse_host_port("10.0.0.5"), None); // 缺端口
        assert_eq!(parse_host_port("10.0.0.5:notaport"), None);
        assert_eq!(parse_host_port("10.0.0.5:0"), None); // 端口 0 无效
        assert_eq!(parse_host_port(":3389"), None); // 缺主机
    }

    #[test]
    fn registry_resolves_configured_resource() {
        let reg = ResourceRegistry::from_configs(&[ResourceConfig {
            id: "rdp-win01".into(),
            name: "Windows 跳板".into(),
            backend: "10.0.0.5:3389".into(),
            allowed_roles: None,
            mode: None,
            agent_local_backend: None,
        }]);
        assert_eq!(
            reg.resolve_backend("rdp-win01", &[]),
            BackendResolution::Allowed("10.0.0.5:3389".into())
        );
        assert_eq!(reg.list().len(), 1);
    }

    #[test]
    fn list_returns_configured_names_not_id_fallback() {
        let reg = ResourceRegistry::from_configs(&[
            ResourceConfig {
                id: "rdp-win01".into(),
                name: "Windows 跳板机".into(),
                backend: "10.0.0.5:3389".into(),
                allowed_roles: None,
                mode: None,
                agent_local_backend: None,
            },
            ResourceConfig {
                id: "rdp-linux01".into(),
                name: "Linux 桌面".into(),
                backend: "192.168.1.20:3390".into(),
                allowed_roles: Some(vec!["ops".into(), "dev".into()]),
                mode: None,
                agent_local_backend: None,
            },
        ]);
        let items = reg.list();
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|r| r.id == "rdp-win01" && r.name == "Windows 跳板机"));
        assert!(items.iter().any(|r| r.id == "rdp-linux01" && r.name == "Linux 桌面"));
        // 简易注册时 name 回退为 id
        reg.register("ad-hoc", "10.0.0.9:3389");
        let ad = reg
            .list()
            .into_iter()
            .find(|r| r.id == "ad-hoc")
            .expect("ad-hoc");
        assert_eq!(ad.name, "ad-hoc");
    }

    #[test]
    fn explicit_host_port_resolved_only_when_allowed() {
        let reg = ResourceRegistry::default();
        // 显式私网 host:port 允许直连（无角色限制）
        assert_eq!(
            reg.resolve_backend("192.168.1.20:3389", &[]),
            BackendResolution::Allowed("192.168.1.20:3389".into())
        );
        // 公网 / DNS 一律拒绝（SSRF 防护）
        assert_eq!(reg.resolve_backend("8.8.8.8:3389", &[]), BackendResolution::NotFound);
        assert_eq!(reg.resolve_backend("evil.example.com:3389", &[]), BackendResolution::NotFound);
        assert_eq!(reg.resolve_backend("not-a-addr", &[]), BackendResolution::NotFound);
    }

    #[test]
    fn registry_backend_must_also_be_valid_host_port() {
        let reg = ResourceRegistry::default();
        reg.register("bad", "10.0.0.5"); // 注册表里缺端口 -> 解析失败
        assert_eq!(reg.resolve_backend("bad", &[]), BackendResolution::NotFound);
        reg.register("ok", "10.0.0.6:3389");
        assert!(matches!(
            reg.resolve_backend("ok", &[]),
            BackendResolution::Allowed(_)
        ));
    }

    #[test]
    fn allowed_roles_matching_role_is_permitted() {
        let reg = ResourceRegistry::from_configs(&[ResourceConfig {
            id: "ops-only".into(),
            name: "运维专用".into(),
            backend: "10.0.0.5:3389".into(),
            allowed_roles: Some(vec!["ops".into(), "admin".into()]),
            mode: None,
            agent_local_backend: None,
        }]);
        assert_eq!(
            reg.resolve_backend("ops-only", &["ops".to_string(), "user".to_string()]),
            BackendResolution::Allowed("10.0.0.5:3389".into())
        );
    }

    #[test]
    fn allowed_roles_mismatch_is_forbidden() {
        let reg = ResourceRegistry::from_configs(&[ResourceConfig {
            id: "ops-only".into(),
            name: "运维专用".into(),
            backend: "10.0.0.5:3389".into(),
            allowed_roles: Some(vec!["ops".into(), "admin".into()]),
            mode: None,
            agent_local_backend: None,
        }]);
        // 角色无交集 -> Forbidden（不得泄露后端地址）
        assert_eq!(
            reg.resolve_backend("ops-only", &["contractor".to_string()]),
            BackendResolution::Forbidden
        );
        assert_eq!(
            reg.resolve_backend("ops-only", &[]),
            BackendResolution::Forbidden
        );
    }

    #[test]
    fn allowed_roles_missing_or_empty_allows_every_role() {
        let reg = ResourceRegistry::from_configs(&[
            ResourceConfig {
                id: "no-roles".into(),
                name: "全放行".into(),
                backend: "10.0.0.5:3389".into(),
                allowed_roles: None,
                mode: None,
                agent_local_backend: None,
            },
            ResourceConfig {
                id: "empty-roles".into(),
                name: "空数组等价全放行".into(),
                backend: "10.0.0.6:3389".into(),
                allowed_roles: Some(vec![]),
                mode: None,
                agent_local_backend: None,
            },
        ]);
        for id in ["no-roles", "empty-roles"] {
            for roles in [
                vec![],
                vec!["user".to_string()],
                vec!["contractor".to_string(), "ops".to_string()],
            ] {
                assert!(
                    matches!(reg.resolve_backend(id, &roles), BackendResolution::Allowed(_)),
                    "{id} 配合角色 {roles:?} 应全放行"
                );
            }
        }
    }

    #[test]
    fn resolve_agent_backend_requires_agent_mode_and_roles() {
        let reg = ResourceRegistry::from_configs(&[
            ResourceConfig {
                id: "agent-host".into(),
                name: "内网 Agent 主机".into(),
                backend: "-".into(),
                allowed_roles: None,
                mode: Some("agent".into()),
                agent_local_backend: Some("127.0.0.1:3390".into()),
            },
            ResourceConfig {
                id: "direct-host".into(),
                name: "公网直连".into(),
                backend: "10.0.0.5:3389".into(),
                allowed_roles: None,
                mode: None,
                agent_local_backend: None,
            },
            ResourceConfig {
                id: "agent-default-backend".into(),
                name: "缺省本地后端".into(),
                backend: "-".into(),
                allowed_roles: None,
                mode: Some("agent".into()),
                agent_local_backend: None,
            },
            ResourceConfig {
                id: "agent-restricted".into(),
                name: "限角色 Agent".into(),
                backend: "-".into(),
                allowed_roles: Some(vec!["ops".into()]),
                mode: Some("agent".into()),
                agent_local_backend: None,
            },
        ]);
        // agent 型：返回配置的本地后端
        assert_eq!(
            reg.resolve_agent_backend("agent-host", &[]),
            Some("127.0.0.1:3390".to_string())
        );
        // 缺省 agent_local_backend → 127.0.0.1:3389
        assert_eq!(
            reg.resolve_agent_backend("agent-default-backend", &[]),
            Some(crate::resources::DEFAULT_AGENT_LOCAL_BACKEND.to_string())
        );
        // direct 型资源不走 Agent 通道
        assert_eq!(reg.resolve_agent_backend("direct-host", &[]), None);
        // 未登记资源
        assert_eq!(reg.resolve_agent_backend("unknown", &[]), None);
        // allowed_roles 限制对 Agent 通道同样生效
        assert_eq!(reg.resolve_agent_backend("agent-restricted", &[]), None);
        assert_eq!(
            reg.resolve_agent_backend("agent-restricted", &["ops".to_string()]),
            Some(crate::resources::DEFAULT_AGENT_LOCAL_BACKEND.to_string())
        );
        // mode 非法字符串按 direct 处理（走 resolve_backend，不走 Agent）
        let bad = ResourceRegistry::from_configs(&[ResourceConfig {
            id: "bad-mode".into(),
            name: "x".into(),
            backend: "10.0.0.7:3389".into(),
            allowed_roles: None,
            mode: Some("fancy".into()),
            agent_local_backend: None,
        }]);
        assert_eq!(bad.resolve_agent_backend("bad-mode", &[]), None);
        assert!(matches!(
            bad.resolve_backend("bad-mode", &[]),
            BackendResolution::Allowed(_)
        ));
    }

    // -----------------------------------------------------------------
    // P4 动态资源注册表单测
    // -----------------------------------------------------------------

    fn dyn_res(id: &str, owner: &str, mode: &str, status: &str) -> DynamicResource {
        DynamicResource {
            id: id.to_string(),
            owner_email: owner.to_string(),
            label: format!("资源 {id}"),
            mode: mode.to_string(),
            backend: if mode == "direct" { "203.0.113.7:3389".into() } else { String::new() },
            agent_local_backend: None,
            status: status.to_string(),
            created_at: 1_000_000,
            last_agent_online_ms: 0,
        }
    }

    #[test]
    fn dyn_insert_get_and_owner_scoped_resolve() {
        let dynr = DynamicRegistry::new();
        dynr.insert(dyn_res("u-ab12cd34", "a@x.com", "direct", "online")).expect("insert");
        // owner 一致：放行 direct 后端
        assert_eq!(
            dynr.resolve_for_user("u-ab12cd34", Some("a@x.com")),
            Some(BackendResolution::Allowed("203.0.113.7:3389".into()))
        );
        // owner 不一致：Forbidden（大小写不敏感）
        assert_eq!(
            dynr.resolve_for_user("u-ab12cd34", Some("b@x.com")),
            Some(BackendResolution::Forbidden)
        );
        // 动态层优先：静态注册表里同名资源也不应被静态层放行给非 owner
        let reg = ResourceRegistry::default();
        reg.dynamic.insert(dyn_res("u-ab12cd34", "a@x.com", "direct", "online")).expect("insert dyn");
        reg.register("u-ab12cd34", "10.0.0.5:3389");
        assert_eq!(
            reg.resolve_backend_for_user("u-ab12cd34", &[], Some("b@x.com")),
            BackendResolution::Forbidden,
            "动态 owner 绑定必须优先于静态层"
        );
        assert_eq!(
            reg.resolve_backend_for_user("u-ab12cd34", &[], Some("a@x.com")),
            BackendResolution::Allowed("203.0.113.7:3389".into())
        );
    }

    #[test]
    fn dyn_revoked_hidden_and_unresolvable() {
        let reg = ResourceRegistry::default();
        reg.dynamic.insert(dyn_res("u-deadbeef", "a@x.com", "direct", "online")).expect("insert");
        reg.dynamic.revoke("u-deadbeef").expect("revoke");
        assert_eq!(
            reg.resolve_backend_for_user("u-deadbeef", &[], Some("a@x.com")),
            BackendResolution::NotFound,
            "revoked 资源对任何人都不存在"
        );
        // list 合并：revoked 不展示
        assert!(!reg.list().iter().any(|r| r.id == "u-deadbeef"));
    }

    #[test]
    fn dyn_quota_counts_only_active_by_owner() {
        let dynr = DynamicRegistry::new();
        dynr.insert(dyn_res("u-1", "a@x.com", "agent", "pending")).expect("1");
        dynr.insert(dyn_res("u-2", "a@x.com", "agent", "offline")).expect("2");
        dynr.insert(dyn_res("u-3", "a@x.com", "direct", "revoked")).expect("3");
        dynr.insert(dyn_res("u-4", "b@x.com", "direct", "online")).expect("4");
        assert_eq!(dynr.count_active_by_owner("a@x.com"), 2, "revoked 不计入配额");
        assert_eq!(dynr.count_active_by_owner("A@X.COM"), 2, "owner 邮箱大小写不敏感");
    }

    #[test]
    fn dyn_update_label_and_backend_mode_immutable() {
        let dynr = DynamicRegistry::new();
        dynr.insert(dyn_res("u-aa11", "a@x.com", "direct", "online")).expect("insert");
        let r = dynr.update("u-aa11", Some("新名字".into()), Some("203.0.113.9:3390".into()), true).expect("update");
        assert_eq!(r.label, "新名字");
        assert_eq!(r.backend, "203.0.113.9:3390");
        assert_eq!(r.status, "online");
        // probe 未通过：backend 变更拒绝
        assert!(dynr.update("u-aa11", None, Some("203.0.113.10:3389".into()), false).is_err());
        // agent 型资源：label 可改，backend 不可改（改接入方式应删除重登记）
        dynr.insert(dyn_res("u-bb22", "a@x.com", "agent", "pending")).expect("agent");
        assert!(dynr.update("u-bb22", Some("x".into()), None, true).is_ok(), "agent label 应可改");
        assert!(dynr.update("u-bb22", None, Some("203.0.113.10:3389".into()), true).is_err(), "agent backend 必须拒绝");
    }

    #[test]
    fn dyn_effective_status_stale_after_seven_days() {
        let mut res = dyn_res("u-cc33", "a@x.com", "agent", "pending");
        assert_eq!(DynamicRegistry::effective_status(&res, res.created_at + 3600), "pending");
        assert_eq!(
            DynamicRegistry::effective_status(&res, res.created_at + STALE_PENDING_SECS),
            "stale",
            "pending 满 7 天未上线 = 待清理"
        );
        res.status = "online".into();
        assert_eq!(
            DynamicRegistry::effective_status(&res, res.created_at + STALE_PENDING_SECS * 10),
            "online",
            "online 不受 7 天规则影响"
        );
    }

    #[test]
    fn dyn_agent_lifecycle_online_offline_enroll() {
        let dynr = DynamicRegistry::new();
        dynr.insert(dyn_res("u-dd44", "a@x.com", "agent", "pending")).expect("insert");
        // pending 不能被普通上线联动翻 online（必须走 enroll）
        dynr.set_agent_online("u-dd44", 5_000);
        assert_eq!(dynr.get("u-dd44").expect("get").status, "pending");
        // enroll：pending → online
        assert!(dynr.mark_enrolled("u-dd44", 6_000), "enroll 应翻转 pending→online");
        let r = dynr.get("u-dd44").expect("get");
        assert_eq!(r.status, "online");
        assert_eq!(r.last_agent_online_ms, 6_000);
        // 断开：online → offline
        dynr.set_agent_offline("u-dd44");
        assert_eq!(dynr.get("u-dd44").expect("get").status, "offline");
        // 再连：offline → online
        dynr.set_agent_online("u-dd44", 9_000);
        assert_eq!(dynr.get("u-dd44").expect("get").status, "online");
        // revoked 不被联动翻转
        dynr.revoke("u-dd44").expect("revoke");
        dynr.set_agent_offline("u-dd44");
        assert_eq!(dynr.get("u-dd44").expect("get").status, "revoked");
    }

    #[test]
    fn dyn_agent_backend_resolution_merged_into_registry() {
        let reg = ResourceRegistry::default();
        reg.dynamic.insert(dyn_res("u-ee55", "a@x.com", "agent", "pending")).expect("insert");
        assert_eq!(
            reg.resolve_agent_backend("u-ee55", &["agent".to_string()]),
            Some(crate::resources::DEFAULT_AGENT_LOCAL_BACKEND.to_string())
        );
        reg.dynamic.revoke("u-ee55").expect("revoke");
        assert_eq!(reg.resolve_agent_backend("u-ee55", &["agent".to_string()]), None);
    }

    #[test]
    fn enrollment_token_one_time_ttl_and_rid_binding() {
        let dynr = DynamicRegistry::new();
        dynr.insert(dyn_res("u-ff66", "a@x.com", "agent", "pending")).expect("insert");
        let tok = dynr.create_enrollment_token("u-ff66", "a@x.com");
        assert!(tok.starts_with("enr-"));
        // rid 不匹配拒绝
        assert_eq!(
            dynr.consume_enrollment_token(&tok, "u-other"),
            Err(EnrollError::RidMismatch)
        );
        // rid 匹配：放行且一次性（token 已消费）
        assert_eq!(dynr.consume_enrollment_token(&tok, "u-ff66"), Ok("a@x.com".to_string()));
        assert_eq!(
            dynr.consume_enrollment_token(&tok, "u-ff66"),
            Err(EnrollError::Invalid),
            "enrollment token 必须一次性"
        );
        // 未知 token 无效
        assert_eq!(
            dynr.consume_enrollment_token("enr-nope", "u-ff66"),
            Err(EnrollError::Invalid)
        );
    }

    #[test]
    fn ssrf_probe_rejects_private_loopback_linklocal_literals() {
        for host in [
            "127.0.0.1", "10.1.2.3", "172.16.0.9", "192.168.1.1", "169.254.169.254",
            "100.64.0.1", "0.0.0.0", "100.100.100.100",
        ] {
            assert_eq!(
                probe_host_forbidden_literal(host),
                Some(true),
                "{host} 必须被判为禁止登记"
            );
        }
        for host in ["203.0.113.7", "8.8.8.8", "198.51.100.9"] {
            assert_eq!(probe_host_forbidden_literal(host), Some(false), "{host} 公网应放行");
        }
        // 域名：字面量阶段不判，交由探测 peer_addr 复检
        assert_eq!(probe_host_forbidden_literal("myhome.ddns.net"), None);
    }
}
