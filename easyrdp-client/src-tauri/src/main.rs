// EasyRDP Tauri 2 应用壳。
//
// 连接中心：保存/删除/列出连接（持久化到应用数据目录 connections.json）。
// RDP 远程画面（FreeRDP FFI）将在 crates/easyrdp-rdp 实现后接入「连接」按钮。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use easyrdp_core::profile::{Resolution, SessionProfile};
use easyrdp_core::rdp_import;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{Manager, State};

/// 一条已保存的连接（含密码；仅持久化在本地应用数据目录）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedConnection {
    id: String,
    profile: SessionProfile,
    #[serde(default)]
    password: String,
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
}

impl From<&SavedConnection> for ConnectionView {
    fn from(c: &SavedConnection) -> Self {
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
        }
    }
}

/// 连接存储：内存缓存 + JSON 文件持久化。
struct ConnectionStore {
    path: PathBuf,
    items: Mutex<Vec<SavedConnection>>,
}

impl ConnectionStore {
    fn load(path: PathBuf) -> Self {
        let items = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            items: Mutex::new(items),
        }
    }

    fn persist(&self, items: &[SavedConnection]) -> Result<(), String> {
        let data = serde_json::to_string_pretty(items).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, data).map_err(|e| e.to_string())
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
            Some(i) => i,
            None => {
                items.push(c);
                items.len() - 1
            }
        };
        self.persist(&items)?;
        Ok(ConnectionView::from(&items[idx]))
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        let mut items = self.items.lock().unwrap();
        items.retain(|x| x.id != id);
        self.persist(&items)
    }
}

#[derive(Deserialize)]
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
    /// fit_window | fullscreen | custom
    #[serde(default)]
    resolution_kind: String,
    #[serde(default)]
    custom_w: u32,
    #[serde(default)]
    custom_h: u32,
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
    profile.validate().map_err(|errs| errs.join("; "))?;
    store.upsert(SavedConnection {
        id: args.id,
        profile,
        password: args.password,
    })
}

/// 列出全部连接（不含密码）。
#[tauri::command]
fn list_connections(store: State<ConnectionStore>) -> Result<Vec<ConnectionView>, String> {
    Ok(store
        .items
        .lock()
        .unwrap()
        .iter()
        .map(ConnectionView::from)
        .collect())
}

/// 删除一条连接。
#[tauri::command]
fn delete_connection(id: String, store: State<ConnectionStore>) -> Result<(), String> {
    store.delete(&id)
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
    };
    if saved.profile.name.is_empty() {
        saved.profile.name = saved.profile.host.clone();
    }
    store.upsert(saved)
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let store = ConnectionStore::load(dir.join("connections.json"));
            app.manage(store);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            save_connection,
            list_connections,
            delete_connection,
            import_rdp
        ])
        .run(tauri::generate_context!())
        .expect("EasyRDP 启动失败");
}
