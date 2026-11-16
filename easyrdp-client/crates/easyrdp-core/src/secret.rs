//! 密码保护：Windows DPAPI（CryptProtectData/CryptUnprotectData）加密存储，
//! 非 Windows 平台暂以 `plain:` 前缀回退（后续迁移 macOS Keychain / libsecret）。
//!
//! 存储格式（connections.json 的 password 字段）：
//! - Windows：`dpapi:<base64(CRYPTPROTECT 密文)>`
//! - 非 Windows：`plain:<明文>`（临时方案，README/FAQ 已注明）
//!
//! 空密码恒存储为空串，不加密、不加前缀。

#[cfg(windows)]
const PREFIX_DPAPI: &str = "dpapi:";
#[cfg(not(windows))]
const PREFIX_PLAIN: &str = "plain:";

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
        Ok(format!("{PREFIX_PLAIN}{plain}"))
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
        stored
            .strip_prefix(PREFIX_PLAIN)
            .map(|s| s.to_string())
            .ok_or_else(|| "密码字段缺少 plain: 前缀".to_string())
    }
}

/// 存储串是否是本实现能识别/处理的格式（空串视为合法）。
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
        stored.starts_with(PREFIX_PLAIN)
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
    fn plain_prefix_roundtrip() {
        let stored = protect("secret").unwrap();
        assert_eq!(stored, "plain:secret");
        assert_eq!(unprotect(&stored).unwrap(), "secret");
        assert_eq!(protect("").unwrap(), "");
    }
}
