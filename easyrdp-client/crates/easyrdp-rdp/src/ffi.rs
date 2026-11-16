//! FreeRDP FFI 集成（仅在 `--features native` 下编译并链接）。
//!
//! 绑定来源：[`freerdp2_sys`] 0.2（bindgen 0.65 生成，链接 `freerdp2 >= 2.4` /
//! `freerdp-client2` / `winpr2`，经 pkg-config/system-deps 定位）。
//!
//! ============ α 版范围与已知限制 ============
//!
//! - **无 user_data 的回调**：FreeRDP 2.x 的 `pPreConnect`/`pPostConnect`
//!   只带 `freerdp*` 一个参数，Rust 侧通过 [`EVENT_ROUTES`]
//!   （instance 指针地址 -> 事件 Sender 的全局注册表）路由事件。
//!   这是 α 版的已知妥协，β 版应改为扩大 `ContextSize` 并在自定义
//!   context 头部存放 `Arc<SessionShared>`。
//! - **事件循环为轮询**：α 版用 `freerdp_check_event_handles` + 20ms sleep
//!   驱动；正确做法是 `freerdp_get_event_handles` + `WaitForMultipleObjects`
//!   （winpr），避免空转。
//! - **像素经 GDI 合成为 BGRA32**：PostConnect 时调用 `gdi_init(instance,
//!   PIXEL_FORMAT_BGRA32)`（`<freerdp/gdi/gdi.h>`）。gdi_init 内部经
//!   `bitmap_cache_register_callbacks` 把 `update->BitmapUpdate` 挂到
//!   `gdi_bitmap_update`（RLE/planar 解压 + 合成进 `gdi->primary_buffer`，
//!   见 libfreerdp/gdi/gdi.c `gdi_init_ex`）。本 crate 随后把 `BitmapUpdate`
//!   替换为链式回调：先调 GDI 原处理器完成合成，再按 `gdi->stride` 从
//!   `primary_buffer` 抠出脏矩形 BGRA32 像素推给上层（前端可直接编码 BMP）。
//! - **密码注入**：`SessionProfile` 当前没有 password 字段，密码经
//!   [`FreerdpSession::set_password`] 在 connect 前注入 settings；
//!   未提供密码时依赖 `instance.Authenticate` 回调（α 版固定回 FALSE，
//!   即密码缺失直接连接失败，不弹交互）。
//! - **忽略证书**：α 版固定 `FreeRDP_IgnoreCertificate = TRUE`（内网验收用）。
//!   β 版必须改为 `VerifyX509Certificate` 回调 + 指纹白名单。
//! - **虚拟通道**：α 版实现 **CLIPRDR 文本剪贴板双向同步**（见下方
//!   「CLIPRDR 剪贴板双向同步」一节）；RDPDR/RDPSND/RD Gateway、
//!   NLA 凭据交互（CredSSP prompting）、动态分辨率（DisplayControl）、
//!   SurfaceBits(RDP8 graphics) 通路仍未实现。
//!
//! 每个 `unsafe` 块都极小化并注明对应 FreeRDP C API 与头文件来源。
//! 符号名与结构体字段均已对照 freerdp2-sys 0.2.0 的 bindgen 输出核实
//! （`freerdp` 为 `rdp_freerdp` 的别名、`rdpContext` 为 `rdp_context` 的别名，
//! settings id 常量为 `u32`、setter 形参为 `size_t`）。
//!
//! ============ Linux 实机验证记录（2026-11） ============
//!
//! 在 Linux x86_64（CentOS Stream 9，FreeRDP 2.11.7 / freerdp-devel
//! 2:2.11.7-15.el9，pkg-config `freerdp2 = 2.11.7`）上验证通过：
//! `cargo check` 与 `cargo build -p easyrdp-rdp --features native` 均成功，
//! 链接 freerdp2 / freerdp-client2 / winpr2 正常。
//!
//! API 差异注意：`freerdp_settings_set_string/_uint32/_bool` 等设置器形参
//! 在 freerdp2-sys 0.2.0 中声明为 `size_t`（x86_64 上是 `u64`），并非
//! `usize`；传 id 时用 `id as usize as _` 保持 32/64 位平台兼容。
//! （settings id 常量本身仍为 `u32`，FreeRDP 2.x 与 3.x 语义一致。）

#![cfg(feature = "native")]

use std::collections::HashMap;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// 帧延迟采样计数器（1/32 抽样落 debug.log）。
static LAT_SAMPLES: AtomicU64 = AtomicU64::new(0);
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use freerdp2_sys as sys;

use easyrdp_core::profile::{Resolution, SessionProfile};

use crate::event_channel::{self, FramebufferEvent};
use crate::frame_jpeg;
use crate::{InputEvent, RdpError, RdpSession};

/// FreeRDP BOOL 的 Rust 常量（`sys::BOOL = i32`，见 `<winpr/wtypes.h>`）。
const TRUE: sys::BOOL = 1;
const FALSE: sys::BOOL = 0;

// ---------------------------------------------------------------------------
// 输入/像素常量（freerdp2-sys 0.2.0 的 bindgen 未导出这些宏，按
// vcpkg freerdp 2.11.2 真实头文件逐条定义核实）：
// - <freerdp/input.h> 33-47 行：KBD_FLAGS_* / PTR_FLAGS_* / WheelRotationMask
// - <winpr/input.h> 30 行：KBDEXT
// - <freerdp/codec/color.h> 43/67 行：FREERDP_PIXEL_FORMAT 宏展开出
//   PIXEL_FORMAT_BGRA32 = (32<<24)|(TYPE_BGRA=4<<16)|(8<<12)|(8<<8)|(8<<4)|8
// ---------------------------------------------------------------------------
const KBD_FLAGS_EXTENDED: u16 = 0x0100;
const KBD_FLAGS_RELEASE: u16 = 0x8000;
const PTR_FLAGS_MOVE: u16 = 0x0800;
const PTR_FLAGS_DOWN: u16 = 0x8000;
const PTR_FLAGS_BUTTON1: u16 = 0x1000; // left
const PTR_FLAGS_BUTTON2: u16 = 0x2000; // right
const PTR_FLAGS_BUTTON3: u16 = 0x4000; // middle
const PTR_FLAGS_WHEEL: u16 = 0x0200;
const PTR_FLAGS_WHEEL_NEGATIVE: u16 = 0x0100;
/// 横向滚轮（MS-RDPEPTR PTR_FLAGS HWHEEL 位，Shift+滚轮/触摸板横扫）。
const PTR_FLAGS_HWHEEL: u16 = 0x0400;
const WHEEL_ROTATION_MASK: u16 = 0x01FF;
const KBDEXT: u32 = 0x0100; // <winpr/input.h>，GetVirtualScanCodeFromVirtualKeyCode 返回值的扩展键位
/// PIXEL_FORMAT_BGRA32（`<freerdp/codec/color.h>` 宏展开，8-8-8-8 BGRA 内存序）。
const PIXEL_FORMAT_BGRA32: u32 = (32 << 24) | (4 << 16) | (8 << 12) | (8 << 8) | (8 << 4) | 8;
/// Windows 滚轮逻辑量：1 格 = 120（WM_WHEEL delta 约定）。
const WHEEL_DELTA: i32 = 120;
/// GetVirtualScanCodeFromVirtualKeyCode 的键盘类型参数：4 = 增强 AT 键盘
/// （<winpr/input.h> 的 KBD4_* 转换表，FreeRDP 默认布局）。
const KEYBOARD_TYPE_AT_ENHANCED: u32 = 4;

// ---------------------------------------------------------------------------
// OpenSSL 3 legacy provider（Windows 静态链接 NLA/NTLM 前置条件）
//
// 真实头文件核实（B:\tools\EasyRDP\vcpkg\installed\x64-windows-static-md\include，
// OpenSSL 3.6.5）：
// - <openssl/crypto.h>：
//   `int OPENSSL_init_crypto(uint64_t opts, const OPENSSL_INIT_SETTINGS *settings);`
//   **该版本没有 OPENSSL_INIT_LOAD_LEGACY_PROVIDER 宏**（3.x 中 legacy provider
//   必须经 OSSL_PROVIDER_load 显式加载）。实际使用的 flags（crypto.h 40-44 行）：
//   OPENSSL_INIT_LOAD_CRYPTO_STRINGS(0x2) | OPENSSL_INIT_ADD_ALL_CIPHERS(0x4)
//   | OPENSSL_INIT_ADD_ALL_DIGESTS(0x8)。
// - <openssl/provider.h>：
//   `OSSL_PROVIDER *OSSL_PROVIDER_load(OSSL_LIB_CTX *, const char *name);`
//   （libctx 传 NULL = 默认 libctx；legacy provider 提供 MD4 —— NLA/NTLM
//   计算 NT 哈希必需）。
// freerdp2-sys 0.2.0 未绑定这两个符号，手写 extern（链接 vcpkg 静态 libcrypto.lib）。
// ---------------------------------------------------------------------------

#[cfg(windows)]
static CRYPTO_LEGACY_INIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

#[cfg(windows)]
fn ensure_openssl_legacy_provider() -> bool {
    *CRYPTO_LEGACY_INIT.get_or_init(|| {
        #[link(name = "libcrypto")]
        extern "C" {
            fn OPENSSL_init_crypto(
                opts: u64,
                settings: *const std::ffi::c_void,
            ) -> std::ffi::c_int;
            fn OSSL_PROVIDER_load(
                libctx: *mut std::ffi::c_void,
                name: *const ::std::os::raw::c_char,
            ) -> *mut std::ffi::c_void;
        }
        // SAFETY：OpenSSL 3 全局初始化（幂等，OpenSSL 内部去重）；
        // 任一 provider load 返回 NULL 表示加载失败（组件缺失）。
        unsafe {
            let opts: u64 = 0x2 | 0x4 | 0x8;
            let init_ok = OPENSSL_init_crypto(opts, std::ptr::null()) == 1;
            // ！！！必须同时显式加载 "default" 与 "legacy"（2026-11 实测定位，
            // 真机 0x2000D transport failed 真因）：
            // OpenSSL 3 一旦显式加载任何 provider，默认 provider 的隐式
            // 自动加载即被关闭。只加载 legacy（MD4/RC4，NTLM 与 RDP 标准安全层
            // 加密必需）会让 SHA256/DRBG（随机数）等 default 算法缺失，
            // TLS/NLA 之后首个加密读写即报
            //   error:0308010C digital envelope routines::unsupported
            //   error:12000090 random number generator::unable to fetch drbg
            // → BIO_read 失败 → ERRCONNECT_CONNECT_TRANSPORT_FAILED。
            let mut ok = init_ok;
            for name in ["default", "legacy"] {
                let cname = CString::new(name).expect("静态字面量无 \\0");
                let provider = OSSL_PROVIDER_load(std::ptr::null_mut(), cname.as_ptr());
                let loaded = !provider.is_null();
                eprintln!("[crypto] provider '{name}' loaded = {loaded}");
                ok = ok && loaded;
            }
            ok
        }
    })
}

#[cfg(not(windows))]
fn ensure_openssl_legacy_provider() -> bool {
    true // 非 Windows 构建由系统 OpenSSL（含 legacy provider 模块）负责
}

/// 静态通道 addin provider 注册（一次性）。
///
/// freerdp_client_load_addins 经 freerdp_load_channel_addin_entry 解析静态
/// 通道入口（cliprdr 等）。静态链接（vcpkg x64-windows-static-md）下没有
/// cliprdr-client.dll 可 LoadLibrary，必须注册内置 provider
/// `freerdp_channels_load_static_addin_entry`（<freerdp/client/channels.h> 34 行）
/// —— wfreerdp（wf_client.c 1037 行）与 freerdp_client_new（client/common/
/// client.c 85 行）均如此初始化；我们自建 instance 无人注册 →
/// "Failed to load channel cliprdr" → PreConnect 失败。
/// freerdp2-sys 0.2.0 未绑定这两个符号，手写 extern（符号在 freerdp-client2.lib）。
#[cfg(windows)]
static ADDIN_PROVIDER_REGISTERED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

#[cfg(windows)]
fn register_static_addin_provider() {
    type PVirtualChannelEntryFn = unsafe extern "C" fn() -> sys::BOOL;
    type LoadAddinEntryFn =
        unsafe extern "C" fn(*const ::std::os::raw::c_char, *const ::std::os::raw::c_char, *const ::std::os::raw::c_char, u32) -> Option<PVirtualChannelEntryFn>;
    extern "C" {
        fn freerdp_register_addin_provider(
            provider: Option<LoadAddinEntryFn>,
            flags: u32,
        ) -> ::std::os::raw::c_int;
        fn freerdp_channels_load_static_addin_entry(
            name: *const ::std::os::raw::c_char,
            subsystem: *const ::std::os::raw::c_char,
            entry_type: *const ::std::os::raw::c_char,
            flags: u32,
        ) -> Option<PVirtualChannelEntryFn>;
    }
    ADDIN_PROVIDER_REGISTERED.get_or_init(|| {
        // SAFETY：<freerdp/addin.h> FREERDP_API；全局一次性注册（内部空指针写）。
        let rc = unsafe { freerdp_register_addin_provider(Some(freerdp_channels_load_static_addin_entry), 0) };
        if rc != 0 {
            eprintln!("[addin] freerdp_register_addin_provider -> {rc}");
        }
    });
}

#[cfg(not(windows))]
fn register_static_addin_provider() {
    // 共享库构建下 addin loader 可 LoadLibrary 解析，无需注册。
}

/// Winsock 初始化（一次性）。getaddrinfo 在 Windows 上要求先 WSAStartup；
/// Rust std 只在自身用到网络 API 时才懒初始化 Winsock，FreeRDP 不做这件事
/// —— 任何进程内首次 getaddrinfo 直接 WSANOTINITIALISED，表现为
/// ERRCONNECT_DNS_NAME_NOT_FOUND（实测 127.0.0.1 也报 DNS 错误）。
#[cfg(windows)]
static WSA_STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

#[cfg(windows)]
fn ensure_winsock_started() {
    #[link(name = "ws2_32")]
    extern "system" {
        fn WSAStartup(wVersionRequested: u16, lpWSAData: *mut std::ffi::c_void) -> i32;
    }
    WSA_STARTED.get_or_init(|| {
        // SAFETY：ws2_32 标准初始化；WSADATA（约 400 字节）栈上缓冲只写。
        const WINSOCK_VERSION_2_2: u16 = (2 << 8) | 2; // MAKEWORD(2,2)
        let mut data = [0u8; 512];
        let rc = unsafe { WSAStartup(WINSOCK_VERSION_2_2, data.as_mut_ptr() as *mut _) };
        if rc != 0 {
            eprintln!("[winsock] WSAStartup -> {rc}");
        }
    });
}

/// `freerdp_get_last_error` / `freerdp_get_last_error_string`
/// （<freerdp/freerdp.h> 513-515 行，2.11.2 已核实存在；freerdp2-sys
/// 0.2.0 未绑定，手写 extern，符号由已链接的 freerdp2.lib 提供）。
extern "C" {
    fn freerdp_get_last_error(context: *mut sys::rdpContext) -> u32;
    fn freerdp_get_last_error_string(error: u32) -> *const ::std::os::raw::c_char;
}

// ---------------------------------------------------------------------------
// freerdp_channels_post_connect 核实结论（2026-11，transport failed 0x2000D 排查）
//
// - 符号存在于静态库 freerdp2.lib（二进制扫描确认），但 vcpkg 2.11.2 安装头
//   `freerdp/channels/channels.h` 与 `freerdp/client/channels.h` 均**未声明**
//   该符号（手写 extern 才能链接）。
// - 对照 2.11.2 源码 libfreerdp/core/freerdp.c 的 freerdp_connect()：
//   在 PostConnect 回调返回 TRUE 之后，FreeRDP **内部已经调用**
//   `freerdp_channels_post_connect(instance->context->channels, instance)`
//   （失败即置 FREERDP_ERROR_POST_CONNECT_FAILED）。
// - 因此应用层**不得**再手动调用它：二次调用会对同一 rdpChannels 重复执行
//   通道初始化/ChannelConnected 发布，cliprdr 等插件会被二次初始化，反而破坏
//   连接。"连接成功后缺 channels_post_connect 导致 transport failed" 的假设
//   对 2.11.2 不成立；真实原因需经下方 WLog 文件日志精确定位。
// ---------------------------------------------------------------------------

/// WinPR WLog 文件日志环境（connect 入口一次性设置，必须早于 FreeRDP 首次
/// 使用 —— WLog 在首次日志输出时才读环境变量初始化 appender）。
///
/// - WLOG_LEVEL=DEBUG / WLOG_APPENDER=FILE / WLOG_FILE_PATH=<应用数据目录>\freerdp.log
/// - 日志路径与 ConfigPath 同策略：优先 %APPDATA%\EasyRDP，不可写退回
///   CARGO_MANIFEST_DIR\target（本机调试）。目录必须先建好，File appender
///   不会自动建目录。
/// 关键实现细节（2026-11 对照 FreeRDP 2.11.2 源码核实）：
/// - WLog 读环境变量用 Win32 `GetEnvironmentVariableA`（wlog.c
///   WLog_InitializeRoot），`std::env::set_var`（SetEnvironmentVariableW）
///   写的是同一环境块，有效；CRT `_putenv` 是另一套环境块，无效。
/// - 本 vcpkg x64-windows-static-md 构建实测：FILE appender 即使配置正确
///   （或缺省 %TEMP%\wlog）也**静默不出文件**（Open 阶段失败被吞），属该
///   静态构建的缺陷；实机诊断时可用外部环境变量 WLOG_APPENDER=CONSOLE
///   观察控制台 DEBUG 日志。此处仍按源码语义设置正确变量，换共享库构建/
///   修复后的 winpr 即可落盘。
#[cfg(windows)]
static WLOG_ENV_CONFIGURED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

#[cfg(windows)]
fn setup_wlog_file_logging() {
    WLOG_ENV_CONFIGURED.get_or_init(|| {
        let dir = std::env::var("APPDATA")
            .ok()
            .map(|d| std::path::PathBuf::from(d).join("EasyRDP"))
            .filter(|p| std::fs::create_dir_all(p).is_ok())
            .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
        let log = dir.join("freerdp.log");
        if std::env::var_os("WLOG_LEVEL").is_none() {
            std::env::set_var("WLOG_LEVEL", "DEBUG");
        }
        if std::env::var_os("WLOG_APPENDER").is_none() {
            std::env::set_var("WLOG_APPENDER", "FILE");
        }
        if std::env::var_os("WLOG_FILEAPPENDER_OUTPUT_FILE_PATH").is_none() {
            std::env::set_var("WLOG_FILEAPPENDER_OUTPUT_FILE_PATH", dir.to_string_lossy().as_ref());
        }
        if std::env::var_os("WLOG_FILEAPPENDER_OUTPUT_FILE_NAME").is_none() {
            std::env::set_var("WLOG_FILEAPPENDER_OUTPUT_FILE_NAME", "freerdp.log");
        }
        eprintln!("[wlog] FreeRDP 日志输出到 {}", log.display());
    });
}

#[cfg(not(windows))]
fn setup_wlog_file_logging() {
    // 非 Windows 构建沿用默认 console appender（α 不做文件日志）
}

/// 读取 FreeRDP 最后错误（数字 + 字符串），供 Disconnected 事件精确归因。
fn last_error_reason(ctx: *mut sys::rdpContext) -> String {
    if ctx.is_null() {
        return String::new();
    }
    // SAFETY：<freerdp/freerdp.h> FREERDP_API；context 在释放前有效。
    unsafe {
        let code = freerdp_get_last_error(ctx);
        if code == 0 {
            return String::new();
        }
        let text = {
            let p = freerdp_get_last_error_string(code);
            if p.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        };
        if text.is_empty() {
            format!(" [last_error={code} (0x{code:X})]")
        } else {
            format!(" [last_error={code} (0x{code:X}): {text}]")
        }
    }
}

/// 解析 "VK:0xNN" 风格的键码字符串（大小写均可），返回 Windows VK 码。
fn parse_vk_code(code: &str) -> Option<u32> {
    let s = code.trim();
    let hex = s
        .strip_prefix("VK:")
        .or_else(|| s.strip_prefix("vk:"))?
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    u32::from_str_radix(hex, 16).ok()
}

/// VK 码是否为扩展键（对应 winpr KBD4X 扩展表，`<winpr/input.h>` KBD4_X*：
/// 方向键/翻页/Insert/Delete/Home/End、右 Ctrl/右 Alt、Win/Apps、小键盘 /、
/// PrintScreen、Pause、多媒体/浏览器键、Power/Sleep）。
fn is_extended_vk(vk: u32) -> bool {
    matches!(vk,
        0x03           // VK_CANCEL
        | 0x13         // VK_PAUSE
        | 0x21..=0x28  // PRIOR NEXT END HOME LEFT UP RIGHT DOWN
        | 0x2C..=0x2E  // SNAPSHOT INSERT DELETE
        | 0x5B..=0x5D  // LWIN RWIN APPS
        | 0x5F         // VK_SLEEP
        | 0x6F         // VK_DIVIDE（小键盘 /，扩展）
        | 0xA3 | 0xA5  // RCONTROL RMENU
        | 0xA6..=0xB7  // 浏览器/音量/媒体/启动键
        | 0xDE         // VK_POWER
        // 注意：VK_NUMLOCK(0x90)/VK_SCROLL(0x91) 走 winpr T 表【非 EXT】
        //（KBD4_T45=VK_NUMLOCK→0x45、KBD4_T46=VK_SCROLL→0x46，scancode.c
        // 核实）；此前误加 EXT 集合 → X 表反查失败 → 两键全灭（2026-11 修复）
    )
}

/// 最近一次 LED sync 的本地 NumLock 状态（2026-11 Numpad 确定性映射）：
/// 服务端对非 EXT 0x47-0x53 码位的解释取决于**服务端自己的 NumLock 状态**
///（sync LED 不一定被会话采纳）——本地 NumLock ON 时直接映射主键盘数字
/// 码位（0x02-0x0A），消除服务端状态不确定性；OFF 时发导航码位（0x47-0x53
/// 非 EXT，服务端按光标键解释=与本地行为一致）。
static LOCAL_NUMLOCK_ON: AtomicBool = AtomicBool::new(false);

