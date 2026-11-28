// EasyRDP 设置持久化（R-20261116-009 设置菜单框架）。
//
// 存储：settings.json，与应用数据目录 connections.json 同目录（app_data_dir）。
// 前端调用：invoke("load_settings") / invoke("save_settings", { map })。
// 合并策略：load 时先落 DEFAULT_SETTINGS 再覆盖已存键 —— 缺字段补默认值、
// 用户新键原样保留，向前兼容（后续需求逐项填充设置项时无需迁移）。

use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use tauri::{AppHandle, Manager};

/// app_data_dir 缓存（F-20261117-001 组合键菜单）：原生窗口代码（无 AppHandle）
/// 读取 settings.json 用。首次任一设置命令调用时登记（load/save 都走）。
static APP_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 默认设置键骨架（与前端 ui/settings.js 的 DEFAULT_SETTINGS 保持一致；
/// 后续需求逐项填充 UI 时在此补键即可）。
fn default_settings() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("autoUpdateCheck".into(), Value::Bool(true));
    m.insert("updateFreq".into(), Value::String("daily".into()));
    m.insert("lang".into(), Value::String("zh-CN".into()));
    m.insert("theme".into(), Value::String("dark".into()));
    m.insert("themeAccent".into(), Value::String("a1".into())); // R-20261116-011
    m.insert("autoRefreshShortcuts".into(), Value::Bool(true));
    // F1-V2 R-20261118-013 会话快照预览（默认开；关 = 停止捕获且前端不请求）
    m.insert("autoSnapshotPreview".into(), Value::Bool(true));
    // F6-V2（R-20261118-015）Git 同步（默认关）
    m.insert("gitSyncEnabled".into(), Value::Bool(false));
    m.insert("gitSyncRemote".into(), Value::String(String::new()));
    m.insert("gitSyncBranch".into(), Value::String("main".into()));
    m.insert("gitSyncAuthMode".into(), Value::String("https_token".into()));
    m.insert("gitSyncSshKey".into(), Value::String(String::new()));
    m.insert("gitSyncAutoCommit".into(), Value::String("each_change".into()));
    m
}

/// settings.json 路径（app_data_dir，与 connections.json 同目录；目录不存在则创建）。
fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let _ = APP_DATA_DIR.set(dir.clone());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("settings.json"))
}

/// 自定义组合键（F-20261117-001）：从 settings.json 的 `customCombos` 键读取
/// （数组：[{"name":"任务管理器","keys":"Ctrl+Shift+Esc"}, ...]）。
/// 供原生窗口 SAS 下拉菜单在弹出时读取（无 AppHandle，走 APP_DATA_DIR 缓存；
/// 缓存未登记时返回空表——菜单仍含内置项）。解析失败的条目跳过。
pub fn read_custom_combos() -> Vec<(String, String)> {
    let Some(dir) = APP_DATA_DIR.get() else { return Vec::new() };
    let Ok(bytes) = std::fs::read(dir.join("settings.json")) else { return Vec::new() };
    let Ok(map) = serde_json::from_slice::<Map<String, Value>>(&bytes) else { return Vec::new() };
    let Some(Value::Array(arr)) = map.get("customCombos") else { return Vec::new() };
    let mut out = Vec::new();
    for item in arr {
        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let keys = item.get("keys").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if name.is_empty() || keys.is_empty() { continue; }
        out.push((name, keys));
    }
    out
}

/// git 同步 token 键（F6-V2 R-20261118-015）：settings.json 中恒为 DPAPI 密文。
const SYNC_TOKEN_KEY: &str = "gitSyncToken";

/// settings.json 读取（命令 load_settings 与 Rust 侧流程共用）。
fn load_settings_map(app: &AppHandle) -> Result<Map<String, Value>, String> {
    let path = settings_path(app)?;
    let mut merged = default_settings();
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(stored) = serde_json::from_slice::<Map<String, Value>>(&bytes) {
            for (k, v) in stored {
                merged.insert(k, v);
            }
        }
    }
    Ok(merged)
}

/// F6-V2：git 同步模块读取原始设置表（gitSyncToken 为 DPAPI 密文，勿回传前端）。
pub(crate) fn load_settings_map_public(app: &AppHandle) -> Map<String, Value> {
    load_settings_map(app).unwrap_or_default()
}

