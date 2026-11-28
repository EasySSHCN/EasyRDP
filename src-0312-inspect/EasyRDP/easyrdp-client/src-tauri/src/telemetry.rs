//! telemetry.rs — 匿名启动遥测上报（官网访客-设备归因链路 · R-20261118-001）
//!
//! 行为：客户端启动后异步（独立线程）上报一次，任何失败完全静默，
//! 绝不阻塞启动/使用。频控：每台设备每天最多 1 次（telemetry.json 按日去重）。
//!
//! 上报协议（明文 HTTPS POST，服务端 api/stats/client.php action=report）：
//!   { action:"report", anonId, ticket_id?, fields:{} }
//! 字段白名单（除此之外一律不发）：anonId / ticket_id / 软件版本 / 操作系统 /
//! 系统架构 / 系统语言代码 / 客户端类型。不含任何个人身份信息。
//!
//! 匿名 ID 与票据：
//!   anonId     = 首次启动生成 32 位随机 hex，持久化到 app_data_dir/telemetry.json；
//!   ticket_id  = 官网下载票据。安装包构建时把 ticket.id 文件放进 exe 同目录
//!   （注入点设计见 docs/方案-官网访客-设备归因链路.md），启动时若 exe 同目录
//!   存在 ticket.id（32-64 位 hex）则随首报携带，实现「下载→安装→首报」归因；
//!   无票据场景仅统计匿名设备。

use serde_json::json;
use std::path::PathBuf;

const API_URL: &str = "https://easyrdp.cn/api/stats/client.php";
const CLIENT_TYPE: &str = "easyrdp-desktop";

/// 遥测本地状态（app_data_dir/telemetry.json）
#[derive(serde::Serialize, serde::Deserialize)]
struct TelemetryState {
    #[serde(default)]
    anon_id: String,
    #[serde(default)]
    ticket_id: String,
    /// 上次成功上报日期（本地 YYYY-MM-DD），同日跳过（每天最多 1 次）
    #[serde(default)]
    last_report: String,
}

fn state_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("telemetry.json"))
}

/// OS CSPRNG 随机 hex（复用 feedback.rs rand_nonce 的后端策略，不引入自研 PRNG）
fn rand_hex(n_bytes: usize) -> String {
    let mut buf = vec![0u8; n_bytes];
    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            if f.read_exact(&mut buf).is_ok() {
                return buf.iter().map(|b| format!("{b:02x}")).collect();
            }
        }
    }
    #[cfg(windows)]
    {
        #[link(name = "bcrypt")]
        unsafe extern "system" {
            fn BCryptGenRandom(halgorithm: usize, pbbuffer: *mut u8, cbbuffer: u32, dwflags: u32) -> i32;
        }
        // BCRYPT_USE_SYSTEM_PREFERRED_RNG = 0x00000002
        if unsafe { BCryptGenRandom(0, buf.as_mut_ptr(), buf.len() as u32, 0x0000_0002) } == 0 {
            return buf.iter().map(|b| format!("{b:02x}")).collect();
        }
    }
    // 极端兜底（不应发生）：时间熵混洗
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for (i, b) in buf.iter_mut().enumerate() {
        *b ^= ((t >> (i * 7)) as u8) | ((t >> (i * 3)) as u8).rotate_left(1);
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// ticket.id 合法性：16-64 位小写 hex（与 anonId 同字符集，服务端 st_valid_anon_id 一致）
fn valid_ticket_id(s: &str) -> bool {
    let n = s.len();
    n >= 16 && n <= 64 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// 读取安装包注入的下载票据：exe 同目录 ticket.id（trim 后 16-64 位 hex）
fn read_injected_ticket() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let raw = std::fs::read_to_string(dir.join("ticket.id")).ok()?;
    let id = raw.trim().to_string();
    if valid_ticket_id(&id) {
        Some(id)
    } else {
        None
    }
}

fn today_local() -> String {
    // 本地日期（遥测按日去重，无需时区精确；读本地日历日即可）
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    // 粗略本地日：秒 → 天偏移（+8h 东八区兜底，误差只影响首报去重边界，可接受）
    let days = (now + 8 * 3600) / 86400;
    let ymd = days * 86400;
    // 用 chrono-free 的简单算法从 epoch 秒推导 YYYY-MM-DD（civil_from_days）
    let z = ymd / 86400 + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// 读取系统语言（settings.json 的 lang；缺失回退 zh-CN）——白名单字段，非个人身份信息
fn read_lang(app: &tauri::AppHandle) -> String {
    let Some(path) = state_path(app) else { return "zh-CN".into() };
    let settings = path.with_file_name("settings.json");
    if let Ok(bytes) = std::fs::read(settings) {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Some(lang) = v.get("lang").and_then(|x| x.as_str()) {
                let lang = lang.trim();
                if !lang.is_empty() && lang.len() <= 16 {
                    return lang.to_string();
                }
            }
        }
    }
    "zh-CN".into()
}

/// 构造上报 HTTP agent（对齐 feedback.rs：ureq 2.x 快捷方法只走 rustls，
/// 必须经 AgentBuilder 显式挂 native_tls::TlsConnector（schannel））
fn http_agent() -> ureq::Agent {
    let connector = native_tls::TlsConnector::builder().build().expect("native_tls connector");
    ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(connector))
        .timeout(std::time::Duration::from_secs(15))
        .build()
}

