// EasyRDP 快捷方式生成（R-20261116-013）。
//
// 方案：整个生成/检测逻辑放在 PowerShell 5.1 脚本字符串里执行（WScript.Shell
// COM + [Environment]::GetFolderPath('Desktop')），Rust 只负责拼参数、跑子进程、
// 判定成败——不引入新的 Windows API 依赖，PS5.1 原生可用。
//
// kind = "desktop"     -> 桌面 EasyRDP.lnk
// kind = "quicklaunch" -> %APPDATA%\Microsoft\Internet Explorer\Quick Launch\
//                         User Pinned\TaskBar\EasyRDP.lnk（目录缺失则创建）
// 重复生成 = 覆盖（CreateShortcut 对已存在的 .lnk 即为覆盖语义）。

use serde::Serialize;
use tauri::AppHandle;

/// create_shortcut 的返回（前端设置页直接展示）。
#[derive(Debug, Serialize)]
pub struct ShortcutResult {
    pub ok: bool,
    pub message: String,
    pub exists: bool,
}

/// 校验 kind 合法性（防注入：kind 只允许白名单值，路径均由本端生成）。
fn validate_kind(kind: &str) -> Result<(), String> {
    match kind {
        "desktop" | "quicklaunch" => Ok(()),
        _ => Err(format!("未知快捷方式类型: {kind}")),
    }
}

/// 单引号转义（PowerShell 单引号字符串内 '' = 字面单引号）。
fn ps_quote(s: &str) -> String {
    s.replace('\'', "''")
}

/// 运行一段 PowerShell 并返回（exit_ok, stdout）。
fn run_powershell(script: &str) -> Result<(bool, String), String> {
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW：GUI 进程下避免闪出控制台窗口
        .output()
        .map_err(|e| format!("启动 PowerShell 失败: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let ok = output.status.success() && !stdout.is_empty();
    if !ok && !stderr.is_empty() {
        return Ok((false, stderr));
    }
    Ok((ok, stdout))
}

/// 生成快捷方式（命令薄封装；R-014 刷新逻辑复用同一实现）。
#[tauri::command]
pub fn create_shortcut(kind: String) -> Result<ShortcutResult, String> {
    validate_kind(&kind)?;
    generate_shortcut(&kind)
}

/// 按 kind 生成/覆盖 .lnk（exe 取 current_exe，目标目录与文件名在 PS 内推导）。
fn generate_shortcut(kind: &str) -> Result<ShortcutResult, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("获取程序路径失败: {e}"))?
        .to_string_lossy()
        .to_string();
    generate_shortcut_for(kind, &exe)
}

/// 按 kind + 显式 exe 路径生成/覆盖 .lnk（R-20261118-006：升级流程把快捷方式
/// 改指新版本文件名时使用——旧进程的 current_exe 指向已改名路径，不可用）。
fn generate_shortcut_for(kind: &str, exe: &str) -> Result<ShortcutResult, String> {
    // dir_expr：在 PS 内求值的目标目录表达式；desktop 用 Known Folder API，
    // quicklaunch 固定 TaskBar 目录（缺失则创建）。
    let dir_expr = match kind {
        "desktop" => "[Environment]::GetFolderPath('Desktop')".to_string(),
        _ => "Join-Path $env:APPDATA 'Microsoft\\Internet Explorer\\Quick Launch\\User Pinned\\TaskBar'".to_string(),
    };
    let script = format!(
        r#"$ErrorActionPreference='Stop'
$exe='{exe}'
$dir={dir_expr}
if(!(Test-Path -LiteralPath $dir)){{ New-Item -ItemType Directory -Path $dir -Force | Out-Null }}
$lnk=Join-Path $dir 'EasyRDP.lnk'
$ws=New-Object -ComObject WScript.Shell
$s=$ws.CreateShortcut($lnk)
$s.TargetPath=$exe
$s.WorkingDirectory=Split-Path -Parent $exe
$s.IconLocation=($exe+',0')
$s.Save()
Write-Output 'SHORTCUT_OK'"#,
        exe = ps_quote(&exe),
        dir_expr = dir_expr,
    );
    let (ok, out) = run_powershell(&script)?;
    let exists = shortcut_exists_ps(kind)?;
    if ok && out.contains("SHORTCUT_OK") && exists {
        Ok(ShortcutResult {
            ok: true,
            message: match kind {
                "desktop" => "已生成桌面快捷方式".into(),
                _ => "已生成快速启动栏快捷方式".into(),
            },
            exists: true,
        })
    } else {
        let msg = if out.is_empty() { "PowerShell 执行失败".into() } else { out };
        Ok(ShortcutResult { ok: false, message: msg, exists })
    }
}