/// 读取全部设置：文件缺失/损坏时返回纯默认值；
/// 已存键覆盖默认键，缺字段自动补默认值。
/// F6-V2：gitSyncToken 恒不回传前端（密文也不回传）——置空并附
/// gitSyncTokenSaved 标记供 UI 回显「已保存」。
#[tauri::command]
pub fn load_settings(app: AppHandle) -> Result<Map<String, Value>, String> {
    let mut map = load_settings_map(&app)?;
    let saved = map
        .get(SYNC_TOKEN_KEY)
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    map.insert(SYNC_TOKEN_KEY.into(), Value::String(String::new()));
    map.insert("gitSyncTokenSaved".into(), Value::Bool(saved));
    Ok(map)
}

/// Rust 侧流程读取单个布尔设置（读取失败或缺省视为 true），
/// 如 R-20261116-014 升级后的 autoRefreshShortcuts 开关。
pub fn get_bool_setting(app: &AppHandle, key: &str) -> bool {
    load_settings_map(app)
        .ok()
        .and_then(|m| m.get(key).cloned())
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// 整体保存设置（前端把合并后的完整 map 传回，避免部分写入丢失键）。
/// F6-V2：gitSyncToken 特殊处理——空值保留旧密文；明文 → DPAPI 加密落盘。
#[tauri::command]
pub fn save_settings(app: AppHandle, mut map: HashMap<String, Value>) -> Result<(), String> {
    if let Some(v) = map.get(SYNC_TOKEN_KEY).cloned() {
        match v.as_str() {
            Some("") | None => {
                // 空值：保留旧密文（前端不回传密文）
                if let Ok(stored) = load_settings_map(&app) {
                    if let Some(old) = stored.get(SYNC_TOKEN_KEY).cloned() {
                        map.insert(SYNC_TOKEN_KEY.into(), old);
                    }
                }
            }
            Some(plain) => {
                if !easyrdp_core::secret::is_protected_format(plain) {
                    match easyrdp_core::secret::protect(plain) {
                        Ok(cipher) => {
                            map.insert(SYNC_TOKEN_KEY.into(), Value::String(cipher));
                        }
                        Err(e) => return Err(format!("git 同步 token 加密失败: {e}")),
                    }
                }
                // 已是密文格式（理论上前端不会回传）原样落盘
            }
        }
    }
    let path = settings_path(&app)?;
    let data = serde_json::to_string_pretty(&map).map_err(|e| e.to_string())?;
    std::fs::write(&path, data).map_err(|e| e.to_string())
}

// ---------------- 窗口几何记忆（R-20261119-002，v0.4.0） ----------------
//
// settings.json 的 `winGeom` 键：{"x":i32,"y":i32,"w":u32,"h":u32,"max":bool}
// —— 全部物理像素。max=true 表示上次关闭时窗口处于最大化，此时 x/y/w/h
// 保留的是「最大化之前」的还原几何（关闭时由 main.rs 采样旧值）。

/// 窗口几何结构（物理像素）。
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub max: bool,
}

/// 读取窗口几何；缺键 / 解析失败返回 None（走 tauri.conf.json 默认几何）。
pub fn load_window_geometry(app: &AppHandle) -> Option<WindowGeometry> {
    let dir = match APP_DATA_DIR.get() {
        Some(d) => d.clone(),
        // 启动 setup 阶段前端尚未调用 load_settings，缓存未登记 ——
        // 与 save_window_geometry 对齐用 app.path() 回退（fix：否则
        // 启动恢复窗口几何永远静默失效）。
        None => match app.path().app_data_dir() {
            Ok(d) => {
                let _ = APP_DATA_DIR.set(d.clone());
                d
            }
            Err(_) => return None,
        },
    };
    let bytes = std::fs::read(dir.join("settings.json")).ok()?;
    let map = serde_json::from_slice::<Map<String, Value>>(&bytes).ok()?;
    let g = map.get("winGeom")?;
    let obj = g.as_object()?;
    let num_i = |k: &str| obj.get(k).and_then(|v| v.as_i64());
    let num_u = |k: &str| obj.get(k).and_then(|v| v.as_u64());
    let x = num_i("x")? as i32;
    let y = num_i("y")? as i32;
    let w = num_u("w")? as u32;
    let h = num_u("h")? as u32;
    let max = obj.get("max").and_then(|v| v.as_bool()).unwrap_or(false);
    if w == 0 || h == 0 {
        return None;
    }
    Some(WindowGeometry { x, y, w, h, max })
}

