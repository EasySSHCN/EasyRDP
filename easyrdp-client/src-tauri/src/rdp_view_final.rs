//! 原生远程桌面窗口（2026-11 架构修正：mstsc 同款独立顶层窗口）。
//!
//! fix15 子窗口方案的 z-order/输入路由/webview 遮盖三问题互相纠缠不可持续
//! → 改为 **独立顶层窗口**（WS_OVERLAPPEDWINDOW，无 SetParent）：Tauri
//! 窗口纯连接管理器，连接时打开本窗口（客户区 = 远端分辨率 1:1），断开
//! （WM_CLOSE）自动停会话并回连接管理器。
//!
//! 帧直绘：`present()` 保存最新 BGRA 帧 + StretchDIBits 即时绘制；
//! WM_PAINT 从帧存储重绘。零 JPEG 编码、零 base64、零 IPC 传输。
//!
//! 输入直通：WndProc 处理鼠标/滚轮/键盘 Win32 消息（客户区 = 远端分辨率
//! 1:1，坐标直用）→ INPUT_TX 通道 → session_start 派生的转发线程 →
//! freerdp input。
//!
//! 仅 Windows（cfg(windows)）；非 Windows 平台保持 webview pull 路径。

#![cfg(windows)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Mutex, OnceLock, Arc};

use easyrdp_rdp::InputEvent;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::PostQuitMessage;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush, DeleteDC,
    DeleteObject, EndPaint, FillRect, GetDC, GetStockObject, InvalidateRect, ReleaseDC,
    SelectObject, SetBkMode, SetDIBitsToDevice, SetTextColor, StretchBlt, StretchDIBits, TextOutW,
    ScreenToClient, TRANSPARENT, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DEFAULT_GUI_FONT,
    DIB_RGB_COLORS, PAINTSTRUCT, SRCCOPY,
};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, GetClipboardData, OpenClipboard,
    RemoveClipboardFormatListener,
};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GetClientRect, GetForegroundWindow, GetMessageW, GetSystemMetrics,
    HTCAPTION, HTCLIENT, HTTOP, HTTOPLEFT, HTTOPRIGHT, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT,
    HTLEFT, HTRIGHT, IDC_ARROW, IsWindow, IsZoomed, LoadCursorW, PostMessageW,
    RegisterClassW, SendMessageW, SetCursor, SetForegroundWindow, SetWindowPos, ShowWindow,
    SM_CXSCREEN, SM_CYSCREEN, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE,
    SW_SHOW, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, TranslateMessage,
    WM_CLOSE, WM_ERASEBKGND, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOVE,
    WM_NCCREATE, WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SETCURSOR,
    WM_SIZE, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_DESTROY, WNDCLASSW, WS_POPUP, WS_VISIBLE,
};

/// 自绘标题栏高度（px）。
const TITLE_BAR_H: i32 = 32;
/// 窗口类名。
const WINDOW_CLASS: &[u16] = &[
    0x0045, 0x0061, 0x0073, 0x0079, 0x0052, 0x0044, 0x0050, 0x005F, 0x004E, 0x0041, 0x0054, 0x0056,
    0x0049, 0x0045, 0x0057, 0x0000, // "EasyRDP_NATIVE_VIEW\0" (UTF-16)
];


/// 跨线程 HWND 包装（HWND = *mut c_void 非 Send；PostMessage/GetDC 可跨线程，
/// DestroyWindow 例外——WM_CLOSE 消息规避）。
#[derive(Debug, Clone, Copy)]
struct SendHwnd(*mut core::ffi::c_void);
unsafe impl Send for SendHwnd {}

/// 每会话视图：窗口句柄 + 远端分辨率。
#[derive(Debug, Clone, Copy)]
struct ViewEntry {
    hwnd: SendHwnd,
    remote_w: u32,
    remote_h: u32,
}

/// 会话 → 视图注册表（pump 按 session_id 取 hwnd 直绘）。
static VIEWS: Mutex<Option<HashMap<String, ViewEntry>>> = Mutex::new(None);
/// 输入路由（R-20261116-007 多会话修正：**按 hwnd 键控**——此前单一全局
/// Sender 在第二个会话 attach 时被覆盖，旧会话窗口的输入全部误入新会话
/// 通道）。WndProc/LL 钩子按事件所属窗口 hwnd 查表转发。
static INPUT_ROUTE: Mutex<Option<HashMap<isize, mpsc::Sender<InputEvent>>>> = Mutex::new(None);

/// 本地 -> 远端剪贴板文本通道（CLIPSYNC-20261115）：WM_CLIPBOARDUPDATE
/// 读到的 CF_UNICODETEXT 经此通道交会话泵侧转发线程调 send_clipboard_text。
/// CLIP_ROUTE 同理按 hwnd 键控。
static CLIP_ROUTE: Mutex<Option<HashMap<isize, mpsc::Sender<String>>>> = Mutex::new(None);

/// R-20261116-007：低级键盘钩子迁移到 input_hook.rs（独立线程 + GetMessage
/// 泵 + 引用计数）。acquire 在 attach 成功后、release 在 WM_DESTROY——
/// 多会话下钩子存活到最后一会话关闭，不再被首个 WM_DESTROY 摘除。
/// 子窗口 hwnd → 会话 id（WM_CLOSE → 关闭通道 → 停会话）。
static CLOSE_TX: Mutex<Option<HashMap<isize, mpsc::Sender<String>>>> = Mutex::new(None);
/// 每窗口最后帧（WM_PAINT 重绘数据源）。
static FRAME_STORE: Mutex<Option<HashMap<isize, (Vec<u8>, u32, u32)>>> = Mutex::new(None);
/// present 统计（调用/绘制/无视图/短缓冲，5s 汇总冲刷——全路径可见，
/// 黑屏三段定位：无视图>0=注册表断、调用=0=pump 断、绘制>0 但黑=绘制问题）。
static PRESENT_STATS: Mutex<(u64, u64, u64, u64, Option<std::time::Instant>)> =
    Mutex::new((0, 0, 0, 0, None));
/// 消息队列探针（2026-11 输入零事件定位）：pump 每 2s PostMessage WM_USER+1
/// → WndProc 收到即日志（前 20 条）——区分"消息派发断"vs"输入真未到达"。
static PROBE_HWNDS: Mutex<Option<HashMap<isize, (u64, std::time::Instant)>>> =
    Mutex::new(None);

/// pump 每 2s 调用：向原生窗口队列投递探针消息（seq 递增）。
pub fn probe_tick(session_id: &str) {
    let entry = {
        let views = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
        views.as_ref().and_then(|m| m.get(session_id)).copied()
    };
    let Some(entry) = entry else { return };
    let mut probes = PROBE_HWNDS.lock().unwrap_or_else(|e| e.into_inner());
    let map = probes.get_or_insert_with(HashMap::new);
    let now = std::time::Instant::now();
    let (seq, last) = map
        .get(&(entry.hwnd.0 as isize))
        .copied()
        .unwrap_or((0u64, now - std::time::Duration::from_secs(10)));
    if now.duration_since(last) < std::time::Duration::from_secs(2) {
        return;
    }
    let seq = seq + 1;
    map.insert(entry.hwnd.0 as isize, (seq, now));
    drop(probes);
    unsafe {
        PostMessageW(
            entry.hwnd.0,
            0x0400 + 1, // WM_USER + 1
            seq as WPARAM,
            0,
        );
    }
}

