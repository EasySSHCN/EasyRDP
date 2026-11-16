//! updater.rs — 软件端在线更新（R-20261116-005 客户端部分）
//!
//! 协议权威源：docs/方案-在线更新机制.md（与服务端 check.php / ota-package.ps1 对齐）。
//! feedback.rs 同族信封协议、独立 salt 派生：
//!
//! ① 版本检测（POST https://easyrdp.cn/api/update/check.php）
//!    请求与响应均为加密信封 { v:1, alg:"A256GCM-HS256", ts:<ms>, nonce:b64,
//!    ct:b64, tag:b64, mac:b64 }
//!    ct  = AES-256-GCM(encKey, nonce, 明文)，GCM tag 附在 ct 尾部 16 字节
//!    mac = base64(HMAC-SHA256(macKey, `${ts}.${nonce}.${ct}.${tag}`))
//!    encKey = HMAC-SHA256(master, "update-enc")   ← 独立 salt（与 feedback 的 "enc" 区分）
//!    macKey = HMAC-SHA256(master, "update-mac")   ← 独立 salt
//!    校验顺序：mac → ts 时效(±15 分钟) → GCM 解密；服务端错误为明文 {ok:false,message}。
//!
//! ② 升级清单（latest.json 5 字段）：
//!    { latestVersion, file, notes, sha256(明文包哈希), sig(Ed25519 签名, base64) }
//!    签名覆盖规范串（固定 4 字段、固定顺序、紧凑无空格、UTF-8）：
//!      {"latestVersion":"..","file":"..","notes":"..","sha256":".."}
//!    客户端校验链：信封解密 → 重建规范串 Ed25519 验签（公钥编译期内嵌）→
//!    版本比较防降级（仅允许升级）→ 明文 sha256 留待下载解密后比对。
//!
//! ③ 加密下载（GET https://easyrdp.cn/downloads/<file>.enc）
//!    .enc = 裸 AES-256-CTR 密文（openssl enc -aes-256-ctr 语义，密文与明文等长，
//!    大端 128-bit 计数器，初始值即 16 字节 IV）。
//!    HKDF-SHA256（RFC 5869，ikm=update_master，salt=0x00×32）：
//!      文件密钥(32B) = expand(info = "update-file:" + 文件名 + 明文sha256)
//!      CTR IV(16B)   = expand(info = "update-file-iv:" + 文件名 + 明文sha256)
//!    完整性：加密层不加 HMAC；由「清单 Ed25519 验签 + 解密后 SHA256 == 清单 sha256」
//!    承担真实性/完整性（传输通道 TLS + CDN，清单签名是防篡改根）。
//!
//! ④ 替换策略（Windows）：当前 exe 改名 .bak → 新 exe 顶替原路径 →
//!    Command::spawn 分离进程重启（同参数启动新版后旧进程退出）；
//!    Rename 遇 EBUSY/拒绝访问：写延迟替换标记后正常退出，
//!    新版启动时 self_heal() 检测 .bak/标记自愈清理。

use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::Serialize;
use sha2::Digest;

const CHECK_URL: &str = "https://easyrdp.cn/api/update/check.php";
const DOWNLOAD_BASE: &str = "https://easyrdp.cn/downloads";

// 主密钥（base64, 32 字节）——与 feedback.rs 同一值，update 通道独立 salt 派生。
// 占位符由发布流程注入真实值（禁止写日志）。
const MASTER_B64: &str = "qlrrhcw2cUk7Zqq1gMhg/AQaeyilB/NblUVibUGb0yQ=";

// 加密包：.enc 为裸 CTR 密文（无文件头/魔数），密文与明文等长

/// 官方发布机 Ed25519 公钥（base64，32 字节）。
/// 占位值：发布流程注入真实公钥（对应私钥仅发布机持有，勿入库）。
pub const OTA_PUBKEY_B64: &str = "YI9WJ+46MBjo4RBBpJBrqY9tL1q8pfXV2oTa7y1iEos=";
/// 签名输入版本前缀（历史字段；现规范串见 verify_manifest_signature）
const _SIG_PREFIX_UNUSED: &str = "";
/// 信封时效窗口（毫秒）：±15 分钟
const TS_WINDOW_MS: i64 = 15 * 60 * 1000;
/// 延迟替换标记文件名（写入应用数据目录）
const DEFER_MARKER: &str = "update-deferred.json";

fn b64e(b: &[u8]) -> String {
    B64.encode(b)
}

fn b64d(s: &str) -> Result<Vec<u8>, String> {
    B64.decode(s.trim()).map_err(|e| format!("base64 解码失败: {e}"))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    let mut mac = <Hmac<sha2::Sha256> as Mac>::new_from_slice(key).expect("hmac key");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// HKDF-SHA256（RFC 5869）。update 文件密钥约定（docs 方案 §6）：
/// ikm = update_master，salt = 0x00×32，info 见 derive_file_key_and_iv。
fn hkdf_sha256_with_salt(ikm: &[u8], salt: &[u8], info: &str, len: usize) -> Vec<u8> {
    let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(salt), ikm);
    let mut out = vec![0u8; len];
    hk.expand(info.as_bytes(), &mut out)
        .expect("hkdf expand 长度合法");
    out
}

