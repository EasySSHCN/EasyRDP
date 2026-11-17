#[used]
pub static FIX_PROOF_6: &str = "FIXPROOF-PART6-INPUT-ARM-LIVE";
#[used]
pub static FIX_PROOF_6B: &str = "INPUTDAOda";


/// 鼠标/键盘事件 → 输入通道（转发线程消费）。
///
/// **测量可靠性（2026-11 v2）**：不依赖定时器——每 100 个事件直接 log
/// 一条汇总；每类输入的首条到达也立即 log（瞬间确认 WndProc 收到输入）。
static INPUT_SEEN: Mutex<[bool; 3]> = Mutex::new([false; 3]); // mouse/keys/first-log 标记
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

fn forward_native_input(hwnd: HWND, ev: InputEvent) {
    if let Some(tx) = INPUT_ROUTE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(ev);
    }
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