/// 为会话创建独立顶层远程桌面窗口（客户区 = 远端分辨率，超屏自适应缩小）。
/// `close_tx`：WM_CLOSE 时回传 session_id（上层停会话并回连接管理器）。
///
/// **建窗在主线程执行（run_on_main_thread）+ 调用方在非主线程阻塞等待**：
/// - 专用线程建窗实测 CreateWindowExW err=183（双路径一致）→ 主线程建窗
///   （Tauri 自身窗口创建成功 = 主线程建窗能力已被证明，消息由 Tauri 事件
///   循环泵）；调用方（attach 派生线程）阻塞等 AtomicBool 无死锁——等待
///   线程非主线程。
/// - **死锁防护（fix20）**：本函数绝不能在主线程调用——调用方（session_start
///   派生线程）保证。闭包内 catch_unwind，panic 也置位标志（不悬挂等待方）。
pub fn attach_for_session(
    app: &tauri::AppHandle,
    session_id: &str,
    remote_w: u32,
    remote_h: u32,
    input_tx: mpsc::Sender<InputEvent>,
    close_tx: mpsc::Sender<String>,
    clip_tx: mpsc::Sender<String>,
) -> bool {
    easyrdp_rdp::debug_log(&format!(
        "[native-view] attach 进入（主线程建窗）session={session_id} remote={remote_w}x{remote_h} 构建=fix50-20261115-CLIPSYNC-GHOSTFIX2"
    ));
    easyrdp_rdp::debug_log("[native-view] 注册窗口类…");
    let Some(atom) = register_class() else {
        return false;
    };
    easyrdp_rdp::debug_log("[native-view] 主线程：创建窗口…");
    let ok_flag = Arc::new(AtomicBool::new(false));
    let flag2 = Arc::clone(&ok_flag);
    let sid = session_id.to_string();
    let dispatch = app.run_on_main_thread(move || {
        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            unsafe { attach_on_main_thread(&sid, atom, remote_w, remote_h, input_tx, close_tx, clip_tx) }
        }))
        .unwrap_or(false);
        flag2.store(ok, Ordering::SeqCst);
    });
    if dispatch.is_err() {
        easyrdp_rdp::debug_log("[native-view] run_on_main_thread 派发失败");
        return false;
    }
    // 等待主线程闭包完成（等待方=attach 派生线程，非主线程 → 无死锁）；
    // 5s 超时兜底（主线程卡死时不悬挂会话）——超时视为失败。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !ok_flag.load(Ordering::SeqCst) {
        if std::time::Instant::now() >= deadline {
            easyrdp_rdp::debug_log("[native-view] 主线程建窗等待超时（5s）");
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    easyrdp_rdp::debug_log("[native-view] 主线程建窗完成");
    true
}

/// 主线程执行的建窗主体（注册表注册 + ShowWindow；消息由 Tauri 事件循环泵）。
unsafe fn attach_on_main_thread(
    session_id: &str,
    atom: u16,
    remote_w: u32,
    remote_h: u32,
    input_tx: mpsc::Sender<InputEvent>,
    close_tx: mpsc::Sender<String>,
    clip_tx: mpsc::Sender<String>,
) -> bool {
        // **WS_POPUP + 自绘标题栏（终局方案）**：客户区 = 整窗（每像素
        // HTCLIENT，输入必然进客户区），顶部 32px 自绘标题栏（深色条 +
        // 标题 + 最小化/关闭按钮 + 拖动移动），画面区 = 标题栏以下全部。
        // 建窗后 TOPMOST + SetForegroundWindow + SetFocus（抢前台）。
        let (mut cw, mut ch) = (remote_w as i32, remote_h as i32 + TITLE_BAR_H);
        let (screen_w, screen_h) = (
            GetSystemMetrics(SM_CXSCREEN),
            GetSystemMetrics(SM_CYSCREEN),
        );
        let style = WS_POPUP | WS_VISIBLE;
        if cw > screen_w || ch > screen_h {
            let scale = (screen_w as f64 / cw as f64).min(screen_h as f64 / ch as f64) * 0.95;
            cw = (cw as f64 * scale) as i32;
            ch = (ch as f64 * scale) as i32;
        }
        // 居中（工作区内）
        let x = ((screen_w - cw) / 2).max(0);
        let y = ((screen_h - ch) / 2).max(0);
        // 类名优先，失败回退类原子（MAKEINTATOM——字符串查找异常时的稳健路径）
        let mut hwnd = CreateWindowExW(
            0,
            WINDOW_CLASS.as_ptr(),
            TITLE.as_ptr(),
            style,
            x,
            y,
            cw,
            ch,
            std::ptr::null_mut(), // 顶层窗口：无父（mstsc 同款架构）
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        );
        if hwnd.is_null() {
            let err = windows_sys::Win32::Foundation::GetLastError();
            easyrdp_rdp::debug_log(&format!(
                "[native-view] 字符串类名建窗失败 err={err} (1407=类未找到/5=拒绝访问/87=参数错)——回退类原子 MAKEINTATOM({atom})"
            ));
            hwnd = CreateWindowExW(
                0,
                atom as usize as *const u16, // MAKEINTATOM(atom)
                TITLE.as_ptr(),
                style,
                x,
                y,
                cw,
                ch,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            );
        }
        if hwnd.is_null() {
            let err = windows_sys::Win32::Foundation::GetLastError();
            easyrdp_rdp::debug_log(&format!(
                "[native-view] CreateWindowExW 类原子路径也失败 err={err}"
            ));
            return false;
        }
        easyrdp_rdp::debug_log("[native-view] CreateWindowExW ok");
        // 输入测量计数器按会话重置（进程级静态此前从不重置 → 后续会话
        // "首条/每 20 条"日志静默 = 测量假象实锤修复）
        INPUT_SEEN.lock().unwrap_or_else(|e| e.into_inner())[0] = false;
        INPUT_SEEN.lock().unwrap_or_else(|e| e.into_inner())[1] = false;
        *INPUT_COUNTS.lock().unwrap_or_else(|e| e.into_inner()) = (0, 0);
        _ = ShowWindow(hwnd, SW_SHOW);
        // 前台/焦点 + **置顶（TOPMOST）**（2026-11 输入无效根因修复 v2）：
        // 顶层窗口创建后默认在 Tauri 连接管理器窗口之后/被其完全遮盖——
        // 用户鼠标键盘全进 Tauri webview，原生窗口收不到任何输入。
        // HWND_TOPMOST（常驻最顶）比 HWND_TOP 更强：不被 Tauri 窗口激活
        // 抢回 Z 序；SetForegroundWindow/SetFocus 补键盘焦点。
        SetWindowPos(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        _ = SetForegroundWindow(hwnd);
        _ = SetFocus(hwnd);
        easyrdp_rdp::debug_log("[native-view] 置顶+前台+焦点已设置");
        // SetCapture 强制捕获（2026-11 输入零事件终极手段）：鼠标事件强制
        // 送达本窗口（远端桌面窗口独占鼠标合理）；首条 WM_MOUSEMOVE 后
        // ReleaseCapture 恢复正常路由（WndProc 内）。
        unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
        }
        easyrdp_rdp::debug_log("[native-view] SetCapture 已设置");
        VIEWS
            .lock().unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashMap::new)
            .insert(
                session_id.to_string(),
                ViewEntry {
                    hwnd: SendHwnd(hwnd),
                    remote_w,
                    remote_h,
                },
            );
        // R-20261116-007：路由按 hwnd 键控注册（多会话并存互不串扰）
        INPUT_ROUTE
            .lock().unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashMap::new)
            .insert(hwnd as isize, input_tx);
        CLIP_ROUTE
            .lock().unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashMap::new)
            .insert(hwnd as isize, clip_tx);
        // CLIPSYNC-20261115：注册剪贴板监听（本地复制 → WM_CLIPBOARDUPDATE
        // → 读 CF_UNICODETEXT → clip_tx → 会话转发线程 send_clipboard_text）
        let clip_ok = AddClipboardFormatListener(hwnd);
        easyrdp_rdp::debug_log(&format!(
            "[native-view] AddClipboardFormatListener rc={clip_ok}"
        ));
        // R-006：低级键盘钩子（input_hook.rs 独立线程 + 引用计数）——
        // 拦截 Win 键转发远端（防本地开始菜单）+ Ctrl+Alt+End → SAS。
        crate::input_hook::acquire();
        CLOSE_TX
            .lock().unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashMap::new)
            .insert(hwnd as isize, close_tx);
        // 客户区实测（修复验证：实测应=窗口尺寸=远端分辨率 1:1）
        let mut crc = RECT::default();
        GetClientRect(hwnd, &mut crc);
        easyrdp_rdp::debug_log(&format!(
            "[native-view] attach(顶层窗口) session={session_id} 窗口={cw}x{ch} 客户区实测={}x{} remote={remote_w}x{remote_h}",
            crc.right - crc.left,
            crc.bottom - crc.top
        ));
        // 消息循环由 Tauri 主线程事件循环泵（主线程建窗）；WM_CLOSE 经
        // close_tx 停会话；WM_PAINT 由 present/重绘覆盖。
        true
    }

/// 窗口标题："EasyRDP — 远程桌面"（UTF-16 预编码）。
static TITLE: &[u16] = &[
    0x0045, 0x0061, 0x0073, 0x0079, 0x0052, 0x0044, 0x0050, 0x0020, 0x2014, 0x0020, 0x8FDC, 0x7A0B,
    0x684C, 0x9762, 0x0000, // "EasyRDP — 远程桌面\0"
];