/// VK 码 -> (RDP 扫描码低 8 位, 是否扩展键)。
///
/// FreeRDP 2.11.2 没有 `freerdp_get_rdp_scancode_from_virtual_key`（已核实
/// `<freerdp/locale/keyboard.h>` 只提供 x11 keycode 系列转换）；正确入口是
/// winpr 的 `GetVirtualScanCodeFromVirtualKeyCode(vk, type)`
/// （`<winpr/input.h>`，WINPR_API）。实现见 winpr/libwinpr/input/scancode.c：
/// vkcode 需按 KBD4X/KBD4T 表选择，**扩展键必须传 `vk | KBDEXT`** 才能命中
/// 扩展表（否则返回 0）。
///
/// 2026-11 修复（真机 debug.log 铁证：`VK 0x12 无对应扫描码`）：
/// winpr KBD4X 表**只登记 L/R 变体**（VK_LSHIFT=0xA0/VK_LCONTROL=0xA2/
/// VK_LMENU=0xA4/VK_R*），通用修饰键 VK_SHIFT=0x10/VK_CONTROL=0x11/
/// VK_MENU=0x12 查表返回 0 —— 前端 e.code 恰好发的是通用 VK（ControlLeft
/// → 0x11），导致所有组合键静默失败。这里把通用修饰键归一为 Left 变体
///（Windows 语义：无 L/R 区分的按下即 Left）。
fn vk_to_rdp_scancode(vk: u32) -> Option<(u8, bool)> {
    let base = vk & 0xFF;
    // 通用修饰键 → Left 变体（winpr KBD4X 只登记 L/R 变体）
    let base = match base {
        0x10 => 0xA0, // VK_SHIFT    -> VK_LSHIFT
        0x11 => 0xA2, // VK_CONTROL  -> VK_LCONTROL
        0x12 => 0xA4, // VK_MENU     -> VK_LMENU
        other => other,
    };
    // 扩展键判定：is_extended_vk 集合，或高 0x0100 伪标记（NumpadEnter：
    // VK_RETURN 0x0D 无 L/R 变体，用该标记区分小键盘 Enter → 0x1C EXT）
    let extended = is_extended_vk(base) || (vk & 0x0100) != 0;
    // Numpad0-9/Decimal：winpr 反查存在【首匹配歧义】——NUMPAD9 的 VK(0x69)
    // 在 KBD4T 中首匹配到 T49（=NUMPAD9 但码位 0x49 是 PgUp！），服务端会把
    // 数字键当 PgUp/End/方向键处理（真机 Numpad「全灭」根因）。码位是标准
    // AT 扫描码（非 EXT，NumLock 开=数字/关=光标，由服务端 LED 状态决定），
    // 直接硬编码，不经 winpr 反查（scancode.c KBD4_T47-T53 核实）。
    // Numpad0-9/Decimal：码位是标准 AT 扫描码（非 EXT），但服务端按**其自身
    // NumLock 状态**解释（sync LED 不一定被会话采纳——真机实证）。确定性方案：
    // 本地 NumLock ON（LOCAL_NUMLOCK_ON，随 LED sync 更新）→ 直接映射主键盘
    // 数字码位（0x02-0x0A，语义=数字，与服务端状态无关）；OFF → 发导航码位
    // （0x47-0x53 非 EXT，服务端按光标键解释=与本地行为一致）。
    if (0x60..=0x69).contains(&base) && LOCAL_NUMLOCK_ON.load(Ordering::SeqCst) {
        let scan: u8 = match base {
            0x60 => 0x0B, // 0
            0x61 => 0x02, // 1
            0x62 => 0x03, // 2
            0x63 => 0x04, // 3
            0x64 => 0x05, // 4
            0x65 => 0x06, // 5
            0x66 => 0x07, // 6
            0x67 => 0x08, // 7
            0x68 => 0x09, // 8
            _ => 0x0A,    // 9
        };
        return Some((scan, false));
    }
    if (0x60..=0x6E).contains(&base) {
        let scan: u8 = match base {
            0x60 => 0x52, // Numpad0
            0x61 => 0x4F, // Numpad1
            0x62 => 0x50, // Numpad2
            0x63 => 0x51, // Numpad3
            0x64 => 0x4B, // Numpad4
            0x65 => 0x4C, // Numpad5
            0x66 => 0x4D, // Numpad6
            0x67 => 0x47, // Numpad7
            0x68 => 0x48, // Numpad8
            0x69 => 0x49, // Numpad9
            0x6E => 0x53, // NumpadDecimal
            // 2026-11 Numpad +/-/* 修复：0x6A-0x6D 不在下方硬编码表内，
            // 此处 return None 会静默吞键（真机 +/- 无效根因）——改为穿透
            // 到 winpr 反查（KBD4_T37=VK_MULTIPLY→0x37 等，scancode.c 核实）。
            0x6A | 0x6B | 0x6C | 0x6D => {
                let sc = unsafe {
                    sys::GetVirtualScanCodeFromVirtualKeyCode(base, KEYBOARD_TYPE_AT_ENHANCED)
                };
                if sc == 0 {
                    return None;
                }
                return Some(((sc & 0xFF) as u8, false));
            }
            _ => return None,
        };
        return Some((scan, false));
    }
    let lookup = if extended { base | KBDEXT } else { base };
    // Numpad0-9 映射核实（scancode.c KBD4 表）：KBD4_T47-T53 =
    // NUMPAD7/8/9/4/6/1/2/3/0/DECIMAL（非 EXT，与方向键/Ins/Del 同码位、
    // 靠 EXT 区分）——Type-4 反查即可正确转换，无需 Type-7。
    let sc = unsafe { sys::GetVirtualScanCodeFromVirtualKeyCode(lookup, KEYBOARD_TYPE_AT_ENHANCED) };
    if sc == 0 {
        return None;
    }
    Some(((sc & 0xFF) as u8, sc & KBDEXT != 0))
}

/// 滚轮 delta（浏览器逻辑量，约 ±100/格，纵向正=向上、横向正=向右）
/// -> 完整 PTR_FLAGS（WHEEL/HWHEEL 基础位 + NEGATIVE + rotation）。
///
/// 2026-11 上滚无效根因修复：RDP rotation 字段按 **±120 单位**解释——
/// 浏览器 deltaY=±100 直接发送时，服务端对 +100 舍入为 0 格（无操作）
/// 而对 -100 舍入为 -1 格（下滚）→ 不对称（真机实证：下滚有效上滚无效）。
/// 修复：归一到最近 ±120 倍数再编码；负值 = NEGATIVE 标志 + `0x100+delta`
/// 8 位二补码（wfreerdp wf_event.c 同款）；|rotation| 钳 255（9 位上限），
/// 超出部分由前端高频事件分次送达。
fn encode_wheel_flags(delta: i32, horizontal: bool) -> Option<u16> {
    // 归一到最近 ±120 倍数（至少 1 格）
    if delta == 0 {
        return None;
    }
    let units = if delta > 0 {
        ((delta + 60) / 120).max(1) * 120
    } else {
        -(((-delta + 60) / 120).max(1) * 120)
    };
    let mut flags = if horizontal { PTR_FLAGS_HWHEEL } else { PTR_FLAGS_WHEEL };
    let mag = units.abs().clamp(120, 255) as i32; // 9 位有符号上限 255
    if units < 0 {
        flags |= PTR_FLAGS_WHEEL_NEGATIVE;
        flags |= (0x100 - mag) as u16; // 8 位二补码负值（wfreerdp 同款）
    } else {
        flags |= mag as u16;
    }
    Some(flags)
}

/// 鼠标按键名 -> PTR_FLAGS_BUTTONn。
fn mouse_button_flags(button: &str) -> Option<u16> {
    match button {
        "left" => Some(PTR_FLAGS_BUTTON1),
        "right" => Some(PTR_FLAGS_BUTTON2),
        "middle" => Some(PTR_FLAGS_BUTTON3),
        _ => None,
    }
}

/// instance 指针地址 -> 事件发送端。
///
/// FreeRDP 2.x 的连接/更新回调只带 `freerdp*`（无 user_data），
/// 用全局注册表把 C 回调路由回对应会话的事件通道。
/// 键为 `instance as usize`；会话 Drop/disconnect 时注销。
static EVENT_ROUTES: Mutex<Option<HashMap<usize, event_channel::Sender>>> = Mutex::new(None);

fn route_register(instance_addr: usize, tx: event_channel::Sender) {
    EVENT_ROUTES
        .lock()
        .expect("EVENT_ROUTES poisoned")
        .get_or_insert_with(HashMap::new)
        .insert(instance_addr, tx);
}

fn route_unregister(instance_addr: usize) {
    if let Some(map) = EVENT_ROUTES.lock().expect("EVENT_ROUTES poisoned").as_mut() {
        map.remove(&instance_addr);
    }
}

fn route_lookup(instance_addr: usize) -> Option<event_channel::Sender> {
    EVENT_ROUTES
        .lock()
        .expect("EVENT_ROUTES poisoned")
        .as_ref()
        .and_then(|m| m.get(&instance_addr).cloned())
}

/// ctx 地址 -> gdi_init 注册的原 `update->BitmapUpdate`（即 `gdi_bitmap_update`，
/// 见 libfreerdp/cache/bitmap.c 269 行 `update->BitmapUpdate = gdi_bitmap_update`）。
/// 我们的链式回调先调它完成 RLE 解压与 GDI 合成，再从 primary_buffer 抠 BGRA。
static GDI_BITMAP_PREV: Mutex<Option<HashMap<usize, sys::pBitmapUpdate>>> = Mutex::new(None);

fn gdi_prev_register(ctx_addr: usize, prev: sys::pBitmapUpdate) {
    GDI_BITMAP_PREV
        .lock()
        .expect("GDI_BITMAP_PREV poisoned")
        .get_or_insert_with(HashMap::new)
        .insert(ctx_addr, prev);
}

fn gdi_prev_lookup(ctx_addr: usize) -> sys::pBitmapUpdate {
    GDI_BITMAP_PREV
        .lock()
        .expect("GDI_BITMAP_PREV poisoned")
        .as_ref()
        .and_then(|m| m.get(&ctx_addr).copied())
        .flatten()
}

fn gdi_prev_unregister(ctx_addr: usize) {
    if let Some(map) = GDI_BITMAP_PREV.lock().expect("GDI_BITMAP_PREV poisoned").as_mut() {
        map.remove(&ctx_addr);
    }
}

// ---------------------------------------------------------------------------
// CLIPRDR 剪贴板双向同步（α：仅 CF_UNICODETEXT 文本）
//
// 通道加载（符号均经 vcpkg freerdp 2.11.2 真实头文件 + freerdp2-sys 0.2.0
// bindgen 输出核实，流程对照 FreeRDP 2.11.2 源码 wf_client.c / client/common/
// cmdline.c `freerdp_client_load_addins` / libfreerdp/core/client.c）：
// 1) connect 时在 `context.pubSub` 上订阅 "ChannelConnected" 事件
//    （`PubSub_Subscribe`，WINPR_API，导出于 winpr2.dll）。
// 2) on_pre_connect 内调用 `freerdp_client_load_addins(channels, settings)`
//    （`<freerdp/client/cmdline.h>` FREERDP_API）：RedirectClipboard=TRUE 时
//    把静态通道 "cliprdr" 经 `VirtualChannelEntryEx` 路径加载（注意不能用
//    `freerdp_channels_load_plugin` —— 它走非 ENTRYEX 路径，而 2.11 的 cliprdr
//    静态入口是 Ex 变体，会以错误布局调用通道初始化）。
// 3) `freerdp_channels_post_connect`（freerdp_connect 内部调用）对每个已加载
//    通道发布 ChannelConnected，`e->pInterface` 即插件 calloc 的
//    `CliprdrClientContext`（cliprdr_main.c VirtualChannelEntryEx）。
// 4) 我们把 Server* 回调换成自己的实现；Client* 回调保留插件自带实现
//    （负责把数据序列化发送到 SVC 通道）。
//
// 远端 -> 本地：ServerFormatList（含 CF_UNICODETEXT=13）-> ClientFormatDataRequest
// -> ServerFormatDataResponse -> 解码 UTF-16LE -> 写本地 Win32 剪贴板 + 推
// FramebufferEvent::ClipboardText。
// 本地 -> 远端：send_clipboard_text 存入本地剪贴板缓存 -> ClientFormatList 公告
// CF_UNICODETEXT -> 远端粘贴时 ServerFormatDataRequest -> ClientFormatDataResponse
// 回传 UTF-16LE 数据。CLIPRDR 不可用时降级方案 B（typing.rs VK 键入序列）。
// ---------------------------------------------------------------------------

/// Windows 标准剪贴板格式 CF_UNICODETEXT（`<winuser.h>` = 13；CLIPRDR 沿用
/// Windows 标准格式 ID，2.11.2 `<freerdp/channels/cliprdr.h>` 无 CB_FORMAT_UNICODETEXT）。
const CF_UNICODETEXT: u32 = 13;

/// 本地剪贴板文本缓存：instance 地址 -> 文本（存在即已向远端公告）。
/// 键为 `instance as usize`；会话断开时移除。
static CLIPBOARD_TEXT: Mutex<Option<HashMap<usize, String>>> = Mutex::new(None);

fn clipboard_text_store(instance_addr: usize, text: String) {
    CLIPBOARD_TEXT
        .lock()
        .expect("CLIPBOARD_TEXT poisoned")
        .get_or_insert_with(HashMap::new)
        .insert(instance_addr, text);
}

fn clipboard_text_lookup(instance_addr: usize) -> Option<String> {
    CLIPBOARD_TEXT
        .lock()
        .expect("CLIPBOARD_TEXT poisoned")
        .as_ref()
        .and_then(|m| m.get(&instance_addr).cloned())
}

/// 已接入的 CliprdrClientContext：instance 地址 -> 上下文指针（存 usize 以便
/// 放入 static Mutex；原始指针类型不满足 Send 约束）。
/// 仅在 connected 期间有效（run_session 收尾统一移除）。
static CLIPRDR_CTX: Mutex<Option<HashMap<usize, usize>>> = Mutex::new(None);

fn cliprdr_ctx_register(instance_addr: usize, ctx: *mut sys::CliprdrClientContext) {
    CLIPRDR_CTX
        .lock()
        .expect("CLIPRDR_CTX poisoned")
        .get_or_insert_with(HashMap::new)
        .insert(instance_addr, ctx as usize);
}

fn cliprdr_ctx_lookup(instance_addr: usize) -> *mut sys::CliprdrClientContext {
    CLIPRDR_CTX
        .lock()
        .expect("CLIPRDR_CTX poisoned")
        .as_ref()
        .and_then(|m| m.get(&instance_addr).copied())
        .map_or(std::ptr::null_mut(), |p| p as *mut sys::CliprdrClientContext)
}

fn cliprdr_unregister(instance_addr: usize) {
    if let Some(map) = CLIPRDR_CTX.lock().expect("CLIPRDR_CTX poisoned").as_mut() {
        map.remove(&instance_addr);
    }
    if let Some(map) = CLIPBOARD_TEXT.lock().expect("CLIPBOARD_TEXT poisoned").as_mut() {
        map.remove(&instance_addr);
    }
}

/// `ChannelConnectedEventArgs`（`<freerdp/event.h>` DEFINE_EVENT_BEGIN(ChannelConnected)）：
/// `{ wEventArgs e; const char* name; void* pInterface; UINT_PTR UserData; }`。
#[repr(C)]
struct ChannelConnectedEventArgs {
    e: sys::wEventArgs,
    name: *const ::std::os::raw::c_char,
    p_interface: *mut ::std::os::raw::c_void,
    user_data: usize,
}

/// `psChannelConnectedEventHandler`（`<freerdp/event.h>`）展开的原始签名。
unsafe extern "C" fn on_channel_connected_raw(
    context: *mut ::std::os::raw::c_void,
    e: *mut sys::wEventArgs,
) {
    on_channel_connected(context, e as *mut ChannelConnectedEventArgs);
}

/// pubsub "ChannelConnected" 订阅回调（wfreerdp `wf_OnChannelConnectedEventHandler`
/// 的对应物）。cliprdr 插件的 `e->pInterface` 就是 `CliprdrClientContext`。
unsafe extern "C" fn on_channel_connected(
    context: *mut ::std::os::raw::c_void,
    e: *mut ChannelConnectedEventArgs,
) {
    let ctx = context as *mut sys::rdpContext;
    if ctx.is_null() || e.is_null() || (*e).name.is_null() {
        return;
    }
    let name = std::ffi::CStr::from_ptr((*e).name).to_bytes();
    // 通道初始化日志（EASYRDP_FRAME_DEBUG=1 时）：e2e 验证 drdynvc/rdpgfx
    // 通道协商结果用。
    if frame_debug_enabled() {
        let name_str = String::from_utf8_lossy(name);
        eprintln!("[gfx] ChannelConnected: {name_str}");
    }
    if name == b"rdpgfx" {
        return on_rdpgfx_channel_connected(ctx, (*e).p_interface);
    }
    if name != b"cliprdr" || (*e).p_interface.is_null() {
        return;
    }
    let instance = (*ctx).instance;
    if instance.is_null() {
        return;
    }
    let cliprdr = (*e).p_interface as *mut sys::CliprdrClientContext;
    // custom 字段 2.x 插件自身不使用（wf_cliprdr 才写它）——复用来携带
    // freerdp instance 指针，便于回调路由。
    (*cliprdr).custom = instance as *mut ::std::os::raw::c_void;
    (*cliprdr).ServerCapabilities = Some(on_cliprdr_server_capabilities);
    (*cliprdr).MonitorReady = Some(on_cliprdr_monitor_ready);
    (*cliprdr).ServerFormatList = Some(on_cliprdr_server_format_list);
    (*cliprdr).ServerFormatDataRequest = Some(on_cliprdr_server_format_data_request);
    (*cliprdr).ServerFormatDataResponse = Some(on_cliprdr_server_format_data_response);
    cliprdr_ctx_register(instance as usize, cliprdr);
}

/// RDPEGFX 通道接通（drdynvc 加载 rdpgfx 插件后经 PubSub 送达，name=="rdpgfx"，
/// `pInterface` 即 `RdpgfxClientContext`）。对应 wfreerdp `wf_channels.c`
/// `wf_OnChannelConnectedEventHandler` 的 gfx 分支：把 GDI 合成层接到 GFX
/// 表面管线（gdi_graphics_pipeline_init 注册 StartFrame/EndFrame/
/// SurfaceCommand 等 gdi_* 处理器）。GFX 输出经 gdi_OutputUpdate 走
/// update_begin_paint/update_end_paint（即我们的 BeginPaint/EndPaint 链式
/// 回调），帧管线无需改动；服务端若不选 GFX 则仍走 BitmapUpdate 路径。
unsafe fn on_rdpgfx_channel_connected(ctx: *mut sys::rdpContext, gfx: *mut ::std::os::raw::c_void) {
    if ctx.is_null() || gfx.is_null() {
        return;
    }
    let gdi = (*ctx).gdi;
    if gdi.is_null() {
        eprintln!("[gfx] rdpgfx connected but gdi not initialized; skip gfx pipeline");
        return;
    }
    // SAFETY：`<freerdp/gdi/gfx.h>` FREERDP_API；gdi_init 已在 PostConnect
    // 完成，gfx 为 drdynvc 插件 calloc 的 RdpgfxClientContext。
    if sys::gdi_graphics_pipeline_init(gdi, gfx as *mut sys::RdpgfxClientContext) == 0 {
        eprintln!("[gfx] gdi_graphics_pipeline_init failed");
    } else if frame_debug_enabled() {
        eprintln!("[gfx] gdi_graphics_pipeline_init ok (RDPEGFX -> GDI surface pipeline)");
    }
}

/// pubsub "ChannelDisconnected"：清除 CLIPRDR 上下文登记。
unsafe extern "C" fn on_channel_disconnected_raw(
    context: *mut ::std::os::raw::c_void,
    e: *mut sys::wEventArgs,
) {
    let ctx = context as *mut sys::rdpContext;
    let e = e as *mut ChannelConnectedEventArgs;
    if ctx.is_null() || e.is_null() || (*e).name.is_null() {
        return;
    }
    let name = std::ffi::CStr::from_ptr((*e).name).to_bytes();
    if name == b"rdpgfx" {
        // 解除 GDI <-> GFX 表面管线绑定（gfx.h gdi_graphics_pipeline_uninit）。
        let gdi = (*ctx).gdi;
        if !gdi.is_null() && !(*e).p_interface.is_null() {
            sys::gdi_graphics_pipeline_uninit(
                gdi,
                (*e).p_interface as *mut sys::RdpgfxClientContext,
            );
        }
        return;
    }
    if name != b"cliprdr" {
        return;
    }
    let instance = (*ctx).instance;
    if instance.is_null() {
        return;
    }
    cliprdr_unregister(instance as usize);
}

// -- cliprdr Client* 调用的极小化封装（数据结构栈上构造，插件同步序列化） --
// 注意：bindgen 把 CLIPRDR 常量导出为 u32，而结构体 msgType/msgFlags 等
// 字段是 UINT16 —— 统一 `as u16` 收窄。

/// 发送 ClientCapabilities（CB_CAPS_VERSION_2 + 长格式名支持），
/// 对应 wf_cliprdr.c `wf_cliprdr_send_client_capabilities`。
unsafe fn cliprdr_send_client_capabilities(
    context: *mut sys::CliprdrClientContext,
) -> Result<(), RdpError> {
    let mut general = sys::CLIPRDR_GENERAL_CAPABILITY_SET {
        capabilitySetType: sys::CB_CAPSTYPE_GENERAL as u16,
        capabilitySetLength: sys::CB_CAPSTYPE_GENERAL_LEN as u16,
        version: sys::CB_CAPS_VERSION_2,
        generalFlags: sys::CB_USE_LONG_FORMAT_NAMES,
    };
    let caps = sys::CLIPRDR_CAPABILITIES {
        msgType: sys::CB_CLIP_CAPS as u16,
        msgFlags: 0,
        dataLen: 4 + 12,
        cCapabilitiesSets: 1,
        capabilitySets: &mut general as *mut _ as *mut sys::CLIPRDR_CAPABILITY_SET,
    };
    let Some(f) = (*context).ClientCapabilities else {
        return Err(RdpError("cliprdr ClientCapabilities 回调缺失".into()));
    };
    if f(context, &caps) != 0 {
        return Err(RdpError("发送 CLIPRDR ClientCapabilities 失败".into()));
    }
    Ok(())
}

/// 发送 FormatList 公告：仅 CF_UNICODETEXT 一个格式。
unsafe fn cliprdr_announce_local_text(
    context: *mut sys::CliprdrClientContext,
) -> Result<(), RdpError> {
    let mut format = sys::CLIPRDR_FORMAT {
        formatId: CF_UNICODETEXT,
        formatName: std::ptr::null_mut(),
    };
    let list = sys::CLIPRDR_FORMAT_LIST {
        msgType: sys::CB_FORMAT_LIST as u16,
        msgFlags: 0,
        dataLen: 4, // 单格式无名字时 PDU 体为 4 字节 formatId
        numFormats: 1,
        formats: &mut format,
    };
    let Some(f) = (*context).ClientFormatList else {
        return Err(RdpError("cliprdr ClientFormatList 回调缺失".into()));
    };
    if f(context, &list) != 0 {
        return Err(RdpError("发送 CLIPRDR FormatList 失败".into()));
    }
    Ok(())
}

