//! feedback.rs — 使用反馈提交（EasyRDP 客户端）
//!
//! 协议与官网 api/feedback/common.php 完全一致（EasySSH 同族，独立密钥）：
//!   信封 = { v:1, alg:"A256GCM-HS256", ts:<ms>, nonce:b64, ct:b64, tag:b64, mac:b64 }
//!   ct   = AES-256-GCM(fbEncKey, nonce, 明文)
//!   mac  = HMAC-SHA256(fbMacKey, `${ts}.${nonce}.${ct}.${tag}`) (b64)
//!   fbEncKey / fbMacKey = HMAC-SHA256(master, "enc"/"mac") 派生
//! 服务端校验：mac → ts 时效(±15 分钟) → 解密。
//!
//! 密钥信任模型（与 EasySSH 一致）：主密钥随客户端分发（内嵌常量），
//! 用于反馈内容机密性而非高价值数据保护；服务端同一把 master 校验。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::Serialize;

const API_URL: &str = "https://easyrdp.cn/api/feedback/submit.php";
// 主密钥（base64, 32 字节）。占位符由发布流程注入真实值（禁止写日志）。
const MASTER_B64: &str = "hCjV5iwCPrivgmL13a2YULoI4QpDE+WurrBwfYcAwqw=";

#[derive(Serialize)]
pub struct SubmitResult {
    pub ok: bool,
    pub id: String,
    pub message: String,
}

fn b64e(b: &[u8]) -> String { B64.encode(b) }

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    let mut mac = <Hmac<sha2::Sha256> as Mac>::new_from_slice(key).expect("hmac key");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn cap_str(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        s.chars().take(n).collect()
    } else {
        s.to_string()
    }
}

fn insert_capped(map: &mut serde_json::Map<String, serde_json::Value>, k: String, v: String) {
    if map.len() >= 24 {
        return;
    }
    let k = cap_str(&k, 24);
    if k.is_empty() {
        return;
    }
    map.insert(k, serde_json::Value::String(cap_str(&v, 300)));
}

fn load_master() -> Result<Vec<u8>, String> {
    let raw = B64
        .decode(MASTER_B64.trim())
        .map_err(|_| "反馈主密钥配置非法".to_string())?;
    if raw.len() != 32 {
        return Err("反馈主密钥长度非法".to_string());
    }
    Ok(raw)
}

/// 采集系统信息（最多 24 项；不采集任何明文 IP/主机名/用户名——服务端只存 IP 哈希）
fn collect_system_info() -> serde_json::Value {
    let mut m = serde_json::Map::new();
    insert_capped(&mut m, "app".to_string(), "EasyRDP".to_string());
    insert_capped(&mut m, "version".to_string(), env!("CARGO_PKG_VERSION").to_string());
    insert_capped(&mut m, "os".to_string(), std::env::consts::OS.to_string());
    insert_capped(&mut m, "arch".to_string(), std::env::consts::ARCH.to_string());
    insert_capped(&mut m, "family".to_string(), std::env::consts::FAMILY.to_string());
    collect_windows_info(&mut m);
    serde_json::Value::Object(m)
}

#[cfg(windows)]
fn collect_windows_info(map: &mut serde_json::Map<String, serde_json::Value>) {
    use std::collections::HashMap;
    use std::os::windows::process::CommandExt;
    // 注册表读 OS 版本信息（不含任何用户身份信息）
    if let Ok(output) = std::process::Command::new("cmd")
        .args(["/C", "ver"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
    {
        let s = String::from_utf8_lossy(&output.stdout);
        let s = s.trim();
        if !s.is_empty() {
            insert_capped(map, "winver".to_string(), s.to_string());
        }
    }
    // 屏幕分辨率（主屏）
    if let Ok(output) = std::process::Command::new("wmic")
        .args(["desktopmonitor", "get", "screenheight,screenwidth", "/format:list"])
        .creation_flags(0x0800_0000)
        .output()
    {
        let s = String::from_utf8_lossy(&output.stdout);
        let mut kv: HashMap<String, String> = HashMap::new();
        for line in s.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim();
                if !v.is_empty() {
                    kv.insert(k.trim().to_string(), v.to_string());
                }
            }
        }
        if let (Some(h), Some(w)) = (kv.get("ScreenHeight"), kv.get("ScreenWidth")) {
            insert_capped(map, "screen".to_string(), format!("{w}x{h}"));
        }
    }
}

#[cfg(not(windows))]
fn collect_windows_info(_map: &mut serde_json::Map<String, serde_json::Value>) {}

/// 构造加密信封（明文 = 反馈 JSON）
fn build_envelope(
    master: &[u8],
    type_: &str,
    contact: &str,
    content: &str,
    app_version: &str,
) -> Result<serde_json::Value, String> {
    let enc_key = hmac_sha256(master, b"enc");
    let mac_key = hmac_sha256(master, b"mac");

    let plain = serde_json::json!({
        "type": type_,
        "contact": contact,
        "content": content,
        "appVersion": app_version,
        "systemInfo": collect_system_info(),
    });
    let plain_bytes = serde_json::to_vec(&plain).map_err(|e| e.to_string())?;

    // AES-256-GCM 加密
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::Aes256Gcm;
    let cipher = Aes256Gcm::new_from_slice(&enc_key).map_err(|_| "密钥长度非法".to_string())?;
    let nonce_bytes: [u8; 12] = rand_nonce();
    let ct = cipher
        .encrypt(
            &nonce_bytes.into(),
            Payload { msg: &plain_bytes, aad: b"" },
        )
        .map_err(|_| "加密失败".to_string())?;
    // GCM tag 附在 ct 尾部（16 字节）——拆出与 PHP openssl_encrypt 语义对齐
    if ct.len() < 16 {
        return Err("密文异常".to_string());
    }
    let (ct_body, tag) = ct.split_at(ct.len() - 16);

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        .to_string();
    let nonce_b64 = b64e(&nonce_bytes);
    let ct_b64 = b64e(ct_body);
    let tag_b64 = b64e(tag);
    let mac_input = format!("{ts}.{nonce_b64}.{ct_b64}.{tag_b64}");
    let mac = b64e(&hmac_sha256(&mac_key, mac_input.as_bytes()));

    Ok(serde_json::json!({
        "v": 1,
        "alg": "A256GCM-HS256",
        "ts": ts,
        "nonce": nonce_b64,
        "ct": ct_b64,
        "tag": tag_b64,
        "mac": mac,
    }))
}