/// hide Tauri 窗口后的输入修复（2026-11 fix30 会话实证根因）：
/// **webview 持有鼠标捕获**——所有鼠标消息被路由到 webview 窗口（手指
/// 光标 = webview 链接光标的铁证；WM_SETCURSOR 查询型仍到达原生窗口而
/// WM_MOUSEMOVE 投递型被捕获截走）。本函数在主线程执行：**抢占捕获到
/// 原生窗口**（远端桌面窗口独占鼠标合理）→ SetForegroundWindow →
/// SetFocus。
pub fn recapture_to_native(app: &tauri::AppHandle, session_id: &str) {
    let sid = session_id.to_string();
    let _ = app.run_on_main_thread(move || {
        let hwnd = VIEWS
            .lock().unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|m| m.get(&sid))
            .map(|e| e.hwnd.0);
        if let Some(h) = hwnd {
            unsafe {
                windows_sys::Win32::UI::Input::KeyboardAndMouse::SetCapture(h);
                SetForegroundWindow(h);
                SetFocus(h);
            }
            easyrdp_rdp::debug_log("[native-view] hide 后 SetCapture(原生)+前台+焦点已重设");
        } else {
            easyrdp_rdp::debug_log("[native-view] hide 后未找到原生窗口（重捕获跳过）");
        }
    });
}

