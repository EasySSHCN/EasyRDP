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
    /// easyrdp-rdp native 引擎（FreeRDP FFI）就绪后为 true；
    /// 当前 stub 实现恒为 false。
    rdp_ready: bool,
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
            rdp_ready: easyrdp_rdp::native_ready(),
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
            Some(i) => {
                // 密码留空 => 保留原密码；否则用新值覆盖
                Self::merge_password(&mut items[i], c.password.clone());
                c.password = items[i].password.clone();
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
            import_rdp,
            test_connection
        ])
        .plugin(tauri_plugin_dialog::init())
        .run(tauri::generate_context!())
        .expect("EasyRDP 启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(id: &str, pw: &str) -> SavedConnection {
        SavedConnection {
            id: id.into(),
            profile: SessionProfile::default(),
            password: pw.into(),
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
        // native 引擎未编译 => false
        assert!(!v.rdp_ready);
    }
}
