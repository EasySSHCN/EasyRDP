// EasyRDP Tauri 2 应用壳。
//
// 连接中心：保存/删除/列出连接（持久化到应用数据目录 connections.json）。
// RDP 远程画面（FreeRDP FFI）将在 crates/easyrdp-rdp 实现后接入「连接」按钮。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use easyrdp_core::groups::{validate_groups, GroupNode};
use easyrdp_core::profile::{GatewayAuth, GatewayConfig, Resolution, SessionProfile};
use easyrdp_core::rdp_import;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{Manager, State};

pub mod session;
mod feedback;
mod settings;
mod shortcuts;
mod telemetry;
mod updater;
#[cfg(windows)]
mod input_hook;
#[cfg(windows)]
#[path = "rdp_view_final.rs"]
pub mod native_view;


/// 一条已保存的连接（含密码；仅持久化在本地应用数据目录）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedConnection {
    id: String,
    profile: SessionProfile,
    #[serde(default)]
    password: String,
    /// 分组名（EasySSH 风格侧栏分节；空 = 未分组）。旧版本 JSON 缺省兼容。
    /// （遗留字段：新版以 group_id 指向分组树节点；无 group_id 的旧数据
    /// 前端按此字符串渲染为顶层虚拟分组。）
    #[serde(default)]
    group: String,
    /// F1-V1：收藏标记。旧版本 JSON 缺省兼容（false）。
    #[serde(default)]
    favorite: bool,
    /// F1-V1：收藏栏内排序（小在前）。
    #[serde(default)]
    fav_order: i64,
    /// F1-V1：分组树节点 id（空 = 未入树，回退到 group 字符串）。
    #[serde(default)]
    group_id: String,
}

/// 返回给前端的视图：绝不含密码。
#[derive(Debug, Serialize)]
struct ConnectionView {
    id: String,
    name: String,
    host: String,
    port: u16,
    username: String,
    resolution_kind: String,
    /// 自定义分辨率宽/高（仅 resolution_kind == "custom" 时非 0；
    /// 2026-11 UI 修复：编辑自定义分辨率连接时回填宽高输入框）。
    custom_w: u32,
    custom_h: u32,
    /// easyrdp-rdp native 引擎（FreeRDP FFI）就绪后为 true；
    /// 当前 stub 实现恒为 false。
    rdp_ready: bool,
    /// 分组名（空 = 未分组）。
    group: String,
    /// F4 网关接入：网关视图（None = 直连）。token/密码不回传明文。
    gateway: Option<GatewayView>,
    /// F1-V1：收藏 / 收藏排序 / 树节点 id / 列表排序偏好。
    favorite: bool,
    fav_order: i64,
    group_id: String,
    order: i64,
    /// 分组色（由 group_id 解析自分组树；None = 无色/未入树）。
    group_color: Option<String>,
}

/// 网关配置回显视图（绝不含 token/密码明文，只报「已保存」标记）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GatewayView {
    host: String,
    port: u16,
    use_tls: bool,
    /// token | user_password | otp_cookie | inherit
    auth: String,
    has_token: bool,
    username: String,
    has_password: bool,
    resource_id: String,
}

impl GatewayView {
    fn from_cfg(gw: &easyrdp_core::profile::GatewayConfig) -> Self {
        let auth = match gw.auth {
            easyrdp_core::profile::GatewayAuth::Token => "token",
            easyrdp_core::profile::GatewayAuth::UserPassword => "user_password",
            easyrdp_core::profile::GatewayAuth::OtpCookie => "otp_cookie",
            easyrdp_core::profile::GatewayAuth::Inherit => "inherit",
        };
        Self {
            host: gw.hostname.clone(),
            port: gw.port,
            use_tls: gw.use_tls,
            auth: auth.to_string(),
            has_token: gw.token.as_deref().map(str::trim).map_or(false, |s| !s.is_empty()),
            username: gw.username.clone().unwrap_or_default(),
            has_password: gw.password.as_deref().map(str::trim).map_or(false, |s| !s.is_empty()),
            resource_id: gw.resource_id.clone(),
        }
    }
}

impl From<&SavedConnection> for ConnectionView {
    fn from(c: &SavedConnection) -> Self {
        Self::build(c, &[])
    }
}

impl ConnectionView {
    /// 构建视图；`groups` 用于把 group_id 解析成分组色（group_color）。
    pub fn build(c: &SavedConnection, groups: &[GroupNode]) -> Self {
        let (custom_w, custom_h) = match c.profile.resolution {
            Resolution::Custom { w, h } => (w, h),
            _ => (0, 0),
        };
        let group_color = if c.group_id.is_empty() {
            None
        } else {
            groups
                .iter()
                .find(|g| g.id == c.group_id)
                .and_then(|g| g.color.clone())
        };
        Self {
            id: c.id.clone(),
            name: c.profile.name.clone(),
            host: c.profile.host.clone(),
            port: c.profile.port,
            username: c.profile.username.clone(),
            resolution_kind: match c.profile.resolution {
                Resolution::FitWindow => "fit_window".into(),
                Resolution::Fullscreen => "fullscreen".into(),
                Resolution::Custom { .. } => "custom".into(),
            },
            custom_w,
            custom_h,
            rdp_ready: easyrdp_rdp::native_ready(),
            group: c.group.clone(),
            // 网关回显视图：username 解密后回传（标识符，沿用现有 UX）；
            // token/password 永不回传明文（只报 has_token/has_password 标记，
            // 前端留空 = 继承旧值，与主密码字段行为一致）。
            gateway: c.profile.gateway.as_ref().map(|gw| {
                let mut tmp = gw.clone();
                let _ = tmp.unprotect_credentials();
                GatewayView::from_cfg(&tmp)
            }),
            favorite: c.favorite,
            fav_order: c.fav_order,
            group_id: c.group_id.clone(),
            order: c.profile.order,
            group_color,
        }
    }
}

/// 连接存储：内存缓存 + JSON 文件持久化。
/// （pub：session.rs 的 pub command 签名引用 `State<ConnectionStore>`）
pub struct ConnectionStore {
    path: PathBuf,
    items: Mutex<Vec<SavedConnection>>,
    /// F1-V1：分组树（与 connections.json 同目录的 groups.json）。
    groups: Mutex<Vec<GroupNode>>,
}

impl ConnectionStore {
    fn load(path: PathBuf) -> Self {
        let items = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // 分组树文件缺失/损坏 → 空树（前端按遗留 group 字符串渲染扁平分组）
        let groups = std::fs::read(path.with_file_name("groups.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<GroupNode>>(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            items: Mutex::new(items),
            groups: Mutex::new(groups),
        }
    }

    fn persist(&self, items: &[SavedConnection]) -> Result<(), String> {
        let data = serde_json::to_string_pretty(items).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, data).map_err(|e| e.to_string())
    }

    /// 当前分组树快照（校验 + 持久化 + 替换，供 save_groups 命令）。
    /// 校验失败（父缺失/成环/超深/非法色）返回错误列表，不落盘。
    fn replace_groups(&self, groups: &[GroupNode]) -> Result<(), String> {
        let mut cleaned: Vec<GroupNode> = groups.to_vec();
        for g in cleaned.iter_mut() {
            g.id = g.id.trim().to_string();
            g.name = g.name.trim().to_string();
            g.normalize_color();
        }
        validate_groups(&cleaned).map_err(|errs| errs.join("; "))?;
        let data = serde_json::to_string_pretty(&cleaned).map_err(|e| e.to_string())?;
        let path = self.path.with_file_name("groups.json");
        std::fs::write(path, data).map_err(|e| format!("写入 groups.json 失败: {e}"))?;
        *self.groups.lock().unwrap() = cleaned;
        Ok(())
    }