/// 会话结束：销毁窗口并清注册表（PostMessage WM_CLOSE → WndProc 自毁）。
pub fn detach(session_id: &str) {
    let hwnd = VIEWS
        .lock().unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .and_then(|m| m.remove(session_id))
        .map(|e| e.hwnd.0);
    if let Some(hwnd) = hwnd {
        // R-20261116-007：只摘除本窗口的路由条目（其余会话路由保留）
        INPUT_ROUTE
            .lock().unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .map(|m| m.remove(&(hwnd as isize)));
        CLIP_ROUTE
            .lock().unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .map(|m| m.remove(&(hwnd as isize)));
        unsafe {
            // CLIPSYNC：注销剪贴板监听（窗口销毁前）
            RemoveClipboardFormatListener(hwnd);
        }
        CLOSE_TX
            .lock().unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .map(|m| m.remove(&(hwnd as isize)));
        FRAME_STORE
            .lock().unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .map(|m| m.remove(&(hwnd as isize)));
        unsafe {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
        easyrdp_rdp::debug_log(&format!("[native-view] detach session={session_id}"));
    }
}

// ---------------------------------------------------------------------------
// GDI 背板（FLASHFIX-20261116）
// ---------------------------------------------------------------------------
// 历史 bug：present_delta() 用 **top-down DIB（biHeight<0）+ 源子矩形** 调
// StretchDIBits。GDI 对 top-down DIB 的源 (x,y) 坐标语义存在历史歧义（不同
// 驱动按自顶向下/自底向上两种基准解析）——脏矩形从**错误的行**取像素贴到
// 窗口：点击触发远端局部重绘时，屏幕闪现"错位/拼贴的桌面内容"，下一全帧
// 又立刻纠正 = "闪一下很快消失"。本修复引入每窗口 GDI 背板（内存 DC + 兼容
// 位图）：帧数据经 SetDIBitsToDevice（无源子矩形参数，top-down 语义无歧义）
// 一次性写入背板，之后全部绘制用 BitBlt/StretchBlt 从背板拷贝（DDB 坐标
// 语义唯一）。present / present_delta / WM_PAINT 三路径统一。

/// 背板结构：内存 DC + 位图 + 位图原始句柄（SelectObject 归还用）。
struct BlitBacking {
    hdc: *mut core::ffi::c_void,
    bmp: *mut core::ffi::c_void,
    old_bmp: *mut core::ffi::c_void,
    w: i32,
    h: i32,
}
unsafe impl Send for BlitBacking {}

static BLIT_BACKING: Mutex<Option<HashMap<isize, BlitBacking>>> = Mutex::new(None);
/// 背板串行锁：pump 线程写背板 / 主线程 WM_PAINT 读背板，禁止交叉。
static BLIT_LOCK: Mutex<()> = Mutex::new(());

/// 把 BGRA 帧（top-down，w×h×4）同步进背板；尺寸变化自动重建位图。
/// 返回 Some((memdc, w, h)) 供调用方 BitBlt/StretchBlt。必须在持有
/// BLIT_LOCK 的情况下调用并在锁内完成绘制。
unsafe fn backing_store_update(
    hwnd: isize,
    bgra: &[u8],
    w: u32,
    h: u32,
) -> Option<(*mut core::ffi::c_void, i32, i32)> {
    if w == 0 || h == 0 || bgra.len() < (w as usize) * (h as usize) * 4 {
        return None;
    }
    let mut map = BLIT_BACKING.lock().unwrap_or_else(|e| e.into_inner());
    let m = map.get_or_insert_with(HashMap::new);
    let need_rebuild = match m.get(&hwnd) {
        Some(b) => b.w != w as i32 || b.h != h as i32,
        None => true,
    };
    if need_rebuild {
        if let Some(old) = m.remove(&hwnd) {
            let _ = SelectObject(old.hdc, old.old_bmp);
            DeleteObject(old.bmp);
            DeleteDC(old.hdc);
        }
        let win_dc = GetDC(hwnd as *mut core::ffi::c_void);
        if win_dc.is_null() {
            return None;
        }
        let mem_dc = CreateCompatibleDC(win_dc);
        let bmp = CreateCompatibleBitmap(win_dc, w as i32, h as i32);
        ReleaseDC(hwnd as *mut core::ffi::c_void, win_dc);
        if mem_dc.is_null() || bmp.is_null() {
            if !mem_dc.is_null() {
                DeleteDC(mem_dc);
            }
            if !bmp.is_null() {
                DeleteObject(bmp);
            }
            return None;
        }
        let old_bmp = SelectObject(mem_dc, bmp);
        m.insert(
            hwnd,
            BlitBacking { hdc: mem_dc, bmp, old_bmp, w: w as i32, h: h as i32 },
        );
    }
    let b = m.get_mut(&hwnd)?;
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: (w * h * 4) as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    // SetDIBitsToDevice 无源子矩形参数：整帧写入，坐标语义无歧义。
    let ok = SetDIBitsToDevice(
        b.hdc,
        0,
        0,
        w as u32,
        h as u32,
        0,
        0,
        0,
        h as u32,
        bgra.as_ptr() as *const core::ffi::c_void,
        &bmi,
        DIB_RGB_COLORS,
    );
    if ok == 0 {
        return None;
    }
    Some((b.hdc, w as i32, h as i32))
}

/// 销毁窗口时释放背板（WM_DESTROY / detach 路径调用）。
unsafe fn backing_store_drop(hwnd: isize) {
    let _g = BLIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut map = BLIT_BACKING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(b) = map.get_or_insert_with(HashMap::new).remove(&hwnd) {
        let _ = SelectObject(b.hdc, b.old_bmp);
        DeleteObject(b.bmp);
        DeleteDC(b.hdc);
    }
}

/// 帧直绘：BGRA（top-down，w×h×4）StretchDIBits 到窗口客户区 + 存帧
///（WM_PAINT 重绘数据源）。由事件泵线程在每帧合成后调用。
/// 返回 false = 会话无原生窗口（webview pull 路径继续工作）。
/// 统计（调用/绘制/无视图/短缓冲）全路径计数，5s 汇总冲刷（顶部判定——
/// 早退路径也可见，消除观测盲区）。
pub fn present(session_id: &str, bgra: &[u8], w: u32, h: u32) -> bool {
    let mut st = PRESENT_STATS.lock().unwrap_or_else(|e| e.into_inner());
    st.0 += 1; // 调用
    let mut flush = |st: &mut (u64, u64, u64, u64, Option<std::time::Instant>)| {
        let (calls, drawn, noview, short, last) = *st;
        let due = last.map_or(true, |t| t.elapsed() >= std::time::Duration::from_secs(5));
        if due {
            easyrdp_rdp::debug_log(&format!(
                "[native-view] present 5s: 调用={calls} 绘制={drawn} 无视图={noview} 短缓冲={short}"
            ));
            *st = (0, 0, 0, 0, Some(std::time::Instant::now()));
        }
    };
    let entry = {
        let views = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
        views.as_ref().and_then(|m| m.get(session_id)).copied()
    };
    let Some(entry) = entry else {
        st.2 += 1; // 无视图
        flush(&mut st);
        return false;
    };
    if w == 0 || h == 0 {
        st.3 += 1; // 短缓冲
        flush(&mut st);
        return true; // 有窗口但尺寸无效：跳过本次
    }
    // **半分辨率自适应（2026-11）**：FLASHFIX-20261116 收紧判别——此前
    // `>= half_len` 会把**任何长度落在半帧与全帧之间的缓冲**（如远端分辨率
    // 变化后的全帧）误判为半帧 → biWidth/stride 与实际数据不符 → 画面被
    // 错位切割成"拼贴块"。现改为精确等长判别，非全/半帧长度一律丢弃等全帧。
    let full_len = w as usize * h as usize * 4;
    let half_w = (w / 2).max(1) as usize;
    let half_h = (h / 2) as usize;
    let half_len = half_w * half_h * 4;
    let (sw, sh) = if bgra.len() >= full_len {
        (w as usize, h as usize)
    } else if half_len > 0 && bgra.len() == half_len {
        (half_w, half_h)
    } else {
        st.3 += 1; // 短缓冲（异常长度，原样计数）
        flush(&mut st);
        return true; // 有窗口但缓冲不完整：跳过本次
    };
    // **首帧标记（GHOSTFIX-20261115 移除）**：判别用红框脚手架已删除——
    // 正式版不绘制任何诊断叠加。直接使用原始帧数据。
    let bgra_draw: &[u8] = bgra;
    // 存最新帧（WM_PAINT 重绘用，存实际源尺寸）
    FRAME_STORE
        .lock().unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(entry.hwnd.0 as isize, (bgra.to_vec(), sw as u32, sh as u32));
    unsafe {
        let hdc = GetDC(entry.hwnd.0);
        if hdc.is_null() {
            // GetDC 失败 = 窗口句柄失效（被销毁）——黑屏+输入全断的共同根因候选；
            // 自愈：移除失效条目（下次 attach 重新注册新窗口）
            let alive = !windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(entry.hwnd.0);
            easyrdp_rdp::debug_log(&format!(
                "[native-view] GetDC 失败 hwnd 失效={alive}（TRUE=窗口已被销毁——追查销毁者）；条目已自愈移除"
            ));
            VIEWS
                .lock().unwrap_or_else(|e| e.into_inner())
                .as_mut()
                .map(|m| m.remove(session_id));
            flush(&mut st);
            return true;
        }
        let mut rc = RECT::default();
        GetClientRect(entry.hwnd.0, &mut rc);
        let dst_w = (rc.right - rc.left).max(1);
        // 画面绘制在标题栏以下（顶部 32px 为自绘标题栏）
        let dst_h = (rc.bottom - rc.top - TITLE_BAR_H).max(1);
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: sw as i32,
                biHeight: -(sh as i32), // top-down（合成缓冲行序）
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: (sw * sh * 4) as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        // FLASHFIX-20261116：优先走背板整帧 StretchBlt（DDB 坐标语义唯一）；
        // 背板失败回退 StretchDIBits 整帧（无源子矩形，历史路径仍安全）。
        let _lock = BLIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let blitted = match backing_store_update(entry.hwnd.0 as isize, bgra_draw, sw as u32, sh as u32) {
            Some((mem_dc, mw, mh)) => {
                StretchBlt(
                    hdc,
                    0,
                    TITLE_BAR_H,
                    dst_w,
                    dst_h,
                    mem_dc,
                    0,
                    0,
                    mw,
                    mh,
                    SRCCOPY,
                ) != 0
            }
            None => false,
        };
        if !blitted {
            _ = StretchDIBits(
                hdc,
                0,
                TITLE_BAR_H,
                dst_w,
                dst_h,
                0,
                0,
                sw as i32,
                sh as i32,
                bgra_draw.as_ptr() as *const core::ffi::c_void,
                &bmi,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
        }
        draw_chrome(hdc, dst_w);
        ReleaseDC(entry.hwnd.0, hdc);
    }
    st.1 += 1; // 绘制成功
    flush(&mut st);
    true
}

/// 自绘标题栏（WS_POPUP 客户区顶部 32px）：深色条 + 标题文字 + 最小化/关闭
/// 按钮（─ ✕ 槽位各 46px）。帧绘制后调用叠加。
///
/// R-20261116-011：标题栏随前端主题深浅切换（set_native_theme_dark 写开关 +
/// 失效窗口重绘）。COLORREF = 0x00BBGGRR。
static NATIVE_DARK: AtomicBool = AtomicBool::new(true);

/// 前端主题切换入口（settings.rs 的 set_native_theme 命令调用）：
/// 置深/浅开关并对全部原生视图窗口 InvalidateRect 触发 WM_PAINT 重绘标题栏。
pub fn set_native_theme_dark(dark: bool) {
    NATIVE_DARK.store(dark, Ordering::SeqCst);
    unsafe {
        let map = VIEWS.lock().unwrap();
        if let Some(m) = map.as_ref() {
            for e in m.values() {
                InvalidateRect(e.hwnd.0, std::ptr::null(), 1);
            }
        }
    }
}

unsafe fn draw_chrome(hdc: *mut core::ffi::c_void, client_w: i32) {
    let dark = NATIVE_DARK.load(Ordering::SeqCst);
    // 深色：RGB(15,17,23)；浅色：RGB(239,241,245)
    let bar_color = if dark { 0x0017110F } else { 0x00F5F1EF };
    // 文字：深色主题白、浅色主题深灰 RGB(27,34,51)
    let txt_color = if dark { 0x00FFFFFF } else { 0x0033221B };
    let bar = CreateSolidBrush(bar_color);
    let rc_bar = RECT {
        left: 0,
        top: 0,
        right: client_w,
        bottom: TITLE_BAR_H,
    };
    FillRect(hdc, &rc_bar, bar);
    DeleteObject(bar);
    SetBkMode(hdc, 1); // TRANSPARENT
    SetTextColor(hdc, txt_color);
    let font = GetStockObject(DEFAULT_GUI_FONT);
    let prev_font = SelectObject(hdc, font);
    let title: Vec<u16> = TITLE.to_vec();
    TextOutW(hdc, 12, 8, title.as_ptr(), (title.len().saturating_sub(1)) as i32);
    SelectObject(hdc, prev_font);
    // 关闭槽红底（右起 46px）
    let close_bg = CreateSolidBrush(0x001C2BC4); // RGB(196,43,28)
    let rc_close = RECT {
        left: client_w - 46,
        top: 0,
        right: client_w,
        bottom: TITLE_BAR_H,
    };
    FillRect(hdc, &rc_close, close_bg);
    DeleteObject(close_bg);
    SetTextColor(hdc, 0x00FFFFFF); // 关闭槽红底恒配白字
    // SAS 槽底色（深色主题深灰 RGB(64,64,64)；浅色主题浅灰 RGB(200,203,210)）
    let sas_color = if dark { 0x00404040 } else { 0x00D2CBC8 };
    let sas_bg = CreateSolidBrush(sas_color);
    let rc_sas = RECT {
        left: client_w - 184,
        top: 0,
        right: client_w - 138,
        bottom: TITLE_BAR_H,
    };
    FillRect(hdc, &rc_sas, sas_bg);
    DeleteObject(sas_bg);
    SetTextColor(hdc, txt_color);
    // 字形：[SAS][─][□][✕] 右起四槽各 46px，槽内左偏 14px
    let sas_text: Vec<u16> = "SAS\0".encode_utf16().collect();
    TextOutW(hdc, client_w - 184 + 10, 8, sas_text.as_ptr(), (sas_text.len() - 1) as i32);
    let glyphs: [[u16; 1]; 3] = [[0x2500], [0x25A1], [0x2715]];
    let xs = [client_w - 138 + 14, client_w - 92 + 14, client_w - 46 + 14];
    for i in 0..3 {
        TextOutW(hdc, xs[i], 8, glyphs[i].as_ptr(), 1);
    }
}

/// 增量直绘（DELTA-20261115 传输优化）：把引擎 Delta 脏矩形写入本地
/// FRAME_STORE 帧缓冲并只重绘脏区（此前原生窗口丢弃 Delta，只能等
/// EndPaint 全帧——order 内容更新延迟被 100ms 节流钳制）。
/// 几何与 present()/paint_from_store() 同款：帧画在 (0, TITLE_BAR_H)，
/// 目标尺寸 = 客户区，源尺寸 = 存帧 (sw, sh)。
/// 前提：存帧尺寸与 Delta 帧尺寸一致（运动期全帧为半分辨率时自动跳过，
/// 由随后的全帧校正）。
pub fn present_delta(
    session_id: &str,
    rects: &[easyrdp_rdp::RectUpdate],
    pixels: &[u8],
    fw: u32,
    fh: u32,
) {
    let entry = {
        let views = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
        views.as_ref().and_then(|m| m.get(session_id)).copied()
    };
    let Some(entry) = entry else { return };
    let mut guard = FRAME_STORE.lock().unwrap_or_else(|e| e.into_inner());
    let Some((bgra, sw, sh)) = guard
        .as_mut()
        .and_then(|m| m.get_mut(&(entry.hwnd.0 as isize)))
        .map(|e| (&mut e.0, e.1, e.2))
    else {
        return;
    };
    if sw != fw || sh != fh || bgra.len() != (sw as usize) * (sh as usize) * 4 {
        return; // 尺寸不一致：等全帧校正
    }
    // TILEGHOST-20261116 防线4（审计#4）：整批像素预校验——中段断流时
    // 前几个 rect 已污染 FRAME_STORE 而未绘制、未回滚，半批混合帧会随
    // 下次背板更新上屏（局部 tile 错配闪现）。入循环前整体校验杜绝。
    let pixel_need: usize = rects
        .iter()
        .map(|r| r.w as usize * 4 * r.h as usize)
        .sum();
    if pixel_need > pixels.len() {
        easyrdp_rdp::debug_log(&format!(
            "[native-view] TILEGHOST 预校验: 批像素不足需 {} 实 {}，整批丢弃等全帧",
            pixel_need,
            pixels.len()
        ));
        return;
    }
    // TILEGHOST-20261116 防线2：异常增量批守卫（双维度）。
    // ① 面积维度：单批总面积超帧 60%（正常交互增量很小）；
    // ② 形态维度：大量 ≤96px 小 tile（>64 个且占 70%+）= 服务端 tile 流
    //    基线分叉/首刷风暴特征（真机残影形态正是 tile 网格平铺）。
    // 命中任一 → 丢弃整批等全帧（500ms 兜底），宁可短暂不更新不铺错误内容。
    let frame_area = (sw as usize) * (sh as usize);
    let batch_area: usize = rects
        .iter()
        .map(|r| (r.w as usize).min(sw as usize) * (r.h as usize).min(sh as usize))
        .sum();
    let n_small = rects.iter().filter(|r| r.w <= 96 && r.h <= 96).count();
    let storm = rects.len() > 64 && n_small * 10 > rects.len() * 7;
    if batch_area * 5 > frame_area * 3 || storm {
        easyrdp_rdp::debug_log(&format!(
            "[native-view] TILEGHOST 守卫: 丢弃异常增量批 (rects={} 面积={:.1}% 小tile={} 风暴={})",
            rects.len(),
            batch_area as f64 / frame_area as f64 * 100.0,
            n_small, storm
        ));
        return;
    }
    let stride = sw as usize * 4;
    let mut drawn: Vec<(i32, i32, i32, i32, i32, i32, i32, i32)> = Vec::new(); // (dx,dy,dw,dh, sx,sy, cw,ch)
    for r in rects {
        let (x0, y0, w, h) = (r.x as usize, r.y as usize, r.w as usize, r.h as usize);
        if w == 0 || h == 0 || x0 + w > sw as usize || y0 + h > sh as usize {
            continue;
        }
        // 像素写入本地帧缓冲（pixels 为各矩形行紧凑排列，同 on_bitmap_update 打包格式）
        let mut off = 0usize;
        for row in y0..y0 + h {
            let dst = row * stride + x0 * 4;
            let n = w * 4;
            if off + n > pixels.len() {
                return; // 像素不足：放弃本批（等全帧兜底）
            }
            bgra[dst..dst + n].copy_from_slice(&pixels[off..off + n]);
            off += n;
        }
        drawn.push((x0 as i32, y0 as i32, w as i32, h as i32, x0 as i32, y0 as i32, w as i32, h as i32));
    }
    if drawn.is_empty() {
        return;
    }
    unsafe {
        let hdc = GetDC(entry.hwnd.0);
        if hdc.is_null() {
            return;
        }
        let client_w = {
            let mut rc = RECT::default();
            GetClientRect(entry.hwnd.0, &mut rc);
            (rc.right - rc.left).max(1)
        };
        let client_h = {
            let mut rc = RECT::default();
            GetClientRect(entry.hwnd.0, &mut rc);
            (rc.bottom - rc.top).max(1)
        };
        let dst_w = client_w;
        let dst_h = (client_h - TITLE_BAR_H).max(1);
        let sx = dst_w as f64 / sw as f64;
        let sy = dst_h as f64 / sh as f64;
        // FLASHFIX-20261116：脏矩形经 GDI 背板 StretchBlt 绘制。原实现直接
        // 用 **top-down DIB + 源子矩形** 调 StretchDIBits——GDI 对该形态的
        // 源 y 基准存在驱动级歧义，脏区从错误行取像素 → 点击触发的局部
        // 重绘闪现错位/拼贴画面，下一全帧立即纠正（用户看到的"闪一下"）。
        // 背板路径：帧先经 SetDIBitsToDevice（无子矩形，无歧义）整体入背板，
        // 再按 DDB 坐标 StretchBlt 脏区；背板失败则放弃本批（等全帧兜底）。
        let _lock = BLIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Some((mem_dc, mw, mh)) =
            backing_store_update(entry.hwnd.0 as isize, bgra, sw as u32, sh as u32)
        else {
            ReleaseDC(entry.hwnd.0, hdc);
            return;
        };
        for (dx, dy, dw, dh, sx0, sy0, sw0, sh0) in drawn {
            // 脏区映射到窗口坐标（缩放 + 标题栏偏移），最小 1px。
            // GHOSTFIX-R021/022（2026-11）：原实现坐标与宽高各自 floor 截断
            //（(dx*sx) as i32 / (dw*sx) as i32）——非整数缩放比（窗口≠远端
            // 分辨率，如 0.95 自适应缩小）下右/底边界最多欠覆盖 1px，
            // 残留一列/一行旧像素 = 拖动窗口边缘残影、按钮悬停边缘残影。
            // 改为**边缘点取整**：左/上 round、右/下按（原点+尺寸）整体
            // round 取边界，w/h = 边界差 → 完整覆盖无欠覆盖（相邻矩形
            // 1px 重叠无害，SRCCOPY 同源自覆盖）。
            let x = ((dx as f64 * sx).round()) as i32;
            let y = TITLE_BAR_H + ((dy as f64 * sy).round()) as i32;
            let x2 = (((dx + dw) as f64 * sx).round()) as i32;
            let y2 = TITLE_BAR_H + (((dy + dh) as f64 * sy).round()) as i32;
            let w = (x2 - x).max(1).min(dst_w - x).max(1);
            let h = (y2 - y).max(1).min(dst_h - (y - TITLE_BAR_H)).max(1);
            // 源矩形按 DDB 坐标（自顶向下）截取并防越界
            let src_x = sx0.clamp(0, mw - 1);
            let src_y = sy0.clamp(0, mh - 1);
            let src_w = sw0.min(mw - src_x).max(1);
            let src_h = sh0.min(mh - src_y).max(1);
            _ = StretchBlt(
                hdc, x, y, w, h,
                mem_dc, src_x, src_y, src_w, src_h,
                SRCCOPY,
            );
        }
        ReleaseDC(entry.hwnd.0, hdc);
    }
}

/// WM_PAINT 重绘：从 FRAME_STORE 取帧 → BeginPaint DC 绘制。
/// GHOSTFIX-20261115：与 present() 同款偏移——帧绘制在标题栏以下
///（y=TITLE_BAR_H 起，高度=客户区-32），否则遮挡重绘时帧内容盖住标题栏、
/// 且不重绘 chrome → 错位残影。绘制后补画标题栏 chrome。
unsafe fn paint_from_store(hwnd: HWND, ps: &mut PAINTSTRUCT) {
    // TILEGHOST-20261116（审计#5）：仅当 FRAME_STORE 帧不旧于背板时才允许
    // 经背板绘制；否则直接从帧走 StretchDIBits（不把旧帧回写背板——
    // 旧克隆帧回写会把背板倒退到旧世代，pump 间隙触发即闪现旧画面）。
    let frame = FRAME_STORE
        .lock().unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&(hwnd as isize)))
        .cloned();
    let Some((bgra, w, h)) = frame else {
        return;
    };
    let mut rc = RECT::default();
    GetClientRect(hwnd, &mut rc);
    let dst_w = (rc.right - rc.left).max(1);
    let dst_h = (rc.bottom - rc.top - TITLE_BAR_H).max(1);
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: w * h * 4,
            ..Default::default()
        },
        ..Default::default()
    };
    // 背板只读优先：背板存在（尺寸匹配）→ BitBlt 现存背板（绝不回写）；
    // 背板不存在 → 允许用本帧初始化（首帧场景）后再 Blt。
    let _lock = BLIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let backing_ready = {
        let mut map = BLIT_BACKING.lock().unwrap_or_else(|e| e.into_inner());
        let m = map.get_or_insert_with(HashMap::new);
        match m.get(&(hwnd as isize)) {
            Some(b) => b.w == w as i32 && b.h == h as i32,
            None => {
                // 初始化背板（首帧/重建）
                drop(map);
                matches!(backing_store_update(hwnd as isize, &bgra, w, h), Some(_))
            }
        }
    };
    let blitted = if backing_ready {
        let mem_dc = {
            let map = BLIT_BACKING.lock().unwrap_or_else(|e| e.into_inner());
            map.as_ref().and_then(|m| m.get(&(hwnd as isize))).map(|b| (b.hdc, b.w, b.h))
        };
        match mem_dc {
            Some((mem_dc, mw, mh)) => {
                // STRIPEFIX-20261116：原实现 BitBlt **1:1 无缩放**拷贝
                //（dst_w×dst_h ← mem_dc 原点）——背板尺寸 = 远端帧分辨率，
                // 窗口被 resize 后 dst_w/dst_h ≠ mw/mh：源按窗口尺寸从
                // 背板**左上角裁剪**取样 → 右/底边缘列错位、拖动缩放时
                // 边缘残留彩色竖条纹（列基准漂移特征）。与 present() 对齐
                // 改 StretchBlt 全源→全目标缩放（mw×mh → dst_w×dst_h）。
                _ = StretchBlt(
                    ps.hdc,
                    0,
                    TITLE_BAR_H,
                    dst_w,
                    dst_h,
                    mem_dc,
                    0,
                    0,
                    mw,
                    mh,
                    SRCCOPY,
                );
                true
            }
            None => false,
        }
    } else {
        // 背板尺寸与帧不符：帧可能比背板旧——走无回退路径直接画帧
        _ = StretchDIBits(
            ps.hdc,
            0,
            TITLE_BAR_H,
            dst_w,
            dst_h,
            0,
            0,
            w as i32,
            h as i32,
            bgra.as_ptr() as *const core::ffi::c_void,
            &bmi,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
        true
    };
    if !blitted {
        _ = StretchDIBits(
            ps.hdc,
            0,
            TITLE_BAR_H,
            dst_w,
            dst_h,
            0,
            0,
            w as i32,
            h as i32,
            bgra.as_ptr() as *const core::ffi::c_void,
            &bmi,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
    // 标题栏 chrome 重绘（遮挡后 WM_PAINT 时保持标题栏一致）
    draw_chrome(ps.hdc, dst_w);
}

/// 首帧标记窗口表：GHOSTFIX-20261115 已移除（红框判别脚手架清理）。

/// 窗口类原子缓存（RegisterClass 成功后记录；1410 复用场景回查）。
static CLASS_ATOM: OnceLock<u16> = OnceLock::new();

/// 窗口类注册（每次 attach 都尝试：RegisterClass 同名重复注册返回 0 +
/// ERROR_CLASS_ALREADY_EXISTS(1410) 属正常——**此前 OnceLock 跳过策略是
/// 静默失败候选：一旦首次注册失败即永久标记已注册**，后续全部建窗失败）。
fn register_class() -> Option<u16> {
    unsafe {
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: windows_sys::Win32::Graphics::Gdi::GetStockObject(
                windows_sys::Win32::Graphics::Gdi::BLACK_BRUSH,
            ),
            lpszMenuName: std::ptr::null(),
            lpszClassName: WINDOW_CLASS.as_ptr(),
        };
        let atom = RegisterClassW(&wc);
        if atom != 0 {
            easyrdp_rdp::debug_log(&format!("[native-view] RegisterClassW ok atom={atom}"));
            let _ = CLASS_ATOM.set(atom as u16);
            return Some(atom as u16);
        }
        // 0 返回：同名已注册（1410）= 类可用（原子取自 CLASS_ATOM 缓存）；其它 = 真失败
        let err = windows_sys::Win32::Foundation::GetLastError();
        if err == 1410 {
            easyrdp_rdp::debug_log("[native-view] RegisterClassW 类已存在（复用 CLASS_ATOM）");
            return CLASS_ATOM.get().copied();
        }
        easyrdp_rdp::debug_log(&format!(
            "[native-view] RegisterClassW 失败 err={err}"
        ));
        None
    }
}

/// WM_LBUTTONDOWN 等按钮消息 → (按钮名, 是否按下)。
fn button_of(msg: u32) -> (&'static str, bool) {
    match msg {
        WM_LBUTTONDOWN => ("left", true),
        WM_LBUTTONUP => ("left", false),
        WM_RBUTTONDOWN => ("right", true),
        WM_RBUTTONUP => ("right", false),
        WM_MBUTTONDOWN => ("middle", true),
        _ => ("middle", false),
    }
}

/// 按钮消息 → 是否按下事件（WM_LBUTTONDOWN/RBUTTONDOWN/MBUTTONDOWN）。
fn down_is(msg: u32) -> bool {
    matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN)
}

