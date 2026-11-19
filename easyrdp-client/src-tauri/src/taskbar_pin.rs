// 任务栏固定引导（R-20261119-005，v0.4.0）。
//
// 根因（需求池 R-20261119-005）：外部工具裸写
// %APPDATA%\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar\*.lnk
// 时 Explorer 有固定列表缓存且对裸写条目不校验即忽略 → 目录里有 lnk 但任务栏不显示。
//
// 方案（应用内引导，替代裸写 TaskBar）：
// ① 启动检测自身是否已被固定（枚举 TaskBar 目录 *.lnk，从 lnk 字节提取目标
//    exe 名与 current_exe 比对——兼容 Explorer 写的 ANSI/UTF-16 双编码 lnk）；
// ② 未固定且用户未拒绝过 → 前端弹一次性提示（UI 在 index.html/main.js），
//    「固定」按钮走系统正规流程：确保开始菜单 Programs 存在 EasyRDP.lnk →
//    ShellExecuteW(lpVerb="pintotaskbar", lnk) 由 Explorer 自己完成固定；
// ③ 「不再提示」/已固定 → 永久静默（settings.json taskbarPinPrompt.dismissed）；
// ④ 配套加固：启动时 SetCurrentProcessExplicitAppUserModelID("EasyRDP.Client")
//    + 确保存在开始菜单快捷方式（消除 Explorer 拒绝识别的诱因）。

use tauri::AppHandle;

#[cfg(windows)]
const AUMID: &str = "EasyRDP.Client";

/// 启动加固（Windows）：登记显式 AppUserModelID。失败静默（仅影响任务栏
/// 分组/固定识别的辅助诱因，不影响主流程）。
pub fn boot(app: &AppHandle) {
    let _ = app;
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
        let wide: Vec<u16> = AUMID.encode_utf16().chain(std::iter::once(0)).collect();
        let _ = SetCurrentProcessExplicitAppUserModelID(wide.as_ptr());
    }
    // 启动时静默确保开始菜单快捷方式存在（消除 Explorer 拒绝识别诱因）
    #[cfg(windows)]
    ensure_start_menu_shortcut();
}

/// TaskBar 固定目录（当前用户）。
#[cfg(windows)]
fn taskbar_pin_dir() -> Option<std::path::PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    Some(std::path::PathBuf::from(appdata)
        .join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar"))
}

/// 开始菜单 Programs 目录（当前用户）。
#[cfg(windows)]
fn start_menu_programs_dir() -> Option<std::path::PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    Some(std::path::PathBuf::from(appdata)
        .join(r"Microsoft\Windows\Start Menu\Programs"))
}

/// 从 .lnk 原始字节提取目标路径（启发式，兼容 ANSI 与 UTF-16 双编码）：
/// 分别按 ASCII 与 UTF-16LE 解码整个字节流，取出所有以 .exe（不区分大小写）
/// 结尾的可打印路径 token；返回其中最长的一条。
#[cfg(windows)]
fn extract_lnk_target(bytes: &[u8]) -> Option<String> {
    // ANSI 扫描
    let mut best: Option<String> = None;
    let mut scan = |text: &str| {
        let lower = text.to_ascii_lowercase();
        let mut start = 0usize;
        while let Some(rel) = lower[start..].find(".exe") {
            let end = start + rel + 4;
            // 向前回溯到路径起点：可打印字符（含 \ / : 空格 等），遇控制字符停
            let mut s = end;
            while s > 0 {
                let c = text.as_bytes()[s - 1];
                if (0x20..=0x7e).contains(&c) { s -= 1; } else { break; }
            }
            let token = text[s..end].trim_start_matches(|c: char| c == '"' || c == '\'').to_string();
            if token.len() >= 8 && token.to_ascii_lowercase().ends_with(".exe") {
                let take = match &best {
                    Some(b) if b.len() >= token.len() => false,
                    _ => true,
                };
                if take { best = Some(token); }
            }
            start = end;
        }
    };
    scan(&String::from_utf8_lossy(bytes));
    // UTF-16LE 扫描（偶偏移对齐）
    if bytes.len() >= 2 {
        let u16s: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let text: String = String::from_utf16_lossy(&u16s);
        scan(&text);
    }
    best
}

/// 当前进程 exe 的文件名（小写，如 "easyrdp.exe"）。
#[cfg(windows)]
fn current_exe_name() -> Option<String> {
    std::env::current_exe().ok().and_then(|p| {
        p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase())
    })
}

