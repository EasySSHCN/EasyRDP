//! 密码保护：Windows DPAPI（CryptProtectData/CryptUnprotectData）加密存储；
//! 非 Windows 平台用本机密钥文件（权限 0600）+ AES-256-GCM 加密存储。
//!
//! 存储格式（connections.json 的 password 字段）：
//! - Windows：`dpapi:<base64(CRYPTPROTECT 密文)>`
//! - 非 Windows：`enc:<base64(nonce || AES-256-GCM 密文)>`，
//!   主密钥为本机 32 字节随机密钥文件（0600）；
//!   旧版 `plain:<明文>` 仍可解（unprotect 兼容读），下次保存自动迁移为 enc:。
//!
//! 空密码恒存储为空串，不加密、不加前缀。

#[cfg(windows)]
const PREFIX_DPAPI: &str = "dpapi:";
#[cfg(not(windows))]
const PREFIX_ENC: &str = "enc:";
#[cfg(not(windows))]
const PREFIX_PLAIN_LEGACY: &str = "plain:";

/// 加密并编码为可存储字符串。空密码原样返回。
pub fn protect(plain: &str) -> Result<String, String> {
    if plain.is_empty() {
        return Ok(String::new());
    }
    #[cfg(windows)]
    {
        protect_dpapi(plain.as_bytes())
    }
    #[cfg(not(windows))]
    {
        protect_aes_gcm(plain.as_bytes())
    }
}

/// 解码并解密存储字符串，返回明文。空串原样返回。
pub fn unprotect(stored: &str) -> Result<String, String> {
    if stored.is_empty() {
        return Ok(String::new());
    }
    #[cfg(windows)]
    {
        let b64 = stored
            .strip_prefix(PREFIX_DPAPI)
            .ok_or_else(|| "密码字段缺少 dpapi: 前缀（旧版明文数据，请重新保存该连接）".to_string())?;
        let bytes = unprotect_dpapi(b64)?;
        String::from_utf8(bytes).map_err(|_| "DPAPI 解密结果不是有效 UTF-8".to_string())
    }
    #[cfg(not(windows))]
    {
        if let Some(b64) = stored.strip_prefix(PREFIX_ENC) {
            let bytes = unprotect_aes_gcm(b64)?;
            return String::from_utf8(bytes).map_err(|_| "解密结果不是有效 UTF-8".to_string());
        }
        // 旧版明文回退数据：兼容读取（下次保存由 protect 自动升级为 enc:）。
        if let Some(plain) = stored.strip_prefix(PREFIX_PLAIN_LEGACY) {
            return Ok(plain.to_string());
        }
        Err("密码字段缺少 enc: 前缀（未知格式，请重新保存该连接）".to_string())
    }
}

/// 存储串是否是本实现能识别/处理的格式（空串视为合法）。
/// 注意：旧版 `plain:` 明文格式视为**未保护**——下次保存会自动升级为 `enc:` 加密。
pub fn is_protected_format(stored: &str) -> bool {
    if stored.is_empty() {
        return true;
    }
    #[cfg(windows)]
    {
        stored.starts_with(PREFIX_DPAPI)
    }
    #[cfg(not(windows))]
    {
        stored.starts_with(PREFIX_ENC)
    }
}

// ---------------- Windows DPAPI ----------------

#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    use std::ptr;

    /// 加密 bytes，返回 DPAPI 密文。失败返回错误文本（来自 GetLastError）。
    pub fn protect(bytes: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };
        let input = CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        };
        // CRYPTPROTECT_UI_FORBIDDEN：无 UI 弹窗；描述串可为空指针。
        let ok = unsafe {
            CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err("DPAPI CryptProtectData 失败".into());
        }
        let data = copy_and_free(out);
        Ok(data)
    }

    /// 解密 DPAPI 密文，返回原始字节。
    pub fn unprotect(bytes: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };
        let mut input = CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        };
        let ok = unsafe {
            CryptUnprotectData(
                &mut input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                0,
                &mut out,
            )
        };
        if ok == 0 {
            return Err("DPAPI CryptUnprotectData 失败（密文损坏或由其他用户加密）".into());
        }
        let data = copy_and_free(out);
        Ok(data)
    }

    /// CryptProtectData/CryptUnprotectData 输出 blob 由 LocalAlloc 分配，复制后 LocalFree。
    fn copy_and_free(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let data = unsafe {
            std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec()
        };
        if !out.pbData.is_null() {
            unsafe { LocalFree(out.pbData as HLOCAL) };
        }
        data
    }
}