/// 检测目标 .lnk 是否已存在（前端用于「已生成（重新生成）」按钮文案）。
#[tauri::command]
pub fn check_shortcut(kind: String) -> Result<bool, String> {
    validate_kind(&kind)?;
    shortcut_exists_ps(&kind)
}

fn shortcut_exists_ps(kind: &str) -> Result<bool, String> {
    let dir_expr = match kind {
        "desktop" => "[Environment]::GetFolderPath('Desktop')".to_string(),
        _ => "Join-Path $env:APPDATA 'Microsoft\\Internet Explorer\\Quick Launch\\User Pinned\\TaskBar'".to_string(),
    };
    let script = format!(
        r#"$dir={dir}
$lnk=Join-Path $dir 'EasyRDP.lnk'
if(Test-Path -LiteralPath $lnk){{ Write-Output 'EXISTS' }}else{{ Write-Output 'MISSING' }}"#,
        dir = dir_expr,
    );
    let (ok, out) = run_powershell(&script)?;
    if !ok {
        return Err(out);
    }
    Ok(out.contains("EXISTS"))
}

/* ---------------- R-20261116-014 升级后快捷方式自动刷新 ---------------- */

/// refresh_shortcuts 命令的返回：refreshed = 目标过期被重写的类别，skipped = 无需重写。
#[derive(Debug, Serialize)]
pub struct RefreshResult {
    pub refreshed: Vec<String>,
    pub skipped: Vec<String>,
}

/// kind -> PS 目标目录表达式（desktop 用 Known Folder API，其余固定 TaskBar 目录）。
fn dir_expr_for(kind: &str) -> String {
    match kind {
        "desktop" => "[Environment]::GetFolderPath('Desktop')".to_string(),
        _ => "Join-Path $env:APPDATA 'Microsoft\\Internet Explorer\\Quick Launch\\User Pinned\\TaskBar'".to_string(),
    }
}

/// 读取现存 .lnk 的 TargetPath；.lnk 不存在返回 Ok(None)。
fn read_shortcut_target_ps(kind: &str) -> Result<Option<String>, String> {
    let script = format!(
        r#"$dir={dir}
$lnk=Join-Path $dir 'EasyRDP.lnk'
if(Test-Path -LiteralPath $lnk){{
  $s=(New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
  Write-Output ('TARGET='+$s.TargetPath)
}}else{{ Write-Output 'MISSING' }}"#,
        dir = dir_expr_for(kind),
    );
    let (ok, out) = run_powershell(&script)?;
    if !ok {
        return Err(out);
    }
    if out.contains("MISSING") {
        return Ok(None);
    }
    for line in out.lines() {
        if let Some(target) = line.strip_prefix("TARGET=") {
            return Ok(Some(target.trim().to_string()));
        }
    }
    Err(out)
}

/// .lnk 目标与当前 exe 是否不一致（大小写不敏感；取不到 exe 时视为不旧，避免误写）。
fn shortcut_stale(target: &str) -> bool {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return false,
    };
    target.trim().to_lowercase() != exe.to_string_lossy().trim().to_lowercase()
}