/// 检测当前 exe 是否已被固定到任务栏：扫描 TaskBar 目录全部 .lnk，
/// 提取目标路径并与 current_exe 的绝对路径或文件名比对。
pub fn is_pinned_to_taskbar() -> bool {
    #[cfg(windows)]
    {
        let Some(dir) = taskbar_pin_dir() else { return false };
        let Some(exe_name) = current_exe_name() else { return false };
        let exe_abs = std::env::current_exe().ok().map(|p| p.to_string_lossy().to_ascii_lowercase());
        let Ok(entries) = std::fs::read_dir(&dir) else { return false };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("lnk")) != Some(true) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let Some(target) = extract_lnk_target(&bytes) else { continue };
            let target_lc = target.to_ascii_lowercase();
            let matched = exe_abs.as_deref() == Some(target_lc.as_str())
                || target_lc.ends_with(&format!("\\{exe_name}"))
                || target_lc.ends_with(&format!("/{exe_name}"))
                || target_lc == exe_name;
            if matched {
                return true;
            }
        }
        false
    }
    #[cfg(not(windows))]
    {
        true // 非 Windows 平台无任务栏固定概念，视为已固定（不弹提示）
    }
}

/// 确保开始菜单 Programs 存在 EasyRDP.lnk（指向 current_exe）。
/// 已存在且目标一致时跳过。返回 lnk 路径。
#[cfg(windows)]
fn ensure_start_menu_shortcut() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = start_menu_programs_dir()?;
    let lnk = dir.join("EasyRDP.lnk");
    // 已存在且目标正确：跳过（不覆盖用户可能的改名/自定义）
    if let Ok(bytes) = std::fs::read(&lnk) {
        if let Some(target) = extract_lnk_target(&bytes) {
            let target_lc = target.to_ascii_lowercase();
            let exe_lc = exe.to_string_lossy().to_ascii_lowercase();
            if target_lc == exe_lc || target_lc.ends_with(&format!("\\{}", exe.file_name()?.to_string_lossy().to_ascii_lowercase())) {
                return Some(lnk);
            }
        }
    }
    let _ = create_shortcut(&lnk, &exe);
    Some(lnk)
}

/// 用 PowerShell WScript.Shell 写 .lnk（仅开始菜单自建快捷方式用——
/// TaskBar 目录仍绝不直接写，那是 R-20261119-005 要根治的行为）。
#[cfg(windows)]
fn create_shortcut(lnk: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.Save()",
        lnk.display().to_string().replace('\'', "''"),
        target.display().to_string().replace('\'', "''"),
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("shortcut create failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

/// 走系统正规流程固定到任务栏：ShellExecuteW lpVerb="pintotaskbar" 打开
/// 开始菜单 lnk，由 Explorer 自己执行固定（其固定列表缓存因此正确感知）。
/// 返回 Ok(true)=已触发固定动词；Ok(false)=已固定无需动作；Err=动词受限
/// （典型为 Win11 部分版本），前端引导用户右键手动固定。
#[cfg(windows)]
pub fn pin_to_taskbar_now() -> Result<bool, String> {
    if is_pinned_to_taskbar() {
        return Ok(false);
    }
    let lnk = ensure_start_menu_shortcut().ok_or_else(|| "开始菜单目录不可用".to_string())?;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    unsafe {
        let verb: Vec<u16> = "pintotaskbar".encode_utf16().chain(std::iter::once(0)).collect();
        let file: Vec<u16> = lnk.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
        let rc = ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL as i32,
        );
        // 返回值 > 32 表示成功
        if rc as isize > 32 {
            Ok(true)
        } else {
            Err(format!("系统固定动词受限（代码 {}），请右键 EasyRDP.exe → 固定到任务栏", rc as isize))
        }
    }
}

#[cfg(not(windows))]
pub fn pin_to_taskbar_now() -> Result<bool, String> {
    Ok(false)
}

// ---------------- Tauri commands ----------------

/// 前端启动查询：是否需要弹固定引导（未固定 && 用户未选过「不再提示」）。
#[tauri::command]
pub fn taskbar_pin_status() -> Result<bool, String> {
    Ok(!is_pinned_to_taskbar() && !crate::settings::load_taskbar_pin_dismissed())
}

/// 用户点击「固定到任务栏」。
#[tauri::command]
pub fn taskbar_pin_now() -> Result<bool, String> {
    pin_to_taskbar_now()
}

/// 用户点击「不再提示」：永久静默。
#[tauri::command]
pub fn taskbar_pin_dismiss(app: AppHandle) -> Result<(), String> {
    crate::settings::save_taskbar_pin_dismissed(&app);
    Ok(())
}
