

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