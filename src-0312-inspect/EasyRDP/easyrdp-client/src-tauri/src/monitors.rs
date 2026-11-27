//! F3-V1 R-20261118-010 多显示器优化：显示器枚举与窗口定位几何。
//!
//! `list_monitors` command（前端「显示」下拉的显示器选择分组）与
//! session_start 的全屏窗口定位 rect 共用同一份 EnumDisplayMonitors
//! 枚举结果，保证：
//! - 前端「显示器 N」序号 == 窗口定位目标（EnumDisplayMonitors 枚举顺序）；
//! - easyrdp-rdp `fullscreen_resolution`（FreeRDP DesktopWidth/Height）
//!   与窗口 rect 取同一显示器几何（两侧均为 0 起枚举序）。
//!
//! 仅 Windows；枚举失败（无显示器等异常环境）返回空列表，前端隐藏分组。

#![cfg(windows)]

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

/// 一台显示器的描述（前端下拉/填充按钮数据源）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// 枚举序号（0 起）＝ Profile.monitor 存储值。
    pub id: u32,
    /// 显示器设备名（`\\.\DISPLAY1`；友好名需 DisplayConfig，α 版不取）。
    pub name: String,
    /// 是否主显示器（MONITORINFO.dwFlags & MONITOR_PRIMARY）。
    pub primary: bool,
    /// 工作区外接矩形 rcMonitor（虚拟屏坐标系，支持负值）。
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// 虚拟屏整体矩形（跨所有显示器窗口定位用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VirtualScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// list_monitors 返回视图。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorsView {
    pub monitors: Vec<MonitorInfo>,
    pub virtual_screen: VirtualScreenRect,
}

/// 枚举全部显示器（EnumDisplayMonitors；顺序 = Windows 枚举顺序，
/// 与 easyrdp-rdp fullscreen_resolution 的序号语义一致）。
pub fn enumerate_monitors() -> Vec<MonitorInfo> {
    struct Ctx {
        monitors: Vec<MonitorInfo>,
    }
    // windows-sys 0.61：MONITORENUMPROC 签名 = (HMONITOR, HDC, *mut RECT, LPARAM)
    unsafe extern "system" fn on_monitor(
        hmon: HMONITOR,
        _hdc: HDC,
        _clip: *mut RECT,
        lparam: LPARAM,
    ) -> i32 {
        // windows-sys 0.61：LPARAM 为 isize 类型别名
        let ctx = &mut *(lparam as isize as *mut Ctx);
        let mut info: MONITORINFOEXW = core::mem::zeroed();
        info.monitorInfo.cbSize = core::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmon, &mut info as *mut MONITORINFOEXW as *mut _) != 0 {
            // szDevice 为 UTF-16 定长数组（CCHDEVICENAME=32），截断到 NUL
            let name_len = info
                .szDevice
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(info.szDevice.len());
            let name = String::from_utf16_lossy(&info.szDevice[..name_len]);
            let rc = info.monitorInfo.rcMonitor;
            ctx.monitors.push(MonitorInfo {
                id: ctx.monitors.len() as u32,
                name,
                primary: info.monitorInfo.dwFlags & 1 != 0, // MONITOR_PRIMARY
                x: rc.left,
                y: rc.top,
                width: rc.right - rc.left,
                height: rc.bottom - rc.top,
            });
        }
        1 // 继续枚举
    }
    let mut ctx = Ctx { monitors: Vec::new() };
    // SAFETY：gdi32 标准枚举；回调仅读 MONITORINFOEXW 并 push，无副作用。
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(on_monitor),
            &mut ctx as *mut Ctx as isize,
        );
    }
    ctx.monitors
}