/// 请求远端回传 CF_UNICODETEXT 数据。
unsafe fn cliprdr_request_unicode_text(
    context: *mut sys::CliprdrClientContext,
) -> Result<(), RdpError> {
    let req = sys::CLIPRDR_FORMAT_DATA_REQUEST {
        msgType: sys::CB_FORMAT_DATA_REQUEST as u16,
        msgFlags: 0,
        dataLen: 4,
        requestedFormatId: CF_UNICODETEXT,
    };
    let Some(f) = (*context).ClientFormatDataRequest else {
        return Err(RdpError("cliprdr ClientFormatDataRequest 回调缺失".into()));
    };
    if f(context, &req) != 0 {
        return Err(RdpError("发送 CLIPRDR FormatDataRequest 失败".into()));
    }
    Ok(())
}

/// 回应远端的数据请求（成功带数据 / 失败空响应）。
unsafe fn cliprdr_respond_data(
    context: *mut sys::CliprdrClientContext,
    ok: bool,
    data: &[u8],
) -> Result<(), RdpError> {
    let resp = sys::CLIPRDR_FORMAT_DATA_RESPONSE {
        msgType: sys::CB_FORMAT_DATA_RESPONSE as u16,
        msgFlags: if ok {
            sys::CB_RESPONSE_OK as u16
        } else {
            sys::CB_RESPONSE_FAIL as u16
        },
        dataLen: data.len() as u32,
        requestedFormatData: data.as_ptr(),
    };
    let Some(f) = (*context).ClientFormatDataResponse else {
        return Err(RdpError("cliprdr ClientFormatDataResponse 回调缺失".into()));
    };
    if f(context, &resp) != 0 {
        return Err(RdpError("发送 CLIPRDR FormatDataResponse 失败".into()));
    }
    Ok(())
}

// -- extern "C" 回调（cliprdr Server*，均在 FreeRDP 内部线程被调用） --

/// 服务端能力协商：α 版不开启文件剪贴等扩展标志，直接 OK。
unsafe extern "C" fn on_cliprdr_server_capabilities(
    _context: *mut sys::CliprdrClientContext,
    _caps: *const sys::CLIPRDR_CAPABILITIES,
) -> sys::UINT {
    0 // CHANNEL_RC_OK
}

/// `MonitorReady`：服务端就绪 -> 回发 ClientCapabilities；若本地已有文本则公告。
unsafe extern "C" fn on_cliprdr_monitor_ready(
    context: *mut sys::CliprdrClientContext,
    _mr: *const sys::CLIPRDR_MONITOR_READY,
) -> sys::UINT {
    let instance = (*context).custom as usize;
    if cliprdr_send_client_capabilities(context).is_err() {
        return 1; // ERROR_INTERNAL_ERROR：公告失败不影响连接
    }
    if clipboard_text_lookup(instance).is_some() {
        let _ = cliprdr_announce_local_text(context);
    }
    0
}

/// 远端公告格式列表：含 CF_UNICODETEXT 时请求数据。
unsafe extern "C" fn on_cliprdr_server_format_list(
    context: *mut sys::CliprdrClientContext,
    list: *const sys::CLIPRDR_FORMAT_LIST,
) -> sys::UINT {
    if list.is_null() || (*list).formats.is_null() {
        return 0;
    }
    let formats = std::slice::from_raw_parts((*list).formats, (*list).numFormats as usize);
    let n = (*list).numFormats;
    let has_text = formats.iter().any(|f| f.formatId == CF_UNICODETEXT);
    crate::debug_log(&format!(
        "[cliprdr] 远端 FormatList: numFormats={n} 含CF_UNICODETEXT={has_text}"
    ));
    if !has_text {
        return 0; // α 只处理文本格式
    }
    match cliprdr_request_unicode_text(context) {
        Ok(()) => {
            crate::debug_log("[cliprdr] 已发 FormatDataRequest(CF_UNICODETEXT)");
            0
        }
        Err(e) => {
            crate::debug_log(&format!("[cliprdr] FormatDataRequest 发送失败: {}", e.0));
            1
        }
    }
}

/// 远端要数据：本地缓存有文本则回 UTF-16LE（含结尾 NUL），否则 FAIL。
unsafe extern "C" fn on_cliprdr_server_format_data_request(
    context: *mut sys::CliprdrClientContext,
    req: *const sys::CLIPRDR_FORMAT_DATA_REQUEST,
) -> sys::UINT {
    let instance = (*context).custom as usize;
    if req.is_null() || (*req).requestedFormatId != CF_UNICODETEXT {
        let _ = cliprdr_respond_data(context, false, &[]);
        return 0;
    }
    match clipboard_text_lookup(instance) {
        Some(text) => {
            let mut utf16: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
            utf16.extend_from_slice(&0u16.to_le_bytes()); // 结尾 NUL
            match cliprdr_respond_data(context, true, &utf16) {
                Ok(()) => 0,
                Err(_) => 1,
            }
        }
        None => {
            let _ = cliprdr_respond_data(context, false, &[]);
            0
        }
    }
}

/// 远端数据回包：解码 UTF-16LE -> 写本地 Win32 剪贴板 + 推 ClipboardText 事件。
unsafe extern "C" fn on_cliprdr_server_format_data_response(
    context: *mut sys::CliprdrClientContext,
    resp: *const sys::CLIPRDR_FORMAT_DATA_RESPONSE,
) -> sys::UINT {
    if resp.is_null() || (*resp).msgFlags != sys::CB_RESPONSE_OK as u16 || (*resp).dataLen == 0 {
        crate::debug_log(&format!(
            "[cliprdr] FormatDataResponse 异常: null={} flags={:#x} len={}",
            resp.is_null(),
            if resp.is_null() { 0 } else { (*resp).msgFlags as _ },
            if resp.is_null() { 0 } else { (*resp).dataLen },
        ));
        return 0; // 远端取数据失败：静默忽略
    }
    let len = (*resp).dataLen as usize;
    let raw = std::slice::from_raw_parts((*resp).requestedFormatData, len);
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .take_while(|&u| u != 0) // 截断到结尾 NUL
        .collect();
    let text = String::from_utf16_lossy(&units);
    if text.is_empty() {
        return 0;
    }
    crate::debug_log(&format!(
        "[cliprdr] 远端文本到达 len={len} chars={}——写本地剪贴板",
        text.chars().count()
    ));
    let instance = (*context).custom as usize;
    match set_local_clipboard_text(&text) {
        Ok(()) => {
            crate::debug_log("[cliprdr] 本地剪贴板写入成功（CLIPSYNC 回环防护已登记）");
            crate::clip_sync_note(&text);
        }
        Err(e) => {
            crate::debug_log(&format!("[cliprdr] 本地剪贴板写入失败: {}", e.0));
        }
    }
    if let Some(tx) = route_lookup(instance) {
        let _ = tx.send(FramebufferEvent::ClipboardText(text));
    }
    0
}

// -- 本地 Win32 剪贴板写入（远端 -> 本地落盘） --

