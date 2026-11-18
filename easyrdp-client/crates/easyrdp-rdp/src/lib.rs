//! EasyRDP 远程桌面引擎层（FreeRDP 集成骨架）。
//!
//! 设计要点：
//! - `default` feature 不依赖 FreeRDP，任何环境都能编译（用于 CI / 单测）。
//! - `native` feature 才链接 `libfreerdp-sys`，启用 `ffi.rs` 中的真实实现路径。
//! - 上层（easyrdp-app / 前端）只面向 [`RdpSession`] trait 与 [`create_session`] 工厂，
//!   引擎未编译时返回 [`StubSession`]，其 `connect` 返回 "engine not compiled" 错误。

use std::fmt;

use easyrdp_core::profile::SessionProfile;

/// 脏矩形（远程桌面逻辑坐标，BGRA32 像素区随 [`FramebufferEvent::Delta`]
/// 的 `pixels` 按同序打包：每矩形 w*h*4 字节 top-down 紧凑排列）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RectUpdate {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// 帧缓冲/会话事件。
///
/// native 引擎的 FFI 回调（`rdpUpdate.BitmapUpdate`、`rdpPointerUpdate.PointerPosition`
/// 等，见 `ffi.rs`）把帧数据/指针更新推入 mpsc channel，由上层消费。
///
/// 2026-11 增量管线：服务端每包 `BITMAP_UPDATE` 的脏矩形直接以
/// [`FramebufferEvent::Delta`] 输出（不等全帧合成），全帧 JPEG
/// 仅在首帧 / 分辨率变化 / 周期兜底（防漂移）/ 增量过大时发送。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FramebufferEvent {
    /// 全帧 JPEG（质量 75 / 4:2:0，见 [`frame_jpeg::encode_frame_jpeg`]）。
    /// 仅在首帧、分辨率变化、周期兜底或增量过大时产生（低频）。
    Framebuffer {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        /// JPEG 编码字节（`mime: image/jpeg`）。
        pixels: Vec<u8>,
    },
    /// 增量脏矩形更新：`pixels` 为各矩形 BGRA32 像素按 `rects` 顺序
    /// 依次打包（每矩形 w*h*4 字节 top-down）。高频小包，远端画面变化
    /// 时即时输出，上层直接贴到对应矩形区域即可。
    Delta {
        /// 远端桌面逻辑尺寸（画布/坐标换算基准）。
        width: u32,
        height: u32,
        rects: Vec<RectUpdate>,
        /// 各矩形 BGRA 像素按 rects 顺序拼接。
        pixels: Vec<u8>,
    },
    /// 远端光标形状更新（2026-11 新增，对齐 mstsc 体验）。
    /// `rgba` 为 width*height*4 字节 RGBA（alpha 0=透明）；`rgba` 为空
    /// 表示恢复系统默认光标（PointerSystem）。
    Cursor {
        hotspot_x: u32,
        hotspot_y: u32,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    /// 远端指针位置更新（对应 FreeRDP `POINTER_POSITION_UPDATE`）。
    Pointer { x: u32, y: u32 },
    /// 原生直绘全帧（2026-11 原生渲染路径新增）：**raw BGRA**（非 JPEG），
    /// width*height*4 字节 top-down——native_view::present 用 StretchDIBits
    /// 零编码直绘。与 [`FramebufferEvent::Framebuffer`]（JPEG）并行发送，
    /// 非 Windows/非原生路径忽略即可（Clone 成本 ~4MB/帧，仅全帧时产生）。
    NativeFrame {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
    /// 远端剪贴板文本（CLIPRDR CF_UNICODETEXT，native 引擎在收到
    /// FormatDataResponse 时推送；引擎已同步写入本地 Win32 剪贴板）。
    ClipboardText(String),
    /// 会话断开（含原因描述）。
    Disconnected(String),
}

/// 一条来自前端的输入事件（渲染管线 -> 引擎）。
///
/// 坐标为**远程桌面逻辑坐标**（前端已按画布/远程分辨率比例换算）；
/// `Key.code` 为 W3C `KeyboardEvent.code` 字符串（如 "KeyA"、"Enter"），
/// 由上层（src-tauri session 桥）映射为 Windows VK 码后再传给 native 引擎；
/// 引擎层只透传记录（stub）或转换为 RDP 输入 PDU（native，β 版）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    /// 鼠标移动（相对远程桌面左上角，像素）。
    MouseMove { x: u32, y: u32 },
    /// 鼠标按键。`button`: "left" | "right" | "middle"。
    MouseButton { button: String, down: bool, x: u32, y: u32 },
    /// 滚轮。`delta` 正值向上。
    Wheel { delta: i32 },
    /// 横向滚轮（2026-11 新增，Shift+滚轮/触摸板横扫）。`delta` 正值向右。
    WheelHorizontal { delta: i32 },
    /// 键盘事件。`code` 为 W3C `KeyboardEvent.code`。
    Key { code: String, down: bool },
    /// Ctrl+Alt+Del 送达远端（SAS）：native 引擎按 MS-RDPBCGR 键盘 PDU
    /// 发送扩展扫描码序列 Ctrl(0x1D ext)+Alt(0x38 ext)+Del(0x53 ext)
    /// 按下/释放——TermSrv 在服务端合成 SAS（本地 OS 无法拦截转发）。
    /// 触发源：原生窗口标题栏 SAS 按钮 / 本地 Ctrl+Alt+End 热键。
    Sas,
}

