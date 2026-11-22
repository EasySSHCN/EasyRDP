
/// 自绘标题栏（WS_POPUP 客户区顶部 32px）：深色条 + 标题文字 + 最小化/关闭
/// 按钮（─ ✕ 槽位各 46px）。帧绘制后调用叠加。
unsafe fn draw_chrome(hdc: *mut core::ffi::c_void, client_w: i32) {
    let bar = CreateSolidBrush(0x0017110F); // RGB(15,17,23) → COLORREF 0x00BBGGRR
    let rc_bar = RECT {
        left: 0,
        top: 0,
        right: client_w,
        bottom: TITLE_BAR_H,
    };
    FillRect(hdc, &rc_bar, bar);
    DeleteObject(bar);
    SetBkMode(hdc, 1); // TRANSPARENT
    SetTextColor(hdc, 0x00FFFFFF); // 白
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
    SetTextColor(hdc, 0x00FFFFFF);
    // 字形 ─ □ ✕（右起三槽各 46px，槽内左偏 14px）
    let glyphs: [[u16; 1]; 3] = [[0x2500], [0x25A1], [0x2715]];
    let xs = [client_w - 138 + 14, client_w - 92 + 14, client_w - 46 + 14];
    for i in 0..3 {
        TextOutW(hdc, xs[i], 8, glyphs[i].as_ptr(), 1);
    }
}

/// WM_PAINT 重绘：从 FRAME_STORE 取帧 → BeginPaint DC 绘制。
unsafe fn paint_from_store(hwnd: HWND, ps: &mut PAINTSTRUCT) {
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
    let dst_h = (rc.bottom - rc.top).max(1);
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
    _ = StretchDIBits(
        ps.hdc,
        0,
        0,
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

/// 首帧标记窗口表（每窗口只画一次判别红框）。
static MARKED_WINDOWS: Mutex<Option<HashMap<isize, bool>>> = Mutex::new(None);

/// 窗口类原子缓存（RegisterClass 成功后记录；1410 复用场景回查）。
static CLASS_ATOM: OnceLock<u16> = OnceLock::new();

/// 窗口类注册（每次 attach 都尝试：RegisterClass 同名重复注册返回 0 +
/// ERROR_CLASS_ALREADY_EXISTS(1410) 属正常——**此前 OnceLock 跳过策略是
/// 静默失败候选：一旦首次注册失败即永久标记已注册**，后续全部建窗失败）。
fn register_class() -> Option<u16> {
    unsafe {
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc_RENAME_TEST),
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