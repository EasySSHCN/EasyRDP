#[used]
pub static FIX_PROOF_7: &str = "FIXPROOF-PART7-CLOSE-FIX41-LIVE";


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

unsafe extern "system" fn wnd_proc_RENAME_TEST(/* BUILDPROOF2-20261115 */
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
        // WM_SETCURSOR（2026-11 光标卡住修复）：客户区（HTCLIENT=1）→
        // 显式 SetCursor 标准箭头 + return TRUE（类 hCursor 虽已设箭头，
        // 双保险确保客户区光标永不"卡住"）；非客户区交 DefWindowProc。
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
        WM_MOUSEMOVE => {
            easyrdp_rdp::debug_log("RDPSCREEN-GEN-0055");easyrdp_rdp::debug_log("ZEBRA2-INPUT-ARM-EXEC");
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
            // fix41：窗口建在 Tauri 主线程——PostQuitMessage 会向 Tauri 主事件循环投递 WM_QUIT，
            // 与 close_rx 恢复 Tauri 窗口竞争 → 主循环退出 = 关窗后主界面永不恢复、进程僵尸残留。只记录，不退出。
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}