/// 一次 RDP 会话的抽象。
pub trait RdpSession: Send {
    /// 建立到 `profile.host:profile.port` 的 RDP 会话。
    /// 引擎未编译（stub）时返回 Err("engine not compiled")。
    fn connect(&mut self, profile: &SessionProfile) -> Result<(), RdpError>;

    /// 断开当前会话（幂等）。
    fn disconnect(&mut self) -> Result<(), RdpError>;

    /// 帧缓冲事件接收端（native 引擎实现；stub 返回 None）。
    fn events(&self) -> Option<&event_channel::Receiver> {
        None
    }

    /// 注入口令（connect 前调用；profile 模型不含密码字段，见 ffi.rs 注释）。
    /// stub 等无口令需求实现可忽略（默认空实现）。
    fn set_password(&mut self, _password: &str) {}

    /// 发送一条输入事件（α 版 stub 记录到内部缓冲供测试断言；
    /// native 引擎转换为 FreeRDP 输入 PDU，见 ffi.rs send_input）。
    /// `Key.code` 为 "VK:0xNN" 风格字符串（native 引擎解析）。
    fn send_input(&mut self, _event: &InputEvent) -> Result<(), RdpError> {
        Ok(())
    }

    /// 本地 -> 远端剪贴板文本（α：CF_UNICODETEXT）。
    ///
    /// native 引擎（ffi.rs）：文本进入本地剪贴板缓存并向远端公告
    /// CLIPRDR FormatList；远端粘贴时经 FormatDataRequest/Response 回传数据。
    /// CLIPRDR 不可用（插件加载失败/未连接完成）时降级为方案 B：
    /// 把文本按 US 布局转 VK 键入序列逐字 send_input（见 `typing.rs`；
    /// 非 ASCII 字符该路径不支持，返回的 Result 携带提示）。
    /// 默认实现（stub 等）：返回不支持错误。
    fn send_clipboard_text(&mut self, _text: &str) -> Result<(), RdpError> {
        Err(RdpError("当前引擎不支持发送剪贴板文本".into()))
    }

    /// 本地 -> 远端文件公告（FILECLIP-20261117：CLIPRDR FileContents 通道）。
    /// `paths` 为本地 HDROP 枚举出的文件/目录绝对路径；远端粘贴时经
    /// FileContentsRequest 回取描述符与内容分块。
    /// 默认实现（stub 等）：返回不支持错误。
    fn send_clipboard_files(&mut self, _paths: &[String]) -> Result<(), RdpError> {
        Err(RdpError("当前引擎不支持文件剪贴板".into()))
    }

    /// 键盘 LED 同步（2026-11 新增，对齐 mstsc 行为）：
    /// 发送 RDP sync PDU（SynchronizeEvent），flags 位：
    /// NUM_LOCK=0x1 / CAPS_LOCK=0x2 / SCROLL_LOCK=0x4 / KANA=0x8
    /// （`<freerdp/input.h>`，freerdp_input_send_synchronize_event）。
    /// 本地 NumLock 开 → 服务端 NumLock 同步开 → 小键盘数字直接生效。
    fn sync_keyboard_leds(&mut self, _flags: u32) -> Result<(), RdpError> {
        Err(RdpError("当前引擎不支持键盘 LED 同步".into()))
    }
}

/// 事件通道：native FFI 回调（C 线程）经 [`event_channel::Sender`] 推送，
/// 上层经 [`event_channel::Receiver`] 消费。
pub mod event_channel {
    // 公开再导出：ffi.rs 经 `crate::event_channel::FramebufferEvent` 引用。
    pub use super::FramebufferEvent;