/// 把文本写入本地系统剪贴板（CF_UNICODETEXT）。非 Windows（native 非 Win 构建）
/// 为空实现（仅推事件，不写本地剪贴板）。
#[cfg(windows)]
fn set_local_clipboard_text(text: &str) -> Result<(), RdpError> {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    // OpenClipboard 可能在其他程序占用剪贴板时失败：短重试（对齐
    // wf_cliprdr.c try_open_clipboard 的重试策略，窗口句柄传 NULL）。
    let mut opened = false;
    for _ in 0..10 {
        // SAFETY：user32 API；hwnd=NULL 允许任意线程打开剪贴板。
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            opened = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !opened {
        return Err(RdpError("OpenClipboard 失败（剪贴板被占用）".into()));
    }
    let result = (|| -> Result<(), RdpError> {
        // SAFETY：剪贴板已打开；GlobalAlloc(GMEM_MOVEABLE) 分配的内存块在
        // SetClipboardData 成功后由系统接管（无需 GlobalFree，失败路径泄漏
        // 一块可接受的小内存，α 版从简）。
        unsafe {
            if EmptyClipboard() == 0 {
                return Err(RdpError("EmptyClipboard 失败".into()));
            }
            let mut utf16: Vec<u16> = text.encode_utf16().collect();
            utf16.push(0); // 结尾 NUL
            let bytes = utf16.len() * 2;
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if h.is_null() {
                return Err(RdpError("GlobalAlloc 失败".into()));
            }
            let dst = GlobalLock(h);
            if dst.is_null() {
                return Err(RdpError("GlobalLock 失败".into()));
            }
            std::ptr::copy_nonoverlapping(utf16.as_ptr() as *const u8, dst as *mut u8, bytes);
            let _ = GlobalUnlock(h);
            if SetClipboardData(CF_UNICODETEXT, h).is_null() {
                return Err(RdpError("SetClipboardData(CF_UNICODETEXT) 失败".into()));
            }
        }
        Ok(())
    })();
    // SAFETY：与 OpenClipboard 配对。
    unsafe { CloseClipboard() };
    result
}

#[cfg(not(windows))]
fn set_local_clipboard_text(_text: &str) -> Result<(), RdpError> {
    Ok(()) // α：非 Windows native 构建不落本地剪贴板（仅推 ClipboardText 事件）
}

// ---------------------------------------------------------------------------
// extern "C" 回调（注册进 FreeRDP instance / update 结构）
// ---------------------------------------------------------------------------

/// `pPreConnect`（C 原型见 `<freerdp/freerdp.h>`：`typedef BOOL (*pPreConnect)(freerdp*)`）。
///
/// α 版在此加载 CLIPRDR 静态通道（剪贴板双向同步的前置条件）：
/// `freerdp_client_load_addins(channels, settings)` 在 `RedirectClipboard=TRUE`
/// 时把静态插件 "cliprdr" 以 ENTRYEX 路径装载（等价 wfreerdp wf_pre_connect）。
/// α 范围只有剪贴板：先清掉 printers/drives/audio 等其余重定向开关，
/// 避免 load_addins 连带加载 rdpdr/rdpsnd（后续版本按需放开）。
unsafe extern "C" fn on_pre_connect(instance: *mut sys::freerdp) -> sys::BOOL {
    let ctx = (*instance).context;
    if ctx.is_null() {
        return FALSE;
    }
    let settings = (*instance).settings;
    let channels = (*ctx).channels;
    if settings.is_null() || channels.is_null() {
        return FALSE;
    }
    // SAFETY：均为 <freerdp/settings.h> 导出 setter；仅写本地 settings 结构。
    // NetworkAutoDetect/SupportHeartbeatPdu/SupportMultitransport（RDP8 特性，
    // 2.11 默认 TRUE）必须关掉：freerdp_client_load_addins（cmdline.c）在三者
    // 任一为真时强制 DeviceRedirection=TRUE，进而加载静态通道 rdpdr ——
    // 本 vcpkg 构建未注册该静态入口，load_addins 会失败（"Failed to load
    // channel rdpdr"）→ PreConnect 失败 → ERRCONNECT_PRE_CONNECT_FAILED。
    for (id, on) in [
        (sys::FreeRDP_AudioPlayback, false),
        (sys::FreeRDP_AudioCapture, false),
        (sys::FreeRDP_RedirectPrinters, false),
        (sys::FreeRDP_RedirectDrives, false),
        (sys::FreeRDP_RedirectHomeDrive, false),
        (sys::FreeRDP_RedirectSerialPorts, false),
        (sys::FreeRDP_RedirectSmartCards, false),
        (sys::FreeRDP_NetworkAutoDetect, false),
        (sys::FreeRDP_SupportHeartbeatPdu, false),
        (sys::FreeRDP_SupportMultitransport, false),
        (sys::FreeRDP_DeviceRedirection, false),
    ] {
        sys::freerdp_settings_set_bool(settings, id as usize as _, if on { TRUE } else { FALSE });
    }
    // RDPEGFX 图形管线（对齐 mstsc 现代图形路径）：SupportGraphicsPipeline=TRUE
    // 使 load_addins 把 "rdpgfx" 加入 DynamicChannelArray 并设置
    // SupportDynamicChannels=TRUE，进而加载静态通道 drdynvc（cmdline.c
    // freerdp_client_load_addins 尾段逻辑）；主连接能力集会声明
    // RNS_UD_CS_SUPPORT_DYNVC_GFX_PROTOCOL（capinfos 由该 setting 驱动）。
    // 不启用 H264/AVC444（vcpkg 静态库无 openh264/MF 解码后端，vendor 的
    // rdpgfx 未定义 WITH_GFX_H264）：能力通告按源码路径自动带
    // AVC_DISABLED（RDPGFX_CAPS_FLAG_AVC_DISABLED，CAPVERSION_10+）。
    //
    // 2026-11 显示回归修复（真机实测：GFX 默认开 → 0 帧/纯黑）：GFX 模式下
    // 服务端走 surface 编码（gdi_OutputUpdate），**不触发 BitmapUpdate 回调**
    // → 挂在 BitmapUpdate 上的帧合成/发射链收不到数据 → 0 帧。改为
    // **默认关闭（走已验证的 BitmapUpdate 位图路径），GFX 为 opt-in**：
    // EASYRDP_ENABLE_GFX=1 时启用，同时 on_update_end_paint 会把合成缓冲
    // 以全帧发出（GFX 帧发射接线，见该函数）。
    let gfx_enabled = std::env::var("EASYRDP_ENABLE_GFX").ok().as_deref() == Some("1");
    if gfx_enabled {
        for (id, on) in [
            (sys::FreeRDP_SupportGraphicsPipeline, true),
            (sys::FreeRDP_GfxH264, false),
            (sys::FreeRDP_GfxAVC444, false),
            (sys::FreeRDP_GfxAVC444v2, false),
            (sys::FreeRDP_GfxProgressive, true),
            (sys::FreeRDP_GfxThinClient, false),
            (sys::FreeRDP_GfxSmallCache, false),
        ] {
            sys::freerdp_settings_set_bool(settings, id as usize as _, if on { TRUE } else { FALSE });
        }
        // RDPEGFX 要求 32bpp 会话色深（mstsc /gfx 同样强制；实测真机
        // 16bpp + GFX 声明 = 服务端全静默：不发位图更新也不开 RDPEGFX DVC）。
        sys::freerdp_settings_set_uint32(
            settings,
            sys::FreeRDP_ColorDepth as usize as _,
            32,
        );
    }
    // SAFETY：<freerdp/client/cmdline.h> FREERDP_API；仅遍历 settings 并加载
    // 内置静态插件（cliprdr），无 I/O。
    if sys::freerdp_client_load_addins(channels, settings) == 0 {
        return FALSE;
    }
    TRUE
}

/// `pPostConnect`（`<freerdp/freerdp.h>`：`typedef BOOL (*pPostConnect)(freerdp*)`）。
///
/// 1) `gdi_init(instance, PIXEL_FORMAT_BGRA32)`：初始化 GDI 合成层并把
///    `update->BitmapUpdate` 接到 `gdi_bitmap_update`（服务端位图解压+合成）。
/// 2) 把 `BitmapUpdate` 换成链式回调 [`on_bitmap_update`]（先 GDI 合成、
///    再从 primary_buffer 抠 BGRA32 脏矩形）。
/// 3) 注册 PointerPosition 回调（GDI 不接管 pointer update）。
///
/// 注意：本回调在 FreeRDP 内部线程被调用，只做指针写入与 C 侧注册。
/// CLIPRDR 无需在此处理：插件的 CliprdrClientContext 由 "ChannelConnected"
/// pubsub 事件在 freerdp_channels_post_connect 阶段送达（freerdp_connect
/// 在 PostConnect 返回后内部调用它）。
unsafe extern "C" fn on_post_connect(instance: *mut sys::freerdp) -> sys::BOOL {
    let ctx = (*instance).context;
    if ctx.is_null() {
        return FALSE;
    }
    // rdpContext.update: *mut rdpUpdate（`<freerdp/update.h>`）
    let update = (*ctx).update;
    if update.is_null() {
        return FALSE;
    }
    // SAFETY：gdi_init 是 `<freerdp/gdi/gdi.h>` 导出 API；失败时其内部
    // 已自清理（gdi_init_ex fail 分支调用 gdi_free），返回 FALSE 即可。
    if sys::gdi_init(instance, PIXEL_FORMAT_BGRA32) == 0 {
        return FALSE;
    }
    // gdi_init（经 bitmap_cache_register_callbacks）已把 BitmapUpdate 设为
    // gdi_bitmap_update；保存后替换为链式回调。同时登记全帧合成缓冲与
    // 帧发射线程（路由副本供发射线程使用）。
    composer_register(ctx as usize);
    ctx_ptr_register(ctx as usize, ctx); // GHOSTFIX2：全帧快照直读 primary_buffer
    if let Some(tx) = route_lookup(instance as usize) {
        ROUTE_BY_CTX
            .lock()
            .expect("ROUTE_BY_CTX poisoned")
            .get_or_insert_with(HashMap::new)
            .insert(ctx as usize, tx);
    }
    emitter_register(ctx as usize);
    gdi_prev_register(ctx as usize, (*update).BitmapUpdate);
    (*update).BitmapUpdate = Some(on_bitmap_update);
    // rdpUpdate.pointer: *mut rdpPointerUpdate
    let pointer = (*update).pointer;
    if !pointer.is_null() {
        (*pointer).PointerPosition = Some(on_pointer_position);
        // 「缺省即 FALSE」的指针形状回调：不注册会在光标变化时掉线（见上方注释）。
        (*pointer).PointerSystem = Some(on_pointer_system);
        (*pointer).PointerColor = Some(on_pointer_color);
        (*pointer).PointerNew = Some(on_pointer_new);
        (*pointer).PointerCached = Some(on_pointer_cached);
        (*pointer).PointerLarge = Some(on_pointer_large);
    }
    // BeginPaint/EndPaint 成对注册：EndPaint 缺失时 update_end_paint 返回
    // FALSE -> fastpath_recv_updates -4 -> ACTIVE 后首个输出包掉线（实测真因）；
    // 只注册 EndPaint 又会漏掉 update_begin_paint 进入的 mux 临界区释放。
    (*update).BeginPaint = Some(on_update_begin_paint);
    (*update).EndPaint = Some(on_update_end_paint);
    // 其余「缺省即 FALSE」的 update 回调空实现。
    (*update).Palette = Some(on_update_palette);
    (*update).PlaySound = Some(on_update_play_sound);
    (*update).Synchronize = Some(on_update_synchronize);
    TRUE
}

/// 帧管线调试开关（环境变量 `EASYRDP_FRAME_DEBUG=1` 时启用；一次性读取缓存）。
/// 输出 gdi 上下文（width/height/stride/dstFormat）、协商后的 DesktopWidth/
/// Height/ColorDepth 以及每个 BITMAP_UPDATE 的脏矩形分布——用于无头诊断
/// 帧格式与分辨率协商结果。
static FRAME_DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn frame_debug_enabled() -> bool {
    *FRAME_DEBUG.get_or_init(|| std::env::var("EASYRDP_FRAME_DEBUG").ok().as_deref() == Some("1"))
}

/// 增量管线开关（2026-11 传输重构后重新默认启用）：
/// delta 禁用的原始原因是 base64 emit 传输丢帧（已定性并根治——帧改走
/// invoke 轮询拉取的可靠槽传输）。增量经 16ms 合并批 + 可靠槽下发，
/// 高频小更新解决拖拽残影/1FPS；EndPaint 全帧（100ms 节流+5s 兜底）
/// 继续作防漂移参考。EASYRDP_DISABLE_DELTAS=1 可强制全帧路径排障。
static DISABLE_DELTAS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn deltas_disabled() -> bool {
    *DISABLE_DELTAS.get_or_init(|| {
        std::env::var("EASYRDP_DISABLE_DELTAS").ok().as_deref() == Some("1")
    })
}

/// RDPEGFX 图形管线 opt-in 开关（默认关，见 apply_profile_settings 注释）。
/// 开启时帧发射改由 on_update_end_paint 驱动（GFX surface 合成不触发
/// BitmapUpdate），BitmapUpdate 仅做合成缓冲写入。
static GFX_ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn gfx_enabled() -> bool {
    *GFX_ENABLED.get_or_init(|| std::env::var("EASYRDP_ENABLE_GFX").ok().as_deref() == Some("1"))
}

/// 输入通路调试开关（`EASYRDP_INPUT_DEBUG=1`；一次性读取缓存）。
/// 每次输入 PDU 发送打印类型/flags/坐标与返回值——排查「远端无反应」用。
static INPUT_DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn input_debug_enabled() -> bool {
    *INPUT_DEBUG.get_or_init(|| std::env::var("EASYRDP_INPUT_DEBUG").ok().as_deref() == Some("1"))
}

/// debug.log 每会话截断标记（connect 一次性）。
static DEBUG_LOG_INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();

// ---------------------------------------------------------------------------
// 全帧合成缓冲（2026-11 真机诊断修复：乱花噪点根因）
//
// 诊断结论（192.168.200.26，EASYRDP_FRAME_DEBUG=1 实测）：
// - gdi 1280x720、stride=5120=width*4、dstFormat=BGRA32 —— primary_buffer
//   的 BGRA32 合成与步长完全正确；
// - 但服务端输出是**脏矩形流**：首刷 240 个 64x64 tile（20x12 铺满 1280x720），
//   之后每包 1~2 个 tile。此前每个 tile 被当独立"整帧"推给上层，上层把
//   64x64 小块拉伸铺满整个 canvas -> 乱花噪点。
// 修复：每个会话（ctx 地址）维护一份全帧 BGRA32 缓冲（尺寸 = gdi width x
// height），on_bitmap_update 把 GDI 合成后的各脏矩形写入缓冲，然后**整帧**
// 推给上层（x=0,y=0,width=fb_w,height=fb_h）。上层拿到的每帧都是完整画面。
// ---------------------------------------------------------------------------

/// ctx 地址 -> 全帧合成缓冲（BGRA32 top-down，行距 = width*4）。
///
/// 2026-11 增量管线：每包 BITMAP_UPDATE 的脏矩形在写入合成缓冲的同时
/// 直接打包成 [`FramebufferEvent::Delta`] 即时输出；全帧 JPEG 仅在
/// 首帧 / 分辨率变化 / 周期兜底时发送（防漂移）。
struct FrameComposer {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    /// 尚未发出过全帧（首帧/缓冲重建后）：下一包必发全帧。
    needs_full: bool,
    /// 上次全帧发射时刻（周期兜底依据）。
    last_full: Option<Instant>,
    /// 运动检测：1s 窗口累计增量字节。
    motion_bytes: u64,
    /// 运动检测窗口起点。
    motion_window: Option<Instant>,
    /// 运动模式截止时刻（增量字节率 > 4MB/s 时置 now+2s；静止 2s 自动恢复）。
    motion_until: Option<Instant>,
    /// 自上次全帧以来合成缓冲有变化（GFX 模式下 EndPaint 依据此发全帧）。
    dirty: bool,
    /// 残影修复（GHOSTFIX-20261115）：EndPaint 快照被 100ms 节流丢弃时置位；
    /// 发射线程 recv 超时后补发最后一帧（拖拽/输入结束的最终画面不再丢失）。
    pending: bool,
    /// DELTA-20261115：上次 EndPaint 时刻（间隔 <60ms = 纯 order 更新
    /// 运动期——窗口拖拽只发 memblt 不触发 BitmapUpdate，增量字节率
    /// 检测不到；EndPaint 频率是普适运动信号）。
    last_endpaint: Option<Instant>,
    /// TILEGHOST-20261116：最近一次 tile 写入 primary_buffer 时刻
    ///（on_bitmap_update 每包末尾更新）。快照方在该时刻 15ms 内推迟快照，
    /// 缩小「tile 流中途快照」的撕裂窗口（与快照一致性校验双重防线）。
    last_tile_write: Option<Instant>,
    /// GHOSTFIX-R021（2026-11）：order/位图批处理进行中标志。BeginPaint 置位、
    /// EndPaint 清除——发射线程 flush_pending_frame 在该标志置位期间**禁止**
    /// 快照（order 更新直改 primary_buffer 不经 FRAME_SYNC，此前的 17 行
    /// 稀疏采样对局部撕裂漏检；批内快照交给 EndPaint 在接收线程自身完成，
    /// 天然一致）。flush 检到置位即保持 pending 顺延。
    painting: bool,
}

/// 全帧兜底间隔：增量管线漂移（GDI 合成与增量贴图误差）的防线。
// TILEGHOST-20261116 残影压制：全帧兜底 5s → 500ms。
// 全帧走进程内 raw BGRA（NativeFrame），不产生任何额外网络流量，
// 仅增进程内 memcpy——基线分叉/异常帧的最长存活时间从 5s 压到 0.5s，
// 用户感知的"残影闪现"时长同比缩短 10 倍。
const FULL_FRAME_FALLBACK: Duration = Duration::from_millis(500);

static FRAME_COMPOSERS: Mutex<Option<HashMap<usize, FrameComposer>>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// 全帧快照直读 primary_buffer（GHOSTFIX2-20261115 拖拽残影根修）
//
// 根因（2026-11-15 20:52 用户会话实证）：合成缓冲 composer.pixels **只在
// BITMAP_UPDATE 脏矩形路径写入**；窗口拖动/文本编辑走 memblt/scrblt 等
// **order 更新**，只改 gdi->primary_buffer，不经过 on_bitmap_update ——
// EndPaint 全帧与 pending 补发都从 composer.pixels 快照 → order 绘制内容
// 永远缺失 → 拖拽残影定格。修复：快照改为直接从 gdi->primary_buffer 整帧
// 拷贝（含 order + bitmap 全部合成结果）。ctx 指针由 on_post_connect 登记。
// ---------------------------------------------------------------------------

/// ctx 指针 Send 包装（静态表跨线程持有；指针只在会话存活期内解引用）。
struct CtxPtr(*mut sys::rdpContext);
unsafe impl Send for CtxPtr {}

/// ctx 地址 -> ctx 指针（快照直读用；指针生命周期 = 会话 OwnedInstance）。
static CTX_PTRS: Mutex<Option<HashMap<usize, CtxPtr>>> = Mutex::new(None);

fn ctx_ptr_register(addr: usize, ptr: *mut sys::rdpContext) {
    CTX_PTRS
        .lock()
        .expect("CTX_PTRS poisoned")
        .get_or_insert_with(HashMap::new)
        .insert(addr, CtxPtr(ptr));
}

fn ctx_ptr_unregister(addr: usize) {
    if let Some(map) = CTX_PTRS.lock().expect("CTX_PTRS poisoned").as_mut() {
        map.remove(&addr);
    }
}

fn ctx_ptr_lookup(addr: usize) -> Option<*mut sys::rdpContext> {
    CTX_PTRS
        .lock()
        .expect("CTX_PTRS poisoned")
        .as_ref()
        .and_then(|m| m.get(&addr))
        .map(|p| p.0)
}

/// TILEGHOST-20261116（审计#2 撕裂修复）：primary_buffer 快照与接收线程
/// tile 合成的互斥锁。快照方：接收线程 EndPaint / 发射线程 pending 补发；
/// 写入方：on_bitmap_update 的 GDI 合成+拷贝段（接收线程，作用域内串行）。
/// 注意 on_bitmap_update 内部自身的兜底快照**不得**再取此锁（同线程重入死锁），
/// 使用裸版 snapshot_primary_frame_unlocked。
static FRAME_SYNC: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 从 gdi primary_buffer 快照完整帧（BGRA32 top-down）。
/// 返回 None = gdi 未就绪/尺寸无效。
///
/// 持 FRAME_SYNC 与 tile 写入互斥（TILEGHOST #2：消除半更新撕裂帧）。
/// 调用方禁止在已持有 FRAME_SYNC 的作用域内调用（同线程重入死锁）。
unsafe fn snapshot_primary_frame(ctx: *mut sys::rdpContext) -> Option<(Vec<u8>, u32, u32)> {
    let _sync = FRAME_SYNC.lock().unwrap_or_else(|e| e.into_inner());
    snapshot_primary_frame_unlocked(ctx)
}

/// 裸快照（无锁）：仅供已持有 FRAME_SYNC 或与写入同线程的调用方使用。
unsafe fn snapshot_primary_frame_unlocked(ctx: *mut sys::rdpContext) -> Option<(Vec<u8>, u32, u32)> {
    if ctx.is_null() {
        return None;
    }
    let gdi = (*ctx).gdi;
    if gdi.is_null() {
        return None;
    }
    let w = (*gdi).width.max(0) as u32;
    let h = (*gdi).height.max(0) as u32;
    let stride = (*gdi).stride as usize;
    let buf = (*gdi).primary_buffer;
    if buf.is_null() || w == 0 || h == 0 || stride < (w as usize) * 4 {
        return None;
    }
    let row_bytes = (w as usize) * 4;
    // TILEGHOST-20261116 快照一致性校验（撕裂免疫）+ GHOSTFIX-R021 强化：
    // FRAME_SYNC 只互斥「快照 vs on_bitmap_update 拷贝段」——
    // ①scrblt/memblt **order 更新**（update_recv_orders，窗口拖动主力）
    //   直改 primary_buffer，不经 on_bitmap_update，不受该锁保护；
    // ②tile 流跨多个 BITMAP_UPDATE PDU 持续进行，锁段之间 primary_buffer
    //   仍处中间态（载入 tile 风暴）。快照落在写入中途 = 「部分新部分旧」
    //   撕裂帧上屏闪现。
    // 【GHOSTFIX-R021 强化】原 17 行×64B 稀疏采样对**局部撕裂**（窗口边缘
    // 旧位残留等小面积错位）漏检率高。改为**整帧双路校验和**：拷贝前后各
    // 对全帧做两条独立 u64 累积链（旋转-异或混合），不一致 = 快照期间有
    // 任意位置写入 → 返回 None 丢弃本次全帧（EndPaint 节流/pending 机制
    // 稍后自然重试）。局部/单字节级写入也必被捕获（碰撞概率 ≈ 2^-128）。
    // 开销：两次全帧顺序读 + 一次拷贝 ≈ 3×memcpy 量级，微秒-毫秒级。
    let checksum = |rows: usize| -> (u64, u64) {
        let mut a: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut b: u64 = 0x1656_67A8_E9E3_7C3D;
        for row in 0..rows {
            let p = buf.add(row * stride);
            let mut i = 0usize;
            while i + 8 <= row_bytes {
                // SAFETY：行内 [i, i+8) ⊆ [0, row_bytes) ⊆ stride 字节行。
                let w = unsafe { (p.add(i) as *const u64).read_unaligned() };
                a = a.wrapping_add(w).rotate_left(27) ^ b;
                b = b.wrapping_add(a).rotate_left(23);
                i += 8;
            }
            while i < row_bytes {
                a = a.wrapping_add(unsafe { *p.add(i) } as u64);
                i += 1;
            }
        }
        (a, b)
    };
    let before = checksum(h as usize);
    let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
    for row in 0..h as usize {
        std::ptr::copy_nonoverlapping(
            buf.add(row * stride),
            out.as_mut_ptr().add(row * row_bytes),
            row_bytes,
        );
    }
    let after = checksum(h as usize);
    if before != after {
        return None; // 撕裂：丢弃，等下一个快照时机
    }
    Some((out, w, h))
}

/// EndPaint / pending 补发共用：节流判定 + primary_buffer 全帧快照。
/// 节流期内返回 None 并置 pending（由 flush_pending_frame 补发）。
fn endpaint_snapshot_throttled(
    ctx: *mut sys::rdpContext,
    ctx_addr: usize,
) -> Option<(Vec<u8>, u32, u32)> {
    let throttled = {
        let now = std::time::Instant::now();
        let mut composers = FRAME_COMPOSERS.lock().expect("FRAME_COMPOSERS poisoned");
        composers.as_mut().and_then(|m| m.get_mut(&ctx_addr)).map_or(false, |c| {
            // DELTA-20261115：运动期节流 100ms→33ms（~30FPS 全帧）——
            // 原生窗口已能直绘 Delta，order 更新不再被固定 100ms 钳制；
            // 静止期保持 100ms 省带宽。运动信号①增量字节率（原有），
            // ②EndPaint 频率（<60ms 间隔——窗口拖拽只发 memblt order，
            // 不触发 BitmapUpdate，字节率检测覆盖不到）。
            let prev_endpaint = c.last_endpaint;
            c.last_endpaint = Some(now);
            let motion = c.motion_until.map_or(false, |t| t > now)
                || prev_endpaint
                    .map_or(false, |t| now.duration_since(t) < Duration::from_millis(60));
            let throttle = if motion { 33 } else { 100 };
            // TILEGHOST-20261116：tile 写入 15ms 内推迟快照——载入 tile 风暴
            // 期 EndPaint 常紧贴最后一个 BITMAP_UPDATE 包，此时 primary_buffer
            // 可能仍处 tile 流中途；推迟到流间歇再快照（pending 已置位，
            // 发射线程 flush 顺延补发）。
            let tile_busy = c
                .last_tile_write
                .map_or(false, |t| now.duration_since(t) < Duration::from_millis(15));
            let t = tile_busy
                || c.last_full
                    .map_or(false, |t| t.elapsed() < Duration::from_millis(throttle));
            if t {
                c.pending = true;
                true
            } else {
                false
            }
        })
    };
    if throttled {
        if !PENDING_LOG_SEEN.swap(true, Ordering::Relaxed) {
            crate::debug_log("GHOSTFIX2-20261115 endpaint throttled -> pending 标记");
        }
        return None;
    }
    // SAFETY：ctx 由 CTX_PTRS 登记，会话存活期内有效（调用点均持会话上下文）。
    unsafe { snapshot_primary_frame(ctx) }
}

// ---------------------------------------------------------------------------
// 帧发射线程（2026-11 拖拽卡顿修复）
//
// 根因：全帧 JPEG 编码（2560x1440 实测 ~33ms）此前在 on_bitmap_update
// 回调内同步执行——该回调运行在 **FreeRDP 传输接收线程**，编码期间服务端
// 推来的位图包全部积压，拖窗口时表现为 1-2 FPS 卡顿。
// 修复：回调只组装任务（memcpy 级），由每会话独立的发射线程依序处理
//（Full → 线程内编码再发；Delta → 直发）。mpsc 单队列保证全帧/增量严格
// 有序，前端串行渲染链的顺序假设不变。
// ---------------------------------------------------------------------------

/// 发射线程任务（数据 clone 跨线程，顺序由单队列保证）。
enum FrameTask {
    /// `motion=true` 时运动期降载：分辨率减半 + q50。
    Full { width: u32, height: u32, bgra: Vec<u8>, motion: bool },
    Delta {
        width: u32,
        height: u32,
        rects: Vec<crate::RectUpdate>,
        pixels: Vec<u8>,
        /// 入队时刻（发射线程测量队列等待，1/32 抽样落 debug.log）
        enqueued: std::time::Instant,
    },
}

/// ctx 地址 -> 发射队列发送端。
static FRAME_EMITTERS: Mutex<Option<HashMap<usize, std::sync::mpsc::Sender<FrameTask>>>> =
    Mutex::new(None);

/// EndPaint 节流置 pending 的首次日志标记（GHOSTFIX-20261115）。
static PENDING_LOG_SEEN: AtomicBool = AtomicBool::new(false);
/// pending 补发首次日志标记（GHOSTFIX-20261115）。
static FLUSH_LOG_SEEN: AtomicBool = AtomicBool::new(false);

/// 残影修复补发（GHOSTFIX-20261115）：EndPaint 节流期丢弃的最后一帧在此
/// 补发——发射线程 recv 超时（20ms）后检查 pending 标志，节流到期即发。
/// 拖拽/划选/打字结束后服务端不再 paint，此补发是最终画面唯一出口。
fn flush_pending_frame(ctx_addr: usize) {
    // GHOSTFIX2-20261115：补发也直接读 primary_buffer（见 CTX_PTRS 注释）。
    let Some(ctx) = ctx_ptr_lookup(ctx_addr) else { return };
    let due = {
        let mut composers = FRAME_COMPOSERS.lock().expect("FRAME_COMPOSERS poisoned");
        let Some(map) = composers.as_mut() else { return };
        let Some(c) = map.get_mut(&ctx_addr) else { return };
        if !c.pending {
            return;
        }
        // GHOSTFIX-R021：order/位图批处理进行中（接收线程正在直改
        // primary_buffer）→ 推迟快照，保持 pending 由下一轮超时再查。
        // 批完成时 EndPaint 自身会在接收线程做一致快照（pending 若仍置位
        // 也只会在批间发射，不会丢帧）。
        if c.painting {
            return;
        }
        let due = c
            .last_full
            .map_or(true, |t| t.elapsed() >= Duration::from_millis(100));
        if !due {
            return; // 仍在节流窗口内：保持 pending，下一轮超时再查
        }
        c.pending = false;
        c.last_full = Some(std::time::Instant::now());
        true
    };
    if !due {
        return;
    }
    // SAFETY：ctx 由 CTX_PTRS 登记，会话存活期内有效。
    let snapshot = unsafe { snapshot_primary_frame(ctx) };
    if let Some((bgra, width, height)) = snapshot {
        if !FLUSH_LOG_SEEN.swap(true, Ordering::Relaxed) {
            crate::debug_log("GHOSTFIX-20261115 pending 补发首次执行（残影修复生效）");
        }
        let motion = composer_motion(&ctx_addr);
        if let Some(map) = FRAME_EMITTERS.lock().expect("FRAME_EMITTERS poisoned").as_ref() {
            if let Some(tx) = map.get(&ctx_addr) {
                let _ = tx.send(FrameTask::Full { width, height, bgra, motion });
            }
        }
    }
}

fn emitter_register(ctx_addr: usize) {
    let (tx, rx) = std::sync::mpsc::channel::<FrameTask>();
    FRAME_EMITTERS
        .lock()
        .expect("FRAME_EMITTERS poisoned")
        .get_or_insert_with(HashMap::new)
        .insert(ctx_addr, tx);
    std::thread::spawn(move || {
        // 线程退出条件：发送端全部 drop（会话注销）后 recv Err。
        // 发射统计（5s 汇总落 debug.log，2026-11 发射停滞排查）。
        let mut last_summary = std::time::Instant::now();
        let (mut n_full, mut n_delta) = (0u64, 0u64);
        // GHOSTFIX-20261115：阻塞 recv 改为 20ms 超时轮询——空闲间隙检查
        // pending 标志，补发 EndPaint 节流期丢弃的最后一帧（残影根因）。
        loop {
            let task = match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(t) => t,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    flush_pending_frame(ctx_addr);
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
            match task {
                FrameTask::Full { width, height, bgra, motion } => {
                    n_full += 1;
                    // 原生直绘路径：raw BGRA 直通（零编码，克隆 ~4MB/帧；
                    // JPEG 编码照常进行——webview 路径仍需 JPEG）。
                    if let Some(tx) = route_lookup_by_ctx_addr(ctx_addr) {
                        let _ = tx.send(FramebufferEvent::NativeFrame {
                            width,
                            height,
                            pixels: bgra.clone(),
                        });
                    }
                    // 全帧 JPEG 编码——在线程内执行，不阻塞 FreeRDP；
                    // 运动期降载：分辨率减半 + q50（静止 2s 自动恢复 q75 全分辨率）
                    let started = std::time::Instant::now();
                    let encode = |motion: bool| -> Result<Vec<u8>, String> {
                        if motion {
                            let (w2, h2, half) = frame_jpeg::halve_bgra(width, height, &bgra);
                            frame_jpeg::encode_frame_jpeg_q(
                                w2,
                                h2,
                                &half,
                                frame_jpeg::JPEG_QUALITY_MOTION,
                            )
                        } else {
                            frame_jpeg::encode_frame_jpeg(width, height, &bgra)
                        }
                    };
                    match encode(motion) {
                        Ok(bytes) => {
                            frame_jpeg::record_encode(
                                started.elapsed().as_micros() as u64,
                                bytes.len(),
                            );
                            if let Some(tx) = route_lookup_by_ctx_addr(ctx_addr) {
                                let _ = tx.send(FramebufferEvent::Framebuffer {
                                    x: 0,
                                    y: 0,
                                    width,
                                    height,
                                    pixels: bytes,
                                });
                            }
                        }
                        Err(e) => {
                            eprintln!("[frame] JPEG 编码失败: {e}");
                        }
                    }
                }
                FrameTask::Delta { width, height, rects, pixels, enqueued } => {
                    frame_jpeg::record_delta(pixels.len(), rects.len());
                    n_delta += 1;
                    // 延迟采样（1/32）：入队→发射线程处理完成（队列等待+发送）
                    let n = LAT_SAMPLES.fetch_add(1, Ordering::Relaxed);
                    if n % 32 == 0 {
                        let us = enqueued.elapsed().as_micros() as u64;
                        crate::debug_log(&format!("[frame-latency] engine queue→emit {us} µs (sample #{n})"));
                    }
                    let routed = route_lookup_by_ctx_addr(ctx_addr);
                    match routed {
                        Some(tx) => {
                            let _ = tx.send(FramebufferEvent::Delta { width, height, rects, pixels });
                        }
                        None => {
                            // 路由已注销（会话收尾）：计丢弃
                            crate::debug_log("[frame-stats] delta 丢弃：路由未注册");
                        }
                    }
                    // 5s 汇总（发射停滞排查）
                    if last_summary.elapsed() >= Duration::from_secs(5) {
                        crate::debug_log(&format!(
                            "[frame-stats] emitter 5s: full={n_full} delta={n_delta}"
                        ));
                        last_summary = std::time::Instant::now();
                        (n_full, n_delta) = (0, 0);
                    }
                }
            }
        }
    });
}

fn emitter_unregister(ctx_addr: usize) {
    if let Some(map) = FRAME_EMITTERS.lock().expect("FRAME_EMITTERS poisoned").as_mut() {
        map.remove(&ctx_addr); // drop sender → 发射线程随 recv Err 退出
    }
}

/// 发射线程用：按 ctx 地址反查事件路由（instance 地址 == ctx 地址在
/// EVENT_ROUTES 中并不成立，这里经 ctx->instance 查不可行——发射线程没有
/// ctx 指针；改为 emitter_register 时顺带保存该会话的 event Sender 副本）。
fn route_lookup_by_ctx_addr(ctx_addr: usize) -> Option<event_channel::Sender> {
    ROUTE_BY_CTX
        .lock()
        .expect("ROUTE_BY_CTX poisoned")
        .as_ref()
        .and_then(|m| m.get(&ctx_addr).cloned())
}

/// ctx 地址 -> 事件 Sender 副本（emitter_register 时保存）。
static ROUTE_BY_CTX: Mutex<Option<HashMap<usize, event_channel::Sender>>> = Mutex::new(None);

fn composer_register(ctx_addr: usize) {
    FRAME_COMPOSERS
        .lock()
        .expect("FRAME_COMPOSERS poisoned")
        .get_or_insert_with(HashMap::new)
        .entry(ctx_addr)
        .or_insert(FrameComposer {
            width: 0,
            height: 0,
            pixels: Vec::new(),
            needs_full: true,
            last_full: None,
            dirty: false,
            pending: false,
            motion_bytes: 0,
            motion_window: None,
            motion_until: None,
            last_endpaint: None,
            last_tile_write: None,
            painting: false,
        });
}

fn composer_unregister(ctx_addr: usize) {
    if let Some(map) = FRAME_COMPOSERS.lock().expect("FRAME_COMPOSERS poisoned").as_mut() {
        map.remove(&ctx_addr);
    }
}

/// `pBitmapUpdate`（`<freerdp/update.h>`：
/// `typedef BOOL (*pBitmapUpdate)(rdpContext*, const BITMAP_UPDATE*)`）。
///
/// 链式处理：
/// 1) 调 GDI 原处理器 `gdi_bitmap_update`：解压服务端位图（RLE/planar）并
///    按服务器像素格式转换合成进 `gdi->primary_buffer`（dstFormat =
///    PIXEL_FORMAT_BGRA32，由 on_post_connect 的 gdi_init 指定）。
/// 2) 从 `gdi->primary_buffer`（行距 `gdi->stride`）为每个脏矩形拷出紧凑
///    排列的 BGRA32 像素，作为 [`FramebufferEvent::Framebuffer`] 推入通道。
unsafe extern "C" fn on_bitmap_update(ctx: *mut sys::rdpContext, bitmap: *const sys::BITMAP_UPDATE) -> sys::BOOL {
    if ctx.is_null() || bitmap.is_null() || (*bitmap).rectangles.is_null() {
        return FALSE;
    }
    let ctx_addr = ctx as usize;

    // TILEGHOST（审计#2）：锁覆盖 GDI 合成+tile 拷贝段，与 primary_buffer
    // 快照（EndPaint/pending 补发）互斥，消除半更新撕裂帧。函数尾自动释放。
    let _framesync = FRAME_SYNC.lock().unwrap_or_else(|e| e.into_inner());

    // -- 阶段 1：GDI 合成（返回 FALSE 表示解压/合成失败，透传错误） --
    let prev = gdi_prev_lookup(ctx_addr);
    if let Some(f) = prev {
        if f(ctx, bitmap) == 0 {
            return FALSE;
        }
    }

    // -- 阶段 2：脏矩形写入全帧合成缓冲，经发射线程整帧/增量推给上层 --
    let instance = (*ctx).instance;
    if route_lookup(instance as usize).is_none() {
        return TRUE; // 未注册路由（如已 disconnect）：静默丢弃，不中断 FreeRDP
    }
    let gdi = (*ctx).gdi;
    if gdi.is_null() {
        return TRUE;
    }
    let fb_w = (*gdi).width.max(0) as u32;
    let fb_h = (*gdi).height.max(0) as u32;
    let stride = (*gdi).stride as usize;
    let buffer = (*gdi).primary_buffer;
    let format = (*gdi).dstFormat;
    if buffer.is_null() || format != PIXEL_FORMAT_BGRA32 {
        // 理论不可达（gdi_init 固定 BGRA32），防御性跳过像素提取。
        return TRUE;
    }
    if fb_w == 0 || fb_h == 0 {
        return TRUE;
    }

    let count = (*bitmap).number.min((*bitmap).count) as usize;
    let rects = std::slice::from_raw_parts((*bitmap).rectangles, count);
    if frame_debug_enabled() {
        let settings = (*instance).settings;
        let u32_at = |s: *mut sys::rdpSettings, id: u32| -> u32 {
            if s.is_null() { 0 } else { unsafe { sys::freerdp_settings_get_uint32(s, id as usize as _) } }
        };
        let r0 = rects.first().copied();
        let (rx0, ry0, rx1, ry1) = r0.map(|r| (r.destLeft, r.destTop, r.destRight, r.destBottom)).unwrap_or((0, 0, 0, 0));
        let rw = r0.map(|r| r.destRight.saturating_sub(r.destLeft) + 1).unwrap_or(0);
        let rh = r0.map(|r| r.destBottom.saturating_sub(r.destTop) + 1).unwrap_or(0);
        let rbytes = r0.map(|r| (r.destRight as usize - r.destLeft as usize + 1) * (r.destBottom as usize - r.destTop as usize + 1) * 4).unwrap_or(0);
        eprintln!(
            "[frame-debug] gdi={}x{} stride={} dstFormat=0x{:08X} primary_buffer={} | DesktopWidth={} DesktopHeight={} ColorDepth={} | rects={} first_rect=({},{})-({},{}) {}x{} bytes={}",
            fb_w, fb_h, stride, format, !buffer.is_null(),
            u32_at(settings, sys::FreeRDP_DesktopWidth),
            u32_at(settings, sys::FreeRDP_DesktopHeight),
            u32_at(settings, sys::FreeRDP_ColorDepth),
            count, rx0, ry0, rx1, ry1, rw, rh, rbytes,
        );
        let sizes = rects.iter().take(4).map(|r| format!("{}x{}", r.destRight - r.destLeft + 1, r.destBottom - r.destTop + 1)).collect::<Vec<_>>().join(",");
        eprintln!("[frame-debug] rect_sizes={sizes}...");
    }

    // -- 全帧合成：把 GDI 合成后的各脏矩形拷入本会话的全帧缓冲 --
    {
        let mut composers = FRAME_COMPOSERS.lock().expect("FRAME_COMPOSERS poisoned");
        let map = composers.get_or_insert_with(HashMap::new);
        let composer = map.entry(ctx_addr).or_insert_with(|| FrameComposer {
            width: fb_w,
            height: fb_h,
            pixels: vec![0u8; (fb_w as usize) * (fb_h as usize) * 4],
            needs_full: true,
            last_full: None,
            dirty: false,
            pending: false,
            motion_bytes: 0,
            motion_window: None,
            motion_until: None,
            last_endpaint: None,
            last_tile_write: None,
            painting: false,
        });
        // gdi 尺寸变化（理论上 α 不发生）：重建缓冲并强制下一包发全帧。
        if composer.width != fb_w || composer.height != fb_h {
            *composer = FrameComposer {
                width: fb_w,
                height: fb_h,
                pixels: vec![0u8; (fb_w as usize) * (fb_h as usize) * 4],
                needs_full: true,
                last_full: None,
                motion_bytes: 0,
                motion_window: None,
                motion_until: None,
                dirty: true,
                pending: false,
                last_endpaint: None,
                last_tile_write: None,
                painting: false,
            };
        }
        // -- 脏矩形写入合成缓冲 + （增量模式）同步打包增量像素 --
        // 全帧/GFX 模式不打包增量（省 memcpy；发射统一在 EndPaint）。
        let pack_delta = !deltas_disabled() && !gfx_enabled();
        let mut delta_pixels: Vec<u8> = Vec::new();
        let mut delta_rects: Vec<crate::RectUpdate> = Vec::new();
        let mut dirty_area = 0usize;
        for r in rects {
            // 脏矩形与帧缓冲求交（BITMAP_DATA 字段见 `<freerdp/update.h>`）。
            let x0 = r.destLeft.min(fb_w.saturating_sub(1));
            let y0 = r.destTop.min(fb_h.saturating_sub(1));
            let x1 = r.destRight.saturating_add(1).min(fb_w);
            let y1 = r.destBottom.saturating_add(1).min(fb_h);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let w = (x1 - x0) as usize;
            let h = (y1 - y0) as usize;
            let row_bytes = w * 4;
            let dst_stride = (fb_w as usize) * 4;
            dirty_area += row_bytes * h;
            if pack_delta {
                delta_rects.push(crate::RectUpdate {
                    x: x0,
                    y: y0,
                    w: w as u32,
                    h: h as u32,
                });
                delta_pixels.reserve(row_bytes * h);
            }
            for row in y0 as usize..y1 as usize {
                let src = buffer.add(row * stride + (x0 as usize) * 4);
                let dst_off = row * dst_stride + (x0 as usize) * 4;
                // SAFETY：src 行范围 = gdi primary_buffer 行内 [x0*4, x0*4+row_bytes)，
                // dst 行范围在 composer.pixels（尺寸 = fb_w*fb_h*4）内，二者不重叠。
                unsafe {
                    // 合成缓冲（供全帧兜底/漂移防护用）
                    std::ptr::copy_nonoverlapping(src, composer.pixels.as_mut_ptr().add(dst_off), row_bytes);
                }
                if pack_delta {
                    // 增量打包（从同一源拷贝到本函数拥有的 Vec）
                    let base = delta_pixels.len();
                    delta_pixels.resize(base + row_bytes, 0);
                    unsafe {
                        std::ptr::copy_nonoverlapping(src, delta_pixels.as_mut_ptr().add(base), row_bytes);
                    }
                }
            }
        }
        // motion 统计：增量模式按打包字节；全帧模式按脏像素面积（字节）
        composer.motion_bytes += if pack_delta { delta_pixels.len() as u64 } else { dirty_area as u64 };
        // -- 发射决策：增量为主，全帧兜底（首帧 / 5s 周期） --
        // 2026-11 拖拽卡顿修复：①去掉「大增量升级全帧」——拖窗口时增量常超
        // 25% 面积，每次都触发 33ms JPEG 编码（且曾阻塞 FreeRDP 接收线程）
        // 是 1-2 FPS 卡顿主因；全帧仅保留首帧与 5s 周期兜底（防漂移）。
        // ②编码移到帧发射线程（见 FrameTask），回调线程只做 memcpy 级入队。
        // ③运动期自适应降载（2026-11）：1s 窗口增量字节率 > 4MB/s 判定运动
        // 模式（持续到静止 2s）：全帧兜底编码分辨率减半 + q50（编码 ~4 倍快、
        // 数据 ~4 倍少），前端 drawImage 放大显示。
        composer.motion_bytes += delta_pixels.len() as u64;
        let now = std::time::Instant::now();
        if composer.motion_window.map_or(true, |t| t.elapsed() >= Duration::from_secs(1)) {
            if composer.motion_bytes > 4 * 1024 * 1024 {
                composer.motion_until = Some(now + Duration::from_secs(2));
            }
            composer.motion_bytes = 0;
            composer.motion_window = Some(now);
        }
        let motion = composer.motion_until.map_or(false, |t| t > now);
        // TILEGHOST-20261116：本包 tile 已全部写入 primary_buffer，记录时刻
        // ——快照方（endpaint_snapshot_throttled）在 15ms 内推迟快照，
        // 缩小「tile 流中途快照」撕裂窗口（全帧模式同样记录，EndPaint 通用）。
        composer.last_tile_write = Some(now);
        composer.dirty = true;
        // 全帧模式（deltas 禁用）：BitmapUpdate 只写合成缓冲 + 置 dirty，
        // 帧发射统一由 on_update_end_paint 驱动（100ms 节流）。
        // 根因（2026-11 拖拽残影实证）：窗口拖动走 memblt/scrblt **order 更新**，
        // 不触发 BitmapUpdate —— 若发射只挂在 BitmapUpdate 链，order 路径
        // 完全无帧（残影/黑块）。EndPaint 在 order/bitmap 两种来源都会触发。
        if deltas_disabled() || gfx_enabled() {
            return TRUE;
        }
        // 增量模式（EASYRDP_ENABLE_DELTAS=1）：delta 直发 + 5s 全帧兜底
        let need_full = composer.needs_full
            || composer
                .last_full
                .map_or(true, |t| t.elapsed() >= FULL_FRAME_FALLBACK);
        let task = if need_full {
            composer.needs_full = false;
            composer.last_full = Some(now);
            // TILEGHOST-20261116 残影根修（审计缺陷#1）：兜底全帧**不得**用
            // composer.pixels——它只含 BITMAP_UPDATE 脏 tile + 大片陈旧/空白，
            // 首个 tile 包即触发 need_full → 整帧=「旧底+规则网格 tile」平铺
            // （载入/点击/IME/开软件全部命中）。必须直读 primary_buffer
            // （与 EndPaint 快照同源，含 order+bitmap 完整画面）。
            // 快照在接收线程内完成（本回调即接收线程），无撕裂竞态（审计#2 同步消解）。
            // GHOST_OLD_FALLBACK=1 时保留旧行为（取证 A/B 对照用，默认修复）。
            let old_fallback = std::env::var("GHOST_OLD_FALLBACK").ok().as_deref() == Some("1");
            let bgra = if old_fallback {
                composer.pixels.clone()
            } else {
                unsafe { snapshot_primary_frame_unlocked(ctx) }
                    .map(|(p, w, h)| { let _ = (w, h); p })
                    .unwrap_or_else(|| composer.pixels.clone())
            };
            FrameTask::Full { width: fb_w, height: fb_h, bgra, motion }
        } else {
            FrameTask::Delta { width: fb_w, height: fb_h, rects: delta_rects, pixels: delta_pixels, enqueued: std::time::Instant::now() }
        };
        if let Some(map) = FRAME_EMITTERS.lock().expect("FRAME_EMITTERS poisoned").as_ref() {
            if let Some(tx) = map.get(&ctx_addr) {
                // 队列满（发射线程卡死）时丢弃该帧而非阻塞 FreeRDP 接收线程
                let _ = tx.send(task);
            }
        }
    }
    TRUE
}

/// `pPointerPosition`（`<freerdp/update.h>`：
/// `typedef BOOL (*pPointerPosition)(rdpContext*, const POINTER_POSITION_UPDATE*)`）。
unsafe extern "C" fn on_pointer_position(
    ctx: *mut sys::rdpContext,
    pos: *const sys::POINTER_POSITION_UPDATE,
) -> sys::BOOL {
    if ctx.is_null() || pos.is_null() {
        return FALSE;
    }
    if let Some(tx) = route_lookup((*ctx).instance as usize) {
        let _ = tx.send(FramebufferEvent::Pointer {
            x: (*pos).xPos,
            y: (*pos).yPos,
        });
    }
    TRUE
}

// ---------------------------------------------------------------------------
// update 回调空实现（必须注册！2026-11 实机定位，ACTIVE 后掉线 -4 真因）：
//
// FreeRDP 2.11.2 update.c 的约定是「回调缺失即失败」：
// - update_end_paint(): update->EndPaint 为 NULL 时**返回 FALSE** ->
//   fastpath_recv_updates() 返回 -4 -> rdp_recv_pdu 负值 -> 连接被判定失败，
//   ACTIVE 后首个 fastpath 输出包就掉线（实测 0x2000D 的第二段真因）。
//   （update_begin_paint 对 NULL BeginPaint 返回 TRUE，但 mux 临界区必须由
//   EndPaint 释放 —— 只注册 EndPaint 会死锁，两者必须成对。）
// - update_recv_pointer()/Palette/PlaySound 等用 IFCALLRESULT(FALSE, ...)：
//   回调缺失 -> FALSE -> update 处理失败 -> 掉线。光标一变就会命中。
//
// 因此凡是「缺省即 FALSE」的回调都要注册恒 TRUE 的空实现（wfreerdp 的
// wf_begin_paint/wf_end_paint 对应物）。
// ---------------------------------------------------------------------------

/// BeginPaint：进入 mux 临界区（update_begin_paint 语义），恒成功。
/// GHOSTFIX-R021：置 painting 标志——order（memblt/scrblt）与位图批处理
/// 期间 primary_buffer 处于批中间态，发射线程 flush_pending_frame 检到
/// 该标志即推迟快照（order 不经 FRAME_SYNC，批内发射线程快照可能撕裂）。
unsafe extern "C" fn on_update_begin_paint(ctx: *mut sys::rdpContext) -> sys::BOOL {
    if !ctx.is_null() {
        let ctx_addr = ctx as usize;
        let mut composers = FRAME_COMPOSERS.lock().expect("FRAME_COMPOSERS poisoned");
        if let Some(map) = composers.as_mut() {
            if let Some(c) = map.get_mut(&ctx_addr) {
                c.painting = true;
            }
        }
    }
    TRUE
}

/// EndPaint：离开 mux 临界区（update_end_paint 语义）。
///
/// 2026-11 GFX 帧发射接线：GFX opt-in 模式下服务端走 surface 编码
///（gdi_OutputUpdate），**不触发 BitmapUpdate** → 挂在 BitmapUpdate 上的
/// 合成/发射链收不到数据（真机实测 0 帧/纯黑根因）。EndPaint 时 primary
/// buffer 即完整合成帧 → 合成缓冲有变化（composer.dirty）则经帧发射线程
/// 发全帧（与 BitmapUpdate 路径同一出口，运动期降载同样生效）。
/// 非 GFX 模式保持恒 TRUE（位图路径帧发射在 BitmapUpdate 链上）。
unsafe extern "C" fn on_update_end_paint(ctx: *mut sys::rdpContext) -> sys::BOOL {
    // 全模式统一由 EndPaint 驱动发射（order/bitmap 两种更新来源统一出口）。
    // 2026-11 残影根因修复（增量模式）：窗口拖动走 memblt/scrblt order 更新
    //（不产生 BITMAP_UPDATE）——此前增量模式下此回调直接 return → order
    // 内容永不发射（真机 8:58 会话实证：delta=3-9/5s 流动但 full 仅 1/5s、
    // 拖窗口残影）。EndPaint + 时间节流在所有模式下生效。
    if !ctx.is_null() {
        let ctx_addr = ctx as usize;
        // GHOSTFIX-R021：清除 painting 标志（本回调在接收线程执行，此后
        // primary_buffer = 本批完整结果，接收线程自身快照天然一致）。
        {
            let mut composers = FRAME_COMPOSERS.lock().expect("FRAME_COMPOSERS poisoned");
            if let Some(map) = composers.as_mut() {
                if let Some(c) = map.get_mut(&ctx_addr) {
                    c.painting = false;
                }
            }
        }
        // GHOSTFIX2-20261115 残影根修：快照**直接读 gdi->primary_buffer**——
        // 合成缓冲 pixels 只含 BITMAP_UPDATE 脏矩形；窗口拖动/文本编辑走
        // memblt/scrblt order 更新（只写 primary_buffer，不进合成缓冲）→
        // 旧快照缺 order 内容 = 拖拽残影定格（20:52 用户会话实证）。节流
        // 逻辑不变：≥100ms 才发，节流期置 pending 由发射线程补发。
        let snapshot = endpaint_snapshot_throttled(ctx, ctx_addr);
        if let Some((bgra, width, height)) = snapshot {
            // 更新节流基准（重新加锁）
            {
                let mut composers = FRAME_COMPOSERS.lock().expect("FRAME_COMPOSERS poisoned");
                if let Some(map) = composers.as_mut() {
                    if let Some(c) = map.get_mut(&ctx_addr) {
                        c.last_full = Some(std::time::Instant::now());
                    }
                }
            }
            let motion = composer_motion(&ctx_addr);
            if let Some(map) = FRAME_EMITTERS.lock().expect("FRAME_EMITTERS poisoned").as_ref() {
                if let Some(tx) = map.get(&ctx_addr) {
                    let _ = tx.send(FrameTask::Full { width, height, bgra, motion });
                }
            }
        }
    }
    TRUE
}

/// 读合成缓冲运动模式（EndPaint 发射用）。
fn composer_motion(ctx_addr: &usize) -> bool {
    FRAME_COMPOSERS
        .lock()
        .expect("FRAME_COMPOSERS poisoned")
        .as_ref()
        .and_then(|m| m.get(ctx_addr))
        .map_or(false, |c| c.motion_until.map_or(false, |t| t > std::time::Instant::now()))
}

/// Palette 更新空实现（α 版不合成调色板，仅需返回 TRUE 避免连接失败）。
unsafe extern "C" fn on_update_palette(
    _ctx: *mut sys::rdpContext,
    _palette: *const sys::PALETTE_UPDATE,
) -> sys::BOOL {
    TRUE
}

/// PlaySound 更新空实现。
unsafe extern "C" fn on_update_play_sound(
    _ctx: *mut sys::rdpContext,
    _play_sound: *const sys::PLAY_SOUND_UPDATE,
) -> sys::BOOL {
    TRUE
}

// ---------------------------------------------------------------------------
// 远端光标形状（2026-11 新增，对齐 mstsc 体验）
//
// 结构核实（freerdp2-sys 0.2.0 bindgen.rs 29367 行）：
// POINTER_COLOR_UPDATE { cacheIndex, xPos, yPos, width, height,
//                        lengthAndMask, lengthXorMask, xorMaskData, andMaskData }
// xorMask 为 BGRA32（FreeRDP 2.x 回调交付前已把 16/24bpp 归一为 32bpp；
// 防御性支持 24bpp）；andMask 为 1bpp 行序掩码（MSB first），位=1 → 像素透明。
// ---------------------------------------------------------------------------

/// 光标位图（xor/and 掩码）→ RGBA。
///
/// FreeRDP 交付语义（libfreerdp/core/update.c `_update_read_pointer_color`
/// 核实）：**掩码行序 bottom-up**；xor 行宽 = (7 + xor_bpp*width)/8 且
/// **padding 到 2 字节边界**；and 为 1bpp 行宽 (7+width)/8 同样 pad 到偶数。
/// xorBpp：PointerColor PDU 恒 24（update.c 硬编码）；PointerNew 携带
/// wire xorBpp（1=单色 / 24 / 32）。
/// 像素合成：and 位=1 → 透明；and=0 → xor 色（mono：xor=1 白 / 0 黑；
/// 24bpp BGR；32bpp BGRA）。
fn color_pointer_to_rgba(
    width: u32,
    height: u32,
    xor_bpp: u32,
    xor: &[u8],
    and: &[u8],
) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let pixels = w.saturating_mul(h);
    if pixels == 0 {
        return Vec::new();
    }
    // 行宽（2 字节对齐，对照 update.c scanlineSize 公式）
    let xor_stride = if xor_bpp == 0 {
        0
    } else {
        ((7 + xor_bpp as usize * w) / 8 + 1) / 2 * 2
    };
    let and_stride = ((7 + w) / 8 + 1) / 2 * 2;
    let mut rgba = vec![0u8; pixels * 4];
    for i in 0..pixels {
        let x = i % w;
        let y = i / w;
        let o = i * 4;
        // bottom-up：掩码第 0 行是画面最底行
        let mask_row = h - 1 - y;
        let xor_bit = |col: usize| -> u32 {
            if xor_bpp == 1 {
                if xor_stride > 0 {
                    let idx = mask_row * xor_stride + col / 8;
                    if idx < xor.len() {
                        return u32::from((xor[idx] >> (7 - (col % 8))) & 1);
                    }
                }
                0
            } else {
                0
            }
        };
        let mut alpha_from_xor: Option<u8> = None;
        match xor_bpp {
            1 => {
                // 单色：and=0 且 xor=1 → 白；and=0 且 xor=0 → 黑
                if xor_bit(x) == 1 {
                    rgba[o] = 0xFF;
                    rgba[o + 1] = 0xFF;
                    rgba[o + 2] = 0xFF;
                }
            }
            24 => {
                let s = mask_row * xor_stride + x * 3;
                if s + 3 <= xor.len() {
                    rgba[o] = xor[s + 2];
                    rgba[o + 1] = xor[s + 1];
                    rgba[o + 2] = xor[s];
                }
            }
            32 => {
                let s = mask_row * xor_stride + x * 4;
                if s + 4 <= xor.len() {
                    rgba[o] = xor[s + 2];
                    rgba[o + 1] = xor[s + 1];
                    rgba[o + 2] = xor[s];
                    // 32bpp：XOR 自带 per-pixel alpha（抗锯齿阴影光标）
                    alpha_from_xor = Some(xor[s + 3]);
                }
            }
            _ => {}
        }
        // alpha 合成（2026-11 黑色方块根因修复）：32bpp 优先用 XOR 自带
        // alpha（服务端发"AND 全 0 + XOR alpha=0"的光标时，旧逻辑强制 255
        // → 全像素不透明黑 RGB = 黑色方块）。XOR alpha>0 → 直接采用；
        // XOR alpha=0 → 回退 AND 掩码（AND=1 透明 / AND=0 不透明）。
        rgba[o + 3] = match alpha_from_xor {
            Some(a) if a > 0 => a,
            _ => {
                let mut a = 0xFFu8;
                if and_stride > 0 {
                    let idx = mask_row * and_stride + x / 8;
                    if idx < and.len() && (and[idx] >> (7 - (x % 8))) & 1 == 1 {
                        a = 0; // AND=1 → 透明
                    }
                }
                a
            }
        };
    }
    rgba
}

