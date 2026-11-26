
/// 会话结束：销毁窗口并清注册表（PostMessage WM_CLOSE → WndProc 自毁）。
pub fn detach(session_id: &str) {
    let hwnd = VIEWS
        .lock().unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .and_then(|m| m.remove(session_id))
        .map(|e| e.hwnd.0);
    if let Some(hwnd) = hwnd {
        *INPUT_ROUTE.lock().unwrap_or_else(|e| e.into_inner()) = None; // 断开路由（转发线程退出）
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