/// 鼠标/键盘事件 → 输入通道（转发线程消费）。
///
/// **测量可靠性（2026-11 v2）**：不依赖定时器——每 100 个事件直接 log
/// 一条汇总；每类输入的首条到达也立即 log（瞬间确认 WndProc 收到输入）。
static INPUT_SEEN: Mutex<[bool; 2]> = Mutex::new([false; 2]); // mouse/keys 标记
static INPUT_COUNTS: Mutex<(u64, u64)> = Mutex::new((0, 0)); // mouse/keys

fn input_stats_bump(kind: u8) {
    // kind: 0=mouse 1=keys
    let mut first = INPUT_SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let mut counts = INPUT_COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    let (is_first, total) = if kind == 0 {
        first[0] = true;
        counts.0 += 1;
        (counts.0 == 1, counts.0)
    } else {
        first[1] = true;
        counts.1 += 1;
        (counts.1 == 1, counts.1)
    };
    drop(first);
    drop(counts);
    // **每会话首条即打印（attach 重置计数）+ 每会话前 20 条逐条打印**
    //（此前计数进程级累计从不重置 → 后续会话"零日志"= 测量假象实锤修复）
    if is_first || total <= 20 {
        easyrdp_rdp::debug_log(&format!(
            "[native-view] 输入到达 kind={kind} 第{total}条——WndProc 输入臂已执行"
        ));
    }
}