/// 推送光标到上层（空 rgba = 恢复系统默认光标）。
unsafe fn push_cursor(
    ctx: *mut sys::rdpContext,
    hotspot_x: u32,
    hotspot_y: u32,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
) -> sys::BOOL {
    let instance = (*ctx).instance;
    let Some(tx) = route_lookup(instance as usize) else {
        return TRUE;
    };
    let _ = tx.send(FramebufferEvent::Cursor { hotspot_x, hotspot_y, width, height, rgba });
    TRUE
}

/// PointerSystem：远端切换到系统默认光标（箭头/漏斗等）→ 通知前端恢复默认。
unsafe extern "C" fn on_pointer_system(
    ctx: *mut sys::rdpContext,
    _system: *const sys::POINTER_SYSTEM_UPDATE,
) -> sys::BOOL {
    if ctx.is_null() {
        return FALSE;
    }
    // SAFETY：ctx 会话生命周期内有效；路由未注册时静默忽略。
    unsafe { push_cursor(ctx, 0, 0, 0, 0, Vec::new()) }
}

/// PointerColor：光标 PDU（xorBpp=24，update.c 对 PTR_MSG_TYPE_COLOR 硬编码）。
unsafe extern "C" fn on_pointer_color(
    ctx: *mut sys::rdpContext,
    color: *const sys::POINTER_COLOR_UPDATE,
) -> sys::BOOL {
    // SAFETY：color 结构生命周期覆盖本调用。
    unsafe { on_pointer_color_with_bpp(ctx, color, 24) }
}