    /// 发送端（被 FFI 回调侧共享，要求 `Send + Clone`）。
    pub type Sender = std::sync::mpsc::Sender<FramebufferEvent>;

    /// 接收端包装。`try_recv` 非阻塞，适合 UI 轮询/重绘循环。
    /// 内部用 `Arc<Mutex>` 以便事件泵线程克隆出可跨线程移交的副本。
    #[derive(Clone)]
    pub struct Receiver(std::sync::Arc<std::sync::Mutex<std::sync::mpsc::Receiver<FramebufferEvent>>>);

    impl Receiver {
        pub fn try_recv(&self) -> Option<FramebufferEvent> {
            self.0.lock().ok()?.try_recv().ok()
        }

        /// 克隆一个可移交（跨线程）的接收端副本。
        pub fn clone_receiver(&self) -> Receiver {
            self.clone()
        }
    }

    /// 创建一对事件通道。
    pub fn channel() -> (Sender, Receiver) {
        let (tx, rx) = std::sync::mpsc::channel();
        (tx, Receiver(std::sync::Arc::new(std::sync::Mutex::new(rx))))
    }
}

/// 引擎层统一错误类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RdpError(pub String);

impl fmt::Display for RdpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for RdpError {}

/// 会话工厂：native 编译时返回 FFI 实现，否则返回 StubSession。
pub fn create_session() -> Box<dyn RdpSession> {
    #[cfg(feature = "native")]
    {
        Box::new(crate::ffi::FreerdpSession::new())
    }
    #[cfg(not(feature = "native"))]
    {
        Box::new(crate::stub::StubSession::new())
    }
}

/// 引擎是否具备真实 RDP 能力（`native` feature 已编译时为 true）。
pub fn native_ready() -> bool {
    #[cfg(feature = "native")]
    {
        true
    }
    #[cfg(not(feature = "native"))]
    {
        false
    }
}

// ---------------------------------------------------------------------------
// 剪贴板回环防护（CLIPSYNC-20261115）
//
// 原生窗口注册了 WM_CLIPBOARDUPDATE 监听（本地 -> 远端触发源）。引擎
// 远端 -> 本地方向 set_local_clipboard_text 会改本地剪贴板 → 触发同一
// 监听 → 若不过滤会把刚从远端收来的文本再发回远端（回环）。约定：
// 任何方向的文本同步前后都调用 clip_sync_note；监听侧发现文本与最近
// 一次同步相同则跳过。
// ---------------------------------------------------------------------------

/// 最近一次双向同步的文本（单会话 α 足够；多会话共享此缓存可接受——
/// 同文本跳过是无害优化）。
static LAST_CLIP_SYNC: Mutex<Option<String>> = Mutex::new(None);

/// 记录一次剪贴板文本同步（发送前 / 远端写入本地后调用）。
pub fn clip_sync_note(text: &str) {
    *LAST_CLIP_SYNC.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.to_string());
}

/// 监听侧判定：文本与最近一次同步相同 → 跳过（true）。
pub fn clip_sync_is_same(text: &str) -> bool {
    LAST_CLIP_SYNC
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_deref()
        == Some(text)
}

// ---------------------------------------------------------------------------
// 文件剪贴板回环防护（FILECLIP-20261117）：远端收来的文件落盘后写入本地
// HDROP 会触发同一 WM_CLIPBOARDUPDATE 监听——监听侧比对路径清单相同则跳过，
// 避免把刚接收的文件再公告回远端（自拉自取回环）。
// ---------------------------------------------------------------------------

/// 最近一次「远端接收」的文件绝对路径清单（有序比对；单会话 α 足够）。
static LAST_FILE_SYNC: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// 记录一次远端文件接收完成（写入本地 HDROP 前调用）。
pub fn file_sync_note(paths: &[String]) {
    *LAST_FILE_SYNC.lock().unwrap_or_else(|e| e.into_inner()) = Some(paths.to_vec());
}

/// 监听侧判定：文件清单与最近一次接收完全相同 → 跳过公告（true）。
pub fn file_sync_is_same(paths: &[String]) -> bool {
    LAST_FILE_SYNC
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        == Some(&paths.to_vec())
}

/// 紧凑时间戳字符串（文件落盘子目录名用）：yyyyMMdd-HHmmss。
pub fn now_compact_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // UTC+8 折算（本地展示口径；目录名用途足够）
    let secs = secs + 8 * 3600;
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil_from_days（Howard Hinnant 算法）
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}{m:02}{d:02}-{h:02}{mi:02}{s:02}")
}