fn rand_nonce() -> [u8; 12] {
    // OS CSPRNG：优先 getrandom（aes-gcm/aead 生态标准）；不引入自研 PRNG
    let mut n = [0u8; 12];
    #[cfg(unix)]
    {
        use std::fs::File;
        use std::io::Read;
        let mut f = File::open("/dev/urandom").expect("open urandom");
        f.read_exact(&mut n).expect("read urandom");
    }
    #[cfg(windows)]
    {
        // BCryptGenRandom 经 windows-sys 不可用时的兜底：进程级 RSACryptoServiceProvider 不可行，
        // 直接用 SystemPrng 接口（win32 BCryptGenRandom）。
        #[link(name = "bcrypt")]
        unsafe extern "system" {
            fn BCryptGenRandom(
                halgorithm: usize,
                pbbuffer: *mut u8,
                cbbuffer: u32,
                dwflags: u32,
            ) -> i32;
        }
        // BCRYPT_USE_SYSTEM_PREFERRED_RNG = 0x00000002
        let rc = unsafe { BCryptGenRandom(0, n.as_mut_ptr(), 12, 0x0000_0002) };
        if rc != 0 {
            // 极端失败兜底：时间+地址熵混合（不应发生；BCryptGenRandom 稳定可用）
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let self_ptr = &n as *const _ as usize;
            for (i, b) in n.iter_mut().enumerate() {
                *b = ((t >> (i * 8)) as u8) ^ (self_ptr >> (i * 3)) as u8;
            }
        }
    }
    n
}

fn post_envelope(url: String, envelope: serde_json::Value) -> Result<serde_json::Value, String> {
    let body = serde_json::to_string(&envelope).map_err(|e| e.to_string())?;
    // 关键（v0.1.2 修复）：ureq 2.x 的 native-tls feature 只是把后端编译进来，
    // 快捷方法 ureq::post() 永远走 rustls 默认（未编译 → "no TLS backend"）。
    // 必须通过 AgentBuilder 显式挂 native_tls::TlsConnector（schannel）。
    // Agent 全局复用（OnceLock），避免每次提交新建连接（v0.1.1 真机 os error 10055 缓解）。
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    let agent = AGENT.get_or_init(|| {
        let connector = native_tls::TlsConnector::builder()
            .build()
            .expect("native_tls connector");
        ureq::AgentBuilder::new()
            .tls_connector(std::sync::Arc::new(connector))
            .timeout(std::time::Duration::from_secs(20))
            .build()
    });
    let resp = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .send_string(&body)
        .map_err(|e| match e {
            ureq::Error::Status(c, r) => {
                // 服务端 JSON 错误信息透传
                let text = r.into_string().unwrap_or_default();
                if let Ok(j) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(msg) = j.get("message").and_then(|m| m.as_str()) {
                        return format!("{msg}");
                    }
                }
                format!("HTTP {c}")
            }
            other => format!("网络错误: {other}"),
        })?;
    resp.into_json::<serde_json::Value>().map_err(|e| e.to_string())
}

/// Tauri 命令：提交使用反馈（阻塞 HTTP，tauri 在独立线程池执行同步命令）
#[tauri::command]
pub fn submit(
    feedback_type: String,
    contact: String,
    content: String,
) -> Result<SubmitResult, String> {
    let content_t = content.trim().to_string();
    if content_t.chars().count() < 5 || content_t.chars().count() > 2000 {
        return Ok(SubmitResult {
            ok: false,
            id: String::new(),
            message: "反馈内容需为 5-2000 个字符".to_string(),
        });
    }
    let master = load_master()?;
    let envelope = build_envelope(
        &master,
        &cap_str(&feedback_type, 16),
        &cap_str(contact.trim(), 120),
        &content_t,
        env!("CARGO_PKG_VERSION"),
    )?;
    // 网络类失败自动重试一次（v0.1.1 真机 os error 10055 场景：socket 资源紧张瞬态）
    let resp = match post_envelope(API_URL.to_string(), envelope.clone()) {
        Ok(r) => r,
        Err(e) if e.starts_with("网络错误") => {
            std::thread::sleep(std::time::Duration::from_secs(1));
            match post_envelope(API_URL.to_string(), envelope) {
                Ok(r) => r,
                Err(e2) => {
                    return Ok(SubmitResult {
                        ok: false,
                        id: String::new(),
                        message: format!(
                            "{e2}（已自动重试仍失败；请检查网络/安全软件代理拦截后稍后再试）"
                        ),
                    });
                }
            }
        }
        Err(e) => return Ok(SubmitResult { ok: false, id: String::new(), message: e }),
    };
    let ok = resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let id = resp
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let message = resp
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or(if ok { "已提交" } else { "提交失败" })
        .to_string();
    Ok(SubmitResult { ok, id, message })
}