/// PointerColor/PointerNew 共用实现（xorBpp 参数化以支持单色光标：
/// PointerNew 的 wire xorBpp 可为 1（单色）/24/32）。
unsafe fn on_pointer_color_with_bpp(
    ctx: *mut sys::rdpContext,
    color: *const sys::POINTER_COLOR_UPDATE,
    xor_bpp: u32,
) -> sys::BOOL {
    if ctx.is_null() || color.is_null() {
        return FALSE;
    }
    let (w, h) = ((*color).width, (*color).height);
    if w == 0 || h == 0 || (*color).xorMaskData.is_null() {
        return TRUE;
    }
    let xor_len = (*color).lengthXorMask as usize;
    let and_len = (*color).lengthAndMask as usize;
    // SAFETY：长度字段由 FreeRDP 按 wire PDU 填充，受限切片供纯函数转换。
    let xor = unsafe { std::slice::from_raw_parts((*color).xorMaskData, xor_len) };
    let and = if (*color).andMaskData.is_null() || and_len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts((*color).andMaskData, and_len) }
    };
    let rgba = color_pointer_to_rgba(w, h, xor_bpp, xor, and);
    // SAFETY：ctx 会话生命周期内有效。
    unsafe { push_cursor(ctx, (*color).xPos, (*color).yPos, w, h, rgba) }
}

/// PointerNew：带缓存号的新光标——按其彩色部分走 Color 路径，
/// xorBpp 取 wire 原始值（1=单色箭头/I 形、24/32 彩色）。
///（cacheIndex 缓存复用 α 版不做：远端重复下发成本可接受）。
unsafe extern "C" fn on_pointer_new(
    ctx: *mut sys::rdpContext,
    pointer: *const sys::POINTER_NEW_UPDATE,
) -> sys::BOOL {
    if ctx.is_null() || pointer.is_null() {
        return FALSE;
    }
    // SAFETY：newUpdate.colorPtrAttr 为内嵌彩色部分（非指针）。
    unsafe { on_pointer_color_with_bpp(ctx, &(*pointer).colorPtrAttr, (*pointer).xorBpp as u32) }
}

/// PointerCached：复用缓存光标——前端保留上次形状，维持现状即可。
unsafe extern "C" fn on_pointer_cached(
    _ctx: *mut sys::rdpContext,
    _cached: *const sys::POINTER_CACHED_UPDATE,
) -> sys::BOOL {
    TRUE
}

/// PointerLarge：超大光标 α 版忽略（罕见，仅影响特殊主题）。
unsafe extern "C" fn on_pointer_large(
    _ctx: *mut sys::rdpContext,
    _large: *const sys::POINTER_LARGE_UPDATE,
) -> sys::BOOL {
    TRUE
}

/// Synchronize 更新空实现（update_recv 的 IFCALLRESULT(TRUE) 缺省本来就是
/// TRUE，但显式注册语义更明确）。
unsafe extern "C" fn on_update_synchronize(_ctx: *mut sys::rdpContext) -> sys::BOOL {
    TRUE
}

/// `pAuthenticate`（`<freerdp/freerdp.h>`）。α 版：密码已在 settings 里，
/// 缺失即失败；β 版在此实现交互式凭据获取。
unsafe extern "C" fn on_authenticate(
    _instance: *mut sys::freerdp,
    _username: *mut *mut ::std::os::raw::c_char,
    _password: *mut *mut ::std::os::raw::c_char,
    _domain: *mut *mut ::std::os::raw::c_char,
) -> sys::BOOL {
    FALSE
}

// ---------------------------------------------------------------------------
// 极小化 unsafe 封装：每个函数对应一个 FreeRDP C API
// ---------------------------------------------------------------------------

/// 包装 `freerdp_new()`（`<freerdp/freerdp.h>`）：创建 freerdp instance。
unsafe fn ffi_instance_new() -> *mut sys::freerdp {
    sys::freerdp_new()
}

/// 包装 `freerdp_free()`（`<freerdp/freerdp.h>`）：释放 instance 本体。
/// 必须先 `freerdp_context_free`。
unsafe fn ffi_instance_free(instance: *mut sys::freerdp) {
    sys::freerdp_free(instance)
}

/// 包装 `freerdp_context_new()`（`<freerdp/freerdp.h>`）：
/// 分配 rdpContext 并初始化 settings/input/update 等子结构。
unsafe fn ffi_context_new(instance: *mut sys::freerdp) -> sys::BOOL {
    sys::freerdp_context_new(instance)
}

/// 包装 `freerdp_context_free()`（`<freerdp/freerdp.h>`）。
unsafe fn ffi_context_free(instance: *mut sys::freerdp) {
    sys::freerdp_context_free(instance)
}

/// 包装 `freerdp_connect()`（`<freerdp/freerdp.h>`）：阻塞式建立连接。
unsafe fn ffi_connect(instance: *mut sys::freerdp) -> sys::BOOL {
    sys::freerdp_connect(instance)
}

/// 包装 `freerdp_disconnect()`（`<freerdp/freerdp.h>`）：优雅断开（GracefulDisconnect
/// 序列由 FreeRDP 内部完成），返回 FALSE 时连接可能已异常中断。
unsafe fn ffi_disconnect(instance: *mut sys::freerdp) -> sys::BOOL {
    sys::freerdp_disconnect(instance)
}

/// 包装 `freerdp_abort_connect()`（`<freerdp/freerdp.h>`，返回 BOOL）：
/// 触发 abortEvent，让阻塞中的 connect/事件循环尽快返回。
/// 返回值仅指示信号置位成功与否，α 版忽略（连接终止由事件循环确认）。
unsafe fn ffi_abort_connect(instance: *mut sys::freerdp) {
    let _ = sys::freerdp_abort_connect(instance);
}

/// 包装 `freerdp_shall_disconnect()`（`<freerdp/freerdp.h>`）：
/// 服务端/本地任一方请求断开时返回 TRUE。
unsafe fn ffi_shall_disconnect(instance: *mut sys::freerdp) -> bool {
    sys::freerdp_shall_disconnect(instance) != 0
}

/// `freerdp_input_send_synchronize_event`（`<freerdp/input.h>` 103 行，
/// 2.11.2 已核实；freerdp2-sys 0.2.0 未绑定，手写 extern）。
/// 输入前置：ACTIVE 后必须先发一次 sync（toggle 锁定键状态），
/// 否则部分 Windows 服务端忽略后续键盘事件（实测真机键盘无反应根因）。
#[link(name = "freerdp2")]
extern "C" {
    fn freerdp_input_send_synchronize_event(input: *mut sys::rdpInput, flags: u32) -> i32;
}

/// 包装 `freerdp_check_event_handles()`（`<freerdp/freerdp.h>`）：
/// 处理一次就绪的通道/socket 事件（事件驱动循环的就绪处理入口）。
/// 返回 FALSE 表示连接错误（`context->LastError` 持有 `FREERDP_ERROR_*`）。
unsafe fn ffi_check_event_handles(ctx: *mut sys::rdpContext) -> sys::BOOL {
    sys::freerdp_check_event_handles(ctx)
}

// ---------------------------------------------------------------------------
// 事件驱动事件循环（2026-11 CPU 修复：替换 α 版 20ms 忙轮询）
//
// 符号核实（vcpkg freerdp 2.11.2 / WinPR 真实头文件）：
// - `freerdp_get_event_handles(rdpContext* context, HANDLE* events, DWORD count)`
//   返回填充的句柄数（0 = 错误）。对照 wfreerdp wf_event_loop（wf_client.c）
//   的标准事件驱动模式。freerdp2-sys 0.2.0 未绑定该符号，手写 extern
//  （符号由已链接的 freerdp2.lib 提供；同符号重复 extern 声明合法）。
// - `WaitForMultipleObjects`：kernel32（extern "system"）。
//
// 循环：阻塞等待 FreeRDP 事件句柄（超时 100ms 兜底：保活/断开检测/stop 标志），
// 就绪后 freerdp_check_event_handles 统一处理。空闲时线程阻塞在内核等待上，
// CPU 占用≈0（目标：空闲 app 进程 <5%）。
// ---------------------------------------------------------------------------

/// 事件句柄缓冲上限：FreeRDP 2.x 通道模型下实际句柄数远小于此（wfreerdp 同为 64）。
#[cfg(windows)]
const EVENT_HANDLE_SLOTS: u32 = 64;
/// 等待超时兜底（毫秒）：保底处理 stop/shall_disconnect 与定时事件。
#[cfg(windows)]
const EVENT_WAIT_TIMEOUT_MS: u32 = 100;

// `freerdp_get_event_handles`（`<freerdp/freerdp.h>`，2.11.2 已核实存在）。
#[cfg(windows)]
extern "C" {
    fn freerdp_get_event_handles(
        context: *mut sys::rdpContext,
        events: *mut *mut ::std::os::raw::c_void,
        count: u32,
    ) -> u32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn WaitForMultipleObjects(
        n_count: u32,
        lp_handles: *const *mut ::std::os::raw::c_void,
        b_wait_all: i32,
        dw_milliseconds: u32,
    ) -> u32;
}

/// Windows 事件驱动循环（wfreerdp wf_event_loop 对应物）。
/// 返回 false 表示连接已死（check_event_handles 失败 / 等待失败）。
#[cfg(windows)]
unsafe fn event_loop_wait_windows(ctx: *mut sys::rdpContext) -> bool {
    let mut handles: [*mut ::std::os::raw::c_void; EVENT_HANDLE_SLOTS as usize] =
        [std::ptr::null_mut(); EVENT_HANDLE_SLOTS as usize];
    let n = freerdp_get_event_handles(ctx, handles.as_mut_ptr(), EVENT_HANDLE_SLOTS);
    if n == 0 || n as usize > handles.len() {
        return false; // 取句柄失败：连接异常
    }
    const WAIT_FAILED: u32 = 0xFFFF_FFFF;
    const WAIT_OBJECT_0: u32 = 0;
    let status = WaitForMultipleObjects(n, handles.as_ptr(), 0, EVENT_WAIT_TIMEOUT_MS);
    if status == WAIT_FAILED {
        return false;
    }
    if (WAIT_OBJECT_0..WAIT_OBJECT_0.wrapping_add(n)).contains(&status) {
        // SAFETY：context 在会话生命周期内有效；本线程是唯一驱动线程。
        return ffi_check_event_handles(ctx) != 0;
    }
    // WAIT_TIMEOUT：无事件，仅做兜底周期（stop/shall_disconnect 由外层判断）。
    true
}

// -- settings（`<freerdp/settings.h>`；id 为 FreeRDP_* 常量，形参 usize） --

/// 包装 `freerdp_settings_set_string()`（`<freerdp/settings.h>`）。
/// FreeRDP 会复制字符串，CString 可在调用后立即释放。
unsafe fn settings_set_string(
    settings: *mut sys::rdpSettings,
    id: u32,
    value: &str,
) -> Result<(), RdpError> {
    let c = CString::new(value).map_err(|_| RdpError(format!("settings 字符串含 \\0: {id}")))?;
    if sys::freerdp_settings_set_string(settings, id as usize as _, c.as_ptr()) == 0 {
        return Err(RdpError(format!("freerdp_settings_set_string 失败: id={id}")));
    }
    Ok(())
}

/// 包装 `freerdp_settings_set_uint32()`（`<freerdp/settings.h>`）。
unsafe fn settings_set_uint32(settings: *mut sys::rdpSettings, id: u32, value: u32) -> Result<(), RdpError> {
    if sys::freerdp_settings_set_uint32(settings, id as usize as _, value) == 0 {
        return Err(RdpError(format!("freerdp_settings_set_uint32 失败: id={id}")));
    }
    Ok(())
}

