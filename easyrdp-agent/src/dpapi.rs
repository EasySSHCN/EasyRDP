//! AgentToken DPAPI 落盘（Windows CurrentUser scope，方案 §5/P3）。
//!
//! `--enroll` 换取到 AgentToken 后经 `CryptProtectData` 加密，Base64 后以
//! `dpapi:<base64>` 形态写回 agent.toml 的 `token` 字段；运行期 `unprotect`
//! 解密。密文绑定当前 Windows 用户，拷贝配置文件到其他机器/用户无法解密。
//!
//! 非 Windows 平台：本模块返回明确错误（首版仅 Windows x64，方案 §5）。

#[cfg(windows)]
pub mod imp {
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };
    use windows_sys::Win32::Foundation::LocalFree;

    // ------------------------------------------------------------------
    // 标准 Base64（stdlib 无 base64；为保持依赖树最小，此处内置 60 行实现）
    // ------------------------------------------------------------------
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn b64_encode(data: &[u8]) -> String {
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = *chunk.get(1).unwrap_or(&0) as u32;
            let b2 = *chunk.get(2).unwrap_or(&0) as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPHABET[(n >> 18) as usize & 63] as char);
            out.push(ALPHABET[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
        }
        out
    }

    pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
        let mut rev = [255u8; 256];
        for (i, &c) in ALPHABET.iter().enumerate() {
            rev[c as usize] = i as u8;
        }
        let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        // 去掉 padding
        let body: Vec<u8> = bytes.iter().copied().take_while(|&b| b != b'=').collect();
        let mut out = Vec::with_capacity(body.len() * 3 / 4);
        for chunk in body.chunks(4) {
            if chunk.len() == 1 {
                return None; // 非法长度
            }
            let mut n: u32 = 0;
            for (i, &c) in chunk.iter().enumerate() {
                let v = rev[c as usize];
                if v == 255 {
                    return None;
                }
                n |= (v as u32) << (18 - 6 * i);
            }
            out.push((n >> 16) as u8);
            if chunk.len() > 2 {
                out.push((n >> 8) as u8);
            }
            if chunk.len() > 3 {
                out.push(n as u8);
            }
        }
        Some(out)
    }

    fn protect_raw(plain: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let mut input = CRYPT_INTEGER_BLOB {
                cbData: plain.len() as u32,
                pbData: plain.as_ptr() as *mut u8,
            };
            let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
            let ok = CryptProtectData(
                &mut input,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
                0,
                &mut output,
            );
            if ok == 0 {
                return Err("CryptProtectData 失败".into());
            }
            let slice =
                std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            LocalFree(output.pbData.cast::<core::ffi::c_void>());
            Ok(slice)
        }
    }

    fn unprotect_raw(blob: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let mut input = CRYPT_INTEGER_BLOB {
                cbData: blob.len() as u32,
                pbData: blob.as_ptr() as *mut u8,
            };
            let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
            let ok = CryptUnprotectData(
                &mut input,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
                0,
                &mut output,
            );
            if ok == 0 {
                return Err("CryptUnprotectData 失败（密文损坏或非本用户加密）".into());
            }
            let slice =
                std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            LocalFree(output.pbData.cast::<core::ffi::c_void>());
            Ok(slice)
        }
    }

    /// 加密并 Base64（返回可直接放进 agent.toml 的 blob 文本）。
    pub fn protect(plain: &str) -> Result<String, String> {
        Ok(b64_encode(&protect_raw(plain.as_bytes())?))
    }

    /// 解 Base64 后经 DPAPI 解密，返回明文 token。
    pub fn unprotect(blob: &str) -> Result<String, String> {
        let raw = b64_decode(blob).ok_or_else(|| "token 密文不是合法 Base64".to_string())?;
        let plain = unprotect_raw(&raw)?;
        String::from_utf8(plain).map_err(|_| "解密结果不是合法 UTF-8".to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn b64_roundtrip_various_lengths() {
            for len in [0usize, 1, 2, 3, 4, 5, 31, 32, 100] {
                let data: Vec<u8> = (0..len as u8).map(|i| i.wrapping_mul(37).wrapping_add(11)).collect();
                let enc = b64_encode(&data);
                assert_eq!(b64_decode(&enc).expect("decode"), data, "len={len}");
            }
            // padding 形态校验
            assert_eq!(b64_encode(b"a"), "YQ==");
            assert_eq!(b64_encode(b"ab"), "YWI=");
            assert_eq!(b64_encode(b"abc"), "YWJj");
            assert!(b64_decode("YQ").is_some(), "无 padding 单块可宽容解码");
            assert_eq!(b64_decode("YQ"), Some(vec![b'a']));
            assert!(b64_decode("a!!!b").is_none(), "非法字符");
        }

        #[test]
        fn dpapi_roundtrip_and_user_binding() {
            let token = "agent-jwt-测试-0123456789";
            let blob = protect(token).expect("protect");
            assert!(!blob.contains(token), "密文不得包含明文");
            assert_eq!(unprotect(&blob).expect("unprotect"), token);
            // 篡改密文 → 解密失败
            let mut tampered = blob.clone().into_bytes();
            let mid = tampered.len() / 2;
            tampered[mid] = b'A';
            assert!(unprotect(&String::from_utf8(tampered).unwrap()).is_err());
            assert!(unprotect("not-base64!!!").is_err());
        }
    }
}

#[cfg(not(windows))]
pub mod imp {
    /// 非 Windows：DPAPI 无对应实现，明确报错（首版仅 Windows x64，方案 §5）。
    pub fn protect(_plain: &str) -> Result<String, String> {
        Err("DPAPI 仅在 Windows 平台可用".into())
    }

    pub fn unprotect(_blob: &str) -> Result<String, String> {
        Err("DPAPI 仅在 Windows 平台可用".into())
    }
}

pub use imp::{protect, unprotect};
