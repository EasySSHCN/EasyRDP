//! libfko 2.6.10 最小 FFI 绑定（fwknop 官方 C 库，vendor 编译，见 build.rs）。
//!
//! 仅暴露 SPA 客户端构造所需 API：fko_new / fko_set_spa_message(_type) /
//! fko_set_spa_hmac_type / fko_spa_data_final / fko_destroy。上层封装见
//! [`crate::knock`]。C 头文件为 vendor/libfko-2.6.10/fko.h；枚举值与该头
//! 文件一致，常量处标注了来源行语义。

use std::ffi::{c_char, c_int, c_short, CString};

/// fko_error_codes_t: FKO_SUCCESS（fko.h）。
pub const FKO_SUCCESS: c_int = 0;
/// fko_msg_type_t: FKO_ACCESS_MSG（COMMAND_MSG=0, ACCESS_MSG=1）。
pub const FKO_ACCESS_MSG: c_short = 1;
/// fko_digest_type_t: FKO_DIGEST_SHA256（UNKNOWN=0, MD5=1, SHA1=2, SHA256=3）。
pub const FKO_DIGEST_SHA256: c_short = 3;
/// fko_hmac_type_t: FKO_HMAC_SHA256（UNKNOWN=0, MD5=1, SHA1=2, SHA256=3）。
pub const FKO_HMAC_SHA256: c_short = 3;

/// 不透明上下文（fko_context.h 的 struct fko_context，仅作指针使用）。
#[repr(C)]
pub struct FkoContext {
    _private: [u8; 0],
}
pub type FkoCtx = *mut FkoContext;

extern "C" {
    pub fn fko_new(ctx: *mut FkoCtx) -> c_int;
    pub fn fko_destroy(ctx: FkoCtx) -> c_int;
    pub fn fko_set_spa_message_type(ctx: FkoCtx, msg_type: c_short) -> c_int;
    pub fn fko_set_spa_message(ctx: FkoCtx, msg_string: *const c_char) -> c_int;
    pub fn fko_set_spa_digest_type(ctx: FkoCtx, digest_type: c_short) -> c_int;
    pub fn fko_set_spa_hmac_type(ctx: FkoCtx, hmac_type: c_short) -> c_int;
    pub fn fko_spa_data_final(
        ctx: FkoCtx,
        enc_key: *const c_char,
        enc_key_len: c_int,
        hmac_key: *const c_char,
        hmac_key_len: c_int,
    ) -> c_int;
    pub fn fko_get_spa_data(ctx: FkoCtx, spa_data: *mut *mut c_char) -> c_int;
    /// 返回静态错误描述字符串（libfko 内部表，不需释放）。
    pub fn fko_errstr(err_code: c_int) -> *const c_char;
}

/// 一次完整的 SPA 包构造（fwknop 原生语义）：
///
/// * `message`：fwknop 访问消息，格式 `<allow_ip>,<proto>/<port>[,...]`
///   （由 [`crate::knock::build_spa_packet`] 组装）。
/// * `key`：加密密钥**原始字节**（fwknop 语义：字符串裸密钥按 UTF-8 字节
///   传入，`--key-gen`/KEY_BASE64 产物按 base64 解码后的原始字节传入；
///   libfko 按指针+长度读取，允许内嵌 NUL），内部按 PBKDF2 派生
///   Rijndael-256 密钥。
/// * `hmac_key`：HMAC-SHA256 密钥原始字节；为空则不签 HMAC。
///
/// 返回 fwknop 线格式：`<b64(iv+ciphertext)><b64(hmac)>`（两段 base64
/// 直接拼接、无 `:` 分隔、无 `=` 填充，见 fko_funcs.c fko_spa_data_final）。
/// 失败返回 libfko 错误码文本。
pub fn spa_data_final_native(
    message: &str,
    key: &[u8],
    hmac_key: &[u8],
) -> Result<Vec<u8>, String> {
    let msg_c = CString::new(message).map_err(|e| format!("SPA 消息含 NUL: {e}"))?;

    unsafe {
        let mut ctx: FkoCtx = std::ptr::null_mut();
        rc(fko_new(&mut ctx))?;
        let _guard = FkoGuard(ctx);

        rc(fko_set_spa_message_type(ctx, FKO_ACCESS_MSG))?;
        rc(fko_set_spa_digest_type(ctx, FKO_DIGEST_SHA256))?;
        rc(fko_set_spa_message(ctx, msg_c.as_ptr()))?;
        if !hmac_key.is_empty() {
            rc(fko_set_spa_hmac_type(ctx, FKO_HMAC_SHA256))?;
        }
        rc(fko_spa_data_final(
            ctx,
            key.as_ptr() as *const c_char,
            key.len() as c_int,
            if hmac_key.is_empty() {
                std::ptr::null()
            } else {
                hmac_key.as_ptr() as *const c_char
            },
            hmac_key.len() as c_int,
        ))?;
        let mut data: *mut c_char = std::ptr::null_mut();
        rc(fko_get_spa_data(ctx, &mut data))?;
        let cstr = std::ffi::CStr::from_ptr(data);
        Ok(cstr.to_bytes().to_vec())
    }
}

struct FkoGuard(FkoCtx);
impl Drop for FkoGuard {
    fn drop(&mut self) {
        unsafe { fko_destroy(self.0) };
    }
}

fn rc(code: c_int) -> Result<(), String> {
    if code == FKO_SUCCESS {
        return Ok(());
    }
    let msg = unsafe {
        std::ffi::CStr::from_ptr(fko_errstr(code))
            .to_string_lossy()
            .into_owned()
    };
    Err(format!("libfko 错误 {}: {}", code, msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// libfko 自洽：构造出的包能被再次构造出相同结构（烟测，防 FFI 层
    /// 签名/枚举错位导致 panic 或错误码）。
    #[test]
    fn fko_spa_smoke() {
        let out = spa_data_final_native("127.0.0.1,tcp/3389", b"test-key", b"test-hmac")
            .expect("libfko 构造失败");
        let s = String::from_utf8(out).unwrap();
        // 原生线格式：b64 密文与 b64 hmac **直接拼接**（fwknop 2.6.10
        // fko_funcs.c fko_spa_data_final 用 strlcat 追加，无 ':' 分隔，
        // 且 base64 尾部 '=' 被剥离——与 spa-interop/emit4.py 对照一致）。
        assert!(!s.contains(':'), "原生线格式不含 ':': {s}");
        // SHA256 HMAC b64 无填充长度 = 43。
        assert!(s.len() > 40 + 43, "包过短: {s}");
    }
}