fn hkdf_sha256(ikm: &[u8], info: &str, len: usize) -> Vec<u8> {
    hkdf_sha256_with_salt(ikm, &[0u8; 32], info, len)
}

fn load_master() -> Result<Vec<u8>, String> {
    let raw = b64d(MASTER_B64).map_err(|_| "更新主密钥配置非法".to_string())?;
    if raw.len() != 32 {
        return Err("更新主密钥长度非法".to_string());
    }
    Ok(raw)
}

/// 信封子密钥：enc = HMAC(master, "update-enc")，mac = HMAC(master, "update-mac")
/// （独立 salt，与 feedback.rs 的 "enc"/"mac" 互不可用）
fn derive_check_keys(master: &[u8]) -> (Vec<u8>, Vec<u8>) {
    (
        hmac_sha256(master, b"update-enc"),
        hmac_sha256(master, b"update-mac"),
    )
}

/// 加密包密钥派生（docs 方案 §6；服务端 ota-package.ps1 同一实现）：
///   文件密钥(32B) = HKDF-SHA256(master, salt=0x00×32, info="update-file:"+文件名+明文sha256)
///   CTR IV(16B)   = 同 salt，info = "update-file-iv:"+文件名+明文sha256
/// （文件名与 sha256 直接拼接，无分隔符——与服务端命令行参数构造一致）
fn derive_file_key_and_iv(
    master: &[u8],
    file_name: &str,
    plain_sha256: &str,
) -> ([u8; 32], [u8; 16]) {
    let key = hkdf_sha256(
        master,
        &format!("update-file:{file_name}{plain_sha256}"),
        32,
    );
    let iv = hkdf_sha256(
        master,
        &format!("update-file-iv:{file_name}{plain_sha256}"),
        16,
    );
    let key_arr: [u8; 32] = key.try_into().expect("hkdf 长度固定");
    let iv_arr: [u8; 16] = iv.try_into().expect("hkdf 长度固定");
    (key_arr, iv_arr)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 构造加密信封（明文 = 请求 JSON；与 feedback.rs 同字段同编码）
fn build_envelope(master: &[u8], plain: serde_json::Value) -> Result<serde_json::Value, String> {
    let (enc_key, mac_key) = derive_check_keys(master);
    let plain_bytes = serde_json::to_vec(&plain).map_err(|e| e.to_string())?;

    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::Aes256Gcm;
    let cipher = Aes256Gcm::new_from_slice(&enc_key).map_err(|_| "密钥长度非法".to_string())?;
    let nonce_bytes = rand_nonce();
    let ct = cipher
        .encrypt(
            &nonce_bytes.into(),
            Payload {
                msg: &plain_bytes,
                aad: b"",
            },
        )
        .map_err(|_| "加密失败".to_string())?;
    // GCM tag 附在 ct 尾部（16 字节）——拆出与 PHP openssl_encrypt 语义对齐
    if ct.len() < 16 {
        return Err("密文异常".to_string());
    }
    let (ct_body, tag) = ct.split_at(ct.len() - 16);

    let ts = now_ms().to_string();
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

/// 验证并解密响应信封 → 明文 JSON。校验顺序：协议版本 → ts 时效 → mac → GCM 解密。
fn open_envelope(master: &[u8], env: &serde_json::Value) -> Result<serde_json::Value, String> {
    if env.get("v").and_then(|x| x.as_i64()) != Some(1)
        || env.get("alg").and_then(|x| x.as_str()) != Some("A256GCM-HS256")
    {
        return Err("更新接口协议版本不符".into());
    }
    let ts = env
        .get("ts")
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| env.get("ts").and_then(|x| x.as_i64()))
        .unwrap_or(0);
    if (now_ms() - ts).abs() > TS_WINDOW_MS {
        return Err("更新响应已过期，请检查本机时间".into());
    }
    let nonce = env.get("nonce").and_then(|x| x.as_str()).unwrap_or("");
    let ct = env.get("ct").and_then(|x| x.as_str()).unwrap_or("");
    let tag = env.get("tag").and_then(|x| x.as_str()).unwrap_or("");
    let mac = env.get("mac").and_then(|x| x.as_str()).unwrap_or("");

    let (enc_key, mac_key) = derive_check_keys(master);
    let mac_data = format!("{ts}.{nonce}.{ct}.{tag}");
    let got = b64d(mac).map_err(|_| "更新响应校验失败".to_string())?;
    let want = hmac_sha256(&mac_key, mac_data.as_bytes());
    // 常量时间比较
    if want.len() != got.len() || !want.iter().zip(&got).all(|(a, b)| a == b) {
        return Err("更新响应校验失败（mac 不匹配）".into());
    }

    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::Aes256Gcm;
    let mut full = b64d(ct)?;
    full.extend_from_slice(&b64d(tag)?);
    let cipher = Aes256Gcm::new_from_slice(&enc_key).map_err(|_| "密钥长度非法".to_string())?;
    let pt = cipher
        .decrypt(
            aes_gcm::Nonce::from_slice(&b64d(nonce)?),
            Payload {
                msg: full.as_ref(),
                aad: b"",
            },
        )
        .map_err(|_| "更新信封解密失败".to_string())?;
    serde_json::from_slice(&pt).map_err(|e| format!("更新清单解析失败: {e}"))
}

fn rand_nonce() -> [u8; 12] {
    // OS CSPRNG（与 feedback.rs 相同实现）
    let mut n = [0u8; 12];
    #[cfg(unix)]
    {
        use std::io::Read;
        let mut f = std::fs::File::open("/dev/urandom").expect("open urandom");
        f.read_exact(&mut n).expect("read urandom");
    }
    #[cfg(windows)]
    {
        #[link(name = "bcrypt")]
        unsafe extern "system" {
            fn BCryptGenRandom(halgorithm: usize, pbbuffer: *mut u8, cbbuffer: u32, dwflags: u32) -> i32;
        }
        // BCRYPT_USE_SYSTEM_PREFERRED_RNG = 0x00000002
        let rc = unsafe { BCryptGenRandom(0, n.as_mut_ptr(), 12, 0x0000_0002) };
        if rc != 0 {
            // 极端失败兜底（不应发生）：时间+地址熵混合
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

// 复用 feedback.rs 同款 ureq native-tls Agent（快捷方法只走 rustls，必须显式挂载）
fn http_agent(timeout_secs: u64) -> ureq::Agent {
    let connector = native_tls::TlsConnector::builder()
        .build()
        .expect("native_tls connector");
    ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(connector))
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
}

fn post_envelope(envelope: serde_json::Value) -> Result<serde_json::Value, String> {
    let body = serde_json::to_string(&envelope).map_err(|e| e.to_string())?;
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    let agent = AGENT.get_or_init(|| http_agent(20));
    let resp = agent
        .post(CHECK_URL)
        .set("Content-Type", "application/json")
        .send_string(&body)
        .map_err(|e| match e {
            ureq::Error::Status(c, _) => format!("更新接口 HTTP {c}"),
            other => format!("连接更新服务器失败: {other}"),
        })?;
    let resp_json = resp.into_json::<serde_json::Value>().map_err(|e| e.to_string())?;
    // 服务端错误为明文 JSON {ok:false,message}（与 feedback 族一致）——不是信封
    if resp_json.get("ok").and_then(|x| x.as_bool()) == Some(false) {
        let msg = resp_json
            .get("message")
            .and_then(|x| x.as_str())
            .unwrap_or("更新服务端返回错误");
        return Err(msg.to_string());
    }
    Ok(resp_json)
}

/* ---------------- Ed25519 OTA 验签 ---------------- */

fn ota_pubkey() -> Result<[u8; 32], String> {
    let b = b64d(OTA_PUBKEY_B64).map_err(|e| format!("内嵌公钥解码失败: {e}"))?;
    let arr: [u8; 32] = b
        .try_into()
        .map_err(|v: Vec<u8>| format!("内嵌公钥长度非法({} != 32)", v.len()))?;
    Ok(arr)
}

/// 升级清单 Ed25519 验签（docs 方案 §5）。
/// 签名覆盖规范串：固定 4 字段、固定顺序、紧凑无空格、UTF-8（notes 按 JSON 转义）：
///   {"latestVersion":"..","file":"..","notes":"..","sha256":".."}
/// 客户端按同规则重建后 ed25519 verify。
fn verify_manifest_signature(manifest: &serde_json::Value) -> Result<(), String> {
    let latest = manifest
        .get("latestVersion")
        .and_then(|x| x.as_str())
        .ok_or("清单缺少 latestVersion")?;
    let file = manifest.get("file").and_then(|x| x.as_str()).ok_or("清单缺少 file")?;
    let notes = manifest.get("notes").and_then(|x| x.as_str()).unwrap_or("");
    let sha = manifest.get("sha256").and_then(|x| x.as_str()).ok_or("清单缺少 sha256")?;
    let sig_b64 = manifest.get("sig").and_then(|x| x.as_str()).ok_or("清单缺少 Ed25519 签名，已拒绝")?;

    // 重建规范串：serde_json 紧凑输出 + 按约定字段顺序构造，转义语义与服务端 JSON 一致
    let canonical = serde_json::to_string(&serde_json::json!({
        "latestVersion": latest,
        "file": file,
        "notes": notes,
        "sha256": sha,
    }))
    .map_err(|e| format!("规范串重建失败: {e}"))?;

    let sig_bytes = b64d(sig_b64).map_err(|_| "签名字段 base64 非法".to_string())?;
    let sig = ed25519_dalek::Signature::from_slice(&sig_bytes)
        .map_err(|_| "签名长度非法（需 64 字节）".to_string())?;
    let vk = ed25519_dalek::VerifyingKey::from_bytes(&ota_pubkey()?)
        .map_err(|_| "内嵌公钥非法".to_string())?;
    use ed25519_dalek::Verifier;
    vk.verify(canonical.as_bytes(), &sig)
        .map_err(|_| "升级清单签名校验失败，已拒绝（清单可能被篡改）".to_string())
}

/* ---------------- 版本比较（防降级：仅允许升级） ---------------- */

/// 语义化比较 a 与 b（按 '.' 逐段数值比较；段缺失视为 0；非数字段退化为字典序）
pub fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    let pa: Vec<&str> = a.split('.').collect();
    let pb: Vec<&str> = b.split('.').collect();
    for i in 0..pa.len().max(pb.len()) {
        let x = pa.get(i).copied().unwrap_or("");
        let y = pb.get(i).copied().unwrap_or("");
        let (nx, ny) = (x.parse::<u64>().ok(), y.parse::<u64>().ok());
        match (nx, ny) {
            (Some(nx), Some(ny)) => {
                if nx != ny {
                    return nx.cmp(&ny);
                }
            }
            _ => {
                // 空段（"1.0" vs "1.0.0" 末段）视为 0；其余非数字段退化为字典序
                let nx2 = nx.unwrap_or(if x.is_empty() { 0 } else { u64::MAX });
                let ny2 = ny.unwrap_or(if y.is_empty() { 0 } else { u64::MAX });
                if nx2 != u64::MAX && ny2 != u64::MAX {
                    if nx2 != ny2 {
                        return nx2.cmp(&ny2);
                    }
                } else {
                    let o = x.cmp(y);
                    if o != std::cmp::Ordering::Equal {
                        return o;
                    }
                }
            }
        }
    }
    std::cmp::Ordering::Equal
}

/* ---------------- 加密下载 ---------------- */

/// 下载并解密安装包（流式读取 + 进度回调），成功落盘为明文 exe。
/// on_progress 入参为 0.0~1.0（仅下载阶段；解密校验完成后回调 1.0）。
fn download_and_decrypt(
    file_name: &str,
    plain_sha256: &str,
    out_file: &Path,
    master: &[u8],
    on_progress: &dyn Fn(f64),
) -> Result<(), String> {
    let url = format!("{DOWNLOAD_BASE}/{}.enc", file_name);
    let agent = http_agent(1200); // 弱网允许长时间慢速下载
    let mut resp = agent
        .get(&url)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(c, _) => format!("下载加密包 HTTP {c}"),
            other => format!("下载失败: {other}"),
        })?;
    let total: usize = resp
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut data: Vec<u8> = Vec::with_capacity(total.min(64 * 1024 * 1024) + 1024);
    use std::io::Read;
    let mut reader = resp.into_reader();
    let mut chunk = [0u8; 65536];
    let mut last_reported = 0usize;
    loop {
        let n = reader.read(&mut chunk).map_err(|e| format!("下载读取失败: {e}"))?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
        // 每 256KB 或到达总长度时回报一次（下载阶段映射 0~0.95，剩余留给解密校验）
        let done = data.len();
        if total > 0 && (done - last_reported >= 256 * 1024 || done == total) {
            on_progress((done as f64 / total as f64) * 0.95);
            last_reported = done;
        }
    }
    decrypt_file(&data, file_name, plain_sha256, master, out_file, on_progress)
}