/// 鼠标/键盘事件 → 输入通道（转发线程消费）。
/// R-20261116-007：按 hwnd 查本窗口会话的路由（多会话输入不再串扰）；
/// 无路由（窗口已销毁/会话已断）静默丢弃。
fn forward_native_input(hwnd: HWND, ev: InputEvent) {
    let tx = INPUT_ROUTE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&(hwnd as isize)))
        .cloned();
    if let Some(tx) = tx {
        let _ = tx.send(ev);
    }
}

/// 该 hwnd 是否为某个活跃会话的原生窗口（input_hook LL 钩子前台判定用）。
pub fn is_session_hwnd(hwnd: HWND) -> bool {
    if hwnd.is_null() {
        return false;
    }
    VIEWS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map_or(false, |m| m.values().any(|e| e.hwnd.0 == hwnd))
}

/// LL 钩子（input_hook.rs）转发 Win 键：查**前台会话窗口**的路由发送
/// down/up（与「⊞ Win」按钮同通道，经 InputEvent::Key "VK:0x5B/0x5C" →
/// 会话转发线程 → freerdp input）。
pub fn forward_win_key_foreground(vk: u32, down: bool) {
    let fg = unsafe { GetForegroundWindow() };
    if is_session_hwnd(fg) {
        forward_native_input(
            fg,
            InputEvent::Key { code: format!("VK:0x{vk:04X}"), down },
        );
    }
}

/// LL 钩子转发远端 SAS（Ctrl+Alt+Del 专用 PDU 序列）。
pub fn forward_sas_foreground() {
    let fg = unsafe { GetForegroundWindow() };
    if is_session_hwnd(fg) {
        forward_native_input(fg, InputEvent::Sas);
    }
}

// ---------------------------------------------------------------------------
// WINKEY → R-20261116-006：低级键盘钩子已迁移至 input_hook.rs（独立线程 +
// GetMessage 泵 + 引用计数 + 按 hwnd 路由转发）。本文件仅保留转发辅助
// forward_win_key_foreground / forward_sas_foreground（见上方）。
// ---------------------------------------------------------------------------