/// 虚拟屏整体（SM_XVIRTUALSCREEN/SM_YVIRTUALSCREEN + 尺寸）。
pub fn virtual_screen() -> VirtualScreenRect {
    // SAFETY：user32 标准查询。
    unsafe {
        VirtualScreenRect {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// 全屏窗口定位 rect：
/// - `span_all=true` -> 虚拟屏整体；
/// - `Some(i)` -> 第 i 台显示器 rcMonitor（越界/枚举失败回退 None = 旧居中路径）；
/// - `None` -> None（主显示器走既有逻辑）。
pub fn fullscreen_window_rect(monitor: Option<u32>, span_all: bool) -> Option<RECT> {
    if span_all {
        let vs = virtual_screen();
        if vs.width > 0 && vs.height > 0 {
            return Some(RECT {
                left: vs.x,
                top: vs.y,
                right: vs.x + vs.width,
                bottom: vs.y + vs.height,
            });
        }
        return None;
    }
    let idx = monitor?;
    enumerate_monitors()
        .into_iter()
        .find(|m| m.id == idx)
        .filter(|m| m.width > 0 && m.height > 0)
        .map(|m| RECT {
            left: m.x,
            top: m.y,
            right: m.x + m.width,
            bottom: m.y + m.height,
        })
}

/// `list_monitors` Tauri command：前端「显示」下拉的显示器选择分组数据源。
#[tauri::command]
pub fn list_monitors() -> MonitorsView {
    MonitorsView {
        monitors: enumerate_monitors(),
        virtual_screen: virtual_screen(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// rect 逐字段比较辅助（RECT 无 PartialEq/Debug derive）。
    fn rect_tuple(rc: RECT) -> (i32, i32, i32, i32) {
        (rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top)
    }

    /// 数据结构可序列化（前端 invoke 返回通道），且缺省显示器列表至少
    /// 包含主显示器（CI/开发机至少一台显示器；无头环境除外则跳过）。
    #[test]
    fn enumerate_returns_primary_and_serde_roundtrip() {
        let view = MonitorsView {
            monitors: enumerate_monitors(),
            virtual_screen: virtual_screen(),
        };
        let json = serde_json::to_string(&view).expect("MonitorsView 可序列化");
        let back: MonitorsView = serde_json::from_str(&json).unwrap();
        assert_eq!(back, view);
        if view.monitors.is_empty() {
            return; // 无头环境
        }
        assert!(
            view.monitors.iter().any(|m| m.primary),
            "至少一台主显示器"
        );
        // 序号连续（前端下拉按 id 寻址）
        for (i, m) in view.monitors.iter().enumerate() {
            assert_eq!(m.id, i as u32);
            assert!(m.width > 0 && m.height > 0);
        }
        // 虚拟屏包含所有显示器（跨屏窗口 rect 覆盖性）
        assert!(view.virtual_screen.width >= view.monitors.iter().map(|m| m.width).max().unwrap());
    }

    /// 定位 rect：None → None（旧居中路径）；越界序号 → None（回退）；
    /// 主显示器（id 0 存在时）→ 正 rect。
    #[test]
    fn fullscreen_window_rect_selection_semantics() {
        assert_eq!(fullscreen_window_rect(None, false).map(rect_tuple), None);
        let monitors = enumerate_monitors();
        assert_eq!(
            fullscreen_window_rect(Some(monitors.len() as u32 + 9), false).map(rect_tuple),
            None,
            "越界序号必须回退而不是取主显示器"
        );
        if let Some(m0) = monitors.first() {
            let rc = fullscreen_window_rect(Some(m0.id), false).expect("主显示器 rect");
            assert_eq!(
                rect_tuple(rc),
                (m0.x, m0.y, m0.width, m0.height),
                "定位 rect = 该显示器 rcMonitor"
            );
        }
        // 跨屏：rect = 虚拟屏整体
        let vs = virtual_screen();
        if vs.width > 0 && vs.height > 0 {
            let rc = fullscreen_window_rect(None, true).expect("跨屏 rect");
            assert_eq!(
                rect_tuple(rc),
                (vs.x, vs.y, vs.width, vs.height),
                "跨屏 rect = 虚拟屏"
            );
        }
    }
}