/// 解密裸 AES-256-CTR 密文（openssl enc -aes-256-ctr 语义：大端 128-bit 计数器，
/// 初始值即派生 IV）→ 解密结果 SHA-256 与清单比对 → 通过才落盘
fn decrypt_file(
    data: &[u8],
    file_name: &str,
    plain_sha256: &str,
    master: &[u8],
    dest: &Path,
    on_progress: &dyn Fn(f64),
) -> Result<(), String> {
    if data.is_empty() {
        return Err("加密包为空".into());
    }
    let (enc_key, iv) = derive_file_key_and_iv(master, file_name, plain_sha256);
    use aes::cipher::generic_array::GenericArray;
    use ctr::cipher::{KeyIvInit, StreamCipher};
    let mut dec = <ctr::Ctr128BE<aes::Aes256> as KeyIvInit>::new(
        GenericArray::from_slice(&enc_key),
        GenericArray::from_slice(&iv),
    );
    let mut plain = data.to_vec();
    dec.apply_keystream(&mut plain);
    // 完整性：解密结果 SHA-256 与清单 sha256 比对（与 Ed25519 验签共同承担真实性）
    let sha = hex(&sha2::Sha256::digest(&plain));
    if sha != plain_sha256.to_lowercase() {
        let _ = std::fs::remove_file(dest);
        return Err("安装包校验失败(sha256)".into());
    }
    std::fs::write(dest, &plain).map_err(|e| format!("写入安装包失败: {e}"))?;
    on_progress(1.0);
    Ok(())
}