/// 读本地剪贴板 CF_UNICODETEXT 文本（CLIPSYNC-20261115）。
/// None = 打开失败/无文本格式。
fn read_clipboard_text() -> Option<String> {
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        // OpenClipboard 短重试（对齐 wf_cliprdr try_open_clipboard）
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(hwnd_of_active_view()) != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if !opened {
            return None;
        }
        let result = (|| -> Option<String> {
            let h = GetClipboardData(CF_UNICODETEXT);
            if h.is_null() {
                return None;
            }
            let ptr = GlobalLock(h) as *const u16;
            if ptr.is_null() {
                return None;
            }
            // 以 NUL 结尾截断（剪贴板 UTF-16 文本契约）
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
                if len > 1 << 22 {
                    break; // 防御：无 NUL 的异常数据（4M units 上限）
                }
            }
            let units = std::slice::from_raw_parts(ptr, len);
            let text = String::from_utf16_lossy(units);
            let _ = GlobalUnlock(h);
            Some(text)
        })();
        CloseClipboard();
        result
    }
}

/// 当前活跃视图 hwnd（OpenClipboard 用；无视图返回 null —— 系统允许）。
fn hwnd_of_active_view() -> *mut core::ffi::c_void {
    VIEWS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.values().next())
        .map(|e| e.hwnd.0)
        .unwrap_or(std::ptr::null_mut())
}

/// WM_CLOSE → 关闭通道回传 session_id（转发线程/上层停会话）。
fn notify_close(hwnd: HWND) {
    let sid = VIEWS
        .lock().unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| {
            m.iter()
                .find(|(_, e)| e.hwnd.0 == hwnd)
                .map(|(sid, _)| sid.clone())
        });
    if let Some(sid) = sid {
        if let Some(tx) = CLOSE_TX
            .lock().unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|m| m.get(&(hwnd as isize)))
        {
            let _ = tx.send(sid);
        }
    }
}

