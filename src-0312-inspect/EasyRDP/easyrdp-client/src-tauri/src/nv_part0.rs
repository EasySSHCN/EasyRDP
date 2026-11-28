// 原生远程桌面窗口（2026-11 架构修正：mstsc 同款独立顶层窗口）。
//
// fix15 子窗口方案的 z-order/输入路由/webview 遮盖三问题互相纠缠不可持续
// → 改为 **独立顶层窗口**（WS_OVERLAPPEDWINDOW，无 SetParent）：Tauri
// 窗口纯连接管理器，连接时打开本窗口（客户区 = 远端分辨率 1:1），断开
// （WM_CLOSE）自动停会话并回连接管理器。
//
// 帧直绘：`present()` 保存最新 BGRA 帧 + StretchDIBits 即时绘制；
// WM_PAINT 从帧存储重绘。零 JPEG 编码、零 base64、零 IPC 传输。
//
// 输入直通：WndProc 处理鼠标/滚轮/键盘 Win32 消息（客户区 = 远端分辨率
// 1:1，坐标直用）→ INPUT_TX 通道 → session_start 派生的转发线程 →
// freerdp input。
//
// 仅 Windows（cfg(windows)）；非 Windows 平台保持 webview pull 路径。


use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Mutex, OnceLock, Arc};

use easyrdp_rdp::InputEvent;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::PostQuitMessage;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetDC, GetStockObject,
    ReleaseDC, SelectObject, SetBkMode, SetTextColor, StretchDIBits, TextOutW, TRANSPARENT,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DEFAULT_GUI_FONT, DIB_RGB_COLORS, PAINTSTRUCT,
    SRCCOPY,

};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
    GetMessageW, GetSystemMetrics, HTCAPTION, IDC_ARROW, IsWindow, IsZoomed, LoadCursorW,
    PostMessageW, RegisterClassW, SendMessageW, SetCursor, SetForegroundWindow, SetWindowPos,
    ShowWindow, SM_CXSCREEN, SM_CYSCREEN, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOW,
    SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, TranslateMessage, WM_CLOSE, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_NCLBUTTONDOWN, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WNDCLASSW, WS_POPUP, WS_VISIBLE,
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
/// 全局输入路由（2026-11 终局：WndProc 直发，零查表——hwnd 键查表在
/// 部分环境静默失败的实锤修复）。
static INPUT_ROUTE: Mutex<Option<mpsc::Sender<InputEvent>>> = Mutex::new(None);
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