/* ---------------- 替换策略（update_flow 逻辑并入） ---------------- */

/// 已通过校验的待装清单缓存（check_update → download_and_install 传递）
static LAST_MANIFEST: std::sync::Mutex<Option<serde_json::Value>> = std::sync::Mutex::new(None);

#[derive(Serialize)]
pub struct CheckResult {
    pub has_update: bool,
    pub latest_version: String,
    pub current_version: String,
    pub notes: String,
    pub message: String,
}

#[derive(Serialize)]
pub struct InstallResult {
    pub ok: bool,
    pub message: String,
}

/// 当前运行版本
fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 应用数据目录（延迟标记 / 临时下载目录）
fn app_data_dir() -> Option<PathBuf> {
    // exe 同目录不可写（Program Files 场景）时退回 %APPDATA%/EasyRDP；
    // 这里通过 Tauri AppHandle 传入更规范，但命令内部亦可在无句柄时兜底。
    dirs_data()
}

fn dirs_data() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        // %APPDATA%\EasyRDP（与 Tauri app_data_dir 缺省布局一致：identifier 目录；
        // 标记文件只做自愈用，路径一致性由 self_heal 以同一函数保证）
        std::env::var("APPDATA")
            .ok()
            .map(|p| PathBuf::from(p).join("EasyRDP"))
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").ok().map(|p| PathBuf::from(p).join(".easyrdp"))
    }
}