// ---------------------------------------------------------------------------
// 调试行持久化（2026-11 输入排查）
//
// GUI（windows-subsystem）进程没有 stderr，`eprintln!` 调试输出完全不可见
// ——输入是否发出、rc 如何，GUI 会话里没有任何观测通道。此辅助把关键调试
// 行追加到 %APPDATA%\EasyRDP\debug.log（默认开启；文件超 8MB 自动停写，
// 防止鼠标移动刷盘失控）。
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
use std::sync::{mpsc, Mutex, OnceLock};

static DEBUG_LOG_WRITES: AtomicU64 = AtomicU64::new(0);
static DEBUG_LOG_DISABLED: AtomicUsize = AtomicUsize::new(0);
/// 日志文件句柄（Mutex 串行化所有写操作——多线程并发写无锁导致内容
/// 交错/时间戳错乱的 meta 根因修复）。
static DEBUG_LOGGER: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn debug_log(line: &str) {
    if DEBUG_LOG_DISABLED.load(Ordering::Relaxed) == 1 {
        return;
    }
    let mut logger = DEBUG_LOGGER.lock().unwrap_or_else(|e| e.into_inner());
    // 首次调用：路径解析（按 PID 命名——多实例交错写同一文件的 meta
    // 根因修复）+ 旧日志清理（保留最新 5 个）+ 8MB 上限检查。
    if logger.is_none() {
        let Some(dir) = std::env::var_os("APPDATA").map(std::path::PathBuf::from) else {
            return;
        };
        let path = dir.join("EasyRDP").join(format!("debug-{}.log", std::process::id()));
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > 8 * 1024 * 1024 {
            DEBUG_LOG_DISABLED.store(1, Ordering::Relaxed);
            return;
        }
        if let Ok(entries) = std::fs::read_dir(path.parent().unwrap_or(std::path::Path::new("."))) {
            let mut olds: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    n.starts_with("debug-") && n.ends_with(".log")
                })
                .filter_map(|e| {
                    let md = e.metadata().ok()?;
                    let t = md.modified().ok()?;
                    Some((t, e.path()))
                })
                .collect();
            olds.sort();
            if olds.len() > 20 {
                for (_, p) in olds.iter().take(olds.len() - 20) {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
        use std::io::Write as _;
        // 主路径打开（重试 1 次应对瞬时共享冲突）→ 失败回退 exe 同目录
        let mut opened = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .or_else(|_| {
                std::thread::sleep(std::time::Duration::from_millis(50));
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
            })
            .or_else(|_| {
                // exe 同目录回退（生产环境实用）
                let exe_dir = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(exe_dir.join(format!("debug-{}.log", std::process::id())))
            });
        if let Err(e) = &opened {
            eprintln!("[debug-diag] debug_log 所有路径打开失败: {e}");
        }
        *logger = opened.ok();
    }
    use std::io::Write as _;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    // 行首进程标识（多实例区分）+ **跨进程命名互斥体排队**（fix30 会话
    // 实锤：两实例并发 append 同一文件 → 内容交错/时间戳错乱——写期间
    // 持锁，写后释放，彻底消除交错）。
    let pid = std::process::id();
    let prefixed = format!("[EasyRDP.exe/{pid}] [{ts}] {line}");
    let lock = debug_lock_handle();
    unsafe {
        WaitForSingleObject(lock as *mut core::ffi::c_void, 0xFFFFFFFF); // INFINITE
        let ok = logger
            .as_mut()
            .and_then(|f| f.write_all(prefixed.as_bytes()).and_then(|_| f.flush()).ok())
            .is_some();
        ReleaseMutex(lock as *mut core::ffi::c_void);
        if !ok {
            return;
        }
    }
}

/// 跨进程日志锁句柄（命名互斥体，进程内缓存一次）。
fn debug_lock_handle() -> isize {
    static HANDLE: OnceLock<isize> = OnceLock::new();
    *HANDLE.get_or_init(|| {
        let name: Vec<u16> = "Local\\EasyRDP_DebugLog_Lock\0"
            .encode_utf16()
            .collect();
        unsafe {
            windows_sys::Win32::System::Threading::CreateMutexW(
                std::ptr::null(),
                0,
                name.as_ptr(),
            ) as isize
        }
    })
}

#[cfg(test)]
mod debug_log_tests {
    use super::*;

    #[test]
    fn debug_log_writes_file() {
        // 直写一次并回读（验证 GUI 观测通道真实落盘）。
        // 主路径 %APPDATA%（真实 GUI 环境）；受限环境（CI/沙箱）自动回退
        // crates/easyrdp-rdp/target/debug.log。日志文件按 PID 命名
        //（debug-{pid}.log——多实例交错写同一文件的 meta 根因修复）。
        debug_log("[test] debug_log_writes_file 探针");
        let primary = std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .map(|d| d.join("EasyRDP").join(format!("debug-{}.log", std::process::id())));
        eprintln!(
            "[debug-diag] primary={primary:?} exists={}",
            primary.as_ref().map(|p| p.exists()).unwrap_or(false)
        );
        let fallback = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("debug.log");
        // exe 同目录回退（debug_log 的第三级回退路径）
        let exe_dir_fallback = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join(format!("debug-{}.log", std::process::id()))));
        let read = |p: std::path::PathBuf| std::fs::read_to_string(p).ok();
        // 主路径与回退路径都可能存在历史内容：必须找到包含本次标记的那份
        //（主路径优先；主路径存在但不含标记时继续检查回退路径）。
        let content = primary
            .and_then(read)
            .filter(|c| c.contains("debug_log_writes_file"))
            .or_else(|| read(fallback).filter(|c| c.contains("debug_log_writes_file")))
            .or_else(|| {
                exe_dir_fallback
                    .and_then(read)
                    .filter(|c| c.contains("debug_log_writes_file"))
            })
            .expect("debug.log（主路径或回退路径）应包含本次探针标记");
        assert!(content.contains("debug_log_writes_file"));
    }
}