/// 包装 `freerdp_settings_set_bool()`（`<freerdp/settings.h>`）。
unsafe fn settings_set_bool(settings: *mut sys::rdpSettings, id: u32, value: bool) -> Result<(), RdpError> {
    if sys::freerdp_settings_set_bool(settings, id as usize as _, if value { TRUE } else { FALSE })
        == 0
    {
        return Err(RdpError(format!("freerdp_settings_set_bool 失败: id={id}")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// FreerdpSession
// ---------------------------------------------------------------------------

/// 由 [`FreerdpSession`] 拥有并最终释放的 FreeRDP instance/context 指针。
///
/// α 版采用「settings 先同步写入、connect 与事件循环在专用线程」的结构：
/// `freerdp_connect` 是阻塞调用，不能在 UI/调用线程上执行。
struct OwnedInstance {
    ptr: *mut sys::freerdp,
}
// SAFETY：instance 指针本身只是地址；真正的使用（connect/事件驱动）被限制在
// 单一事件循环线程内（FreeRDP 要求单线程驱动），跨线程仅传递指针所有权。
unsafe impl Send for OwnedInstance {}

/// 真实 RDP 会话（native 编译时由 [`crate::create_session`] 返回）。
pub struct FreerdpSession {
    /// connect 前经 `set_password` 注入；`SessionProfile` 目前无 password 字段。
    password: Option<String>,
    /// 建立成功后持有 instance，disconnect/Drop 时按序释放。
    owned: Option<OwnedInstance>,
    /// 事件循环线程句柄（connect 由它执行）。
    worker: Option<JoinHandle<()>>,
    /// 线程间停止信号。
    stop: Arc<AtomicBool>,
    /// 会话存活标志（对外状态查询）。
    connected: Arc<AtomicBool>,
    /// 事件接收端（供上层 `events()`）。
    receiver: event_channel::Receiver,
}

// SAFETY：与 OwnedInstance 相同的约定 —— FFI 指针只在事件循环线程解引用，
// Rust 侧共享的只有 AtomicBool 与 mpsc 通道（均为线程安全类型）。
unsafe impl Send for FreerdpSession {}

impl FreerdpSession {
    /// GHOSTFIX-20261115 残影修复②辅助：发送全屏 RefreshRect PDU
    ///（MS-RDPBCGR 2.2.11.2.1）。仅当 FreeRDP 已注册 RefreshRect 回调且
    /// settings.RefreshRect 能力位开启时发送；失败静默（残影兜底手段，
    /// 主修复是 pending 补发）。前 5 次落日志观测。
    fn send_refresh_rect_full_in(&self, instance: *mut sys::freerdp, log: &dyn Fn(String)) {
        static REFRESH_LOGGED: AtomicU64 = AtomicU64::new(0);
        unsafe {
            let ctx = (*instance).context;
            if ctx.is_null() {
                return;
            }
            let update = (*ctx).update;
            if update.is_null() {
                return;
            }
            let Some(f) = (*update).RefreshRect else {
                let n = REFRESH_LOGGED.fetch_add(1, Ordering::Relaxed);
                if n < 5 {
                    log("GHOSTFIX-20261115 refresh: RefreshRect 回调未注册（跳过）".into());
                }
                return;
            };
            let settings = (*instance).settings;
            let enabled = !settings.is_null()
                && sys::freerdp_settings_get_bool(settings, sys::FreeRDP_RefreshRect as usize as _) != 0;
            if !enabled {
                let n = REFRESH_LOGGED.fetch_add(1, Ordering::Relaxed);
                if n < 5 {
                    log("GHOSTFIX-20261115 refresh: settings.RefreshRect 未开启（跳过）".into());
                }
                return;
            }
            // 区域 = 全帧（gdi 尺寸；gdi 未就绪则用 u16 上限，服务端自行裁剪）
            let gdi = (*ctx).gdi;
            let (w, h) = if !gdi.is_null() {
                (
                    (*gdi).width.max(1).min(0xFFFF) as u16,
                    (*gdi).height.max(1).min(0xFFFF) as u16,
                )
            } else {
                (0xFFFFu16, 0xFFFFu16)
            };
            let area = sys::_RECTANGLE_16 { left: 0, top: 0, right: w, bottom: h };
            let rc = f(ctx, 1, &area);
            let n = REFRESH_LOGGED.fetch_add(1, Ordering::Relaxed);
            if n < 5 {
                log(format!("GHOSTFIX-20261115 refresh: 全屏 RefreshRect rc={rc} {w}x{h}"));
            }
        }
    }

    pub fn new() -> Self {
        let (_tx, receiver) = event_channel::channel();
        Self {
            password: None,
            owned: None,
            worker: None,
            stop: Arc::new(AtomicBool::new(false)),
            connected: Arc::new(AtomicBool::new(false)),
            receiver,
        }
    }

    /// 注入口令（在 [`RdpSession::connect`] 之前调用）。
    ///
    /// 背景约束：`SessionProfile` 模型没有 password 字段（口令属于凭据层，
    /// 不落盘到 profile JSON），所以 α 版由上层显式传入。
    pub fn set_password(&mut self, password: impl Into<String>) {
        self.password = Some(password.into());
    }

    /// 把 [`SessionProfile`] 翻译成 FreeRDP settings 键值
    /// （纯函数部分独立成 `plan_settings` 便于文档化/审查，实际写入在
    /// [`apply_profile_settings`]）。
    ///
    /// 映射：
    /// - `host/port/username/password` -> `FreeRDP_ServerHostname/ServerPort/Username/Password`
    /// - `color_depth`                 -> `FreeRDP_ColorDepth`
    /// - `Resolution::FitWindow`       -> 主显示器当前分辨率（[`crate::fit_window_resolution`]；
    ///    动态分辨率 DisplayControl/MonitorLayout PDU 留待 β，见 README 未实现清单）
    /// - `Resolution::Fullscreen`      -> `FreeRDP_Fullscreen = TRUE`（几何由 FreeRDP
    ///    客户端侧按主显示器解析）
    /// - `Resolution::Custom { w, h }` -> `FreeRDP_DesktopWidth/Height`
    pub(crate) fn plan_settings(profile: &SessionProfile) -> Vec<(String, String)> {
        let mut kv = vec![
            ("ServerHostname".to_string(), profile.host.clone()),
            ("ServerPort".to_string(), profile.port.to_string()),
            ("Username".to_string(), profile.username.clone()),
            ("ColorDepth".to_string(), profile.color_depth.to_string()),
            ("IgnoreCertificate".to_string(), "true".into()),
        ];
        match profile.resolution {
            Resolution::FitWindow => {
                // 主显示器当前分辨率（自适应，2026-11 替换 1280x720 硬编码）。
                let (w, h) = crate::fit_window_resolution();
                kv.push(("DesktopWidth".to_string(), w.to_string()));
                kv.push(("DesktopHeight".to_string(), h.to_string()));
            }
            Resolution::Fullscreen => kv.push(("Fullscreen".to_string(), "true".into())),
            Resolution::Custom { w, h } => {
                kv.push(("DesktopWidth".to_string(), w.to_string()));
                kv.push(("DesktopHeight".to_string(), h.to_string()));
            }
        }
        kv
    }
}

impl Default for FreerdpSession {
    fn default() -> Self {
        Self::new()
    }
}

/// 实际写入 settings（对应 `plan_settings` 的注释映射）。
///
/// 前置条件：`instance.context`/`instance.settings` 已由 `freerdp_context_new`
/// 初始化。所有 FFI 细节见上方 `settings_set_*` 包装函数。
unsafe fn apply_profile_settings(
    instance: *mut sys::freerdp,
    profile: &SessionProfile,
    password: Option<&str>,
) -> Result<(), RdpError> {
    let settings = (*instance).settings;
    if settings.is_null() {
        return Err(RdpError("freerdp_context_new 未初始化 settings".into()));
    }

    // FreeRDP_ServerHostname / ServerPort / Username —— `<freerdp/settings.h>`
    settings_set_string(settings, sys::FreeRDP_ServerHostname, profile.host.trim())?;
    settings_set_uint32(settings, sys::FreeRDP_ServerPort, profile.port as u32)?;
    settings_set_string(settings, sys::FreeRDP_Username, &profile.username)?;
    // 口令：profile 不携带，来自凭据层注入（见 set_password）。
    if let Some(pw) = password {
        settings_set_string(settings, sys::FreeRDP_Password, pw)?;
        // 有密码即自动登录（对应 mstsc 默认行为；FreeRDP_AutoLogonEnabled=704，
        // settings.h 597 行已核实）。
        settings_set_bool(settings, sys::FreeRDP_AutoLogonEnabled, true)?;
    }
    // ClientHostname / ClientProductId（settings.h 547-548 行，id 134/135）：
    // NLA 与服务器会话信息要求客户端标识；缺省时 Windows 计算机名。
    let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "EASYRDP".to_string());
    settings_set_string(settings, sys::FreeRDP_ClientHostname, &hostname)?;
    settings_set_string(settings, sys::FreeRDP_ClientProductId, "EasyRDP")?;
    // Authentication（NLA/CredSSP）保持 FreeRDP 默认启用（FreeRDP_ExtSecurity
    // 默认 FALSE，默认安全协商协议，不显式改写）。
    settings_set_uint32(settings, sys::FreeRDP_ColorDepth, profile.color_depth)?;

    // TILEGHOST-20261116 残影根修：禁用服务端位图缓存。
    // 真机（192.168.200.26）残影形态 = 64x64 tile 网格平铺 + 旧文字叠加，
    // 闪现即被下一批正确更新覆盖——与"memblt order 引用位图缓存条目，
    // 缓存内容失同步/损坏后被合成到 primary_buffer"完全吻合（v0.1.1 背板
    // 直绘修复呈现层无效，证明垃圾来自源头合成缓冲）。
    // 禁用 BitmapCache（含 V3）后服务端经 capability 协商不再发缓存引用
    // order，改发直接位图数据（带宽略增，换取画面正确性；α 阶段稳定优先）。
    settings_set_bool(settings, sys::FreeRDP_BitmapCacheEnabled, false)?;
    settings_set_bool(settings, sys::FreeRDP_BitmapCacheV3Enabled, false)?;

    // α 版：跳过证书校验（内网验收）。β 版换 VerifyX509Certificate 回调。
    settings_set_bool(settings, sys::FreeRDP_IgnoreCertificate, true)?;

    // 密码缺失时依赖 Authenticate 回调 —— α 版该回调固定 FALSE，避免交互挂起。
    (*instance).Authenticate = Some(on_authenticate);

    match profile.resolution {
        Resolution::FitWindow => {
            // 主显示器当前分辨率（自适应）；服务端拒绝时以协商值生效，
            // 实际尺寸经帧事件的 width/height 回传前端。
            let (w, h) = crate::fit_window_resolution();
            settings_set_uint32(settings, sys::FreeRDP_DesktopWidth, w)?;
            settings_set_uint32(settings, sys::FreeRDP_DesktopHeight, h)?;
        }
        Resolution::Fullscreen => {
            // FreeRDP_Fullscreen 只是客户端窗口模式标志（自建 instance 无窗口，
            // 不影响服务端桌面尺寸）。远程分辨率必须显式写 DesktopWidth/Height
            //（2026-11 修复：此前 Fullscreen 分支不写尺寸 → 服务端用默认分辨率，
            // 表现为「全屏模式分辨率不生效」）。取主显示器实际分辨率。
            settings_set_bool(settings, sys::FreeRDP_Fullscreen, true)?;
            let (w, h) = crate::fit_window_resolution();
            settings_set_uint32(settings, sys::FreeRDP_DesktopWidth, w)?;
            settings_set_uint32(settings, sys::FreeRDP_DesktopHeight, h)?;
        }
        Resolution::Custom { w, h } => {
            settings_set_uint32(settings, sys::FreeRDP_DesktopWidth, w)?;
            settings_set_uint32(settings, sys::FreeRDP_DesktopHeight, h)?;
        }
    }

    // FreeRDP_ConfigPath（id 1793）：证书库根目录（known_hosts / server / certs）。
    // 默认 %APPDATA%\freerdp 在受限环境不可写时，certificate store 初始化失败
    // 会让安全协商直接报 ERRCONNECT_SECURITY_NEGO。优先用 %APPDATA%\EasyRDP，
    // 不可写则退回构建目录下（仅影响本机调试，功能等价）。
    let config_dir = std::env::var("APPDATA")
        .ok()
        .map(|d| std::path::PathBuf::from(d).join("EasyRDP\\freerdp-config"))
        .filter(|p| std::fs::create_dir_all(p).is_ok())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target\\freerdp-config")
        });
    let _ = std::fs::create_dir_all(&config_dir);
    settings_set_string(settings, sys::FreeRDP_ConfigPath, &config_dir.to_string_lossy())?;

    // 重定向开关（α 版默认关；profile.redirect 仅剪贴板默认开）。
    settings_set_bool(settings, sys::FreeRDP_RedirectClipboard, profile.redirect.clipboard)?;
    settings_set_bool(settings, sys::FreeRDP_RedirectPrinters, profile.redirect.printers)?;
    settings_set_bool(settings, sys::FreeRDP_RedirectDrives, profile.redirect.drives)?;
    // 注意：FreeRDP 2.x 没有 FreeRDP_RedirectCameras（摄像头重定向是 3.x 特性），
    // profile.redirect.camera 在 α 版被忽略并在 README 中标注。

    Ok(())
}

impl RdpSession for FreerdpSession {
    /// trait 侧口令注入：与固有方法 [`FreerdpSession::set_password`] 等价，
    /// 保证经 `Box<dyn RdpSession>` 调用时口令真正写入（否则会被 trait
    /// 默认空实现静默吞掉）。
    fn set_password(&mut self, password: &str) {
        FreerdpSession::set_password(self, password);
    }

    /// 输入通路：把 [`InputEvent`] 转成 FreeRDP 输入 PDU（FastPath）。
    ///
    /// 符号依据（vcpkg freerdp 2.11.2 头文件实测核实）：
    /// - `<freerdp/input.h>` 110 行：
    ///   `BOOL freerdp_input_send_mouse_event(rdpInput*, UINT16 flags, UINT16 x, UINT16 y)`
    ///   （不是 `freerdp_input_send_mouse`，2.x 无此符号）
    /// - `<freerdp/input.h>` 104 行：
    ///   `BOOL freerdp_input_send_keyboard_event(rdpInput*, UINT16 flags, UINT16 code)`
    ///   （flags+code 两个独立参数，2.x 不用 RDP_INPUT_SCANCODE 结构体）
    /// - `<freerdp/input.h>` 33-47 行：KBD_FLAGS_* / PTR_FLAGS_* 常量
    /// - `<winpr/input.h>` 878 行：
    ///   `DWORD GetVirtualScanCodeFromVirtualKeyCode(DWORD vkcode, DWORD type)`
    ///
    /// 已连接标志检查同时防范 use-after-free：事件循环线程退出后
    /// instance 指针已被释放，未连接时绝不解引用。
    fn send_input(&mut self, event: &InputEvent) -> Result<(), RdpError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(RdpError("未连接：输入事件需在会话建立后发送".into()));
        }
        let instance = self
            .owned
            .as_ref()
            .map(|o| o.ptr)
            .ok_or_else(|| RdpError("未连接：会话句柄不存在".into()))?;
        // SAFETY：connected==true 期间事件循环线程持有 instance；
        // 读取 input 子结构指针（freerdp_context_new 已初始化）。
        let input = unsafe { (*instance).input };
        if input.is_null() {
            return Err(RdpError("FreeRDP input 子结构未初始化".into()));
        }
        let dbg = input_debug_enabled();
        // 持久化输入日志（GUI 无 stderr，文件是唯一观测通道；默认开启）。
        let log = |s: String| {
            crate::debug_log(&s);
            if dbg {
                eprintln!("{s}");
            }
        };

        match event {
            InputEvent::MouseMove { x, y } => unsafe {
                let rc = sys::freerdp_input_send_mouse_event(input, PTR_FLAGS_MOVE, *x as u16, *y as u16);
                log(format!("[input] move x={x} y={y} rc={rc}"));
                if rc == 0 {
                    return Err(RdpError("发送鼠标移动失败".into()));
                }
            },
            InputEvent::MouseButton { button, down, x, y } => {
                let btn = mouse_button_flags(button)
                    .ok_or_else(|| RdpError(format!("未知鼠标按键: {button}")))?;
                // 按键事件必须携带当前坐标（MS-RDPEPTR：button 事件含位置）。
                let flags = if *down { btn | PTR_FLAGS_DOWN } else { btn };
                unsafe {
                    let rc = sys::freerdp_input_send_mouse_event(input, flags, *x as u16, *y as u16);
                    log(format!("[input] {button} down={down} flags={flags:#06x} x={x} y={y} rc={rc}"));
                    if rc == 0 {
                        return Err(RdpError(format!("发送鼠标按键 {button} 失败")));
                    }
                }
                // GHOSTFIX-20261115 残影修复②：按键抬起（拖拽/划选结束）→
                // 发送 RefreshRect PDU 请求服务端重发受影响区域，兜底消除
                // 增量更新未覆盖的残留像素。受 FreeRDP RefreshRect 能力位
                // 约束（协商未开启则跳过——与 mstsc 行为一致）。
                if !*down {
                    self.send_refresh_rect_full_in(instance, &log);
                }
            }
            InputEvent::Wheel { delta } => {
                if let Some(flags) = encode_wheel_flags(*delta, false) {
                    unsafe {
                        let rc = sys::freerdp_input_send_mouse_event(input, flags, 0, 0);
                        log(format!("[input] wheel delta={delta} flags={flags:#06x} rc={rc}"));
                        if rc == 0 {
                            return Err(RdpError("发送滚轮事件失败".into()));
                        }
                    }
                }
            }
            InputEvent::WheelHorizontal { delta } => {
                // 横向滚轮：HWHEEL 位，rotation/符号编码规则与纵向一致（正值向右）
                if let Some(flags) = encode_wheel_flags(*delta, true) {
                    unsafe {
                        let rc = sys::freerdp_input_send_mouse_event(input, flags, 0, 0);
                        log(format!("[input] hwheel delta={delta} flags={flags:#06x} rc={rc}"));
                        if rc == 0 {
                            return Err(RdpError("发送横向滚轮事件失败".into()));
                        }
                    }
                }
            }
            InputEvent::Key { code, down } => {
                let vk = parse_vk_code(code)
                    .ok_or_else(|| RdpError(format!("键码格式非法（期望 VK:0xNN）: {code}")))?;
                let (scan, extended) = vk_to_rdp_scancode(vk)
                    .ok_or_else(|| RdpError(format!("VK 0x{vk:02X} 无对应扫描码")))?;
                let mut flags = 0u16;
                if extended {
                    flags |= KBD_FLAGS_EXTENDED;
                }
                if !*down {
                    flags |= KBD_FLAGS_RELEASE;
                }
                unsafe {
                    let rc = sys::freerdp_input_send_keyboard_event(input, flags, scan as u16);
                    log(format!("[input] key vk=0x{vk:02X} scan=0x{scan:02X} ext={extended} down={down} flags={flags:#06x} rc={rc}"));
                    if rc == 0 {
                        return Err(RdpError(format!("发送键盘事件 VK:0x{vk:02X} 失败")));
                    }
                }
            }
            InputEvent::Sas => {
                // Ctrl+Alt+Del → 远端 SAS：扩展扫描码序列（0xE0 前缀 = EXT 位）。
                // 顺序对齐实体键盘 SAS 序列：Ctrl↓ Alt↓ Del↓ Del↑ Alt↑ Ctrl↑。
                // TermSrv 收到该序列后在服务端会话内合成 SAS（本机 SendSAS
                // 只作用于本地会话，不能用）。
                let seq: [(u16, bool); 6] = [
                    (0x1D, true),  // Ctrl ext down
                    (0x38, true),  // Alt ext down
                    (0x53, true),  // Del ext down
                    (0x53, false), // Del ext up
                    (0x38, false), // Alt ext up
                    (0x1D, false), // Ctrl ext up
                ];
                for (scan, down) in seq {
                    let flags = if down { 0u16 } else { KBD_FLAGS_RELEASE } | KBD_FLAGS_EXTENDED;
                    unsafe {
                        let rc = sys::freerdp_input_send_keyboard_event(input, flags, scan);
                        log(format!("[input] sas scan=0x{scan:02X} down={down} flags={flags:#06x} rc={rc}"));
                        if rc == 0 {
                            return Err(RdpError(format!("发送 SAS 序列 scan=0x{scan:02X} 失败")));
                        }
                    }
                }
                log("[input] SAS 序列发送完成（Ctrl+Alt+Del → 远端）".into());
            }
        }
        Ok(())
    }

    /// 本地 -> 远端键盘 LED 同步（sync PDU）。
    fn sync_keyboard_leds(&mut self, flags: u32) -> Result<(), RdpError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(RdpError("未连接：LED 同步需在会话建立后发送".into()));
        }
        let instance = self
            .owned
            .as_ref()
            .map(|o| o.ptr)
            .ok_or_else(|| RdpError("未连接：会话句柄不存在".into()))?;
        // SAFETY：connected==true 期间事件循环线程持有 instance；
        // 读取 input 子结构指针（freerdp_context_new 已初始化）。
        let input = unsafe { (*instance).input };
        if input.is_null() {
            return Err(RdpError("FreeRDP input 子结构未初始化".into()));
        }
        // SAFETY：input 子结构 ACTIVE 后可发送（SynchronizeEvent 带_flags 参数，
        // <freerdp/input.h> 103 行，2.11.2 已核实）。
        let rc = unsafe { freerdp_input_send_synchronize_event(input, flags) };
        LOCAL_NUMLOCK_ON.store(flags & 0x1 != 0, Ordering::SeqCst);
        crate::debug_log(&format!("[input] sync(前端触发 LED 同步) flags={flags:#06x} rc={rc}"));
        if rc == 0 {
            return Err(RdpError(format!("发送键盘 LED 同步失败 flags={flags:#06x}")));
        }
        Ok(())
    }

    /// 本地 -> 远端剪贴板文本（α 主路径 CLIPRDR；不可用降级方案 B 键入模拟）。
    ///
    /// 主路径：文本进入本地剪贴板缓存后向远端公告 CLIPRDR FormatList
    /// （CF_UNICODETEXT）；远端粘贴时经 ServerFormatDataRequest 向我们取数据，
    /// 由 [`on_cliprdr_server_format_data_request`] 回传 UTF-16LE。
    /// 方案 B（CLIPRDR 上下文缺失，如插件加载失败/尚未 MonitorReady 之前）：
    /// 文本按 US 101 布局转 VK 键入序列逐字 `send_input`（`typing.rs`）；
    /// 含非 ASCII 字符时该路径无法表达，返回 Err 并列出需剪贴板通道的字符。
    fn send_clipboard_text(&mut self, text: &str) -> Result<(), RdpError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(RdpError("未连接：剪贴板同步需在会话建立后使用".into()));
        }
        let instance = self
            .owned
            .as_ref()
            .map(|o| o.ptr)
            .ok_or_else(|| RdpError("未连接：会话句柄不存在".into()))?;
        let instance_addr = instance as usize;
        clipboard_text_store(instance_addr, text.to_string());
        // CLIPSYNC 回环防护登记：本地监听器看到同文本时跳过回发
        crate::clip_sync_note(&text);
        let cliprdr = cliprdr_ctx_lookup(instance_addr);
        if !cliprdr.is_null() {
            // SAFETY：connected 期间 CliprdrClientContext 有效（插件随连接分配）。
            if unsafe { cliprdr_announce_local_text(cliprdr) }.is_ok() {
                return Ok(());
            }
            // 公告失败（如远端尚未 MonitorReady）-> 落到方案 B
        }
        // -- 方案 B：VK 键入模拟 --
        let steps = crate::typing::text_to_typing_steps(text);
        for ev in crate::typing::typing_steps_to_key_events(&steps) {
            RdpSession::send_input(self, &ev)?;
        }
        let unsupported = crate::typing::unsupported_chars(text);
        if !unsupported.is_empty() {
            let shown: String = unsupported.iter().take(8).collect();
            return Err(RdpError(format!(
                "方案 B 键入模拟不支持非 ASCII 字符（{shown}…），需 CLIPRDR 通道就绪后再同步"
            )));
        }
        Ok(())
    }

    fn connect(&mut self, profile: &SessionProfile) -> Result<(), RdpError> {
        if self.owned.is_some() || self.worker.is_some() {
            return Err(RdpError("会话已建立，请先 disconnect".into()));
        }
        if profile.host.trim().is_empty() {
            return Err(RdpError("host 不能为空".into()));
        }

        // -- 阶段 -1：WLog 文件日志环境（早于任何 FreeRDP 首次使用，见函数注释）--
        setup_wlog_file_logging();
        // 输入调试日志按会话截断（新会话重新开始记录，便于对齐 GUI 复测）。
        DEBUG_LOG_INIT.get_or_init(|| {
            if let Some(dir) = std::env::var_os("APPDATA").map(std::path::PathBuf::from) {
                let _ = std::fs::remove_file(dir.join("EasyRDP").join("debug.log"));
            }
            // 受限环境的回退路径（见 lib::debug_log）一并截断
            let _ = std::fs::remove_file(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("debug.log"),
            );
            crate::debug_log(&format!(
                "=== 新会话 {}@{}:{} ===",
                profile.username,
                profile.host.trim(),
                profile.port
            ));
        });

        // -- 阶段 0：OpenSSL legacy provider（NLA/NTLM 需要 MD4；幂等，OnceLock） --
        // 失败仅记录（ensure 内部 eprintln），不阻断连接：legacy 缺失时
        // 非 NLA 会话仍可工作。
        let _ = ensure_openssl_legacy_provider();
        // Winsock 初始化（getaddrinfo 前置，见 ensure_winsock_started 注释）。
        ensure_winsock_started();
        // 静态通道 addin provider（cliprdr 等静态插件的入口解析，必须注册）。
        register_static_addin_provider();

        // -- 阶段 1：创建 instance/context 并注册回调（同步、无 I/O） --
        // SAFETY：freerdp_new 返回裸指针，失败（NULL）立即报错；
        // 后续每一步失败都走 teardown_task 释放。
        let instance = unsafe { ffi_instance_new() };
        if instance.is_null() {
            return Err(RdpError("freerdp_new 失败".into()));
        }
        let mut teardown = Teardown {
            instance,
            done: false,
        };

        // 注册连接状态回调（必须在 freerdp_connect 之前）。
        // SAFETY：仅写入 instance 结构体字段，同线程。
        unsafe {
            (*instance).PreConnect = Some(on_pre_connect);
            (*instance).PostConnect = Some(on_post_connect);
        }

        // SAFETY：freerdp_context_new 要求 instance 由 freerdp_new 创建且未初始化过。
        if unsafe { ffi_context_new(instance) } == 0 {
            teardown.release();
            return Err(RdpError("freerdp_context_new 失败".into()));
        }

        // -- 阶段 2：settings 映射（同步、无 I/O） --
        // SAFETY：context_new 之后 settings 已初始化。
        if let Err(e) = unsafe { apply_profile_settings(instance, profile, self.password.as_deref()) }
        {
            teardown.release();
            return Err(e);
        }

        // -- 阶段 2.5：订阅通道连接事件（CLIPRDR 前置） --
        // cliprdr 插件的 CliprdrClientContext 在 freerdp_connect 内部
        // （freerdp_channels_post_connect）经 "ChannelConnected" pubsub 事件送达，
        // 订阅必须发生在 connect 之前（对照 wfreerdp wf_pre_connect）。
        // SAFETY：pubSub 由 freerdp_context_new 初始化，"ChannelConnected"/
        // "ChannelDisconnected" 事件类型由 FreeRDP 内置事件表注册；
        // PubSub_Subscribe 为 winpr2.dll 导出（WINPR_API）。
        unsafe {
            let pubsub = (*(*instance).context).pubSub;
            if sys::PubSub_Subscribe(
                pubsub,
                b"ChannelConnected\0".as_ptr() as *const ::std::os::raw::c_char,
                Some(on_channel_connected_raw),
            ) != 0
            {
                teardown.release();
                return Err(RdpError("订阅 ChannelConnected 事件失败".into()));
            }
            sys::PubSub_Subscribe(
                pubsub,
                b"ChannelDisconnected\0".as_ptr() as *const ::std::os::raw::c_char,
                Some(on_channel_disconnected_raw),
            );
        }

        // -- 阶段 3：注册事件路由 + 事件循环线程（freerdp_connect 在线程内执行） --
        // 本会话的事件通道：Sender 交给 FFI 回调路由表，Receiver 归 self.receiver，
        // 上层经 RdpSession::events() 消费。
        let (tx, rx) = event_channel::channel();
        let instance_addr = instance as usize;
        route_register(instance_addr, tx);

        let stop = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicBool::new(false));
        let host = profile.host.trim().to_string();

        let owned = OwnedInstance { ptr: instance };
        // instance 的 freerdp_free 责任移交主线程 disconnect()（join 之后）：
        // worker 只做 disconnect/gdi_free/context_free，避免 worker 释放后
        // 主线程 abort_connect 访问悬垂指针（0xC0000005 崩溃，实测修复）。
        teardown.done = true;

        let stop_t = Arc::clone(&stop);
        let connected_t = Arc::clone(&connected);
        // teardown 所有权移交线程：connect 失败/成功后的释放都在线程内完成，
        // disconnect() 通过 stop + abort_connect 让线程退出并释放。
        let worker = std::thread::spawn(move || {
            run_session(owned, stop_t, connected_t, instance_addr, host);
        });

        self.owned = Some(OwnedInstance { ptr: instance });
        self.worker = Some(worker);
        self.stop = stop;
        self.connected = connected;
        self.receiver = rx;
        Ok(())
    }

    fn disconnect(&mut self) -> Result<(), RdpError> {
        // GracefulDisconnect 顺序（幂等）：
        //   1) stop 标志 + freerdp_abort_connect 唤醒阻塞中的 connect/事件循环
        //   2) join 事件循环线程（线程内：freerdp_disconnect -> 释放 instance）
        //   3) 注销事件路由
        // SAFETY：instance 指针此时仍被 worker 线程使用，这里只解引用它
        // 调 abort_connect（FreeRDP abortEvent 写入是线程安全的）。
        self.stop.store(true, Ordering::SeqCst);
        // abort_connect 仅在会话仍连接时调用：worker 失败路径退出后 instance
        // 的 context 已释放，且 instance_free 由本函数 join 之后统一执行，
        // 提前对已失效指针调 abort_connect 会访问悬垂内存（实测 0xC0000005）。
        if let Some(owned) = &self.owned {
            if self.connected.load(Ordering::SeqCst) {
                unsafe { ffi_abort_connect(owned.ptr) };
            }
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        // instance 本体释放（context/gdi 已由 worker 在退出前释放）。
        if let Some(owned) = self.owned.take() {
            // SAFETY：worker 已 join 退出，无并发使用者。
            unsafe { ffi_instance_free(owned.ptr) };
        }
        self.connected.store(false, Ordering::SeqCst);
        // 路由注销放在 join 之后，确保线程内不会再往通道发送。
        Ok(())
    }

    fn events(&self) -> Option<&event_channel::Receiver> {
        Some(&self.receiver)
    }
}

impl Drop for FreerdpSession {
    fn drop(&mut self) {
        let _ = self.disconnect();
    }
}

/// 读取本地键盘 LED toggle 状态并组装 RDP sync flags
///（NUM_LOCK=0x1 / CAPS_LOCK=0x2 / SCROLL_LOCK=0x4；KANA 不适用 Windows）。
/// GetKeyState 低位 = toggle 状态（`<winuser.h>`，windows-sys
/// Win32_UI_WindowsAndMessaging 已在依赖中）。非 Windows 恒 0。
#[cfg(windows)]
fn local_keyboard_led_flags() -> u32 {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, VK_CAPITAL, VK_NUMLOCK, VK_SCROLL,
    };
    // SAFETY：user32 标准查询，无副作用（不切换键盘状态）。
    unsafe {
        let mut f = 0u32;
        if GetKeyState(VK_NUMLOCK as i32) & 1 != 0 {
            f |= 0x1;
        }
        if GetKeyState(VK_CAPITAL as i32) & 1 != 0 {
            f |= 0x2;
        }
        if GetKeyState(VK_SCROLL as i32) & 1 != 0 {
            f |= 0x4;
        }
        f
    }
}