    fn groups_snapshot(&self) -> Vec<GroupNode> {
        self.groups.lock().unwrap().clone()
    }

    /// 更新一条连接的列表元数据（收藏/排序/分组归属），None = 保持原值。
    /// 返回更新后的视图（供前端回显）。
    fn update_meta(
        &self,
        id: &str,
        favorite: Option<bool>,
        fav_order: Option<i64>,
        order: Option<i64>,
        group_id: Option<String>,
    ) -> Result<ConnectionView, String> {
        let mut items = self.items.lock().unwrap();
        let idx = items
            .iter()
            .position(|x| x.id == id)
            .ok_or_else(|| format!("连接不存在: {id}"))?;
        let c = &mut items[idx];
        if let Some(v) = favorite {
            c.favorite = v;
        }
        if let Some(v) = fav_order {
            c.fav_order = v;
        }
        if let Some(v) = order {
            c.profile.order = v;
        }
        if let Some(v) = group_id {
            c.group_id = v.trim().to_string();
        }
        let groups = self.groups_snapshot();
        let view = ConnectionView::build(c, &groups);
        drop(items);
        self.persist(&self.items.lock().unwrap())?;
        Ok(view)
    }

    fn upsert(&self, mut c: SavedConnection) -> Result<ConnectionView, String> {
        let mut items = self.items.lock().unwrap();
        if c.id.is_empty() {
            let n = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            c.id = format!("conn-{n}");
            items.push(c);
            self.persist(&items)?;
            let idx = items.len() - 1;
            return Ok(ConnectionView::from(&items[idx]));
        }
        let idx = match items.iter().position(|x| x.id == c.id) {
            Some(i) => {
                // 密码留空 => 保留原密码；否则用新值覆盖
                Self::merge_password(&mut items[i], c.password.clone());
                c.password = items[i].password.clone();
                // F1-V1：表单编辑不带收藏/树归属等列表元数据（前端编辑路径
                // 这些字段恒为零值）→ 元数据全零值时继承原值，避免编辑表单
                // 意外清掉收藏/拖拽排序结果。
                if !c.favorite && c.fav_order == 0 && c.group_id.is_empty() && c.profile.order == 0 {
                    c.favorite = items[i].favorite;
                    c.fav_order = items[i].fav_order;
                    c.group_id = items[i].group_id.clone();
                    c.profile.order = items[i].profile.order;
                }
                items[i] = c;
                i
            }
            None => {
                items.push(c);
                items.len() - 1
            }
        };
        self.persist(&items)?;
        Ok(ConnectionView::from(&items[idx]))
    }

    /// 更新已有连接：密码留空则保留原密码，否则覆盖。
    fn merge_password(existing: &mut SavedConnection, new_password: String) {
        if !new_password.is_empty() {
            existing.password = new_password;
        }
        // 为空 => 保持原密码不动
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        let mut items = self.items.lock().unwrap();
        items.retain(|x| x.id != id);
        self.persist(&items)
    }

    /// 导出全部连接为 JSON 文件（connections.json 的副本；密码保持本机加密格式）。
    /// 返回导出条数。
    fn export_to_file(&self, path: &str) -> Result<usize, String> {
        let items = self.items.lock().unwrap().clone();
        let data = serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?;
        std::fs::write(path, data).map_err(|e| format!("写入导出文件失败: {e}"))?;
        Ok(items.len())
    }