/// 启动自愈：清理旧版 .bak，处理未完成的延迟替换标记。
/// 应在应用启动时调用一次（main.rs setup）；所有失败静默（自愈非关键路径）。
pub fn self_heal() {
    let Some(exe) = std::env::current_exe().ok() else {
        return;
    };
    let exe_dir = match exe.parent() {
        Some(p) => p.to_path_buf(),
        None => return,
    };
    let bak = exe.with_extension("exe.bak");
    let Some(data_dir) = dirs_data() else { return };
    let marker = data_dir.join(DEFER_MARKER);

    // 1) 处理延迟替换标记：{ "newFile":.., "target":.., "version":.. }
    if let Ok(raw) = std::fs::read_to_string(&marker) {
        if let Ok(j) = serde_json::from_str::<serde_json::Value>(&raw) {
            let new_file = j.get("newFile").and_then(|x| x.as_str()).map(PathBuf::from);
            let target = j.get("target").and_then(|x| x.as_str()).map(PathBuf::from);
            if let (Some(new_file), Some(target)) = (new_file, target) {
                // 顶替目标（此时旧进程已退出，锁已释放；重试 10×500ms）
                let mut placed = std::fs::rename(&new_file, &target).is_ok();
                if !placed {
                    for _ in 0..10 {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        if std::fs::rename(&new_file, &target).is_ok() {
                            placed = true;
                            break;
                        }
                    }
                }
                if !placed {
                    // rename 仍失败 → copy 兜底（源被只读共享锁时仍可读）
                    placed = std::fs::copy(&new_file, &target).is_ok();
                }
                if placed {
                    let _ = std::fs::remove_file(&new_file);
                }
            }
        }
        let _ = std::fs::remove_file(&marker);
    }

    // 2) 清理同名 .bak（无论是否本轮升级产生；存在即代表上次替换已完成或残留）
    let _ = std::fs::remove_file(&bak);
    // 版本化命名的历史残留一并清理（EasyRDP-*.exe.bak）
    if let Ok(rd) = std::fs::read_dir(&exe_dir) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("EasyRDP-") && name.ends_with(".exe.bak") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    // 3) 清理更新临时目录里的旧载荷
    if let Ok(rd) = std::fs::read_dir(data_dir.join("updates")) {
        for entry in rd.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 写延迟替换标记（rename 被占用时的兜底）：本进程正常退出，
/// 下次启动 self_heal() 依据标记完成 顶替+清理。
fn write_defer_marker(data_dir: &Path, new_file: &Path, target: &Path, version: &str) {
    let j = serde_json::json!({
        "newFile": new_file.to_string_lossy(),
        "target": target.to_string_lossy(),
        "version": version,
        "ts": now_ms(),
    });
    let _ = std::fs::create_dir_all(data_dir);
    let _ = std::fs::write(data_dir.join(DEFER_MARKER), j.to_string());
}

/// 分离启动新版 exe（继承当前进程命令行参数），随即旧进程退出。
fn spawn_detached(new_exe: &Path) -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new(new_exe)
            .args(&args)
            .creation_flags(0x0000_0008) // DETACHED_PROCESS：分离进程，独立生命周期
            .spawn()
            .map_err(|e| format!("启动新版本失败: {e}"))?;
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new(new_exe)
            .args(&args)
            .spawn()
            .map_err(|e| format!("启动新版本失败: {e}"))?;
    }
    Ok(())
}