#[cfg(not(windows))]
mod keyfile {
    use std::fs;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::PathBuf;

    /// 主密钥文件路径：`$XDG_DATA_HOME/easyrdp/secret.key`（缺省 `~/.local/share/easyrdp/secret.key`）。
    pub fn key_path() -> Result<PathBuf, String> {
        let base = match std::env::var("XDG_DATA_HOME") {
            Ok(p) if !p.is_empty() => PathBuf::from(p),
            _ => {
                let home = std::env::var("HOME")
                    .map(PathBuf::from)
                    .map_err(|_| "无法确定用户主目录（HOME 未设置），密钥文件不可用".to_string())?;
                home.join(".local").join("share")
            }
        };
        Ok(base.join("easyrdp").join("secret.key"))
    }

    /// 加载或首次创建 32 字节随机主密钥文件。目录 0700、文件 0600。
    pub fn load_or_create() -> Result<[u8; 32], String> {
        let path = key_path()?;
        let dir = path
            .parent()
            .ok_or_else(|| "密钥文件路径无效".to_string())?;
        fs::create_dir_all(dir)
            .map_err(|e| format!("密钥目录创建失败 {}: {e}", dir.display()))?;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        if path.exists() {
            let bytes = fs::read(&path)
                .map_err(|e| format!("密钥文件读取失败 {}: {e}", path.display()))?;
            if bytes.len() == 32 {
                let mut key = [0u8; 32];
                key.copy_from_slice(&bytes);
                return Ok(key);
            }
            return Err(format!(
                "密钥文件格式异常（长度 {} != 32）: {}",
                bytes.len(),
                path.display()
            ));
        }
        use rand::RngCore;
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        // 先以 0600 创建，防止任何窗口期暴露。
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true).mode(0o600);
        let mut f = opts
            .open(&path)
            .map_err(|e| format!("密钥文件创建失败 {}: {e}", path.display()))?;
        use std::io::Write;
        f.write_all(&key)
            .map_err(|e| format!("密钥文件写入失败 {}: {e}", path.display()))?;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        Ok(key)
    }
}

#[cfg(not(windows))]
fn protect_aes_gcm(bytes: &[u8]) -> Result<String, String> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use base64::Engine;
    use rand::RngCore;

    let key = keyfile::load_or_create()?;
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(&key)
        .map_err(|_| "AES-GCM 密钥初始化失败".to_string())?;
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    // AAD 绑定格式前缀，防密文跨格式挪用。
    let ct = cipher
        .encrypt(
            nonce.as_ref().into(),
            Payload {
                msg: bytes,
                aad: b"easyrdp-enc:v1",
            },
        )
        .map_err(|_| "AES-GCM 加密失败".to_string())?;
    let mut blob = Vec::with_capacity(12 + ct.len());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ct);
    Ok(format!(
        "{PREFIX_ENC}{}",
        base64::engine::general_purpose::STANDARD.encode(blob)
    ))
}

#[cfg(not(windows))]
fn unprotect_aes_gcm(b64: &str) -> Result<Vec<u8>, String> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use base64::Engine;

    let blob = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("密码字段 base64 解码失败: {e}"))?;
    if blob.len() < 12 + 16 {
        return Err("enc: 密文长度异常（缺少 nonce 或认证标签）".to_string());
    }
    let key = keyfile::load_or_create()?;
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(&key)
        .map_err(|_| "AES-GCM 密钥初始化失败".to_string())?;
    let (nonce, ct) = blob.split_at(12);
    cipher
        .decrypt(
            nonce.into(),
            Payload {
                msg: ct,
                aad: b"easyrdp-enc:v1",
            },
        )
        .map_err(|_| "AES-GCM 解密失败（密文损坏、被篡改或由其他机器密钥加密）".to_string())
}

