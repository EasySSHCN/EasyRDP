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

pub mod session;
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
    #[serde(default)]
    group: String,
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
}

impl From<&SavedConnection> for ConnectionView {
    fn from(c: &SavedConnection) -> Self {
        let (custom_w, custom_h) = match c.profile.resolution {
            Resolution::Custom { w, h } => (w, h),
            _ => (0, 0),
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
        }
    }
}

/// 连接存储：内存缓存 + JSON 文件持久化。
/// （pub：session.rs 的 pub command 签名引用 `State<ConnectionStore>`）
pub struct ConnectionStore {
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

    /// 导出全部连接为 JSON 文件（connections.json 的副本；密码保持本机加密格式）。
    /// 返回导出条数。
    fn export_to_file(&self, path: &str) -> Result<usize, String> {
        let items = self.items.lock().unwrap().clone();
        let data = serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?;
        std::fs::write(path, data).map_err(|e| format!("写入导出文件失败: {e}"))?;
        Ok(items.len())
    }

    /// 从 JSON 文件导入连接（id 冲突跳过；明文密码转本机加密格式）。
    /// 返回（新增条数, 跳过条数）。
    fn import_from_file(&self, path: &str) -> Result<(usize, usize), String> {
        let raw = std::fs::read(path).map_err(|e| format!("读取导入文件失败: {e}"))?;
        let incoming: Vec<SavedConnection> =
            serde_json::from_slice(&raw).map_err(|e| format!("JSON 解析失败: {e}"))?;
        let mut items = self.items.lock().unwrap();
        let mut added = 0usize;
        let mut skipped = 0usize;
        for mut c in incoming {
            if c.id.is_empty() || items.iter().any(|x| x.id == c.id) {
                skipped += 1;
                continue;
            }
            // 明文密码 -> 本机加密格式（已是加密格式则原样保留；失败则丢弃密码）
            if !c.password.is_empty() && !easyrdp_core::secret::is_protected_format(&c.password) {
                c.password = easyrdp_core::secret::protect(&c.password).unwrap_or_default();
            }
            items.push(c);
            added += 1;
        }
        drop(items);
        self.persist(&self.items.lock().unwrap())?;
        Ok((added, skipped))
    }

    /// 按 id 查找连接，返回（profile, 存储态口令）副本（session_start 用）。
    pub fn find(&self, id: &str) -> Option<(SessionProfile, String)> {
        self.items
            .lock()
            .unwrap()
            .iter()
            .find(|x| x.id == id)
            .map(|c| (c.profile.clone(), c.password.clone()))
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
    };
    if saved.profile.name.is_empty() {
        saved.profile.name = saved.profile.host.clone();
    }
    store.upsert(saved)
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
            delete_connection,
            import_rdp,
            export_connection,
            export_connections,
            import_connections,
            test_connection,
            session::session_start,
            session::session_input,
            session::session_clipboard_set,
            session::session_stop,
            session::send_win_key,
            session::sync_keyboard_leds,
            session::session_pull_frame,
            session::session_pull_cursor,
            session::frontend_log
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
            group: String::new(),
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
                profile: SessionProfile::default(),
                password: String::new(),
                group: String::new(),
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
}