#[cfg(not(windows))]
fn local_keyboard_led_flags() -> u32 {
    0
}

/// 连接成功后发键盘 sync：LED 状态取自本地（mstsc 同款行为），
/// 保证服务端 NumLock/CapsLock 与客户端一致。
unsafe fn send_initial_keyboard_sync(instance: *mut sys::freerdp) {
    let input = unsafe { (*instance).input };
    if input.is_null() {
        return;
    }
    let flags = local_keyboard_led_flags();
    LOCAL_NUMLOCK_ON.store(flags & 0x1 != 0, Ordering::SeqCst);
    // SAFETY：input 子结构由 context_new 初始化，ACTIVE 后可发送。
    let rc = unsafe { freerdp_input_send_synchronize_event(input, flags) };
    crate::debug_log(&format!("[input] sync(连接后 LED 同步) flags={flags:#06x} rc={rc}"));
    if input_debug_enabled() {
        eprintln!("[input-debug] sync event flags={flags:#06x} rc={rc}");
    }
}

/// 结构化释放守卫：任何一步失败时保证 `freerdp_context_free` + `freerdp_free`
/// 恰好执行一次（对应 C 侧常见的 goto teardown 模式）。
struct Teardown {
    instance: *mut sys::freerdp,
    done: bool,
}

// SAFETY：Teardown 只携带指针地址；其释放只发生在独占该指针的
// 事件循环线程内（run_session 收尾）或 connect 同步路径（无线程并发）。
unsafe impl Send for Teardown {}

impl Teardown {
    /// 立即释放（connect 同步路径失败时）。
    fn release(mut self) {
        self.done = true;
        // SAFETY：instance 已不再被任何线程使用（connect 尚未进入线程阶段）。
        unsafe {
            ffi_context_free(self.instance);
            ffi_instance_free(self.instance);
        }
    }
}

impl Drop for Teardown {
    fn drop(&mut self) {
        if !self.done {
            // SAFETY：与 release 相同的前置条件（同步路径，无并发使用）。
            unsafe {
                ffi_context_free(self.instance);
                ffi_instance_free(self.instance);
            }
        }
    }
}

/// 事件循环线程体：freerdp_connect -> 轮询 freerdp_check_event_handles ->
/// 退出时按序释放 context/gdi（instance 本体由主线程 disconnect() 在
/// join 之后统一 `freerdp_free`，见 disconnect 注释）。
fn run_session(
    owned: OwnedInstance,
    stop: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    instance_addr: usize,
    host: String,
) {
    let instance = owned.ptr;

    // SAFETY：instance 由本线程独占（调用线程已放弃所有权）。
    let ok = unsafe { ffi_connect(instance) } != 0;
    if !ok {
        connected.store(false, Ordering::SeqCst);
        if let Some(tx) = route_lookup(instance_addr) {
            // 失败原因精确化：追加 freerdp_get_last_error 的数字与字符串
            // （必须在 disconnect/context_free 之前读取 context）。
            let ctx = unsafe { (*instance).context };
            let detail = last_error_reason(ctx);
            let _ = tx.send(FramebufferEvent::Disconnected(format!(
                "freerdp_connect 到 {host} 失败（检查网络/凭据/证书策略）{detail}"
            )));
        }
        route_unregister(instance_addr);
        cliprdr_unregister(instance_addr);
        // connect 失败后仍需清理：disconnect（清理半开连接状态，幂等）->
        // context_free（instance_free 由主线程 join 后执行）。
        // SAFETY：连接失败路径，无并发使用者。
        unsafe {
            ffi_disconnect(instance);
            ffi_context_free(instance);
        }
        return;
    }
    connected.store(true, Ordering::SeqCst);

    let ctx = unsafe { (*instance).context };
    let ctx_addr = ctx as usize;

    // ACTIVE 后发键盘 sync（LED 状态同步）：本地 NumLock/CapsLock/ScrollLock
    // toggle 状态 → 服务端 LED 同步（mstsc 同款行为；缺失时部分服务端忽略
    // 键盘 PDU，且小键盘数字依赖服务端 NumLock 状态正确）。
    unsafe { send_initial_keyboard_sync(instance) };
    // 延迟重发由下方事件循环内定时触发（避免跨线程 instance 生命周期竞争）。
    let mut led_resync_done = false;
    let active_at = std::time::Instant::now();

    // 事件循环：Windows 走 freerdp_get_event_handles + WaitForMultipleObjects
    // 的事件驱动模式（空闲阻塞在内核等待，无空转）；非 Windows 平台暂留
    // 20ms 轮询兜底（β 版接 poll/epoll）。
    loop {
        if stop.load(Ordering::SeqCst) || unsafe { ffi_shall_disconnect(instance) } {
            break;
        }
        // ACTIVE 后 1.5s 重发 LED sync（首次 sync 在会话刚激活时发出，可能
        // 被服务端忽略——输入通道未就绪；重发确保 NumLock 状态生效，小键盘
        // 数字键才被服务端解释为数字。2026-11 Numpad 实证）。
        if !led_resync_done && active_at.elapsed() >= Duration::from_millis(1500) {
            led_resync_done = true;
            unsafe { send_initial_keyboard_sync(instance) };
        }
        #[cfg(windows)]
        {
            // SAFETY：context 在会话生命周期内有效；本线程是唯一驱动线程。
            let alive = unsafe { event_loop_wait_windows(ctx) };
            if !alive {
                break;
            }
        }
        #[cfg(not(windows))]
        {
            // SAFETY：context 在会话生命周期内有效；本线程是唯一驱动线程。
            let alive = unsafe { ffi_check_event_handles(ctx) } != 0;
            if !alive {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    connected.store(false, Ordering::SeqCst);
    if let Some(tx) = route_lookup(instance_addr) {
        let reason = if stop.load(Ordering::SeqCst) {
            "本地请求断开".to_string()
        } else {
            // 服务端断开/连接错误：同样带上 last error 细节。
            let ctx = unsafe { (*instance).context };
            format!("服务端断开或连接错误{}", last_error_reason(ctx))
        };
        let _ = tx.send(FramebufferEvent::Disconnected(reason));
    }
    route_unregister(instance_addr);
    gdi_prev_unregister(ctx_addr);
    composer_unregister(ctx_addr);
    ctx_ptr_unregister(ctx_addr); // GHOSTFIX2：ctx 指针登记随之失效
    emitter_unregister(ctx_addr); // drop 发送端 → 发射线程退出
    if let Some(map) = ROUTE_BY_CTX.lock().expect("ROUTE_BY_CTX poisoned").as_mut() {
        map.remove(&ctx_addr);
    }
    // 清理剪贴板缓存与 CLIPRDR 上下文登记（pubsub 订阅随 context_free 释放）
    cliprdr_unregister(instance_addr);

    // SAFETY：事件循环已退出，本线程是 instance 的最后使用者；
    // freerdp_disconnect（GracefulDisconnect）必须先于 gdi_free/context_free/free
    // （gdi_free 释放 GDI 合成层，对应 on_post_connect 的 gdi_init；
    // 连接失败时 gdi 未初始化，gdi_free 内部对 NULL 安全，幂等）。
    unsafe {
        ffi_disconnect(instance);
        sys::gdi_free(instance);
        ffi_context_free(instance);
    } // freerdp_free 由主线程 disconnect() 在 join 后执行
}

// ---------------------------------------------------------------------------
// 单测（仅 native 构建时编译；默认 CI 不执行）
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use easyrdp_core::profile::Resolution;

    #[test]
    fn plan_settings_maps_host_port_and_custom_resolution() {
        let mut p = SessionProfile::default();
        p.host = "10.0.0.8".into();
        p.port = 3390;
        p.username = "ops".into();
        p.resolution = Resolution::Custom { w: 1920, h: 1080 };
        let kv = FreerdpSession::plan_settings(&p);
        assert!(kv.contains(&("ServerHostname".to_string(), "10.0.0.8".to_string())));
        assert!(kv.contains(&("ServerPort".to_string(), "3390".to_string())));
        assert!(kv.contains(&("DesktopWidth".to_string(), "1920".to_string())));
    }

    #[test]
    fn plan_settings_fitwindow_uses_primary_monitor_resolution() {
        let mut p = SessionProfile::default();
        p.host = "h".into();
        p.resolution = Resolution::FitWindow;
        let kv = FreerdpSession::plan_settings(&p);
        let (w, h) = crate::fit_window_resolution();
        assert!(kv.contains(&("DesktopWidth".to_string(), w.to_string())));
        assert!(kv.contains(&("DesktopHeight".to_string(), h.to_string())));
    }

    #[test]
    fn parse_vk_code_accepts_vk_prefix_hex() {
        assert_eq!(parse_vk_code("VK:0x41"), Some(0x41));
        assert_eq!(parse_vk_code("VK:0xff"), Some(0xFF));
        assert_eq!(parse_vk_code("vk:0x1B"), Some(0x1B));
        assert_eq!(parse_vk_code("KeyA"), None);
        assert_eq!(parse_vk_code("VK:zz"), None);
        assert_eq!(parse_vk_code(""), None);
    }

    #[test]
    fn vk_to_rdp_scancode_maps_standard_and_extended_keys() {
        // GetVirtualScanCodeFromVirtualKeyCode(vk, 4)：AT 增强 101 键表。
        // A=0x41 -> 0x1E（非扩展）；Enter=0x0D -> 0x1C；右方向键 0x27 ->
        // 扩展键 0x4D（KBDEXT 位）。
        let (scan, ext) = vk_to_rdp_scancode(0x41).expect("VK_A");
        assert_eq!((scan, ext), (0x1E, false));
        let (scan, ext) = vk_to_rdp_scancode(0x0D).expect("VK_RETURN");
        assert_eq!((scan, ext), (0x1C, false));
        let (scan, ext) = vk_to_rdp_scancode(0x27).expect("VK_RIGHT");
        assert_eq!((scan, ext), (0x4D, true));
        let (scan, ext) = vk_to_rdp_scancode(0xA3).expect("VK_RCONTROL");
        assert_eq!((scan, ext), (0x1D, true));
        assert!(vk_to_rdp_scancode(0xFF).is_none());
    }

    /// 2026-11 修饰键修复回归：通用 VK_SHIFT/CONTROL/MENU（winpr KBD4X
    /// 未登记，查表返回 0）必须归一为 L 变体后查表成功——真实 bug 锚点
    ///（此前该测试只在 --features native 下运行，而 CI 只跑默认特性，
    /// 修饰键查表失败从未被捕获；真机 debug.log `VK 0x12 无对应扫描码`）。
    #[test]
    fn vk_to_rdp_scancode_maps_generic_modifier_keys() {
        // LCONTROL=0xA2 -> 0x1D 非扩展；LMENU=0xA4 -> 0x38 非扩展；
        // LSHIFT=0xA0 -> 0x2A 非扩展（KBD4X 表）。
        let (scan, ext) = vk_to_rdp_scancode(0x11).expect("VK_CONTROL(通用)");
        assert_eq!((scan, ext), (0x1D, false));
        let (scan, ext) = vk_to_rdp_scancode(0x12).expect("VK_MENU(通用)");
        assert_eq!((scan, ext), (0x38, false));
        let (scan, ext) = vk_to_rdp_scancode(0x10).expect("VK_SHIFT(通用)");
        assert_eq!((scan, ext), (0x2A, false));
    }

    /// 特殊键全量表（对照 winpr scancode.c KBD4X 表逐键核实，2026-11）：
    /// EXT 类（Home=0x47/End=0x4F/PgUp=0x49/PgDn=0x51/Ins=0x52/Del=0x53/
    /// 方向键 0x48/0x4B/0x4D/0x50、Win=0x5B/0x5C、NumpadEnter=0x1C）必须
    /// 带 KBDEXT；Numpad 数字不带 EXT。
    #[test]
    fn vk_to_rdp_scancode_maps_special_keys_full_table() {
        let cases: &[(u32, u8, bool)] = &[
            (0x24, 0x47, true),  // Home
            (0x23, 0x4F, true),  // End
            (0x21, 0x49, true),  // PageUp
            (0x22, 0x51, true),  // PageDown
            (0x2D, 0x52, true),  // Insert
            (0x2E, 0x53, true),  // Delete
            (0x26, 0x48, true),  // Up
            (0x25, 0x4B, true),  // Left
            (0x27, 0x4D, true),  // Right
            (0x28, 0x50, true),  // Down
            (0x5B, 0x5B, true),  // LWin（工具条 Win 按钮路径）
            (0x5C, 0x5C, true),  // RWin
            (0x5D, 0x5D, true),  // Apps/ContextMenu
            (0x90, 0x45, false), // NumLock（winpr T 表非 EXT，2026-11 修复）
            (0x91, 0x46, false), // ScrollLock（winpr T 表非 EXT，2026-11 修复）
            (0x14, 0x3A, false), // CapsLock
            (0x61, 0x4F, false), // Numpad1（硬编码 AT 码位，2026-11 修复）
            (0x65, 0x4C, false), // Numpad5
            (0x69, 0x49, false), // Numpad9（此前 winpr 反查歧义→PgUp）
        ];
        for &(vk, scan, ext) in cases {
            LOCAL_NUMLOCK_ON.store(false, Ordering::SeqCst); // Numpad 映射走 OFF 分支
            let got = vk_to_rdp_scancode(vk)
                .unwrap_or_else(|| panic!("VK 0x{vk:02X} 无对应扫描码（winpr KBD4X 反查失败）"));
            assert_eq!(got, (scan, ext), "VK 0x{vk:02X} 转换不符");
        }
        LOCAL_NUMLOCK_ON.store(false, Ordering::SeqCst); // 收尾复位
        // NumpadEnter：0x0D 带 EXT 位（伪标记）→ 0x1C EXT（与主 Enter 区分）
        let got = vk_to_rdp_scancode(0x0100 | 0x0D).expect("VK_RETURN(Numpad EXT)");
        assert_eq!(got, (0x1C, true));
    }

    /// Numpad 数字确定性映射（2026-11）：本地 NumLock ON → 主键盘数字码位
    ///（0x02-0x0A），消除服务端 LED 状态不确定性（真机实证：0x47 非 EXT 被
    /// 服务端当 Home）。OFF → 保持导航码位（与本地行为一致）。
    #[test]
    fn vk_to_rdp_scancode_numpad_digit_uses_local_numlock() {
        LOCAL_NUMLOCK_ON.store(true, Ordering::SeqCst);
        assert_eq!(vk_to_rdp_scancode(0x61), Some((0x02, false))); // Numpad1 → 主键盘 1
        assert_eq!(vk_to_rdp_scancode(0x69), Some((0x0A, false))); // Numpad9 → 主键盘 9
        assert_eq!(vk_to_rdp_scancode(0x60), Some((0x0B, false))); // Numpad0 → 主键盘 0
        LOCAL_NUMLOCK_ON.store(false, Ordering::SeqCst);
        assert_eq!(vk_to_rdp_scancode(0x61), Some((0x4F, false))); // OFF → End 码位
        assert_eq!(vk_to_rdp_scancode(0x67), Some((0x47, false))); // OFF → Home 码位
        // Numpad +/-/*（不受 NumLock 影响，winpr 反查；2026-11 静默吞键修复）
        assert_eq!(vk_to_rdp_scancode(0x6B), Some((0x4E, false))); // NumpadAdd
        assert_eq!(vk_to_rdp_scancode(0x6D), Some((0x4A, false))); // NumpadSubtract
        assert_eq!(vk_to_rdp_scancode(0x6A), Some((0x37, false))); // NumpadMultiply
    }

    /// 滚轮编码（2026-11 对照 wfreerdp wf_event.c 官方编码 + ±120 归一）：
    /// rotation 字段携带 Windows 原始 delta 单位；负值 = NEGATIVE 标志 +
    /// `0x100 + delta` 8 位二补码；横向用 HWHEEL 位。浏览器 100/格 归一 120。
    #[test]
    fn wheel_flags_use_delta_units_and_negative_flag() {
        // 上滚 120：WHEEL | 120 = 0x0278（rotation 携带原始 delta 单位）
        assert_eq!(encode_wheel_flags(120, false), Some(PTR_FLAGS_WHEEL | 120));
        // 上滚 100（浏览器 delta）：归一到 120 → WHEEL | 120
        assert_eq!(encode_wheel_flags(100, false), Some(PTR_FLAGS_WHEEL | 120));
        // 下滚 -100：归一 -120 → NEGATIVE + (0x100-120=0x88)
        assert_eq!(
            encode_wheel_flags(-100, false),
            Some(PTR_FLAGS_WHEEL | PTR_FLAGS_WHEEL_NEGATIVE | 0x88)
        );
        // 大量滚动钳 9 位有符号上限 255
        assert_eq!(
            encode_wheel_flags(120 * 999, false),
            Some(PTR_FLAGS_WHEEL | 255)
        );
        assert_eq!(encode_wheel_flags(0, false), None);
        // 横向：HWHEEL 位
        assert_eq!(
            encode_wheel_flags(120, true),
            Some(PTR_FLAGS_HWHEEL | 120)
        );
    }

    #[test]
    fn mouse_button_flags_map_names() {
        assert_eq!(mouse_button_flags("left"), Some(PTR_FLAGS_BUTTON1));
        assert_eq!(mouse_button_flags("right"), Some(PTR_FLAGS_BUTTON2));
        assert_eq!(mouse_button_flags("middle"), Some(PTR_FLAGS_BUTTON3));
        assert_eq!(mouse_button_flags("x1"), None);
    }

    /// 单色光标（xorBpp=1 双平面）：xor=0 黑 / 1 白，and=1 透明。
    /// 2x1 光标：左像素黑透明（and=1）、右像素白不透明。
    #[test]
    fn color_pointer_mono_black_white_transparent() {
        let xor = [0b0100_0000u8, 0x00]; // 1bpp MSB first：bit7=0（左黑）、bit6=1（右白），pad
        let and = [0b1000_0000u8, 0x00]; // and：bit7=1（左透明）、bit6=0（右不透明）
        let rgba = color_pointer_to_rgba(2, 1, 1, &xor, &and);
        assert_eq!(rgba.len(), 8);
        assert_eq!(&rgba[0..4], &[0, 0, 0, 0]); // 左：黑、透明（and=1）
        assert_eq!(&rgba[4..8], &[255, 255, 255, 255]); // 右：白、不透明
    }

    /// 24bpp 光标：行宽 2 字节对齐 + bottom-up 行序（w=3 → stride=10）。
    #[test]
    fn color_pointer_24bpp_padding_and_bottom_up() {
        // 底行（mask row0）pixel1 = BGR(1,2,3)；顶行（row1）pixel0 = BGR(9,8,7)
        let mut xor = vec![0u8; 20]; // stride 10 × 2 行
        xor[3] = 1;
        xor[4] = 2;
        xor[5] = 3; // row0 x=1
        xor[10] = 9;
        xor[11] = 8;
        xor[12] = 7; // row1 x=0
        let and = vec![0u8; 4]; // 全不透明
        let rgba = color_pointer_to_rgba(3, 2, 24, &xor, &and);
        assert_eq!(rgba.len(), 24);
        // 顶行（视觉 y=0）x=0 ← mask row1
        assert_eq!(&rgba[0..3], &[7, 8, 9]);
        // 底行（视觉 y=1）x=1 ← mask row0
        let o = (1 * 3 + 1) * 4;
        assert_eq!(&rgba[o..o + 3], &[3, 2, 1]);
        assert_eq!(rgba[o + 3], 255);
    }

    /// 32bpp 光标：BGRA → RGBA、bottom-up；**XOR 自带 alpha 优先**（黑色
    /// 方块根因修复：服务端"AND 全 0 + XOR alpha=0"的光标不再强制不透明）。
    #[test]
    fn color_pointer_32bpp_bgra_bottom_up_and_mask() {
        // 2x2：xor alpha=0x12（>0）→ 直接采用（AND 掩码被覆盖）
        let xor = [
            0xFF, 0x00, 0x00, 0x12, // row0 x0 蓝
            0xFF, 0xFF, 0xFF, 0x12, // row0 x1 白
            0x00, 0x00, 0xFF, 0x12, // row1 x0 红
            0x00, 0xFF, 0x00, 0x12, // row1 x1 绿
        ];
        let and = [0x40u8, 0x00]; // row0: x0=不透明 x1=透明；row1: 全不透明
        let rgba = color_pointer_to_rgba(2, 2, 32, &xor, &and);
        // 视觉顶行（mask row1）：红、绿——alpha=0x12（xor alpha 优先）
        assert_eq!(&rgba[0..4], &[0xFF, 0x00, 0x00, 0x12]);
        assert_eq!(&rgba[4..8], &[0x00, 0xFF, 0x00, 0x12]);
        // 视觉底行（mask row0）：蓝、白——alpha 同上
        assert_eq!(&rgba[8..12], &[0x00, 0x00, 0xFF, 0x12]);
        assert_eq!(&rgba[12..16], &[0xFF, 0xFF, 0xFF, 0x12]);
    }

    /// 32bpp xor alpha=0：回退 AND 掩码（黑色方块场景——AND 全 0 不再强制
    /// 不透明黑；AND=1 处透明）。
    #[test]
    fn color_pointer_32bpp_zero_xor_alpha_falls_back_to_and_mask() {
        let xor = [0u8; 16]; // 全黑、alpha=0
        // 行 stride=2、bottom-up：视觉 row0 = mask row1（字节 2-3）
        let and = [0x00u8, 0x00, 0x40, 0x00]; // 视觉 row0 x1 透明
        let rgba = color_pointer_to_rgba(2, 2, 32, &xor, &and);
        // row0 x1（AND=1）→ 透明；其余（AND=0）→ 不透明黑
        assert_eq!(rgba[(0 * 2 + 1) * 4 + 3], 0);
        assert_eq!(rgba[(0 * 2 + 0) * 4 + 3], 255);
        assert_eq!(rgba[(1 * 2 + 0) * 4 + 3], 255);
        assert_eq!(rgba[(1 * 2 + 1) * 4 + 3], 255);
    }

    /// Windows 真实调用 OPENSSL_init_crypto + OSSL_PROVIDER_load("legacy")：
    /// 初始化必须返回 1 且 legacy provider 加载成功（NLA/NTLM MD4 前置）。
    #[test]
    #[cfg(windows)]
    fn openssl_legacy_provider_initializes_on_windows() {
        assert!(ensure_openssl_legacy_provider(), "legacy provider 加载失败");
        // 幂等：第二次调用仍成功（OnceLock 缓存）。
        assert!(ensure_openssl_legacy_provider());
    }
}