/// 上报一次（阻塞 HTTP；调用方保证在独立线程中执行）。任何失败静默返回。
fn report_once(app: &tauri::AppHandle) {
    let Some(path) = state_path(app) else { return };

    // 读取/初始化状态（损坏/缺失回退新状态）
    let mut state: TelemetryState = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(TelemetryState { anon_id: String::new(), ticket_id: String::new(), last_report: String::new() });
    if !valid_ticket_id(&state.anon_id) {
        state.anon_id = rand_hex(16);
    }
    // 票据优先级：已持久化 > 本次读取安装包注入的 ticket.id
    if state.ticket_id.is_empty() {
        if let Some(tid) = read_injected_ticket() {
            state.ticket_id = tid;
        }
    } else if !valid_ticket_id(&state.ticket_id) {
        state.ticket_id.clear();
        if let Some(tid) = read_injected_ticket() {
            state.ticket_id = tid;
        }
    }

    // 频控：同设备每天最多 1 次
    let today = today_local();
    if state.last_report == today {
        // 状态可能刚补写了 anonId/ticketId，持久化后返回
        if let Ok(b) = serde_json::to_vec(&state) {
            let _ = std::fs::write(&path, b);
        }
        return;
    }

    // 白名单字段（服务端 st_allowed_fields 兼容；地域由服务端按掩码 IP 生成）
    let fields = json!({
        "软件版本": env!("CARGO_PKG_VERSION"),
        "操作系统": std::env::consts::OS,
        "系统架构": std::env::consts::ARCH,
        "系统语言代码": read_lang(app),
        "客户端类型": CLIENT_TYPE,
    });

    let mut body = json!({ "action": "report", "anonId": state.anon_id, "fields": fields });
    if !state.ticket_id.is_empty() {
        body["ticket_id"] = json!(state.ticket_id);
    }

    let ok = (|| -> Option<bool> {
        let agent = http_agent();
        let resp = agent
            .post(API_URL)
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .ok()?;
        let j: serde_json::Value = resp.into_json().ok()?;
        Some(j.get("ok").and_then(|v| v.as_bool()).unwrap_or(false))
    })()
    .unwrap_or(false);

    if ok {
        state.last_report = today;
    }
    // 无论成败都持久化（anonId/ticketId 首次写入；失败下次启动重试，仍受按日频控约束的语义由 last_report 决定）
    if let Ok(b) = serde_json::to_vec(&state) {
        let _ = std::fs::write(&path, b);
    }
}

/// 启动时调用：后台线程异步上报一次，失败静默，绝不阻塞启动/使用。
pub fn boot(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        report_once(&app);
    });
}