/// 全量刷新两类快捷方式：目标过期才重写（复用 generate_shortcut 的覆盖语义）。
fn refresh_all() -> Result<RefreshResult, String> {
    let mut r = RefreshResult { refreshed: vec![], skipped: vec![] };
    for kind in ["desktop", "quicklaunch"] {
        match read_shortcut_target_ps(kind) {
            Ok(Some(target)) if shortcut_stale(&target) => {
                match generate_shortcut(kind) {
                    Ok(sr) if sr.ok => r.refreshed.push(kind.to_string()),
                    _ => r.skipped.push(kind.to_string()), // 重写失败按跳过计（静默策略）
                }
            }
            _ => r.skipped.push(kind.to_string()), // .lnk 不存在 / 读取失败 / 目标一致
        }
    }
    Ok(r)
}

/// Tauri 命令：手动刷新快捷方式（返回刷新与跳过明细）。
#[tauri::command]
pub fn refresh_shortcuts() -> Result<RefreshResult, String> {
    refresh_all()
}

/// 供升级流程调用：读 settings.autoRefreshShortcuts，关则跳过。
/// 返回 (refreshed, skipped)。阻塞执行（含 PowerShell 子进程），调用方负责放入线程。
pub fn refresh_shortcuts_if_stale(app: &AppHandle) -> Result<(Vec<String>, Vec<String>), String> {
    if !crate::settings::get_bool_setting(app, "autoRefreshShortcuts") {
        return Ok((vec![], vec![]));
    }
    let r = refresh_all()?;
    Ok((r.refreshed, r.skipped))
}

/// R-20261118-006：升级落位后把桌面/快速启动栏快捷方式强制改指新版本文件。
/// 与 refresh_shortcuts_if_stale 不同——本函数接收显式新 exe 路径（升级进程的
/// current_exe 指向已改名旧路径，不可用）；.lnk 缺失时同样创建；阻塞执行。
pub fn refresh_shortcuts_to(exe: &std::path::Path) -> Result<(Vec<String>, Vec<String>), String> {
    let exe_s = exe.to_string_lossy().to_string();
    let mut refreshed = vec![];
    let mut skipped = vec![];
    for kind in ["desktop", "quicklaunch"] {
        let needs = match read_shortcut_target_ps(kind) {
            Ok(Some(target)) => target.trim().to_lowercase() != exe_s.trim().to_lowercase(),
            Ok(None) => true, // .lnk 缺失 → 创建
            Err(_) => true,   // 读取失败 → 直接覆盖写最稳
        };
        if needs {
            match generate_shortcut_for(kind, &exe_s) {
                Ok(sr) if sr.ok => refreshed.push(kind.to_string()),
                _ => skipped.push(kind.to_string()),
            }
        } else {
            skipped.push(kind.to_string());
        }
    }
    Ok((refreshed, skipped))
}

/// 升级成功路径调用：分离线程执行刷新，失败静默（不阻塞升级/退出流程）。
pub fn refresh_shortcuts_after_update(app: &AppHandle) {
    let a = app.clone();
    std::thread::spawn(move || {
        let _ = refresh_shortcuts_if_stale(&a);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_whitelist_rejects_unknown() {
        assert!(validate_kind("desktop").is_ok());
        assert!(validate_kind("quicklaunch").is_ok());
        assert!(validate_kind("startup').Remove-Item").is_err());
    }

    #[test]
    fn ps_quote_escapes_single_quotes() {
        assert_eq!(ps_quote("a'b"), "a''b");
        assert_eq!(ps_quote("plain"), "plain");
    }

    #[test]
    fn stale_check_compares_current_exe_case_insensitively() {
        // 与当前 exe 相同（仅大小写不同）→ 不算过期；不同路径 → 过期
        let exe = std::env::current_exe().unwrap();
        let exe_str = exe.to_string_lossy().to_string();
        assert!(!shortcut_stale(&exe_str.to_uppercase()));
        assert!(shortcut_stale("C:\\somewhere-else\\EasyRDP.exe"));
    }

    #[test]
    fn dir_expr_for_known_kinds() {
        assert!(dir_expr_for("desktop").contains("GetFolderPath"));
        assert!(dir_expr_for("quicklaunch").contains("TaskBar"));
    }
}