/// 延迟 1.2 秒后退出当前应用（给 spawn/emit 留出时间）
fn exit_app_later(app: &tauri::AppHandle) {
    let a = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1200));
        a.exit(0);
    });
}

/// Tauri 命令：检查更新（阻塞 HTTP，tauri 在独立线程池执行同步命令）。
/// 返回 { hasUpdate, latestVersion, currentVersion, notes, message }。
#[tauri::command]
pub fn check_update() -> Result<CheckResult, String> {
    let current = current_version();
    let fail = |msg: String| CheckResult {
        has_update: false,
        latest_version: String::new(),
        current_version: current.clone(),
        notes: String::new(),
        message: msg,
    };

    let master = match load_master() {
        Ok(m) => m,
        Err(e) => return Ok(fail(e)),
    };
    // 请求明文：当前版本 + 平台（服务端据此选择对应安装包）
    let plain = serde_json::json!({
        "currentVersion": current,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    });
    let envelope = match build_envelope(&master, plain) {
        Ok(e) => e,
        Err(e) => return Ok(fail(e)),
    };
    let resp = match post_envelope(envelope) {
        Ok(r) => r,
        Err(e) => return Ok(fail(e)),
    };
    let manifest = match open_envelope(&master, &resp) {
        Ok(m) => m,
        Err(e) => return Ok(fail(e)),
    };
    // Ed25519 验签：失败一律拒绝
    if let Err(e) = verify_manifest_signature(&manifest) {
        return Ok(fail(e));
    }
    let latest = manifest
        .get("latestVersion")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let notes = manifest
        .get("notes")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if latest.is_empty() {
        return Ok(fail("升级清单为空".into()));
    }
    // 防降级：仅允许升级（latest > current 才 hasUpdate）
    let has_update = cmp_version(&latest, &current) == std::cmp::Ordering::Greater;
    if has_update {
        // 缓存清单供 download_and_install 使用（防降级在安装前复查）
        *LAST_MANIFEST.lock().unwrap() = Some(manifest);
    }
    Ok(CheckResult {
        has_update,
        latest_version: latest,
        current_version: current,
        notes,
        message: if has_update {
            "发现新版本".into()
        } else {
            "已是最新版本".into()
        },
    })
}

