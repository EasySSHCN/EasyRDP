
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
) -> bool {
    easyrdp_rdp::debug_log(&format!(
        "[native-view] attach 进入（主线程建窗）session={session_id} remote={remote_w}x{remote_h} 构建=fix35-20261114-1925"
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
            unsafe { attach_on_main_thread(&sid, atom, remote_w, remote_h, input_tx, close_tx) }
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