/// `Resolution::FitWindow` 策略的实际请求分辨率：**主显示器当前分辨率**
/// （Windows `GetSystemMetrics(SM_CXSCREEN/SM_CYSCREEN)`，user32）；
/// 获取失败或非 Windows 回退 α 默认 1280x720。
///
/// 该值同时用于 FreeRDP `DesktopWidth/Height` settings（ffi.rs）与上层
/// 画面视图的初始画布尺寸（src-tauri session.rs）。服务端协商后的实际
/// 分辨率以帧事件的 width/height 为准（前端按帧负载自适应画布）。
pub fn fit_window_resolution() -> (u32, u32) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
        };
        // SAFETY：user32 标准查询，无副作用。
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        if w > 0 && h > 0 {
            (w as u32, h as u32)
        } else {
            (1280, 720)
        }
    }
    #[cfg(not(windows))]
    {
        (1280, 720)
    }
}

pub mod ffi;
pub mod frame_jpeg;
/// EasyRDP 云网关本地回环桥（F4 网关接入）：TCP <-> WS 隧道转发。
/// 与 FreeRDP 编译无关（default feature 也可用，便于单测/集成）。
pub mod gateway_bridge;
pub mod stub;
pub mod typing;

/// Windows 静态链接 FreeRDP 的通道入口桩（仅 native feature 参与：
/// 满足 freerdp-client2.lib tables.c 静态入口表的符号引用，见模块注释）。
#[cfg(all(windows, feature = "native"))]
pub mod channel_stubs;

#[cfg(test)]
mod tests {
    use super::*;
    use easyrdp_core::profile::SessionProfile;

    #[test]
    fn factory_returns_stub_when_not_native() {
        if native_ready() {
            return; // native 构建下跳过（下面三个 stub 行为测试同理）
        }
        assert!(!native_ready());
        let s = create_session();
        // stub 会话没有事件通道
        assert!(s.events().is_none());
    }

    #[test]
    fn stub_connect_reports_engine_not_compiled() {
        if native_ready() {
            return; // native 构建下 create_session 返回真实引擎，本测试不适用
        }
        let mut s = create_session();
        let mut p = SessionProfile::default();
        p.host = "10.0.0.8".into();
        p.port = 3389;
        let err = s.connect(&p).unwrap_err();
        assert_eq!(err.0, "engine not compiled");
    }

    #[test]
    fn stub_disconnect_is_idempotent_ok() {
        if native_ready() {
            return; // 见 factory_returns_stub_when_not_native
        }
        let mut s = create_session();
        s.disconnect().unwrap();
        s.disconnect().unwrap(); // 二次调用仍 Ok
    }

    #[test]
    fn stub_connect_fails_for_any_profile_shape() {
        if native_ready() {
            return; // 见 factory_returns_stub_when_not_native
        }
        let mut s = create_session();
        let mut p = SessionProfile::default();
        p.host = "example.com".into();
        p.resolution = easyrdp_core::profile::Resolution::Custom { w: 1920, h: 1080 };
        assert!(s.connect(&p).is_err());
    }
}