    /// 从 JSON 文件导入连接（id 冲突跳过；明文密码转本机加密格式）。
    ///
    /// R-20261118-009：导入前先做 JSON Profile Schema 校验
    /// （easyrdp_core::schema::validate_profile_json，宽松模式）：
    /// - 硬错误（缺 host / 类型错 / 端口越界 / 非法枚举）→ 跳过该条并在
    ///   返回错误中逐条列出（带 JSON Pointer 路径），不影响其余条目导入；
    /// - 未知字段仅警告（照常导入）；
    /// - Schema 校验通过后再走 serde 反序列化与模型级 `validate()`。
    /// 返回（新增条数, 跳过条数）。
    fn import_from_file(&self, path: &str) -> Result<(usize, usize), String> {
        let raw = std::fs::read(path).map_err(|e| format!("读取导入文件失败: {e}"))?;
        let incoming: Vec<serde_json::Value> =
            serde_json::from_slice(&raw).map_err(|e| format!("JSON 解析失败: {e}"))?;
        let mut items = self.items.lock().unwrap();
        let mut added = 0usize;
        let mut skipped = 0usize;
        let mut rejected: Vec<String> = Vec::new();
        for (idx, entry) in incoming.iter().enumerate() {
            // 1) Schema 校验（宽松：硬错误才拒绝；未知字段警告放行）。
            //    校验对象是条目内嵌的 profile（兼容顶层即 profile 的裸文件）。
            let profile_value = entry.get("profile").unwrap_or(entry);
            if let Err(issues) = easyrdp_core::schema::validate_profile_json(
                profile_value,
                easyrdp_core::schema::ValidationMode::Lenient,
            ) {
                let label = profile_value
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("<未命名>");
                rejected.push(format!(
                    "第 {} 条（{label}）:\n{}",
                    idx + 1,
                    issues
                        .iter()
                        .map(|i| format!("  - {i}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
                skipped += 1;
                continue;
            }
            // 2) serde 反序列化
            let c: SavedConnection = match serde_json::from_value(entry.clone()) {
                Ok(c) => c,
                Err(e) => {
                    rejected.push(format!("第 {} 条: 反序列化失败: {e}", idx + 1));
                    skipped += 1;
                    continue;
                }
            };
            // 3) 模型级校验（Schema 通过后才进入既有 validate）
            if let Err(errs) = c.profile.validate() {
                let label = if c.profile.name.is_empty() { "<未命名>" } else { c.profile.name.as_str() };
                rejected.push(format!(
                    "第 {} 条（{label}）:\n{}",
                    idx + 1,
                    errs.iter()
                        .map(|e| format!("  - {e}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
                skipped += 1;
                continue;
            }
            let mut c = c;
            if c.id.is_empty() || items.iter().any(|x| x.id == c.id) {
                skipped += 1;
                continue;
            }
            // 明文密码 -> 本机加密格式（已是加密格式则原样保留；失败则丢弃密码）
            if !c.password.is_empty() && !easyrdp_core::secret::is_protected_format(&c.password) {
                c.password = easyrdp_core::secret::protect(&c.password).unwrap_or_default();
            }
            // 发布门禁：导入连接的网关凭据同步加密（旧导出文件可能含明文凭据）
            if let Some(gw) = c.profile.gateway.as_mut() {
                let _ = gw.protect_credentials();
            }
            items.push(c);
            added += 1;
        }
        drop(items);
        if !rejected.is_empty() {
            return Err(format!(
                "导入完成但有 {} 条被拒绝：\n{}",
                rejected.len(),
                rejected.join("\n")
            ));
        }
        self.persist(&self.items.lock().unwrap())?;
        Ok((added, skipped))
    }

    /// 按 id 查找连接，返回（profile, 存储态口令）副本（session_start 用）。
    /// 网关凭据在返回前解密为明文（存储层恒为 DPAPI 密文）；
    /// 解密失败（跨用户密文/损坏）保留原值——连接尝试会因凭据无效失败，
    /// 不阻塞 profile 读取。
    pub fn find(&self, id: &str) -> Option<(SessionProfile, String)> {
        self.items
            .lock()
            .unwrap()
            .iter()
            .find(|x| x.id == id)
            .map(|c| {
                let mut profile = c.profile.clone();
                if let Some(gw) = profile.gateway.as_mut() {
                    let _ = gw.unprotect_credentials();
                }
                (profile, c.password.clone())
            })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveArgs {
    #[serde(default)]
    id: String,
    name: String,
    host: String,
    #[serde(default)]
    port: u16,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    /// 分组名（可选，空 = 未分组）
    #[serde(default)]
    group: String,
    /// fit_window | fullscreen | custom
    /// （字段名 camelCase：前端 invoke 传 resolutionKind/customW/customH；
    /// 2026-11 修复：此前缺 rename_all，serde 静默吞掉 camelCase 字段，
    /// resolution_kind 恒为 "" → 恒 FitWindow，用户设置分辨率从不生效。）
    #[serde(default)]
    resolution_kind: String,
    #[serde(default)]
    custom_w: u32,
    #[serde(default)]
    custom_h: u32,
    /// F4 网关接入（可选）：折叠区展开且填了网关地址时提交。
    #[serde(default)]
    gateway: Option<GatewayArgs>,
    /// F1-V1 列表元数据（前端编辑已有连接时回传；新建恒为零值）。
    #[serde(default)]
    favorite: bool,
    #[serde(default)]
    fav_order: i64,
    #[serde(default)]
    group_id: String,
    #[serde(default)]
    order: i64,
}

/// 网关输入（camelCase 由前端 invoke 提交；token/password 留空 = 保留原值）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GatewayArgs {
    host: String,
    #[serde(default)]
    port: u16,
    #[serde(default)]
    use_tls: bool,
    /// token | user_password
    #[serde(default)]
    auth: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    resource_id: String,
}

impl GatewayArgs {
    /// 组装 GatewayConfig；编辑场景下 token/password 留空 = 继承已保存值。
    fn into_config(self, existing: Option<&GatewayConfig>) -> GatewayConfig {
        let mut gw = GatewayConfig {
            hostname: self.host.trim().to_string(),
            port: if self.port == 0 { 443 } else { self.port },
            use_tls: self.use_tls,
            auth: match self.auth.as_str() {
                "user_password" => GatewayAuth::UserPassword,
                "otp_cookie" => GatewayAuth::OtpCookie,
                "inherit" => GatewayAuth::Inherit,
                _ => GatewayAuth::Token,
            },
            resource_id: self.resource_id.trim().to_string(),
            token: (!self.token.trim().is_empty()).then(|| self.token.trim().to_string()),
            username: (!self.username.trim().is_empty())
                .then(|| self.username.trim().to_string()),
            password: (!self.password.trim().is_empty())
                .then(|| self.password.trim().to_string()),
        };
        // 凭据留空继承旧值（对齐密码 merge_password 语义）
        if let Some(old) = existing {
            if gw.token.is_none() && old.auth == gw.auth {
                gw.token = old.token.clone();
            }
            if gw.username.is_none() {
                gw.username = old.username.clone();
            }
            if gw.password.is_none() && old.auth == gw.auth {
                gw.password = old.password.clone();
            }
        }
        gw
    }
}

/// 保存（新建或更新）一条连接。
#[tauri::command]
fn save_connection(
    args: SaveArgs,
    store: State<ConnectionStore>,
) -> Result<ConnectionView, String> {
    let mut profile = SessionProfile::default();
    profile.name = args.name.trim().to_string();
    profile.host = args.host.trim().to_string();
    profile.port = if args.port == 0 { 3389 } else { args.port };
    profile.username = args.username.trim().to_string();
    profile.resolution = match args.resolution_kind.as_str() {
        "fullscreen" => Resolution::Fullscreen,
        "custom" if args.custom_w > 0 && args.custom_h > 0 => Resolution::Custom {
            w: args.custom_w,
            h: args.custom_h,
        },
        _ => Resolution::FitWindow,
    };
    if profile.name.is_empty() {
        profile.name = profile.host.clone();
    }
    // F4 网关接入：提交了网关参数 → 写入 profile（凭据留空继承旧值）；
    // 未提交 → 直连（gateway = None）。
    let existing_gw = store
        .find(&args.id)
        .and_then(|(p, _)| p.gateway);
    profile.gateway = match args.gateway {
        Some(gw_args) => Some(gw_args.into_config(existing_gw.as_ref())),
        None => None,
    };
    profile.validate().map_err(|errs| errs.join("; "))?;
    // 发布门禁：网关凭据（token/username/password）DPAPI 加密后落盘。
    // 已是密文格式幂等跳过；旧明文数据在此自动完成迁移加密。
    if let Some(gw) = profile.gateway.as_mut() {
        gw.protect_credentials()
            .map_err(|e| format!("网关凭据加密失败: {e}"))?;
    }
    // 密码加密后存储：Windows 用 DPAPI（"dpapi:<base64>"），
    // 非 Windows 暂用 "plain:<明文>" 回退；空密码保持空串。
    let stored_password = if args.password.is_empty() || easyrdp_core::secret::is_protected_format(&args.password) {
        args.password
    } else {
        easyrdp_core::secret::protect(&args.password).map_err(|e| format!("密码加密失败: {e}"))?
    };
    store.upsert(SavedConnection {
        id: args.id,
        profile,
        password: stored_password,
        group: args.group.trim().to_string(),
        favorite: args.favorite,
        fav_order: args.fav_order,
        group_id: args.group_id.trim().to_string(),
    })
}

/// 列出全部连接（不含密码）。
#[tauri::command]
fn list_connections(store: State<ConnectionStore>) -> Result<Vec<ConnectionView>, String> {
    let groups = store.groups_snapshot();
    Ok(store
        .items
        .lock()
        .unwrap()
        .iter()
        .map(|c| ConnectionView::build(c, &groups))
        .collect())
}

/// 列出分组树（F1-V1）。空树 = 尚未使用分组树（前端回退遗留 group 字符串）。
#[tauri::command]
fn list_groups(store: State<ConnectionStore>) -> Result<Vec<GroupNode>, String> {
    Ok(store.groups_snapshot())
}

/// 保存分组树（全量替换；后端校验：父存在/不成环/深度≤3/色板合法）。
#[tauri::command]
fn save_groups(groups: Vec<GroupNode>, store: State<ConnectionStore>) -> Result<(), String> {
    store.replace_groups(&groups)
}

/// 更新一条连接的列表元数据（收藏 / 收藏排序 / 列表排序 / 分组归属）。
/// Option 字段 None = 保持原值。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn update_connection_meta(
    id: String,
    favorite: Option<bool>,
    fav_order: Option<i64>,
    order: Option<i64>,
    group_id: Option<String>,
    store: State<ConnectionStore>,
) -> Result<ConnectionView, String> {
    store.update_meta(&id, favorite, fav_order, order, group_id)
}

/// 删除一条连接。
#[tauri::command]
fn delete_connection(id: String, store: State<ConnectionStore>) -> Result<(), String> {
    store.delete(&id)
}

/// 连通性测试：TCP 握手 host:port，返回耗时（毫秒）。
#[tauri::command]
fn test_connection(host: String, port: u16) -> Result<u128, String> {
    use std::net::ToSocketAddrs;
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("DNS 解析失败: {e}"))?
        .next()
        .ok_or("DNS 解析无结果")?;
    let start = SystemTime::now();
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(3))
        .map(|_| {
            SystemTime::now()
                .duration_since(start)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        })
        .map_err(|e| format!("连接失败: {e}"))
}

/// 导出全部连接为 JSON 文件（connections.json 的副本；密码保持本机加密格式）。
/// 返回导出的连接条数。
#[tauri::command]
fn export_connections(path: String, store: State<ConnectionStore>) -> Result<usize, String> {
    store.export_to_file(&path)
}

/// 从 JSON 文件导入连接（格式同导出文件 / connections.json）：
/// id 冲突则跳过该条；明文密码在导入时转为本机加密格式。
/// 返回（新增条数, 跳过条数）。
#[tauri::command]
fn import_connections(path: String, store: State<ConnectionStore>) -> Result<(usize, usize), String> {
    store.import_from_file(&path)
}

/// 导出单条连接为 .rdp 文件（mstsc 兼容格式）。
///
/// 密码安全约束：.rdp 文件**不含密码字段**——`export_to_string` 只序列化
/// profile（host/port/分辨率/重定向等），存储态加密口令根本不参与导出。
#[tauri::command]
fn export_connection(id: String, path: String, store: State<ConnectionStore>) -> Result<(), String> {
    let (profile, _stored_password) = store
        .find(&id)
        .ok_or_else(|| format!("连接不存在: {id}"))?;
    // 注意：find 返回的第二个元素（加密口令）在此被有意丢弃，绝不写入 .rdp。
    easyrdp_core::export::export_to_file(&profile, std::path::Path::new(&path))
        .map_err(|e| format!("写入 .rdp 失败: {e}"))
}

/// 导入 mstsc 导出的 .rdp 文件并保存为新连接，返回连接视图。
#[tauri::command]
fn import_rdp(path: String, store: State<ConnectionStore>) -> Result<ConnectionView, String> {
    let profile = rdp_import::import(&PathBuf::from(&path)).map_err(|e| e.to_string())?;
    profile.validate().map_err(|errs| errs.join("; "))?;
    let mut saved = SavedConnection {
        id: String::new(),
        profile,
        password: String::new(),
        group: String::new(),
        favorite: false,
        fav_order: 0,
        group_id: String::new(),
    };
    if saved.profile.name.is_empty() {
        saved.profile.name = saved.profile.host.clone();
    }
    store.upsert(saved)
}

// ---------------- R-20261118-008 批量导入与去重 ----------------

/// 批量导入预览条目（回传前端向导表格；duplicate=true 高亮「已存在」）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewEntry {
    source: String,
    entry_index: usize,
    name: String,
    host: String,
    port: u16,
    username: String,
    duplicate: bool,
}

/// 批量导入预览结果：可导入候选 + 逐条失败明细。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportPreview {
    entries: Vec<PreviewEntry>,
    failures: Vec<easyrdp_core::batch_import::ImportFailure>,
}

/// 批量导入预览：解析待导入文件（.rdp 与 .json 混选），与现有连接
/// 按「主机+端口（大小写不敏感）」比对并标注重复项。
/// 失败条目不全量失败，逐条报告原因（缺 Computer、JSON 非法、类型错等）。
#[tauri::command]
fn import_preview(paths: Vec<String>, store: State<ConnectionStore>) -> Result<ImportPreview, String> {
    let pb: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let parsed = easyrdp_core::batch_import::parse_paths(&pb);
    let existing = store.items.lock().unwrap();
    let existing_profiles: Vec<&SessionProfile> = existing.iter().map(|c| &c.profile).collect();
    // 已见键集合：现有连接 + 本次批次内前序候选（批次内互重也标注）
    let mut seen: std::collections::HashSet<String> = existing_profiles
        .iter()
        .map(|p| easyrdp_core::batch_import::dedup_key(&p.host, p.port))
        .collect();
    let mut entries = Vec::new();
    let mut failures = Vec::new();
    for e in parsed {
        match e {
            easyrdp_core::batch_import::ParsedEntry::Ok(c) => {
                let key = easyrdp_core::batch_import::dedup_key(&c.profile.host, c.profile.port);
                let duplicate = !seen.insert(key);
                let name = if c.profile.name.trim().is_empty() {
                    c.profile.host.clone()
                } else {
                    c.profile.name.clone()
                };
                entries.push(PreviewEntry {
                    source: c.source,
                    entry_index: c.entry_index,
                    name,
                    host: c.profile.host,
                    port: c.profile.port,
                    username: c.profile.username,
                    duplicate,
                });
            }
            easyrdp_core::batch_import::ParsedEntry::Fail(f) => failures.push(f),
        }
    }
    Ok(ImportPreview { entries, failures })
}

/// 批量导入提交参数（camelCase）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchImportArgs {
    /// 预览时的文件列表（提交时重新解析，防止序列化 profile 回传）。
    paths: Vec<String>,
    /// 去重策略：skip | overwrite | keep_both。
    strategy: String,
    /// 用户勾选的条目（source+entryIndex 定位；group 为该条目标分组）。
    items: Vec<BatchImportItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchImportItem {
    source: String,
    entry_index: usize,
    /// 目标分组树节点 id（F1 分组树；空 = 未分组）。
    #[serde(default)]
    group_id: String,
}

/// 批量导入提交结果摘要。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchImportReport {
    added: usize,
    skipped: usize,
    overwritten: usize,
    failed: usize,
    failures: Vec<String>,
}

impl ConnectionStore {
    /// 批量导入合并：按 plan_merge 计划落库。
    /// - 覆盖：只替换 profile（保留 id/分组/收藏/密码——密码仅在候选带
    ///   明文密码时经 DPAPI 加密后覆盖）；
    /// - 新增/两者保留：新 id 入库，group 采用用户在向导中指定的分组，
    ///   明文密码 DPAPI 加密存储。
    fn merge_batch(
        &self,
        candidates: Vec<(easyrdp_core::batch_import::ImportCandidate, String)>,
        strategy: easyrdp_core::batch_import::DedupStrategy,
    ) -> easyrdp_core::batch_import::MergeReport {
        let total = candidates.len();
        let mut report = easyrdp_core::batch_import::MergeReport::default();
        let mut items = self.items.lock().unwrap();
        // plan_merge 需要现有 profile 快照（含本批次新增，逐条累积）
        let mut current: Vec<SessionProfile> = items.iter().map(|c| c.profile.clone()).collect();
        for (cand, group_id) in candidates {
            let plan = easyrdp_core::batch_import::plan_merge(&current, std::slice::from_ref(&cand), strategy);
            let mut c = SavedConnection {
                id: String::new(),
                profile: cand.profile.clone(),
                password: String::new(),
                group: String::new(),
                favorite: false,
                fav_order: 0,
                group_id: group_id.trim().to_string(),
            };
            if c.profile.name.trim().is_empty() {
                c.profile.name = c.profile.host.clone();
            }
            // 明文密码 -> 本机加密格式（失败则丢弃密码，不阻塞导入）
            if let Some(pw) = &cand.plaintext_password {
                c.password = easyrdp_core::secret::protect(pw).unwrap_or_default();
            }
            // 网关凭据同步加密（旧导出文件可能含明文凭据）
            if let Some(gw) = c.profile.gateway.as_mut() {
                let _ = gw.protect_credentials();
            }
            match plan.into_iter().next().unwrap_or(easyrdp_core::batch_import::PlanAction::Add) {
                easyrdp_core::batch_import::PlanAction::Add => {
                    c.id = format!("conn-{}", SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
                    current.push(c.profile.clone());
                    items.push(c);
                    report.added += 1;
                }
                easyrdp_core::batch_import::PlanAction::Skip => report.skipped += 1,
                easyrdp_core::batch_import::PlanAction::Overwrite { existing } => {
                    let old = &mut items[existing];
                    // 保留 id/分组/收藏/收藏排序/树归属/列表排序；密码仅在新明文非空时覆盖
                    let kept_id = old.id.clone();
                    let kept_group = old.group.clone();
                    let kept_group_id = old.group_id.clone();
                    let kept_fav = old.favorite;
                    let kept_fav_order = old.fav_order;
                    let kept_order = old.profile.order;
                    let kept_name = old.profile.name.clone();
                    let kept_password = if c.password.is_empty() { old.password.clone() } else { c.password.clone() };
                    c.id = kept_id;
                    c.group = kept_group;
                    c.group_id = kept_group_id;
                    c.favorite = kept_fav;
                    c.fav_order = kept_fav_order;
                    c.profile.order = kept_order;
                    c.profile.name = kept_name;
                    c.password = kept_password;
                    current[existing] = c.profile.clone();
                    items[existing] = c;
                    report.overwritten += 1;
                }
                easyrdp_core::batch_import::PlanAction::KeepBoth { name } => {
                    c.profile.name = name;
                    c.id = format!("conn-{}", SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
                    current.push(c.profile.clone());
                    items.push(c);
                    report.added += 1;
                }
            }
        }
        if let Err(e) = self.persist(&items) {
            return easyrdp_core::batch_import::MergeReport {
                failures: vec![format!("持久化失败: {e}")],
                failed: total,
                ..Default::default()
            };
        }
        report
    }
}

/// 批量导入提交：重新解析文件 → 过滤用户勾选条目 → 按策略合并落库。
/// 返回导入结果摘要（成功/跳过/覆盖/失败 + 失败明细）。
#[tauri::command]
fn import_commit(args: BatchImportArgs, store: State<ConnectionStore>) -> Result<BatchImportReport, String> {
    let strategy = easyrdp_core::batch_import::DedupStrategy::from_str(&args.strategy);
    let pb: Vec<PathBuf> = args.paths.iter().map(PathBuf::from).collect();
    let parsed = easyrdp_core::batch_import::parse_paths(&pb);
    // 预览勾选集合：source + entry_index
    let selected: std::collections::HashSet<(String, usize)> = args
        .items
        .iter()
        .map(|i| (i.source.clone(), i.entry_index))
        .collect();
    let groups: std::collections::HashMap<(String, usize), String> = args
        .items
        .iter()
        .map(|i| ((i.source.clone(), i.entry_index), i.group_id.clone()))
        .collect();
    let mut candidates = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut failed = 0usize;
    for e in parsed {
        match e {
            easyrdp_core::batch_import::ParsedEntry::Ok(c) => {
                let key = (c.source.clone(), c.entry_index);
                if selected.contains(&key) {
                    let group_id = groups.get(&key).cloned().unwrap_or_default();
                    candidates.push((c, group_id));
                }
            }
            easyrdp_core::batch_import::ParsedEntry::Fail(f) => {
                // 提交时才解析失败的条目（预览后文件被改/删）：计入失败明细
                if selected.contains(&(f.source.clone(), f.entry_index)) {
                    failed += 1;
                    failures.push(format!("{}[{}]: {}", f.source, f.entry_index, f.reason));
                }
            }
        }
    }
    let mut report = store.merge_batch(candidates, strategy);
    report.failed += failed;
    report.failures.extend(failures);
    Ok(BatchImportReport {
        added: report.added,
        skipped: report.skipped,
        overwritten: report.overwritten,
        failed: report.failed,
        failures: report.failures,
    })
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // setup 失败会把错误写进 exe 同目录 startup-error.txt（GUI 模式下
            // panic 对话框可能被吞，落盘文件便于用户截图反馈）。
            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                let dir = app.path().app_data_dir()?;
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                // 禁用现代指针输入（2026-11 输入无效排查）：Win10/11 触摸/
                // 精密触摸板场景下系统发 WM_POINTER* 而非旧版鼠标消息——
                // 原生远端窗口 WndProc 只处理旧版消息。进程级一次性调用。
                #[cfg(windows)]
                unsafe {
                    let rc = windows_sys::Win32::UI::Input::Pointer::EnableMouseInPointer(0);
                    let _ = rc;
                }
                let store = ConnectionStore::load(dir.join("connections.json"));
                app.manage(store);
                app.manage(session::ActiveSessions::default());
                // 在线更新（R-20261116-005）：启动自愈——清理旧版 .bak /
                // 完成上次被占用的延迟替换（updater.rs 替换策略兜底）
                updater::self_heal();
                // R-20261116-014：延迟替换的新版在本进程启动时完成安装 →
                // 启动时后台静默刷新过期快捷方式（autoRefreshShortcuts 关则跳过）
                shortcuts::refresh_shortcuts_after_update(app.handle());
                // R-20261118-001 匿名启动遥测：后台线程异步上报一次（失败静默，
                // 不阻塞启动；携带官网下载票据 ticket.id 以建立归因关联）
                telemetry::boot(app.handle().clone());
                Ok(())
            })();
            if let Err(e) = result {
                let exe_dir = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let _ = std::fs::write(exe_dir.join("startup-error.txt"), format!("setup failed: {e}"));
                return Err(e.into());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            save_connection,
            list_connections,
            list_groups,
            save_groups,
            update_connection_meta,
            delete_connection,
            import_rdp,
            export_connection,
            export_connections,
            import_connections,
            import_preview,
            import_commit,
            test_connection,
            session::session_start,
            feedback::submit,
            session::session_input,
            session::session_clipboard_set,
            session::session_stop,
            session::send_win_key,
            session::sync_keyboard_leds,
            session::session_pull_frame,
            session::session_pull_cursor,
            session::frontend_log,
            session::session_active_ids,
            settings::load_settings,
            settings::save_settings,
            // ---- R-20261116-011 主题 / R-20261116-013 快捷方式（dev-ui-items）----
            settings::set_native_theme,
            shortcuts::create_shortcut,
            shortcuts::check_shortcut,
            // ---- R-20261116-014 手动刷新快捷方式 ----
            shortcuts::refresh_shortcuts,
            updater::check_update,
            updater::download_and_install
        ])
        .plugin(tauri_plugin_dialog::init())
        .run(tauri::generate_context!())
        .expect("EasyRDP 启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn conn(id: &str, pw: &str) -> SavedConnection {
        let mut profile = SessionProfile::default();
        // Schema 校验（R-20261118-009）要求 host 非空，夹具补齐
        profile.host = "10.0.0.1".into();
        profile.name = format!("conn-{id}");
        SavedConnection {
            id: id.into(),
            profile,
            password: pw.into(),
            group: String::new(),
            favorite: false,
            fav_order: 0,
            group_id: String::new(),
        }
    }

    #[test]
    fn empty_password_keeps_original() {
        let mut existing = conn("c1", "old-secret");
        ConnectionStore::merge_password(&mut existing, String::new());
        assert_eq!(existing.password, "old-secret");
    }

    #[test]
    fn nonempty_password_overwrites() {
        let mut existing = conn("c1", "old-secret");
        ConnectionStore::merge_password(&mut existing, "new-pass".into());
        assert_eq!(existing.password, "new-pass");
    }

    #[test]
    fn connection_view_never_has_password_and_reports_not_ready() {
        let c = conn("c1", "secret");
        let v = ConnectionView::from(&c);
        // ConnectionView 根本没有 password 字段；序列化后也不得出现
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("secret"));
        assert!(!json.contains("password"));
        // native 引擎未编译 => false（native 构建下为 true，跳过该断言）
        if !easyrdp_rdp::native_ready() {
            assert!(!v.rdp_ready);
        }
    }

    /// 持久化文件中的密码必须是加密格式（Windows: dpapi:base64），且不含明文。
    /// 临时文件放在 CARGO_MANIFEST_DIR 内（沙箱限制子进程写 %TEMP%）。
    #[test]
    #[cfg(windows)]
    fn persisted_password_is_dpapi_encrypted() {
        use base64::Engine;
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-data"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("conn-store-{}.json", std::process::id()));
        let store = ConnectionStore::load(path.clone());

        let mut profile = SessionProfile::default();
        profile.host = "127.0.0.1".into();
        profile.name = "加密存储测试".into();
        let stored = easyrdp_core::secret::protect("s3cret-机密").unwrap();
        let view = store.upsert(SavedConnection {
            id: String::new(),
            profile,
            password: stored.clone(),
            group: String::new(),
            favorite: false,
            fav_order: 0,
            group_id: String::new(),
        });
        assert!(view.is_ok());

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("dpapi:"), "持久化文件应含 dpapi: 前缀");
        assert!(!raw.contains("s3cret-机密"), "持久化文件不得含明文密码");

        // 从文件读回再解密，应得到原明文
        let back: Vec<SavedConnection> = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.len(), 1);
        let plain = easyrdp_core::secret::unprotect(&back[0].password).unwrap();
        assert_eq!(plain, "s3cret-机密");