/// 保存窗口几何（读改写 settings.json，保留其余键；失败静默——几何记忆
/// 属体验增强，不影响主流程）。
pub fn save_window_geometry(app: &AppHandle, g: WindowGeometry) {
    let dir = match APP_DATA_DIR.get() {
        Some(d) => d.clone(),
        // 首次启动 APP_DATA_DIR 尚未登记（load_settings 未被调用过）时登记
        None => match app.path().app_data_dir() {
            Ok(d) => {
                let _ = APP_DATA_DIR.set(d.clone());
                d
            }
            Err(_) => return,
        },
    };
    let path = dir.join("settings.json");
    let mut map = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Map<String, Value>>(&b).ok())
        .unwrap_or_default();
    let mut obj = Map::new();
    obj.insert("x".into(), Value::from(g.x));
    obj.insert("y".into(), Value::from(g.y));
    obj.insert("w".into(), Value::from(g.w));
    obj.insert("h".into(), Value::from(g.h));
    obj.insert("max".into(), Value::from(g.max));
    map.insert("winGeom".into(), Value::Object(obj));
    if let Ok(data) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(&path, data);
    }
}

/// 任务栏固定引导状态（R-20261119-005，v0.4.0）：settings.json 的
/// `taskbarPinPrompt` 键：{"dismissed":bool}。dismissed=true 永久静默。
pub fn load_taskbar_pin_dismissed() -> bool {
    let Some(dir) = APP_DATA_DIR.get() else { return false };
    let Ok(bytes) = std::fs::read(dir.join("settings.json")) else { return false };
    let Ok(map) = serde_json::from_slice::<Map<String, Value>>(&bytes) else { return false };
    map.get("taskbarPinPrompt")
        .and_then(|v| v.get("dismissed"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// 持久化「不再提示」（读改写，保留其余键）。
pub fn save_taskbar_pin_dismissed(app: &AppHandle) {
    let dir = match APP_DATA_DIR.get() {
        Some(d) => d.clone(),
        None => match app.path().app_data_dir() {
            Ok(d) => {
                let _ = APP_DATA_DIR.set(d.clone());
                d
            }
            Err(_) => return,
        },
    };
    let path = dir.join("settings.json");
    let mut map = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Map<String, Value>>(&b).ok())
        .unwrap_or_default();
    let mut obj = Map::new();
    obj.insert("dismissed".into(), Value::Bool(true));
    map.insert("taskbarPinPrompt".into(), Value::Object(obj));
    if let Ok(data) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(&path, data);
    }
}

/// 原生 RDP 窗口标题栏深浅切换（R-20261116-011）：前端主题变化时调用。
/// Windows 走 native_view::set_native_theme_dark（置开关 + InvalidateRect 重绘）；
/// 非 Windows 平台为 no-op（native_view 模块本就仅 Windows 编译）。
#[tauri::command]
pub fn set_native_theme(dark: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        crate::native_view::set_native_theme_dark(dark);
    }
    #[cfg(not(windows))]
    {
        let _ = dark;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认值骨架齐全，且 load 合并策略：缺字段补默认、新键保留。
    #[test]
    fn defaults_cover_required_keys_and_merge_keeps_new_keys() {
        let defaults = default_settings();
        for k in ["autoUpdateCheck", "updateFreq", "lang", "theme", "autoRefreshShortcuts", "autoSnapshotPreview"] {
            assert!(defaults.contains_key(k), "缺默认键: {k}");
        }
        // 模拟「已存文件只有 lang + 用户新键」：合并后 lang 用已存值，默认键补齐，新键保留
        let mut stored = Map::new();
        stored.insert("lang".into(), Value::String("en".into()));
        stored.insert("customFutureKey".into(), Value::Bool(false));
        let mut merged = default_settings();
        for (k, v) in stored {
            merged.insert(k, v);
        }
        assert_eq!(merged.get("lang"), Some(&Value::String("en".into())));
        assert_eq!(merged.get("autoUpdateCheck"), Some(&Value::Bool(true)));
        assert_eq!(merged.get("customFutureKey"), Some(&Value::Bool(false)));
    }
}
