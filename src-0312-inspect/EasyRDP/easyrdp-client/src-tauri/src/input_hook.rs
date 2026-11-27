//! R-20261116-006：低级键盘钩子（WH_KEYBOARD_LL）——独立线程 + 消息泵。
//!
//! 设计（生命周期）：
//! - **独立钩子线程**：LL 钩子回调只在安装线程泵消息时被系统投递。此前把
//!   钩子装在 Tauri 主线程（靠事件循环泵）——首会话 WM_DESTROY 即卸载、
//!   多会话下先断开的会话会杀掉其余会话的 Win 键转发，且主线程卡顿时
//!   钩子超时被系统静默摘除。现改为专用线程：装钩 → GetMessageW 泵 →
//!   收 WM_QUIT 卸钩退出。
//! - **引用计数**：`acquire()`（每次原生窗口 attach 成功后调用）/`release()`
//!   （每个窗口 WM_DESTROY 调用）。首个 acquire 拉起线程，计数归零时向
//!   钩子线程投递 WM_QUIT，线程内 UnhookWindowsHookEx 后退出——防泄漏。
//! - **拦截策略**（与既有「⊞ Win」按钮同一条发送通道）：
//!   - VK_LWIN(0x5B)/VK_RWIN(0x5C) 按下/抬起：转调 native_view 的
//!     Win 键转发函数发 down/up 序列（explorer 全局钩子先于窗口消息打开
//!     本地开始菜单，WndProc 返回 0 拦不住），返回 1 抑制本地；
//!   - Ctrl+Alt+End（VK_END=0x23 + Ctrl+Alt 按住）：转发远端 SAS，返回 1；
//!   - 其余按键一律 CallNextHookEx 放行（不干扰现有输入直通）。
//!   - 仅当前台窗口是某个会话原生窗口时才拦截（焦点路由到激活窗口的会话）。
//!
//! 仅 Windows（cfg(windows)）。

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};

/// 钩子句柄（钩子线程内设置；卸载也在该线程）。
static HOOK_HANDLE: AtomicUsize = AtomicUsize::new(0);
/// 钩子线程 id（release 投递 WM_QUIT 用；0 = 未启动）。
static HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);
/// 活跃原生窗口引用计数（>0 = 有会话需要钩子）。
static REF_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 首条拦截日志标记（限频取证）。
static WIN_LOGGED: AtomicUsize = AtomicUsize::new(0);

/// R-006 入口：会话原生窗口 attach 成功后调用。首次调用拉起钩子线程。
pub fn acquire() {
    let n = REF_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    if n == 1 {
        start_hook_thread();
    }
    easyrdp_rdp::debug_log(&format!("[input-hook] acquire（活跃会话钩子引用={n}）"));
}

/// R-006 出口：窗口 WM_DESTROY（含 detach/会话关闭路径）时调用。
/// 计数归零时向钩子线程投递 WM_QUIT——线程卸钩并退出（防泄漏）。
pub fn release() {
    let old = REF_COUNT.fetch_sub(1, Ordering::SeqCst);
    let n = old.saturating_sub(1);
    easyrdp_rdp::debug_log(&format!("[input-hook] release（剩余引用={n}）"));
    if old <= 1 {
        let tid = HOOK_THREAD_ID.load(Ordering::SeqCst);
        if tid != 0 {
            unsafe {
                PostThreadMessageW(tid, WM_QUIT, 0, 0);
            }
        }
    }
}

/// 拉起钩子线程：线程内安装钩子（须先建立消息队列语义——LL 钩子回调由
/// 本线程 GetMessageW 泵驱动），收到 WM_QUIT 后卸钩退出。
fn start_hook_thread() {
    // 防并发重复拉起（两个会话同时 attach）：CAS 占位线程 id
    if HOOK_THREAD_ID
        .compare_exchange(0, u32::MAX, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return; // 已有线程在启动/运行
    }
    std::thread::spawn(move || unsafe {
        HOOK_THREAD_ID.store(GetCurrentThreadId(), Ordering::SeqCst);
        let hh = SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_keyboard_proc), std::ptr::null_mut(), 0);
        HOOK_HANDLE.store(hh as usize, Ordering::SeqCst);
        easyrdp_rdp::debug_log(&format!(
            "[input-hook] WH_KEYBOARD_LL 已安装（独立线程 tid={}）rc={}",
            HOOK_THREAD_ID.load(Ordering::SeqCst),
            !hh.is_null()
        ));
        let mut msg = MSG::default();
        // GetMessageW 返回 0 = 收到 WM_QUIT；-1 = 错误（同样退出卸钩）
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {}
        let hh = HOOK_HANDLE.swap(0, Ordering::SeqCst);
        if hh != 0 {
            UnhookWindowsHookEx(hh as *mut core::ffi::c_void);
        }
        HOOK_THREAD_ID.store(0, Ordering::SeqCst);
        easyrdp_rdp::debug_log("[input-hook] WH_KEYBOARD_LL 已卸载，钩子线程退出");
    });
}

/// 当前前台窗口是否为某个会话的原生窗口（native_view 注册表判定）。
fn foreground_is_session() -> bool {
    crate::native_view::is_session_hwnd(unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() })
}

/// 本地 Ctrl+Alt 是否按住（GetAsyncKeyState 高位 = 按下）。
fn ctrl_alt_down() -> bool {
    unsafe {
        (GetAsyncKeyState(0x11) as u16) & 0x8000 != 0 // VK_CONTROL
            && (GetAsyncKeyState(0x12) as u16) & 0x8000 != 0 // VK_MENU
    }
}

/// LL 钩子回调（钩子线程内执行——低延迟：命中路径只做查表 + 通道 send）。
unsafe extern "system" fn ll_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 && foreground_is_session() {
        let kb = &*(lparam as *const KBDLLHOOKSTRUCT);
        let vk = kb.vkCode & 0xFF;
        let down = wparam == WM_KEYDOWN as usize || wparam == WM_SYSKEYDOWN as usize;
        let up = wparam == WM_KEYUP as usize || wparam == WM_SYSKEYUP as usize;
        if down || up {
            if vk == 0x5B || vk == 0x5C {
                // Win 键（extended 0xE0 前缀键，scanCode 仅供日志取证）：
                // 与「⊞ Win」按钮同通道转发 down/up，返回 1 抑制本地。
                let n = WIN_LOGGED.fetch_add(1, Ordering::Relaxed);
                if n < 10 {
                    easyrdp_rdp::debug_log(&format!(
                        "[input-hook] 拦截 Win 键 vk=0x{vk:02X} scan=0x{:02X} down={down} → 转发远端",
                        kb.scanCode & 0xFF
                    ));
                }
                crate::native_view::forward_win_key_foreground(vk, down);
                return 1;
            }
            if vk == 0x23 && ctrl_alt_down() {
                // Ctrl+Alt+End → 远端 SAS（Ctrl+Alt+Del）
                if down {
                    easyrdp_rdp::debug_log("[input-hook] 拦截 Ctrl+Alt+End → SAS");
                    crate::native_view::forward_sas_foreground();
                }
                return 1;
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}
