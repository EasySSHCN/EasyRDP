
/// 主线程执行的建窗主体（注册表注册 + ShowWindow；消息由 Tauri 事件循环泵）。
unsafe fn attach_on_main_thread(
    session_id: &str,
    atom: u16,
    remote_w: u32,
    remote_h: u32,
    input_tx: mpsc::Sender<InputEvent>,
    close_tx: mpsc::Sender<String>,
) -> bool {
        // **WS_POPUP + 自绘标题栏（终局方案）**：客户区 = 整窗（每像素
        // HTCLIENT，输入必然进客户区），顶部 32px 自绘标题栏（深色条 +
        // 标题 + 最小化/关闭按钮 + 拖动移动），画面区 = 标题栏以下全部。
        // 建窗后提到顶 + SetForegroundWindow + SetFocus（抢前台）。
        // R-20261120-002：不再常驻置顶（TOPMOST），仅一次性 HWND_TOP 提到 Z 序顶部。
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
        // 前台/焦点（2026-11 输入无效根因修复 v2，R-20261120-002 去强制置顶）：
        // 顶层窗口创建后默认在 Tauri 连接管理器窗口之后/被其完全遮盖——
        // 用户鼠标键盘全进 Tauri webview，原生窗口收不到任何输入。
        // R-20261120-002：HWND_TOPMOST（常驻最顶）改为 HWND_TOP（一次性提到
        // Z 序顶部）：仍能拿到前台与键盘焦点，但其它普通窗口可正常覆盖本窗口。
        SetWindowPos(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        _ = SetForegroundWindow(hwnd);
        _ = SetFocus(hwnd);
        easyrdp_rdp::debug_log("[native-view] 提到顶+前台+焦点已设置（非常驻置顶）");
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
        *INPUT_ROUTE.lock().unwrap_or_else(|e| e.into_inner()) = Some(input_tx);
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