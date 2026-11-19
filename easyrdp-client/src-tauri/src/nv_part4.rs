
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
    // **半分辨率自适应（2026-11）**：emitter 运动期降载发送 w/2×h/2 半帧
    //（JPEG 管线优化遗留——原生直绘无编码成本，StretchDIBits 自带缩放，
    // 半帧直接拉伸到客户区即可，无需引擎改动）。识别：len == 全帧 → 1:1；
    // len == 半帧 → 源尺寸减半；其余 → 短缓冲计数。
    let full_len = w as usize * h as usize * 4;
    let half_w = (w / 2).max(1) as usize;
    let half_h = (h / 2).max(1) as usize;
    let half_len = half_w * half_h * 4;
    let (sw, sh) = if bgra.len() >= full_len {
        (w as usize, h as usize)
    } else if half_len > 0 && bgra.len() >= half_len {
        (half_w, half_h)
    } else {
        st.3 += 1; // 短缓冲（异常长度，原样计数）
        flush(&mut st);
        return true; // 有窗口但缓冲不完整：跳过本次
    };
    // **首帧判别标记（2026-11）**：每窗口首帧左上角红色 8px 边框——
    // 用户看到红框 = 可见窗口是我们的原生窗口；看不到红框 = 用户看到的
    // 是其他渲染来源（旧构建僵尸窗口/webview 残留）→ 问题完全不同。
    let marked_owned: Vec<u8>;
    {
        let mut guard = MARKED_WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
        let marked = guard.get_or_insert_with(HashMap::new);
        let already = marked.get(&(entry.hwnd.0 as isize)).copied();
        if already == Some(true) {
            marked_owned = bgra.to_vec();
        } else {
            marked.insert(entry.hwnd.0 as isize, true);
            let mut v = bgra.to_vec();
            // 左上角红色 8px 边框（BGRA: B=0,G=0,R=255,A=255），宽 80px
            let stride = sw * 4;
            for y in 0..8usize {
                for x in 0..80usize {
                    let o = y * stride + x * 4;
                    if o + 3 < v.len() {
                        v[o] = 0;
                        v[o + 1] = 0;
                        v[o + 2] = 255;
                        v[o + 3] = 255;
                    }
                }
            }
            easyrdp_rdp::debug_log("[native-view] 首帧判别标记（红框）已绘制");
            marked_owned = v;
        }
    }
    let bgra_draw: &[u8] = marked_owned.as_slice();
    // 存最新帧（WM_PAINT 重绘用，存实际源尺寸）
    FRAME_STORE
        .lock().unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(entry.hwnd.0 as isize, (marked_owned.clone(), sw as u32, sh as u32));
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
        draw_chrome(hdc, dst_w);
        ReleaseDC(entry.hwnd.0, hdc);
    }
    st.1 += 1; // 绘制成功
    flush(&mut st);
    true
}