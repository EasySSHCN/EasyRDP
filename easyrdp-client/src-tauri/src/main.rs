// EasyRDP Tauri 2 应用壳（MVP 骨架）。
//
// 目前提供两个示例 command：
// - import_rdp(path): 导入 mstsc 导出的 .rdp 文件
// - list_profiles():  列出已保存的会话配置（MVP 阶段从内存/JSON 文件读取）
//
// RDP 连接（FreeRDP FFI）将在 crates/easyrdp-rdp 实现后接入。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use easyrdp_core::rdp_import;
use easyrdp_core::SessionProfile;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Manager, State};

/// MVP 阶段的进程内 Profile 存储（正式版将持久化到应用数据目录）。
struct ProfileStore {
    profiles: Mutex<Vec<SessionProfile>>,
}

/// 导入一个 mstsc 导出的 .rdp 文件，返回解析出的 SessionProfile（JSON）。
/// 导入成功后同时加入内存存储。
#[tauri::command]
fn import_rdp(path: String, store: State<ProfileStore>) -> Result<serde_json::Value, String> {
    let profile = rdp_import::import(&PathBuf::from(&path)).map_err(|e| e.to_string())?;
    profile.validate().map_err(|errs| errs.join("; "))?;
    let json = serde_json::to_value(&profile).map_err(|e| e.to_string())?;
    store.profiles.lock().unwrap().push(profile);
    Ok(json)
}

/// 列出当前已导入的会话配置。
#[tauri::command]
fn list_profiles(store: State<ProfileStore>) -> Result<serde_json::Value, String> {
    let profiles = store.profiles.lock().unwrap();
    serde_json::to_value(profiles.as_slice()).map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .manage(ProfileStore {
            profiles: Mutex::new(Vec::new()),
        })
        .invoke_handler(tauri::generate_handler![import_rdp, list_profiles])
        .run(tauri::generate_context!())
        .expect("EasyRDP 启动失败");
}