        // base64 编码合法（STANDARD 引擎可解码）
        let b64 = stored.strip_prefix("dpapi:").unwrap();
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();

        let _ = std::fs::remove_file(&path);
    }

    /// 导入合并：id 冲突跳过、明文密码转加密格式；旧版 JSON（无 group 字段）兼容。
    /// 临时文件放在 CARGO_MANIFEST_DIR 内（沙箱限制子进程写 %TEMP%）。
    #[test]
    #[cfg(windows)]
    fn import_connections_merges_skips_conflicts_and_protects_passwords() {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-data"));
        std::fs::create_dir_all(&dir).unwrap();
        let in_path = dir.join(format!("import-in-{}.json", std::process::id()));
        let store_path = dir.join(format!("import-store-{}.json", std::process::id()));

        let mut existing = conn("conn-1", "keep-me");
        existing.group = "公司".into();
        let incoming = vec![
            existing,
            conn("conn-2", "plain-new"), // 新 id + 明文密码 -> 导入并加密
            SavedConnection {            // 旧版导出兼容：无 group 字段也能反序列化
                id: "conn-3".into(),
                profile: {
                    let mut p = SessionProfile::default();
                    p.host = "10.0.0.3".into();
                    p
                },
                password: String::new(),
                group: String::new(),
                favorite: false,
                fav_order: 0,
                group_id: String::new(),
            },
            conn("conn-1", "dup"), // id 冲突 -> 跳过
        ];
        // 用 serde 序列化时先去掉一条的 group 字段以模拟旧版格式
        let mut v = serde_json::to_value(&incoming).unwrap();
        v[2].as_object_mut().unwrap().remove("group");
        std::fs::write(&in_path, serde_json::to_vec(&v).unwrap()).unwrap();

        let store = ConnectionStore::load(store_path.clone());
        // 预置一条 conn-1 制造冲突
        let _ = store.upsert(SavedConnection {
            id: "conn-1".into(),
            profile: SessionProfile::default(),
            password: easyrdp_core::secret::protect("original").unwrap(),
            group: String::new(),
            favorite: false,
            fav_order: 0,
            group_id: String::new(),
        });

        let (added, skipped) = store.import_from_file(in_path.to_string_lossy().as_ref()).unwrap();
        // 两条 conn-1（首条与末条）都与预置连接冲突 -> 跳过 2；conn-2/conn-3 新增
        assert_eq!((added, skipped), (2, 2));

        let items = store.items.lock().unwrap();
        assert_eq!(items.len(), 3);
        // 冲突条目保持原密码，未被覆盖
        let c1 = items.iter().find(|x| x.id == "conn-1").unwrap();
        assert_eq!(easyrdp_core::secret::unprotect(&c1.password).unwrap(), "original");
        // 明文密码已转加密格式且可解回
        let c2 = items.iter().find(|x| x.id == "conn-2").unwrap();
        assert!(easyrdp_core::secret::is_protected_format(&c2.password));
        assert_eq!(easyrdp_core::secret::unprotect(&c2.password).unwrap(), "plain-new");
        let _ = std::fs::remove_file(&in_path);
        let _ = std::fs::remove_file(&store_path);
    }

    /// R-20261118-009：导入含坏条目的 JSON——硬错误条目被拒绝（逐条可读错误
    /// 带 JSON Pointer），其余条目正常导入（不全量失败）；未知字段仅警告放行。
    #[test]
    #[cfg(windows)]
    fn import_rejects_bad_entries_but_imports_good_ones() {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-data"));
        std::fs::create_dir_all(&dir).unwrap();
        let in_path = dir.join(format!("import-schema-{}.json", std::process::id()));
        let store_path = dir.join(format!("import-schema-store-{}.json", std::process::id()));

        let good = json!({
            "id": "good-1",
            "profile": { "name": "ok", "host": "10.0.0.9", "port": 3389 },
            "password": ""
        });
        let bad_missing_host = json!({
            "id": "bad-1",
            "profile": { "name": "no-host", "port": 3389 },
            "password": ""
        });
        let bad_port_string = json!({
            "id": "bad-2",
            "profile": { "name": "port-str", "host": "10.0.0.9", "port": "3389" },
            "password": ""
        });
        let warn_unknown = json!({
            "id": "warn-1",
            "profile": { "name": "warn", "host": "10.0.0.9", "legacyExtra": true },
            "password": ""
        });
        std::fs::write(
            &in_path,
            serde_json::to_vec(&json!([good, bad_missing_host, bad_port_string, warn_unknown])).unwrap(),
        )
        .unwrap();

        let store = ConnectionStore::load(store_path.clone());
        let result = store.import_from_file(in_path.to_string_lossy().as_ref());
        // 坏条目被拒：返回 Err 且错误逐条带 JSON Pointer
        let err = result.unwrap_err();
        assert!(err.contains("#/host") || err.contains("缺少必填字段: host"), "{err}");
        assert!(err.contains("#/port") && err.contains("类型错误"), "{err}");
        assert!(err.contains("bad-1") || err.contains("no-host"));
        assert!(err.contains("第 2 条") && err.contains("第 3 条"), "{err}");

        // 好条目（含未知字段警告条目）已入库
        let items = store.items.lock().unwrap();
        assert!(items.iter().any(|x| x.id == "good-1"));
        assert!(items.iter().any(|x| x.id == "warn-1"), "未知字段警告不应阻塞导入");
        assert!(!items.iter().any(|x| x.id == "bad-1" || x.id == "bad-2"));
        drop(items);
        let _ = std::fs::remove_file(&in_path);
        let _ = std::fs::remove_file(&store_path);
    }

    /// 导出文件是完整 connections.json 副本（条数一致、不含明文密码）。
    #[test]
    #[cfg(windows)]
    fn export_connections_writes_full_store_copy() {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-data"));
        std::fs::create_dir_all(&dir).unwrap();
        let out_path = dir.join(format!("export-out-{}.json", std::process::id()));
        let store = ConnectionStore::load(dir.join(format!("export-src-{}.json", std::process::id())));
        let mut p = SessionProfile::default();
        p.host = "10.1.1.1".into();
        p.name = "导出测试".into();
        let _ = store.upsert(SavedConnection {
            id: "conn-x".into(),
            profile: p,
            password: easyrdp_core::secret::protect("pw-导出").unwrap(),
            group: "客户A".into(),
            favorite: false,
            fav_order: 0,
            group_id: String::new(),
        });
        let n = store.export_to_file(out_path.to_string_lossy().as_ref()).unwrap();
        assert_eq!(n, 1);
        let raw = std::fs::read_to_string(&out_path).unwrap();
        let back: Vec<SavedConnection> = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].group, "客户A");
        assert!(!raw.contains("pw-导出"));
        let _ = std::fs::remove_file(&out_path);
    }

    // -----------------------------------------------------------------
    // 发布门禁：网关凭据 DPAPI 加密存储
    // -----------------------------------------------------------------

    /// 存储层全链路：upsert（密文落盘）→ connections.json 无明文 →
    /// find() 解密回明文（session_start 运行时路径）。
    #[test]
    #[cfg(windows)]
    fn gateway_credentials_persist_encrypted_and_find_decrypts() {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-data"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("gw-store-{}.json", std::process::id()));
        let store = ConnectionStore::load(path.clone());

        let mut profile = SessionProfile::default();
        profile.host = "10.0.0.5".into();
        profile.gateway = Some(GatewayConfig {
            hostname: "gw.example.com".into(),
            auth: GatewayAuth::Token,
            resource_id: "rdp-win01".into(),
            token: Some("plain-jwt-secret".into()),
            username: Some("zhangsan".into()),
            password: Some("plain-gw-pw".into()),
            ..GatewayConfig::default()
        });
        // 模拟 save_connection 落盘前保护（与命令内同一调用点）
        profile.gateway.as_mut().unwrap().protect_credentials().unwrap();
        store
            .upsert(SavedConnection { id: "gw-1".into(), profile, password: String::new(), group: String::new(), favorite: false, fav_order: 0, group_id: String::new() })
            .unwrap();

        // 1) 持久化文件不得含明文凭据
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("plain-jwt-secret"), "落盘 JSON 不得含明文 token");
        assert!(!raw.contains("plain-gw-pw"), "落盘 JSON 不得含明文密码");
        assert!(raw.contains("dpapi:"), "网关凭据应为 dpapi: 密文形态");

        // 2) find()（session_start 路径）解密回明文
        let (p, _) = store.find("gw-1").unwrap();
        let gw = p.gateway.unwrap();
        assert_eq!(gw.token.as_deref(), Some("plain-jwt-secret"));
        assert_eq!(gw.username.as_deref(), Some("zhangsan"));
        assert_eq!(gw.password.as_deref(), Some("plain-gw-pw"));

        // 3) items 内部（存储态）保持密文
        let items = store.items.lock().unwrap();
        let stored = items.iter().find(|x| x.id == "gw-1").unwrap();
        let st = stored.profile.gateway.as_ref().unwrap();
        assert_ne!(st.token.as_deref(), Some("plain-jwt-secret"));
        assert_eq!(easyrdp_core::secret::unprotect(st.token.as_deref().unwrap()).unwrap(), "plain-jwt-secret");

        // 4) 回显视图（list 路径）不回传 token/password 明文
        let view = ConnectionView::from(stored);
        let vjson = serde_json::to_string(&view).unwrap();
        assert!(!vjson.contains("plain-jwt-secret"));
        assert!(!vjson.contains("plain-gw-pw"));
        let gwv = view.gateway.unwrap();
        assert!(gwv.has_token);
        assert_eq!(gwv.username, "zhangsan"); // username 解密回显（沿用 UX）

        let _ = std::fs::remove_file(&path);
    }

    /// 旧数据迁移：明文凭据的旧 connections.json 读取成功，重新保存后自动加密。
    #[test]
    #[cfg(windows)]
    fn gateway_legacy_plaintext_store_migrates_on_resave() {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-data"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("gw-legacy-{}.json", std::process::id()));
        // 直接写入一份明文凭据的旧版存储
        let mut profile = SessionProfile::default();
        profile.host = "10.0.0.5".into();
        profile.gateway = Some(GatewayConfig {
            hostname: "gw.example.com".into(),
            auth: GatewayAuth::Token,
            token: Some("legacy-plain-jwt".into()),
            ..GatewayConfig::default()
        });
        let legacy = vec![SavedConnection { id: "gw-old".into(), profile, password: String::new(), group: String::new(), favorite: false, fav_order: 0, group_id: String::new() }];
        std::fs::write(&path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        // 旧文件加载（不迁移磁盘）→ find 可读
        let store = ConnectionStore::load(path.clone());
        let (p, _) = store.find("gw-old").unwrap();
        assert_eq!(p.gateway.as_ref().unwrap().token.as_deref(), Some("legacy-plain-jwt"));

        // 重新保存（protect → upsert）→ 磁盘变为密文
        let (mut p, _) = store.find("gw-old").unwrap();
        p.gateway.as_mut().unwrap().protect_credentials().unwrap();
        store.upsert(SavedConnection { id: "gw-old".into(), profile: p, password: String::new(), group: String::new(), favorite: false, fav_order: 0, group_id: String::new() }).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("legacy-plain-jwt"), "重存后明文必须消失（迁移完成）");
        assert!(raw.contains("dpapi:"));

        let _ = std::fs::remove_file(&path);
    }

    // -----------------------------------------------------------------
    // F1-V1 收藏与分组增强（R-20261118-007）：元数据持久化 + 分组树校验
    // -----------------------------------------------------------------

    /// update_meta：收藏/排序/分组归属部分更新，None 字段保持原值并落盘。
    #[test]
    fn update_meta_sets_favorite_order_and_group() {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-data"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("meta-store-{}.json", std::process::id()));
        let store = ConnectionStore::load(path.clone());
        let c = conn("m-1", "");
        store.upsert(c).unwrap();

        let v = store
            .update_meta("m-1", Some(true), Some(3), Some(5), Some("grp-a".into()))
            .unwrap();
        assert!(v.favorite && v.fav_order == 3 && v.order == 5 && v.group_id == "grp-a");

        // None = 保持原值
        let v2 = store.update_meta("m-1", None, Some(1), None, None).unwrap();
        assert!(v2.favorite, "favorite 保持");
        assert_eq!(v2.fav_order, 1);
        assert_eq!(v2.order, 5);
        assert_eq!(v2.group_id, "grp-a");

        // 落盘后重读仍在（serde default 之外的显式字段）
        let raw = std::fs::read_to_string(&path).unwrap();
        let back: Vec<SavedConnection> = serde_json::from_str(&raw).unwrap();
        assert!(back[0].favorite && back[0].fav_order == 1 && back[0].group_id == "grp-a");
        assert_eq!(back[0].profile.order, 5);
        let _ = std::fs::remove_file(&path);
    }

    /// replace_groups：合法树落盘 groups.json；成环树拒绝且不改变内存态。
    #[test]
    fn replace_groups_validates_and_persists() {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-data"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("groups-store-{}.json", std::process::id()));
        let store = ConnectionStore::load(path.clone());

        let ok = vec![
            GroupNode { id: "a".into(), name: "公司".into(), color: Some("#EF4444".into()), ..GroupNode::default() },
            GroupNode { id: "b".into(), name: "研发".into(), parent: Some("a".into()), order: 1, ..GroupNode::default() },
        ];
        store.replace_groups(&ok).unwrap();
        assert_eq!(store.groups_snapshot().len(), 2);
        let raw = std::fs::read_to_string(path.with_file_name("groups.json")).unwrap();
        assert!(raw.contains("研发"));

        // 成环树 → 拒绝，内存态保持旧树
        let cycle = vec![
            GroupNode { id: "x".into(), name: "X".into(), parent: Some("y".into()), ..GroupNode::default() },
            GroupNode { id: "y".into(), name: "Y".into(), parent: Some("x".into()), ..GroupNode::default() },
        ];
        assert!(store.replace_groups(&cycle).is_err());
        assert_eq!(store.groups_snapshot().len(), 2);
        let _ = std::fs::remove_file(path.with_file_name("groups.json"));
        let _ = std::fs::remove_file(&path);
    }

    /// 旧 connections.json 兼容：无 favorite/fav_order/group_id 字段读取成功，
    /// 编辑保存（元数据全零值）继承原收藏标记不被清掉。
    #[test]
    fn old_json_compat_and_upsert_inherits_meta() {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-data"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("compat-store-{}.json", std::process::id()));
        let legacy = r#"[{
            "id": "old-1",
            "profile": { "name": "旧连接", "host": "10.0.0.1", "order": 2 },
            "password": "",
            "group": "客户A",
            "favorite": true,
            "fav_order": 4
        }]"#;
        std::fs::write(&path, legacy).unwrap();
        let store = ConnectionStore::load(path.clone());
        let items = store.items.lock().unwrap();
        assert!(items[0].favorite && items[0].fav_order == 4);
        assert_eq!(items[0].profile.order, 2);
        assert!(items[0].group_id.is_empty());
        drop(items);

        // 编辑保存：不带任何元数据（全零值）→ 继承 favorite/fav_order/order
        let mut p = SessionProfile::default();
        p.host = "10.0.0.1".into();
        p.name = "旧连接".into();
        let _ = store.upsert(SavedConnection {
            id: "old-1".into(),
            profile: p,
            password: String::new(),
            group: "客户A".into(),
            favorite: false,
            fav_order: 0,
            group_id: String::new(),
        }).unwrap();
        let items = store.items.lock().unwrap();
        assert!(items[0].favorite, "编辑保存后收藏标记应继承");
        assert_eq!(items[0].fav_order, 4);
        assert_eq!(items[0].profile.order, 2);
        drop(items);
        let _ = std::fs::remove_file(&path);
    }

    /// ConnectionView 回显：group_id 解析出 group_color；未入树为 None。
    #[test]
    fn connection_view_resolves_group_color() {
        let groups = vec![
            GroupNode { id: "a".into(), name: "公司".into(), color: Some("#22c55e".into()), ..GroupNode::default() },
        ];
        let mut c = conn("v-1", "");
        c.group_id = "a".into();
        let v = ConnectionView::build(&c, &groups);
        assert_eq!(v.group_color.as_deref(), Some("#22c55e"));
        let mut c2 = conn("v-2", "");
        c2.group_id = "ghost".into();
        let v2 = ConnectionView::build(&c2, &groups);
        assert_eq!(v2.group_color, None);
    }

    // -----------------------------------------------------------------
    // R-20261118-008 批量导入与去重
    // -----------------------------------------------------------------

    use easyrdp_core::batch_import::{DedupStrategy, ImportCandidate, PlanAction};

    fn cand(host: &str, port: u16, name: &str, pw: Option<&str>) -> ImportCandidate {
        ImportCandidate {
            source: "f.rdp".into(),
            entry_index: 0,
            profile: SessionProfile {
                name: name.into(),
                host: host.into(),
                port,
                ..SessionProfile::default()
            },
            plaintext_password: pw.map(String::from),
        }
    }

    fn batch_store(tag: &str) -> ConnectionStore {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-data"));
        std::fs::create_dir_all(&dir).unwrap();
        ConnectionStore::load(dir.join(format!("batch-{tag}-{}.json", std::process::id())))
    }

    /// 策略① 跳过重复：同键（大小写不敏感）候选被跳过，不新增。
    #[test]
    fn batch_merge_skip_strategy() {
        let store = batch_store("skip");
        let mut p = SessionProfile::default();
        p.host = "RDP.Corp.COM".into();
        p.port = 3389;
        p.name = "旧".into();
        store.upsert(SavedConnection {
            id: "old-1".into(),
            profile: p,
            password: String::new(),
            group: "公司".into(),
            favorite: true,
            fav_order: 1,
            group_id: String::new(),
        }).unwrap();
        let r = store.merge_batch(
            vec![(cand("rdp.corp.com", 3389, "新", None), String::new())],
            DedupStrategy::Skip,
        );
        assert_eq!((r.added, r.skipped, r.overwritten), (0, 1, 0));
        let items = store.items.lock().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].group, "公司", "跳过不触碰现有连接");
        drop(items);
        let _ = std::fs::remove_file(store.path.clone());
    }

    /// 策略② 覆盖重复：新 profile 替换旧数据，保留分组/收藏/id；
    /// 候选带明文密码时经 DPAPI 加密存储。
    #[test]
    #[cfg(windows)]
    fn batch_merge_overwrite_strategy_preserves_meta_and_encrypts_password() {
        let store = batch_store("overwrite");
        let mut p = SessionProfile::default();
        p.host = "10.0.0.9".into();
        p.name = "旧".into();
        store.upsert(SavedConnection {
            id: "old-1".into(),
            profile: p,
            password: String::new(),
            group: "公司".into(),
            favorite: true,
            fav_order: 7,
            group_id: "grp-a".into(),
        }).unwrap();
        let r = store.merge_batch(
            vec![(cand("10.0.0.9", 3389, "新名", Some("plain-import-pw")), "ignored".into())],
            DedupStrategy::Overwrite,
        );
        assert_eq!((r.added, r.skipped, r.overwritten), (0, 0, 1));
        let items = store.items.lock().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "old-1", "覆盖保留 id");
        assert_eq!(items[0].profile.name, "旧", "覆盖保留原显示名");
        assert_eq!(items[0].group, "公司", "覆盖保留分组");
        assert!(items[0].favorite && items[0].fav_order == 7, "覆盖保留收藏");
        assert_eq!(items[0].group_id, "grp-a");
        assert!(easyrdp_core::secret::is_protected_format(&items[0].password), "导入明文密码须 DPAPI 加密");
        assert_eq!(easyrdp_core::secret::unprotect(&items[0].password).unwrap(), "plain-import-pw");
        drop(items);
        let _ = std::fs::remove_file(store.path.clone());
    }

    /// 策略③ 两者保留：重名自动加 "(2)" 后缀，新连接使用向导指定分组。
    #[test]
    fn batch_merge_keep_both_strategy() {
        let store = batch_store("keepboth");
        let mut p = SessionProfile::default();
        p.host = "h1".into();
        p.name = "同名".into();
        store.upsert(SavedConnection {
            id: "old-1".into(),
            profile: p,
            password: String::new(),
            group: String::new(),
            favorite: false,
            fav_order: 0,
            group_id: String::new(),
        }).unwrap();
        let r = store.merge_batch(
            vec![(cand("H1", 3389, "同名", None), "grp-imp".into())],
            DedupStrategy::KeepBoth,
        );
        assert_eq!(r.added, 1);
        let items = store.items.lock().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].profile.name, "同名 (2)", "重名自动加后缀");
        assert_eq!(items[1].group_id, "grp-imp", "新连接采用向导指定分组树节点");
        drop(items);
        let _ = std::fs::remove_file(store.path.clone());
    }

    /// 计划动作透传：非重复键恒 Add（三分支一致）。
    #[test]
    fn batch_plan_non_duplicate_is_add() {
        let existing = vec![SessionProfile { host: "a".into(), ..Default::default() }];
        let inc = cand("b", 3390, "x", None);
        for s in [DedupStrategy::Skip, DedupStrategy::Overwrite, DedupStrategy::KeepBoth] {
            assert_eq!(
                easyrdp_core::batch_import::plan_merge(&existing, std::slice::from_ref(&inc), s),
                vec![PlanAction::Add],
                "{s:?}"
            );
        }
    }
}