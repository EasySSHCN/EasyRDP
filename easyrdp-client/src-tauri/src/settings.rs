// EasyRDP 设置持久化（R-20261116-009 设置菜单框架）。
//
// 存储：settings.json，与应用数据目录 connections.json 同目录（app_data_dir）。
// 前端调用：invoke("load_settings") / invoke("save_settings", { map })。
// 合并策略：load 时先落 DEFAULT_SETTINGS 再覆盖已存键 —— 缺字段补默认值、
// 用户新键原样保留，向前兼容（后续需求逐项填充设置项时无需迁移）。

use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

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
    m
}

/// settings.json 路径（app_data_dir，与 connections.json 同目录；目录不存在则创建）。
fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("settings.json"))
}

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

/// 读取全部设置：文件缺失/损坏时返回纯默认值；
/// 已存键覆盖默认键，缺字段自动补默认值。
#[tauri::command]
pub fn load_settings(app: AppHandle) -> Result<Map<String, Value>, String> {
    load_settings_map(&app)
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
#[tauri::command]
pub fn save_settings(app: AppHandle, map: HashMap<String, Value>) -> Result<(), String> {
    let path = settings_path(&app)?;
    let data = serde_json::to_string_pretty(&map).map_err(|e| e.to_string())?;
    std::fs::write(&path, data).map_err(|e| e.to_string())
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
        for k in ["autoUpdateCheck", "updateFreq", "lang", "theme", "autoRefreshShortcuts"] {
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