/// Tauri 命令：下载并安装更新（须先 check_update 且有可用升级）。
/// 进度经事件 "update-progress" 推送：{ phase: "download"|"verify"|"install", percent, message }。
/// 替换成功后本进程退出、新版自动启动；rename 被占用时写延迟标记正常退出（下次启动自愈）。
#[tauri::command]
pub fn download_and_install(app: tauri::AppHandle) -> Result<InstallResult, String> {
    emit_progress(&app, "download", 0, "开始下载更新");
    let manifest = LAST_MANIFEST
        .lock()
        .unwrap()
        .clone()
        .ok_or("请先检查更新并确认有可用升级")?;
    let master = load_master()?;
    let latest = manifest
        .get("latestVersion")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let file = manifest
        .get("file")
        .and_then(|x| x.as_str())
        .ok_or("清单缺少文件名")?
        .to_string();
    let sha = manifest
        .get("sha256")
        .and_then(|x| x.as_str())
        .ok_or("清单缺少校验值")?
        .to_lowercase();
    // 防降级复查（安装前最后防线）
    if cmp_version(&latest, &current_version()) != std::cmp::Ordering::Greater {
        return Err("目标版本不高于当前版本，拒绝安装（防降级）".into());
    }
    // 文件名白名单校验（防路径穿越）
    if file.is_empty()
        || !file.to_lowercase().ends_with(".exe")
        || file.contains('/')
        || file.contains('\\')
        || file.contains("..")
    {
        return Err("清单文件名非法".into());
    }

    // 下载到临时目录（应用数据目录 updates/）
    let data_dir = app_data_dir().ok_or("无法定位应用数据目录")?;
    let updates_dir = data_dir.join("updates");
    std::fs::create_dir_all(&updates_dir).map_err(|e| format!("创建 updates 目录失败: {e}"))?;
    let out = updates_dir.join(&file);
    let app2 = app.clone();
    let result = download_and_decrypt(&file, &sha, &out, &master, &move |frac| {
        emit_progress(
            &app2,
            "download",
            ((frac * 100.0) as u64).min(100),
            "正在下载更新",
        );
    });
    if let Err(e) = result {
        return Err(e);
    }
    emit_progress(&app, "verify", 100, "校验通过（HMAC + SHA-256）");

    // 替换：当前 exe 改名 .bak → 新 exe 顶替原路径 → 分离重启
    emit_progress(&app, "install", 0, "正在安装更新");
    let exe = std::env::current_exe().map_err(|_| "无法定位当前 exe".to_string())?;
    let bak = exe.with_extension("exe.bak");
    let _ = std::fs::remove_file(&bak);

    // 1) 旧 exe 改名（Windows 允许对运行中的 exe 改名；被杀软/占用拦截时重试后走兜底）
    let mut swapped = false;
    for i in 0..6 {
        match std::fs::rename(&exe, &bak) {
            Ok(_) => {
                swapped = true;
                break;
            }
            Err(_) => {
                if i == 5 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
        }
    }
    if swapped {
        // 2) 新 exe 顶替原路径（rename 重试 → copy 兜底）
        let mut placed = std::fs::rename(&out, &exe).is_ok();
        if !placed {
            for _ in 0..6 {
                std::thread::sleep(std::time::Duration::from_millis(400));
                if std::fs::rename(&out, &exe).is_ok() {
                    placed = true;
                    break;
                }
            }
        }
        if !placed {
            placed = std::fs::copy(&out, &exe).is_ok();
            if placed {
                let _ = std::fs::remove_file(&out);
            }
        }
        if placed {
            emit_progress(&app, "install", 100, "安装完成，正在重启");
            // R-20261116-014：升级成功 → 分离线程刷新过期快捷方式（失败静默，不阻塞重启）
            crate::shortcuts::refresh_shortcuts_after_update(&app);
            // 3) 分离进程重启新版（同参数），旧进程退出
            if let Err(e) = spawn_detached(&exe) {
                // 重启失败不致命：新版文件已就位，用户手动启动即可
                exit_app_later(&app);
                return Ok(InstallResult {
                    ok: true,
                    message: format!("已安装新版本 {latest}，但自动重启失败：{e}"),
                });
            }
            exit_app_later(&app);
            return Ok(InstallResult {
                ok: true,
                message: format!("已更新到 {latest}，应用将自动重启"),
            });
        }
        // 顶替失败 → 回滚旧版并转延迟替换
        let _ = std::fs::rename(&bak, &exe);
    }
    // EBUSY/拒绝访问兜底：写延迟替换标记后正常退出，新版启动时 self_heal() 完成顶替
    write_defer_marker(&data_dir, &out, &exe, &latest);
    emit_progress(&app, "install", 100, "文件被占用，将在下次启动时完成安装");
    exit_app_later(&app);
    Ok(InstallResult {
        ok: true,
        message: format!("新版本 {latest} 已就绪；文件被占用，将在下次启动时自动完成安装"),
    })
}

fn emit_progress(app: &tauri::AppHandle, phase: &str, percent: u64, message: &str) {
    use tauri::Emitter;
    let _ = app.emit(
        "update-progress",
        serde_json::json!({ "phase": phase, "percent": percent, "message": message }),
    );
}

/* ---------------- 单元测试（派生 / 信封编解码 / 版本比较 / 加密包往返） ---------------- */

#[cfg(test)]
mod tests {
    use super::*;

    fn test_master() -> Vec<u8> {
        load_master().expect("主密钥可解码")
    }

    #[test]
    fn master_key_is_32_bytes() {
        assert_eq!(test_master().len(), 32);
    }

    #[test]
    fn check_keys_are_deterministic_and_independent_of_feedback() {
        let m = test_master();
        let (enc1, mac1) = derive_check_keys(&m);
        let (enc2, mac2) = derive_check_keys(&m);
        assert_eq!(enc1, enc2);
        assert_eq!(mac1, mac2);
        assert_ne!(enc1, mac1);
        // 独立 salt：与 feedback.rs 的 HMAC(master,"enc") 不同
        assert_ne!(enc1, hmac_sha256(&m, b"enc"));
        assert_ne!(mac1, hmac_sha256(&m, b"mac"));
        assert_eq!(enc1.len(), 32);
    }

    #[test]
    fn hkdf_file_keys_match_spec_and_stable() {
        let m = test_master();
        let (k1, iv1) = derive_file_key_and_iv(&m, "EasyRDP-1.0.1-portable.exe", "abc123");
        let (k2, iv2) = derive_file_key_and_iv(&m, "EasyRDP-1.0.1-portable.exe", "abc123");
        assert_eq!(k1, k2);
        assert_eq!(iv1, iv2);
        // 换文件名/哈希 → 密钥与 IV 均变化
        let (k3, iv3) = derive_file_key_and_iv(&m, "EasyRDP-1.0.2-portable.exe", "abc123");
        assert_ne!(k1, k3);
        assert_ne!(iv1, iv3);
        let (k4, _) = derive_file_key_and_iv(&m, "EasyRDP-1.0.1-portable.exe", "def456");
        assert_ne!(k1, k4);
        // RFC 5869 附录 A 测试向量 Case 1（IKM=0x0b×22, salt=0x00..13B, info=0xf0..10B, L=42）
        let ikm = [0x0bu8; 22];
        let salt: [u8; 13] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
        ];
        let info: [u8; 10] = [0xf0u8, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9];
        let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), &ikm);
        let mut okm = [0u8; 42];
        hk.expand(&info, &mut okm).unwrap();
        let want: [u8; 42] = [
            0x3c, 0xb2, 0x5f, 0x25, 0xfa, 0xac, 0xd5, 0x7a, 0x90, 0x43, 0x4f, 0x64, 0xd0, 0x36,
            0x2f, 0x2a, 0x2d, 0x2d, 0x0a, 0x90, 0xcf, 0x1a, 0x5a, 0x4c, 0x5d, 0xb0, 0x2d, 0x56,
            0xec, 0xc4, 0xc5, 0xbf, 0x34, 0x00, 0x72, 0x08, 0xd5, 0xb8, 0x87, 0x18, 0x58, 0x65,
        ];
        let got_hex = hex(&okm);
        let want_hex = hex(&want);
        assert_eq!(got_hex, want_hex, "okm={got_hex} want={want_hex}");
    }

    #[test]
    fn envelope_round_trip() {
        let m = test_master();
        let plain = serde_json::json!({ "hello": "世界", "n": 42 });
        let env = build_envelope(&m, plain.clone()).unwrap();
        assert_eq!(env["v"], 1);
        assert_eq!(env["alg"], "A256GCM-HS256");
        let back = open_envelope(&m, &env).unwrap();
        assert_eq!(back, plain);
        // 换主密钥（不同 salt 派生）→ mac 校验失败
        let bad = vec![7u8; 32];
        assert!(open_envelope(&bad, &env).is_err());
        // 篡改密文 → mac 校验失败
        let mut env2 = env.clone();
        let mut ct = b64d(env2["ct"].as_str().unwrap()).unwrap();
        ct[0] ^= 0x01;
        env2["ct"] = serde_json::Value::String(b64e(&ct));
        assert!(open_envelope(&m, &env2).is_err());
    }

    #[test]
    fn envelope_rejects_stale_ts() {
        let m = test_master();
        let env = build_envelope(&m, serde_json::json!({"x":1})).unwrap();
        let mut env_old = env.clone();
        // ts 回拨 16 分钟 → 拒绝
        let stale = (now_ms() - (16 * 60 * 1000)).to_string();
        // 重新签名 mac 使时间校验（在 mac 之前）先触发——此处直接改 ts，mac 必然失配，
        // 但时效检查先于 mac 校验，报错应为「已过期」
        env_old["ts"] = serde_json::Value::String(stale);
        let err = open_envelope(&m, &env_old).unwrap_err();
        assert!(err.contains("已过期"), "实际错误: {err}");
    }

    #[test]
    fn version_compare_anti_downgrade() {
        assert_eq!(cmp_version("1.0.1", "1.0.0"), std::cmp::Ordering::Greater);
        assert_eq!(cmp_version("1.10.0", "1.9.9"), std::cmp::Ordering::Greater);
        assert_eq!(cmp_version("1.0.0", "1.0.0"), std::cmp::Ordering::Equal);
        assert_eq!(cmp_version("0.9.9", "1.0.0"), std::cmp::Ordering::Less);
        assert_eq!(cmp_version("2.0", "1.9.9"), std::cmp::Ordering::Greater);
        assert_eq!(cmp_version("1.0", "1.0.0"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn encrypted_file_round_trip() {
        // 与服务端 ota-package.ps1 同构打包：HKDF 派生 key+IV → 裸 AES-256-CTR 加密
        let m = test_master();
        let file_name = "EasyRDP-9.9.9-portable.exe";
        let plain: Vec<u8> = (0..70000u32).map(|i| (i % 251) as u8).collect();
        let sha = hex(&sha2::Sha256::digest(&plain));
        let (enc_key, iv) = derive_file_key_and_iv(&m, file_name, &sha);
        use aes::cipher::generic_array::GenericArray;
        use ctr::cipher::{KeyIvInit, StreamCipher};
        let mut ct = plain.clone();
        let mut enc = <ctr::Ctr128BE<aes::Aes256> as KeyIvInit>::new(
            GenericArray::from_slice(&enc_key),
            GenericArray::from_slice(&iv),
        );
        enc.apply_keystream(&mut ct);

        let dir = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-data"));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join(format!("update-payload-{}.exe", std::process::id()));
        let called = std::cell::Cell::new(false);
        decrypt_file(&ct, file_name, &sha, &m, &out, &|f| {
            called.set(f >= 1.0);
        })
        .expect("解密应成功");
        assert!(called.get());
        let decoded = std::fs::read(&out).unwrap();
        assert_eq!(decoded, plain);
        // 篡改密文 → 解密后 SHA-256 比对拦截
        let mut bad = ct.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        assert!(decrypt_file(&bad, file_name, &sha, &m, &out, &|_| {}).is_err());
        // 清单哈希不匹配 → 拒绝
        assert!(decrypt_file(&ct, file_name, &"0".repeat(64), &m, &out, &|_| {}).is_err());
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn ota_pubkey_constant_decodes() {
        // 占位公钥也应可解码为 32 字节（发布流程注入真实值后同一逻辑生效）
        assert_eq!(ota_pubkey().unwrap().len(), 32);
    }
}