/// 建窗期消息追踪（前 40 条逐条落 debug.log——CreateWindowExW 内 WM_NCCREATE
/// 返回 0 会导致 hwnd=null 且 LastError 不变，正是 err=0 的已知模式）。
static CREATE_MSG_LOG: AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// 输入类消息追踪计数（上限 60——建窗后的鼠标/键盘消息逐条取证）。
static INPUT_TRACE: AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// 探针回执计数（WM_USER+1 收到——消息派发存活证明）。
static PROBE_RECV: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// 输入臂直连日志计数（每类前 20 条逐条 log，绕过统计间接层）。
static RAW_MOUSE_LOGGED: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);
static RAW_KEY_LOGGED: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);
/// SetCapture 释放标记（首条 WM_MOUSEMOVE 后释放）。
/// WM_POINTER 首条到达标记（现代指针输入诊断）。
static POINTER_SEEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // 建窗期消息逐条追踪（前 40 条）+ **输入类消息逐条追踪（上限 60）**：
    // 探针证明 WndProc 存活收消息，但输入臂零执行——追踪建窗后的输入类
    // 消息是否到达（WM_MOUSEMOVE=0x0200/按钮 0x0201-0x0209/滚轮 0x020A/
    // 键盘 0x0100-0x0108/指针 0x0241-0x0249）。
    let n = CREATE_MSG_LOG.fetch_add(1, Ordering::Relaxed);
    let input_ish = matches!(msg, 0x0100..=0x0108 | 0x0200..=0x020A | 0x0241..=0x0249);
    let input_trace = INPUT_TRACE.fetch_add(1, Ordering::Relaxed);
    if n < 40 || (input_ish && input_trace < 60) {
        easyrdp_rdp::debug_log(&format!(
            "[native-view] wnd_proc msg={msg:#06x} wparam={wparam:#x} lparam={lparam:#x} EDITTOOL-LAND-TEST-20261114"
        ));
    }
    match msg {
        // 消息队列探针回执（pump 每 2s PostMessage；收到 = 派发链存活）
        0x0401 => {
            let n = PROBE_RECV.fetch_add(1, Ordering::Relaxed);
            if n < 20 {
                easyrdp_rdp::debug_log(&format!(
                    "[native-view] 探针回执 seq={}（消息派发正常）",
                    wparam
                ));
            }
            0
        }
        // 现代指针输入（Win10/11 触摸/精密触摸板可能发 POINTER 而非旧版
        // 鼠标消息）：首条到达即日志（诊断"输入无效"是否为指针消息形态）
        0x0241 | 0x0246 | 0x0247 | 0x0249 => {
            if !POINTER_SEEN.swap(true, Ordering::Relaxed) {
                easyrdp_rdp::debug_log(&format!(
                    "[native-view] WM_POINTER 消息到达 msg={msg:#06x}——服务端发的是指针形态输入"
                ));
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_NCCREATE => {
            // **必须传 DefWindowProc 并返回非 0**，否则建窗中止（null + err 不变）
            let r = DefWindowProcW(hwnd, msg, wparam, lparam);
            if n < 40 {
                easyrdp_rdp::debug_log(&format!(
                    "[native-view] WM_NCCREATE → DefWindowProc 返回 {r:#x}"
                ));
            }
            r
        }
        // RESIZEFIX-20261116：无边框窗口边缘 resize 热区（用户实测缺陷：鼠标
        // 指向窗口边缘不变形状、无法拖边缩放）。根因：WndProc 从未处理
        // WM_NCHITTEST，DefWindowProc 对 WS_POPUP 全客户区返回 HTCLIENT。
        // 方案：客户区四周 8px 热区返回标准 HT* 码，Windows 原生处理边缘
        // 光标与拖拽缩放（进入 WM_NCLBUTTONDOWN sizing 循环，与自绘标题栏
        // 拖动互不干扰——标题栏拖动走 WM_LBUTTONDOWN 内 SendMessage HTCAPTION
        // 客户区路径，不受影响）。注意：
        // - 标题栏（8..32）必须保持 HTCLIENT：返回 HTCAPTION 会把按钮点击
        //   变成非客户区消息，WM_LBUTTONDOWN 的最小化/最大化/关闭/SAS 全失效；
        // - 顶边按钮槽位（右起 184px）优先让位 HTCLIENT，避免顶边热区吞掉
        //   按钮上半部点击；
        // - 最大化状态一律 HTCLIENT（不可 resize，且 DefWindowProc 对 zoomed
        //   边缘返回移动热区的行为不适用于本自绘布局）。
        WM_NCHITTEST => {
            if IsZoomed(hwnd) != 0 {
                return HTCLIENT as _;
            }
            // lparam = 屏幕坐标（多显示器可为负，按有符号 16 位解码）
            let sx = (lparam as isize) as i16 as i32;
            let sy = ((lparam as isize) >> 16) as i16 as i32;
            let mut pt = POINT { x: sx, y: sy };
            unsafe { ScreenToClient(hwnd, &mut pt) };
            let mut rc = RECT::default();
            unsafe { GetClientRect(hwnd, &mut rc) };
            let cw = rc.right - rc.left;
            let ch = rc.bottom - rc.top;
            if pt.x < 0 || pt.y < 0 || pt.x >= cw || pt.y >= ch {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            const EDGE: i32 = 8;
            // 标题栏按钮/SAS 槽位（右起 184px，y<32）优先可点
            if pt.y < TITLE_BAR_H && pt.x >= cw - 184 {
                return HTCLIENT as _;
            }
            if pt.y < EDGE && pt.x < EDGE {
                HTTOPLEFT as _
            } else if pt.y < EDGE && pt.x >= cw - EDGE {
                HTTOPRIGHT as _
            } else if pt.y < EDGE {
                HTTOP as _
            } else if pt.y >= ch - EDGE && pt.x < EDGE {
                HTBOTTOMLEFT as _
            } else if pt.y >= ch - EDGE && pt.x >= cw - EDGE {
                HTBOTTOMRIGHT as _
            } else if pt.y >= ch - EDGE {
                HTBOTTOM as _
            } else if pt.x < EDGE {
                HTLEFT as _
            } else if pt.x >= cw - EDGE {
                HTRIGHT as _
            } else {
                HTCLIENT as _ // 标题栏与其余客户区：保持既有 WM_LBUTTONDOWN 逻辑
            }
        }
        // WM_SETCURSOR（2026-11 光标卡住修复）：客户区（HTCLIENT=1）→
        // 显式 SetCursor 标准箭头 + return TRUE（类 hCursor 虽已设箭头，
        // 双保险确保客户区光标永不"卡住"）；非客户区交 DefWindowProc。
        // GHOSTFIX-20261115 本地窗口残影修复：
        // - WM_ERASEBKGND 返回 1 跳过默认黑刷（类背景 BLACK_BRUSH）——移动/
        //   尺寸变化时先黑闪再贴帧的"黑块残影"消失，由 WM_PAINT 全量覆盖。
        // - WM_MOVE/WM_SIZE 失效客户区 → WM_PAINT 从 FRAME_STORE 全帧重绘
        //  （CS_HREDRAW|CS_VREDRAW 已覆盖尺寸变化；WM_MOVE 补移动路径，
        //   含标题栏拖动结束后的区域校准），帧源始终是最后一帧。
        WM_ERASEBKGND => 1,
        // TILEGHOST-20261116（审计#6）：WM_MOVE 不再失效客户区（WS_POPUP
        // 无边框移动不产生客户区内容偏移——每次 MOVE 全窗失效会让 WM_PAINT
        // 频繁用 FRAME_STORE 旧帧整窗重绘（闪旧帧），拖动窗口时被放大）。
        // 仅 WM_SIZE（客户区尺寸真变）失效重绘。
        WM_SIZE => {
            if IsWindow(hwnd) != 0 {
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            }
            0
        }
        WM_SETCURSOR => {
            let ht = (lparam as isize as u16) & 0xFFFF; // LOWORD = 命中测试码
            if ht == 1 {
                // HTCLIENT
                unsafe {
                    SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_ARROW));
                }
                return 1; // TRUE：已处理
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            paint_from_store(hwnd, &mut ps);
            EndPaint(hwnd, &ps);
            0
        }
        // CLIPSYNC-20261115：本地剪贴板变化（0x031D = WM_CLIPBOARDUPDATE）
        // → 读 CF_UNICODETEXT → 回环防护判定 → clip_tx → 会话转发线程
        // send_clipboard_text（本地 -> 远端）。
        0x031D => {
            match read_clipboard_text() {
                Some(text) if !text.is_empty() => {
                    if easyrdp_rdp::clip_sync_is_same(&text) {
                        // 最近一次双向同步的同文本（远端写入本地/已发远端）：跳过
                    } else {
                        static CLIP_SENT_LOGGED: AtomicU32 = AtomicU32::new(0);
                        let n = CLIP_SENT_LOGGED.fetch_add(1, Ordering::Relaxed);
                        if n < 10 {
                            easyrdp_rdp::debug_log(&format!(
                                "[cliprdr] 本地剪贴板文本变化 chars={}——发往远端 FormatList",
                                text.chars().count()
                            ));
                        }
                        if let Some(tx) = CLIP_ROUTE
                            .lock().unwrap_or_else(|e| e.into_inner())
                            .as_ref()
                            .and_then(|m| m.get(&(hwnd as isize)))
                        {
                            let _ = tx.send(text);
                        }
                    }
                }
                _ => {}
            }
            0
        }
        WM_MOUSEMOVE => {
            // GHOSTFIX-20261115：删除逐事件同步日志（"RDPSCREEN-GEN-0055"/
            // "ZEBRA2-INPUT-ARM-EXEC"）——每次鼠标移动同步写文件导致掉帧、
            // 加剧残影观感。input_stats_bump 内的限频汇总保留。
            input_stats_bump(0);
            let (x, y) = lparam_point(lparam);
            // 标题栏区不转发（客户区 y≥32 才是远端画面区）
            if y >= TITLE_BAR_H {
                forward_native_input(
                    hwnd,
                    InputEvent::MouseMove {
                        x: x.max(0) as u32,
                        y: (y - TITLE_BAR_H).max(0) as u32,
                    },
                );
            }
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
        | WM_MBUTTONUP => {
            input_stats_bump(0);
            let (x, y) = lparam_point(lparam);
            // 标题栏命中（y<32）：最小化/最大化/关闭/拖动
            if down_is(msg) && y < TITLE_BAR_H {
                let cw = {
                    let mut rc = RECT::default();
                    GetClientRect(hwnd, &mut rc);
                    rc.right - rc.left
                };
                if x >= cw - 46 {
                    // 关闭
                    easyrdp_rdp::debug_log("[native-view] 标题栏：关闭按钮点击");
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    return 0;
                }
                if x >= cw - 92 {
                    // 最大化切换
                    easyrdp_rdp::debug_log("[native-view] 标题栏：最大化按钮点击");
                    let zoomed = IsZoomed(hwnd) != 0;
                    ShowWindow(hwnd, if zoomed { SW_RESTORE } else { SW_MAXIMIZE });
                    return 0;
                }
                if x >= cw - 138 {
                    // 最小化
                    easyrdp_rdp::debug_log("[native-view] 标题栏：最小化按钮点击");
                    ShowWindow(hwnd, SW_MINIMIZE);
                    return 0;
                }
                if x >= cw - 184 {
                    // SAS：远端 Ctrl+Alt+Del（本地 OS 截获无法转发，走专用 PDU 序列）
                    easyrdp_rdp::debug_log("[native-view] 标题栏：SAS 按钮点击 → 远端 Ctrl+Alt+Del");
                    forward_native_input(hwnd, InputEvent::Sas);
                    return 0;
                }
                // 拖动移动（标准 borderless 技巧）
                easyrdp_rdp::debug_log("[native-view] 标题栏：拖动开始");
                ReleaseCapture();
                SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
                return 0;
            }
            let (button, down) = match msg {
                WM_LBUTTONDOWN => ("left", true),
                WM_LBUTTONUP => ("left", false),
                WM_RBUTTONDOWN => ("right", true),
                WM_RBUTTONUP => ("right", false),
                WM_MBUTTONDOWN => ("middle", true),
                _ => ("middle", false),
            };
            forward_native_input(
                hwnd,
                InputEvent::MouseButton {
                    button: button.into(),
                    down,
                    x: x.max(0) as u32,
                    y: (y - TITLE_BAR_H).max(0) as u32,
                },
            );
            SetFocus(hwnd); // 点击即聚焦（键盘事件跟随）
            0
        }
        WM_MOUSEWHEEL => {
            input_stats_bump(0);
            let delta = ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32;
            forward_native_input(hwnd, InputEvent::Wheel { delta });
            0
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            input_stats_bump(1);
            let vk = wparam & 0xFF;
            let raw = RAW_KEY_LOGGED.fetch_add(1, Ordering::Relaxed);
            if raw < 20 {
                easyrdp_rdp::debug_log(&format!(
                    "[native-view] 直连 WM_KEYDOWN #{raw} vk={vk:#04x}"
                ));
            }
            forward_native_input(
                hwnd,
                InputEvent::Key {
                    code: format!("VK:{vk:#06X}"),
                    down: true,
                },
            );
            0
        }
        WM_KEYUP | WM_SYSKEYUP => {
            input_stats_bump(1);
            let vk = wparam & 0xFF;
            forward_native_input(
                hwnd,
                InputEvent::Key {
                    code: format!("VK:{vk:#06X}"),
                    down: false,
                },
            );
            0
        }
        WM_CLOSE => {
            easyrdp_rdp::debug_log("[native-view] WM_CLOSE 到达（用户关闭窗口）");
            notify_close(hwnd);
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            easyrdp_rdp::debug_log("[native-view] WM_DESTROY 到达（窗口销毁——fix41 已移除 PostQuitMessage）");
            // FLASHFIX-20261116：释放本窗口 GDI 背板（内存 DC + 位图）。
            unsafe { backing_store_drop(hwnd as isize) };
            // R-006：注销低级键盘钩子引用（input_hook 引用计数归零即卸钩）
            crate::input_hook::release();
            // fix41：窗口建在 Tauri 主线程——PostQuitMessage 会向 Tauri 主事件循环投递 WM_QUIT，
            // 与 close_rx 恢复 Tauri 窗口竞争 → 主循环退出 = 关窗后主界面永不恢复、进程僵尸残留。只记录，不退出。
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}


/// 会话是否启用了原生窗口（前端据此回连接管理器）。
pub fn has_view(session_id: &str) -> bool {
    VIEWS
        .lock().unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|m| m.contains_key(session_id))
        .unwrap_or(false)
}



// 占位函数清理：draw_frame 已被 paint_from_store/present 直绘替代
fn lparam_point(lparam: LPARAM) -> (i32, i32) {
    let l = lparam as isize as i32;
    (l & 0xFFFF, (l >> 16) & 0xFFFF)
}