#[cfg(windows)]
fn protect_dpapi(bytes: &[u8]) -> Result<String, String> {
    use base64::Engine;
    let cipher = dpapi::protect(bytes)?;
    Ok(format!(
        "{PREFIX_DPAPI}{}",
        base64::engine::general_purpose::STANDARD.encode(cipher)
    ))
}

#[cfg(windows)]
fn unprotect_dpapi(b64: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let cipher = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("密码字段 base64 解码失败: {e}"))?;
    dpapi::unprotect(&cipher)
}

#[cfg(test)]
#[cfg(windows)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_roundtrip() {
        let plain = "P@ssw0rd-中文-🔐";
        let stored = protect(plain).unwrap();
        assert!(stored.starts_with("dpapi:"));
        assert!(!stored.contains(plain), "存储串不得含明文");
        assert_eq!(unprotect(&stored).unwrap(), plain);
    }

    #[test]
    fn dpapi_ciphertext_is_random_per_call() {
        let stored1 = protect("same-secret").unwrap();
        let stored2 = protect("same-secret").unwrap();
        assert_ne!(stored1, stored2, "DPAPI 密文应每次不同（含随机熵）");
    }

    #[test]
    fn dpapi_empty_password_untouched() {
        assert_eq!(protect("").unwrap(), "");
        assert_eq!(unprotect("").unwrap(), "");
    }

    #[test]
    fn dpapi_rejects_missing_prefix() {
        assert!(protect("x").is_ok());
        assert!(unprotect("plain:legacy-plaintext").is_err());
        assert!(unprotect("dpapi:!!!not-base64!!!").is_err());
    }

    #[test]
    fn dpapi_rejects_tampered_ciphertext() {
        let stored = protect("secret").unwrap();
        let b64 = stored.strip_prefix("dpapi:").unwrap();
        // 破坏密文中部一个 base64 字符（避开尾部 padding；仍保持合法字符集）
        let mut chars: Vec<char> = b64.chars().collect();
        let idx = chars.len() / 2;
        let alt: &[char] = if chars[idx] == 'A' { &['B'] } else { &['A'] };
        chars[idx] = alt[0];
        let tampered: String = chars.into_iter().collect();
        assert_ne!(tampered, stored);
        assert!(unprotect(&tampered).is_err());
    }
}

#[cfg(test)]
#[cfg(not(windows))]
mod non_windows_tests {
    use super::*;

    #[test]
    fn enc_roundtrip() {
        let plain = "secret-中文-🔐";
        let stored = protect(plain).unwrap();
        assert!(stored.starts_with("enc:"), "应为 enc: 加密前缀: {stored}");
        assert!(!stored.contains(plain), "存储串不得含明文");
        assert_eq!(unprotect(&stored).unwrap(), plain);
    }

    #[test]
    fn enc_ciphertext_is_random_per_call() {
        let s1 = protect("same-secret").unwrap();
        let s2 = protect("same-secret").unwrap();
        assert_ne!(s1, s2, "随机 nonce 应使每次密文不同");
    }

    #[test]
    fn enc_empty_password_untouched() {
        assert_eq!(protect("").unwrap(), "");
        assert_eq!(unprotect("").unwrap(), "");
    }

    #[test]
    fn legacy_plain_prefix_still_readable() {
        // 旧版 plain: 明文数据兼容读取；且不视为受保护格式（保存时自动升级加密）。
        assert_eq!(unprotect("plain:legacy-plaintext").unwrap(), "legacy-plaintext");
        assert!(!is_protected_format("plain:legacy-plaintext"));
        assert!(is_protected_format(&protect("x").unwrap()));
    }

    #[test]
    fn enc_rejects_tampered_ciphertext() {
        let stored = protect("secret").unwrap();
        let b64 = stored.strip_prefix("enc:").unwrap();
        let mut chars: Vec<char> = b64.chars().collect();
        // 破坏密文中部一个 base64 字符（避开尾部 padding）
        let idx = chars.len() / 2;
        chars[idx] = if chars[idx] == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();
        assert_ne!(tampered, stored);
        assert!(unprotect(&tampered).is_err());
        assert!(unprotect("enc:!!!not-base64!!!").is_err());
        assert!(unprotect("no-prefix").is_err());
    }
}
