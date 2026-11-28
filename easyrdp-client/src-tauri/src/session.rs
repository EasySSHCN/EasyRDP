//! 远程会话桥：连接管理 <-> easyrdp-rdp 引擎 <-> 前端画面视图。
//!
//! 职责：
//! - [`session_start`]：按 profile 创建 stub/native 会话；native 成功 connect 后
//!   起事件泵线程把 [`FramebufferEvent`] 经 **二进制 Channel** 转给前端
//!   （2026-11 性能优化：帧不经 base64/JSON 字符串）；stub（引擎未编译）
//!   进入**演示帧模式**：本地生成 1280x720 测试图案，走同一条管线供前端联调。
//! - [`session_input`]：前端输入事件 -> [`RdpSession::send_input`]
//!   （键盘在桥内完成 W3C code -> Windows VK 映射，见 [`vk_from_code`]）。
//! - [`session_stop`]：停止事件泵、断开会话。
//! - [`send_win_key`]：工具条按钮程序化发送 Win 键（本地系统截获物理 Win 键）。
//! - [`sync_keyboard_leds`]：本地键盘 LED toggle -> 远端 sync PDU。
//!
//! ## 前后端事件协议（native 接入对照）
//!
//! 后端 -> 前端：
//! - 帧数据：session_start 的 `onFrame` **二进制 Channel**（raw → ArrayBuffer），
//!   协议见 [`build_frame_message`]（kind/width/height/seq/rects/payload）。
//! - `rdp-cursor-<session_id>`：`{ hotspot_x, hotspot_y, width, height, data,
//!   reset }`（远端光标形状，RGBA 位图 base64）。
//! - `rdp-session-<session_id>`：`{ session_id, kind, reason?, x?, y?, text? }`
//!   `kind`: `"pointer"` | `"clipboard"` | `"disconnected"`。
//!
//! 前端 -> 后端（invoke）：
//! - `session_input(session_id, input)`，`input` 按 `type` 区分：
//!   `{ type: "MouseMove", x, y }` / `{ type: "MouseButton", button, down, x, y }`
//!   / `{ type: "Wheel", delta }` / `{ type: "WheelHorizontal", delta }`
//!   / `{ type: "Key", code, down }`。
//!   坐标必须已是**远程逻辑坐标**（前端按画布/远程分辨率比例换算）。
//! - `session_clipboard_set(session_id, text)`：本地 -> 远端剪贴板文本。
//! - `send_win_key(session_id)` / `sync_keyboard_leds(session_id)`。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use tauri::Manager;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// 本地剪贴板载荷（CLIPSYNC + FILECLIP-20261117）：原生窗口监听器读到什么
/// 就发什么——文本走 CF_UNICODETEXT 公告，文件走 CF_HDROP + FileContents 通道。
#[derive(Clone, Debug)]
pub enum ClipPayload {
    Text(String),
    Files(Vec<String>),
}

/// 泵转发延迟抽样计数（1/32 抽样落 debug.log）。
static PUMP_LAT_SAMPLES: AtomicU64 = AtomicU64::new(0);
/// 帧入槽计数（2026-11 传输重构：pull 模式统计）。
static PUMP_PUSH: AtomicU64 = AtomicU64::new(0);
/// 前端拉帧计数。
static PULL_COUNT: AtomicU64 = AtomicU64::new(0);

/// 前端轮询拉帧（2026-11 传输重构核心）：返回帧槽中最新帧二进制消息
///（raw 二进制响应 → 前端 ArrayBuffer；空响应 = 暂无新帧）。
/// invoke 响应走 fetch-based IPC，无 emit/eval 大字符串丢失问题。
#[tauri::command]
pub fn session_pull_frame(
    session_id: String,
    sessions: State<ActiveSessions>,
) -> Result<tauri::ipc::Response, String> {
    PULL_COUNT.fetch_add(1, Ordering::Relaxed);
    let map = sessions.0.lock().unwrap();
    // 排空队列并拼接全部帧（前端按 payload_len 迭代解析；顺序保序）
    let body: Vec<u8> = map
        .get(&session_id)
        .map(|h| {
            let mut q = h.frame_slot.lock().unwrap();
            let mut body = Vec::new();
            for m in q.drain(..) {
                body.extend_from_slice(&m);
            }
            body
        })
        .unwrap_or_default();
    drop(map);
    // 空 = 无新帧：返回 1 字节占位（前端按长度判定）
    let mut out = body;
    if out.is_empty() {
        out.push(0xFF);
    }
    // 采样（1/64）落 debug.log：拉帧诊断（帧条数与字节数）
    let n = PULL_COUNT.load(Ordering::Relaxed);
    if n % 64 == 1 {
        easyrdp_rdp::debug_log(&format!(
            "[pull] session={session_id} frames={} bytes={}",
            out.len(),
            out.len()
        ));
    }
    Ok(tauri::ipc::Response::new(out))
}

/// 前端拉取光标（2026-11：与帧同路走 pull，emit 事件通道不可靠）。
/// 返回 None = 暂无更新（前端保持现状）。
#[tauri::command]
pub fn session_pull_cursor(
    session_id: String,
    sessions: State<ActiveSessions>,
) -> Result<Option<CursorPullView>, String> {
    let map = sessions.0.lock().unwrap();
    let data = map.get(&session_id).and_then(|h| h.cursor_slot.lock().unwrap().take());
    drop(map);
    match data {
        Some(c) => {
            easyrdp_rdp::debug_log(&format!(
                "[pull-cursor] {}x{} reset={} bytes={}",
                c.width,
                c.height,
                c.rgba.is_empty(),
                c.rgba.len()
            ));
            Ok(Some(CursorPullView {
                reset: c.rgba.is_empty(),
                hotspot_x: c.hotspot_x,
                hotspot_y: c.hotspot_y,
                width: c.width,
                height: c.height,
                rgba_base64: base64::engine::general_purpose::STANDARD.encode(&c.rgba),
            }))
        }
        None => Ok(None),
    }
}

/// 光标拉取载荷（rgba 空 = reset）。
#[derive(Serialize)]
pub struct CursorPullView {
    reset: bool,
    hotspot_x: u32,
    hotspot_y: u32,
    width: u32,
    height: u32,
    rgba_base64: String,
}

/// 前端日志直通（2026-11 发射停滞排查）：前端 JS 侧的解析/绘制/心跳数据
/// 写入 debug.log（GUI 无 devtools 控制台可见性，文件是唯一观测通道）。
#[tauri::command]
pub fn frontend_log(session_id: String, line: String) -> Result<(), String> {
    easyrdp_rdp::debug_log(&format!("[frontend:{session_id}] {line}"));
    Ok(())
}

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, State};

use easyrdp_rdp::event_channel::FramebufferEvent;
use easyrdp_rdp::{create_session, native_ready, InputEvent, RdpSession};

use crate::ConnectionStore;

/// 帧发射间隔（仅 demo 演示帧使用）：本地只保留最新帧，每 100ms 发一次。
pub const FRAME_EMIT_INTERVAL: Duration = Duration::from_millis(100);

/// 演示（stub）帧分辨率。
pub const DEMO_WIDTH: u32 = 1280;
pub const DEMO_HEIGHT: u32 = 720;

/// 增量合并窗口：积攒 ≤16ms 的增量包一次性发射（约一帧的节拍），
/// 即时转发不等 100ms —— 消除旧管线的 9FPS 卡顿。
const DELTA_FLUSH_INTERVAL: Duration = Duration::from_millis(16);

/// 增量批大小上限：超过即立即发射（防止长积压放大端到端延迟）。
const DELTA_FLUSH_BYTES: usize = 2 * 1024 * 1024;

// ---------------------------------------------------------------------------
// 前端输入载荷
// ---------------------------------------------------------------------------

/// 前端 `session_input` 的 `input` 参数（serde 按 `type` 标签区分）。
///
/// 2026-11 输入排查根因修复：此前 `rename_all = "camelCase"` 把标签改成
/// "mouseMove"/"key"，而前端实际发送 PascalCase（"MouseMove"/"Key"）——
/// **所有输入 invoke 都在 Tauri 反序列化层被静默拒绝**（命令未执行、无日志），
/// GUI 表现为鼠标键盘完全无效（无头探针直调 send_input 不经过此层，故探针正常）。
/// 现移除 rename_all，标签保持前端契约的 PascalCase，另留 camelCase alias 兼容。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum InputPayload {
    #[serde(alias = "mouseMove")]
    MouseMove { x: u32, y: u32 },
    #[serde(alias = "mouseButton")]
    MouseButton { button: String, down: bool, x: u32, y: u32 },
    #[serde(alias = "wheel")]
    Wheel { delta: i32 },
    /// 横向滚轮（Shift+滚轮/触摸板横扫），delta 正值向右。
    #[serde(alias = "wheelHorizontal")]
    WheelHorizontal { delta: i32 },
    #[serde(alias = "key")]
    Key { code: String, down: bool },
}

impl From<&InputPayload> for InputEvent {
    fn from(p: &InputPayload) -> Self {
        match p {
            InputPayload::MouseMove { x, y } => InputEvent::MouseMove { x: *x, y: *y },
            InputPayload::MouseButton { button, down, x, y } => InputEvent::MouseButton {
                button: button.clone(),
                down: *down,
                x: *x,
                y: *y,
            },
            InputPayload::Wheel { delta } => InputEvent::Wheel { delta: *delta },
            InputPayload::WheelHorizontal { delta } => {
                InputEvent::WheelHorizontal { delta: *delta }
            }
            InputPayload::Key { code, down } => InputEvent::Key {
                code: code.clone(),
                down: *down,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// 纯函数：坐标换算 / VK 映射（单测覆盖）
// ---------------------------------------------------------------------------

/// 把画布内光标坐标换算成远程桌面逻辑坐标（letterbox 居中留边也一并处理）。
///
/// `cursor_x/cursor_y`：画布内像素坐标（可能落在留边内，会被钳制到 [0, 远端尺寸)）。
/// `canvas_w/canvas_h`：画布 CSS 尺寸；`remote_w/remote_h`：远程桌面分辨率。
/// 返回 `(rx, ry)`。
pub fn map_point(
    cursor_x: f64,
    cursor_y: f64,
    canvas_w: f64,
    canvas_h: f64,
    remote_w: u32,
    remote_h: u32,
) -> (u32, u32) {
    if canvas_w <= 0.0 || canvas_h <= 0.0 || remote_w == 0 || remote_h == 0 {
        return (0, 0);
    }
    // 等比缩放 + 居中（letterbox）：scale 取 min，两轴方向留边。
    let scale = (canvas_w / remote_w as f64).min(canvas_h / remote_h as f64);
    let draw_w = remote_w as f64 * scale;
    let draw_h = remote_h as f64 * scale;
    let off_x = (canvas_w - draw_w) / 2.0;
    let off_y = (canvas_h - draw_h) / 2.0;
    let rx = ((cursor_x - off_x) / scale).floor();
    let ry = ((cursor_y - off_y) / scale).floor();
    let rx = rx.clamp(0.0, remote_w as f64 - 1.0) as u32;
    let ry = ry.clamp(0.0, remote_h as f64 - 1.0) as u32;
    (rx, ry)
}

/// W3C `KeyboardEvent.code` -> Windows 虚拟键码（VK_*）。
///
/// 2026-11 补全为全量表：修饰键/编辑键/锁键/Numpad/标点/打印键。
/// 未识别的 code 返回 None（上层忽略该键）。
pub fn vk_from_code(code: &str) -> Option<u32> {
    // 字母 A-Z：VK 0x41..0x5B 与 "KeyA".."KeyZ" 线性对应
    if let Some(rest) = code.strip_prefix("Key") {
        if rest.len() == 1 {
            let c = rest.as_bytes()[0];
            if c.is_ascii_uppercase() {
                return Some(c as u32);
            }
        }
        return None;
    }
    // 数字 0-9：VK 0x30..0x3A 与 "Digit0".."Digit9" 线性对应
    if let Some(rest) = code.strip_prefix("Digit") {
        if rest.len() == 1 && rest.as_bytes()[0].is_ascii_digit() {
            return Some(rest.as_bytes()[0] as u32);
        }
        return None;
    }
    // F1-F24：VK 0x70-0x87
    if let Some(rest) = code.strip_prefix('F') {
        if let Ok(n) = rest.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some(0x70 + n - 1);
            }
        }
        return None;
    }
    // 数字键盘（2026-11 补全）：Numpad0-9=0x60-0x69
    if let Some(rest) = code.strip_prefix("Numpad") {
        if rest == "Enter" {
            return Some(0x0D); // VK_RETURN（2026-11 补：NumpadEnter）
        }
        if rest.len() == 1 && rest.as_bytes()[0].is_ascii_digit() {
            return Some(0x60 + (rest.as_bytes()[0] - b'0') as u32);
        }
        return Some(match rest {
            "Multiply" => 0x6A,  // VK_MULTIPLY
            "Add" => 0x6B,       // VK_ADD
            "Separator" => 0x6C, // VK_SEPARATOR
            "Subtract" => 0x6D,  // VK_SUBTRACT
            "Decimal" => 0x6E,   // VK_DECIMAL
            "Divide" => 0x6F,    // VK_DIVIDE
            _ => return None,
        });
    }
    Some(match code {
        "Enter" | "NumpadEnter" => 0x0D,    // VK_RETURN
        "Escape" => 0x1B,                   // VK_ESCAPE
        "Space" => 0x20,                    // VK_SPACE
        "Backspace" => 0x08,                // VK_BACK
        "Tab" => 0x09,                      // VK_TAB
        "CapsLock" => 0x14,                 // VK_CAPITAL
        "ShiftLeft" | "ShiftRight" => 0x10, // VK_SHIFT
        "ControlLeft" | "ControlRight" => 0x11, // VK_CONTROL
        "AltLeft" | "AltRight" => 0x12,     // VK_MENU
        "MetaLeft" => 0x5B,                 // VK_LWIN（2026-11 补全）
        "MetaRight" => 0x5C,                // VK_RWIN
        "ContextMenu" => 0x5D,              // VK_APPS
        "ArrowUp" => 0x26,                  // VK_UP
        "ArrowDown" => 0x28,                // VK_DOWN
        "ArrowLeft" => 0x25,                // VK_LEFT
        "ArrowRight" => 0x27,               // VK_RIGHT
        "Delete" => 0x2E,                   // VK_DELETE
        "Insert" => 0x2D,                   // VK_INSERT
        "Home" => 0x24,                     // VK_HOME
        "End" => 0x23,                      // VK_END
        "PageUp" => 0x21,                   // VK_PRIOR
        "PageDown" => 0x22,                 // VK_NEXT
        "PrintScreen" => 0x2C,              // VK_SNAPSHOT（2026-11 补全）
        "ScrollLock" => 0x91,               // VK_SCROLL（2026-11 补全）
        "Pause" => 0x13,                    // VK_PAUSE（2026-11 补全）
        "NumLock" => 0x90,                  // VK_NUMLOCK（2026-11 补全）
        "Backquote" => 0xC0,                // VK_OEM_3
        "Minus" => 0xBD,                    // VK_OEM_MINUS
        "Equal" => 0xBB,                    // VK_OEM_PLUS
        "BracketLeft" => 0xDB,              // VK_OEM_4
        "BracketRight" => 0xDD,             // VK_OEM_6
        "Backslash" => 0xDC,                // VK_OEM_5
        "Semicolon" => 0xBA,                // VK_OEM_1
        "Quote" => 0xDE,                    // VK_OEM_7
        "Comma" => 0xBC,                    // VK_OEM_COMMA
        "Period" => 0xBE,                   // VK_OEM_PERIOD
        "Slash" => 0xBF,                    // VK_OEM_2
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// 键盘 LED 同步（2026-11：Numpad 数字依赖服务端 NumLock 与客户端一致）
// ---------------------------------------------------------------------------

/// 本地 LED toggle 状态 -> RDP sync flags 纯函数（`<freerdp/input.h>`）：
/// KBD_SYNC_NUM_LOCK=0x1 / KBD_SYNC_CAPS_LOCK=0x2 / KBD_SYNC_SCROLL_LOCK=0x4 /
/// KBD_SYNC_KANA=0x8。
pub fn build_led_flags(num_lock: bool, caps_lock: bool, scroll_lock: bool, kana: bool) -> u32 {
    let mut f = 0u32;
    if num_lock {
        f |= 0x1;
    }
    if caps_lock {
        f |= 0x2;
    }
    if scroll_lock {
        f |= 0x4;
    }
    if kana {
        f |= 0x8;
    }
    f
}

// ---------------------------------------------------------------------------
// 帧节流（demo 演示帧用）：本地积攒最新帧，定时发射
// ---------------------------------------------------------------------------

/// 最新帧槽：多帧并发到达时只保留最新一帧（旧帧立即被覆盖丢弃）。
#[derive(Default)]
pub struct LatestFrameSlot {
    latest: Option<FramebufferEvent>,
}

impl LatestFrameSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// 放入一帧：若槽里已有帧则覆盖（只留最新）。
    pub fn offer(&mut self, ev: FramebufferEvent) {
        self.latest = Some(ev);
    }

    /// 取走当前帧（若有）并清空槽。
    pub fn take(&mut self) -> Option<FramebufferEvent> {
        self.latest.take()
    }

    pub fn is_empty(&self) -> bool {
        self.latest.is_none()
    }
}

/// 是否到达发射时机（纯函数，便于单测）。
pub fn should_emit(last_emit: Option<Instant>, now: Instant, interval: Duration) -> bool {
    match last_emit {
        None => true,
        Some(t) => now.duration_since(t) >= interval,
    }
}

/// 生成 32bpp BGRA、行序 top-down 的测试图案：静态渐变底 + 随 tick 移动的色带，
/// 保证每帧内容不同（前端可见帧率变化）。
pub fn test_pattern_bgra(width: u32, height: u32, tick: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity((width * height * 4) as usize);
    let band = (tick % (height.max(1))) as i64;
    for y in 0..height {
        for x in 0..width {
            let base_r = (x * 255 / width.max(1)) as u8;
            let base_g = (y * 255 / height.max(1)) as u8;
            let in_band = ((y as i64 - band).abs() < 2) as u8;
            let b = 60 + in_band * 195;
            buf.extend_from_slice(&[b, base_g, base_r, 0xFF]);
        }
    }
    buf
}

/// BGRA(top-down) -> base64(32bpp BMP)。BMP 默认行序 bottom-up，这里翻转。
/// 返回错误表示像素长度与尺寸不符。
///
/// 2026-11 主管线已切换 JPEG；本函数保留供帧诊断/对照实验。
#[allow(dead_code)]
pub fn bgra_to_bmp_base64(width: u32, height: u32, bgra_top_down: &[u8]) -> Result<String, String> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| "尺寸溢出".to_string())?;
    if bgra_top_down.len() != expected {
        return Err(format!(
            "像素长度不符: 期望 {expected} 字节，实际 {}",
            bgra_top_down.len()
        ));
    }
    let stride = width as usize * 4;
    let mut bmp = Vec::with_capacity(54 + expected);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((54u32 + expected as u32)).to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(width as i32).to_le_bytes());
    bmp.extend_from_slice(&(height as i32).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&(expected as u32).to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    for row in (0..height as usize).rev() {
        bmp.extend_from_slice(&bgra_top_down[row * stride..(row + 1) * stride]);
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(bmp))
}

// ---------------------------------------------------------------------------
// 会话表与 Tauri command
// ---------------------------------------------------------------------------

/// 一个活跃会话：引擎对象 + 停止信号 + 事件泵线程句柄。
pub struct SessionHandle {
    session: Mutex<Box<dyn RdpSession>>,
    stop: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
    /// 演示模式（stub）标记：仅用于状态展示。
    pub demo: bool,
    /// 帧槽（2026-11 传输重构）：pump 写入最新帧二进制消息，前端经
    /// `session_pull_frame` **invoke 轮询拉取**（raw 二进制响应）。
    /// 不再用 app.emit/Channel 传帧——大 payload 高频 emit 在部分 webview
    /// 静默丢失（真机实证：后端 fallback 计数持续增长而前端 idle 44s）。
    /// invoke 响应走 fetch-based IPC，raw 二进制可靠。
    pub frame_slot: Arc<Mutex<VecDeque<Vec<u8>>>>,
    /// 光标槽（2026-11：与帧同路走 pull，emit 事件通道不可靠）。
    /// None = 无更新；Some(reset=true) = 恢复系统默认光标。
    pub cursor_slot: Arc<Mutex<Option<CursorSlotData>>>,
}

/// 光标槽载荷（pump 写入、session_pull_cursor 读出）。
#[derive(Debug, Clone, Serialize)]
pub struct CursorSlotData {
    pub hotspot_x: u32,
    pub hotspot_y: u32,
    pub width: u32,
    pub height: u32,
    /// RGBA（width*height*4）；空 = 恢复系统默认光标（reset）。
    pub rgba: Vec<u8>,
}

impl SessionHandle {
    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
        let _ = self.session.lock().unwrap().disconnect();
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 全局活跃会话表。
#[derive(Default)]
pub struct ActiveSessions(pub Mutex<HashMap<String, SessionHandle>>);

/// 脏矩形载荷（delta 帧二进制协议内联字段）。
#[derive(Serialize, Clone)]
struct RectPayload {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// 远端光标事件载荷（`rdp-cursor-<sid>`）：`data` 为 base64 RGBA
/// （width*height*4）；`reset=true` 表示恢复默认光标。
#[derive(Serialize, Clone)]
struct CursorPayload {
    hotspot_x: u32,
    hotspot_y: u32,
    width: u32,
    height: u32,
    data: String,
    reset: bool,
}

/// 非 帧 事件载荷（`rdp-session-<sid>`）。
#[derive(Serialize, Clone)]
struct SessionEventPayload {
    session_id: String,
    /// "pointer" | "clipboard" | "disconnected"
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    x: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    y: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

/// 帧二进制协议（全部 LE，前端 parseBinaryFrame 对照）：
/// ```text
/// u8  kind    0=full(JPEG) 1=delta(BGRA 打包)
/// u32 width / u32 height / u32 seq / u16 rect_count
/// rects: rect_count × (u32 x, u32 y, u32 w, u32 h)   （full 时 rect_count=0）
/// bytes: kind=0 → JPEG 字节；kind=1 → 各矩形 BGRA 按序打包
/// ```
/// 帧二进制协议（全部 LE，前端 parseBinaryFrames 对照）：
/// ```text
/// u8  kind    0=full(JPEG) 1=delta(BGRA 打包)
/// u32 width / u32 height / u32 seq / u16 rect_count
/// u32 payload_len
/// rects: rect_count × (u32 x, u32 y, u32 w, u32 h)   （full 时 rect_count=0）
/// bytes: payload_len 字节（kind=0 → JPEG；kind=1 → 各矩形 BGRA 按序打包）
/// ```
/// 响应体可含**多条连续帧**（pull 排空队列时拼接），前端按 payload_len 迭代。
pub fn build_frame_message(
    kind: u8,
    seq: u64,
    width: u32,
    height: u32,
    rects: &[RectPayload],
    payload: &[u8],
) -> Vec<u8> {
    let mut m = Vec::with_capacity(1 + 12 + 4 + 2 + rects.len() * 16 + payload.len());
    m.push(kind);
    m.extend_from_slice(&width.to_le_bytes());
    m.extend_from_slice(&height.to_le_bytes());
    m.extend_from_slice(&(seq as u32).to_le_bytes());
    m.extend_from_slice(&(rects.len() as u16).to_le_bytes());
    m.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    for r in rects {
        m.extend_from_slice(&r.x.to_le_bytes());
        m.extend_from_slice(&r.y.to_le_bytes());
        m.extend_from_slice(&r.w.to_le_bytes());
        m.extend_from_slice(&r.h.to_le_bytes());
    }
    m.extend_from_slice(payload);
    m
}

/// 待发射的增量批：矩形与打包像素同序累积。
struct DeltaBatch {
    width: u32,
    height: u32,
    rects: Vec<easyrdp_rdp::RectUpdate>,
    pixels: Vec<u8>,
    first_at: Instant,
}

impl DeltaBatch {
    fn push(&mut self, rects: &[easyrdp_rdp::RectUpdate], pixels: &[u8]) {
        self.rects.extend_from_slice(rects);
        self.pixels.extend_from_slice(pixels);
    }

    fn due(&self) -> bool {
        self.first_at.elapsed() >= DELTA_FLUSH_INTERVAL || self.pixels.len() >= DELTA_FLUSH_BYTES
    }
}

/// F4-V1：一次性网关 OTP 码槽。UI 收到「otp_required」错误后弹码，经
/// `set_gateway_otp` 写入；下一次 session_start 取走并清空，注入该次
/// profile.gateway.otp_code（登录 /api/auth/login 时作为 mfa_code 提交）。
pub static PENDING_GATEWAY_OTP: Mutex<Option<String>> = Mutex::new(None);

/// 记录用户输入的网关 OTP 验证码（一次性，取走即清空）。
#[tauri::command]
pub fn set_gateway_otp(code: String) -> Result<(), String> {
    *PENDING_GATEWAY_OTP.lock().unwrap() = Some(code);
    Ok(())
}

// ---------------------------------------------------------------------------
// 端口敲门（P0-P2）执行层
// ---------------------------------------------------------------------------

/// 连接重试去重表：profile_id → 上次敲门完成时刻。
/// 窗口内（dedup_secs，sequence=seqTimeout+10 / spa=fwTimeout）重试连接
/// 不再重复敲门；条目惰性过期清理。
static KNOCK_DEDUP: std::sync::OnceLock<Mutex<std::collections::HashMap<String, (Instant, u64)>>> =
    std::sync::OnceLock::new();

fn knock_dedup() -> &'static Mutex<std::collections::HashMap<String, (Instant, u64)>> {
    KNOCK_DEDUP.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// P0 pre_connect_command 钩子：非空则执行本地命令。
///
/// Shell 语义（明确文档化，不做任何字符串拼接）：
/// - Windows：`cmd.exe /S /C "<命令>"`（与用户在 cmd 中手输等价）；
/// - 非 Windows：`sh -c "<命令>"`。
/// 命令内容由用户配置决定，本函数不插入/不解释任何动态数据（防注入约束：
/// 导入路径同样原样存储）。超时 15s；退出码 0 = 成功，否则中止连接。
fn run_pre_connect_command(cmd: &str) -> Result<(), String> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return Ok(());
    }
    easyrdp_rdp::debug_log(&format!("[pre-connect] 执行: {cmd}"));
    #[cfg(windows)]
    let mut child = {
        // R-20261126-007：连接前命令钩子 spawn cmd.exe 时不分配新控制台
        //（CREATE_NO_WINDOW），否则每次连接闪现黑色控制台窗口。
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("cmd.exe");
        c.args(["/S", "/C", cmd])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        c.spawn()
            .map_err(|e| format!("命令启动失败（{cmd}）: {e}"))?
    };
    #[cfg(not(windows))]
    let mut child = std::process::Command::new("sh")
        .args(["-c", cmd])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("命令启动失败（{cmd}）: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match child.try_wait().map_err(|e| format!("命令等待失败: {e}"))? {
            Some(status) => {
                let code = status.code().unwrap_or(-1);
                if code == 0 {
                    easyrdp_rdp::debug_log("[pre-connect] 命令成功（exit 0）");
                    return Ok(());
                }
                return Err(format!("命令「{cmd}」退出码 {code}（须为 0）"));
            }
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return Err(format!("命令「{cmd}」超时（>15s，已终止）"));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// P1/P2 敲门执行（含连接重试去重）：窗口内已敲过则直接放行。
fn run_knock_once(
    profile_id: &str,
    knock: &easyrdp_core::knock::KnockConfig,
    host: &str,
    rdp_port: &u16,
) -> Result<easyrdp_core::knock::KnockOutcome, easyrdp_core::knock::KnockError> {
    {
        let mut dedup = knock_dedup().lock().unwrap();
        dedup.retain(|_, (at, secs)| at.elapsed().as_secs() < *secs);
        if let Some((at, _)) = dedup.get(profile_id) {
            easyrdp_rdp::debug_log(&format!(
                "[knock] session={profile_id} 去重窗口内（{}s 前已敲门），跳过重复敲门",
                at.elapsed().as_secs()
            ));
            return Ok(easyrdp_core::knock::KnockOutcome { at: *at, dedup_secs: 0 });
        }
    }
    // 轮询开门端口：敲门配置 openPorts 第一段（默认 tcp/3389）；与 RDP 端口一致。
    let mut cfg = knock.clone();
    if knock.knock_type == easyrdp_core::knock::KnockType::Sequence && knock.open_ports.trim().is_empty() {
        cfg.open_ports = format!("tcp/{rdp_port}");
    }
    let outcome = easyrdp_core::knock::perform_knock(host, &cfg)?;
    let mut dedup = knock_dedup().lock().unwrap();
    dedup.insert(profile_id.to_string(), (outcome.at, outcome.dedup_secs.max(30)));
    Ok(outcome)
}

/// 建立会话：native 成功 connect 后泵引擎事件（帧经二进制 Channel 下发）；
/// stub 进入演示帧模式。
#[tauri::command]
pub fn session_start(    profile_id: String,    password: String,
    // 前端 `new Channel()` 序列化为 channel id 字符串；用 JavaScriptChannelId
    // （可 Deserialize）+ channel_on 还原为 Channel。旧 api 无 Channel 构造器
    // 时前端不传该参数（None）→ 帧回退 base64 listen 事件。
    store: State<ConnectionStore>,
    sessions: State<ActiveSessions>,
    app: AppHandle,
) -> Result<SessionStartView, String> {

    let (mut profile, stored_password) = store
        .find(&profile_id)
        .ok_or_else(|| format!("连接不存在: {profile_id}"))?;
    // F4-V1：注入一次性网关 OTP（重试链路：UI 弹码 -> set_gateway_otp -> 重连）
    if let Some(gw) = profile.gateway.as_mut() {
        if let Some(code) = PENDING_GATEWAY_OTP.lock().unwrap().take() {
            gw.otp_code = Some(code);
        }
    }
    let plain_password = if !password.is_empty() {
        password
    } else if stored_password.is_empty() {
        String::new()
    } else {
        easyrdp_core::secret::unprotect(&stored_password).map_err(|e| format!("口令解密失败: {e}"))?
    };

    // 防重复：同一连接已活跃则先停旧的（含原生窗口销毁——防僵尸窗口堆叠：
    // 旧 TOPMOST 窗口残留会盖住/抢占新窗口的输入 = "输入无效"实锤候选）
    if sessions.0.lock().unwrap().contains_key(&profile_id) {
        #[cfg(windows)]
        crate::native_view::detach(&profile_id);
        stop_session_handle(&profile_id, &sessions);
    }

    let session_id = profile_id.clone();

    // ---------------- 端口敲门（P0-P2，RDP 连接前接入点）----------------
    // 顺序：preConnectCommand 钩子 → 敲门（sequence/SPA）→ session.connect。
    // 任一环节失败即中止连接（含机器可读码 knock_invalid/knock_timeout）。
    run_pre_connect_command(&profile.pre_connect_command)
        .map_err(|e| format!("preConnectCommand 失败: {e}"))?;
    if profile.knock.enabled {
        if let Err(e) = run_knock_once(&profile_id, &profile.knock, &profile.host, &profile.port) {
            easyrdp_rdp::debug_log(&format!(
                "[knock] session={profile_id} 失败 code={} err={e}",
                e.code()
            ));
            return Err(format!("敲门未通过（{}）: {e}", e.code()));
        }
        easyrdp_rdp::debug_log(&format!(
            "[knock] session={profile_id} 敲门完成且 tcp/{} 可达（开门窗口 {}s）",
            profile.port, profile.knock.fw_timeout
        ));
        // 连接日志（前端 frontend_log 同一 debug 通道）加敲门结果行
        easyrdp_rdp::debug_log(&format!(
            "[frontend:{profile_id}] 敲门结果: ok type={} ports={}",
            match profile.knock.knock_type {
                easyrdp_core::knock::KnockType::Spa => "spa".to_string(),
                easyrdp_core::knock::KnockType::Sequence => "sequence".to_string(),
            },
            if profile.knock.knock_type == easyrdp_core::knock::KnockType::Sequence {
                format!("{:?}", profile.knock.sequence)
            } else {
                profile.knock.open_ports.clone()
            }
        ));
    }

    let mut session = create_session();
    session.set_password(&plain_password);
    let mut demo = false;
    if let Err(e) = session.connect(&profile) {
        if e.0 != easyrdp_rdp::stub::NOT_COMPILED {
            return Err(format!("连接失败: {}", e.0));
        }
        demo = true;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let frame_slot = Arc::new(Mutex::new(VecDeque::new()));
    let cursor_slot = Arc::new(Mutex::new(None));
    // 原生渲染路径（2026-11 架构修正：mstsc 同款独立顶层窗口，Windows+非 demo）：
    // 窗口 WndProc 输入 → 通道 → 转发线程 → freerdp input；
    // pump 全帧就绪 → StretchDIBits 直绘；WM_CLOSE → 停会话回连接管理器。
    //
    // **死锁修复（fix20）**：session_start 在 Tauri 主线程运行——此前内部
    // run_on_main_thread + 阻塞等 AtomicBool = 主线程等自己执行闭包 = 全应用
    // 冻结。修复：**非阻塞**——spawn 独立线程执行 attach（该线程调
    // run_on_main_thread 安全，调用线程非主线程）；attach 结果经事件回前端，
    // session_start 立即返回（native_view 为乐观标志，前端等 open 事件再切 UI）。
    let (input_tx, input_rx) = mpsc::channel::<InputEvent>();
    let (close_tx, close_rx) = mpsc::channel::<String>();
    // CLIPSYNC-20261115 + FILECLIP-20261117：本地剪贴板变化 → 原生窗口
    // WM_CLIPBOARDUPDATE → clip_tx → 本转发线程分发（文本 send_clipboard_text /
    // 文件 send_clipboard_files 公告）。
    let (clip_tx, clip_rx) = mpsc::channel::<ClipPayload>();
    let app_for_thread = app.clone();
    let mut native_view_attached = false;
    #[cfg(windows)]
    {
        if !demo {
            native_view_attached = true; // 乐观标志：attach 异步执行，结果经事件回前端
            let app2 = app_for_thread.clone();
            let app3 = app_for_thread.clone();
            let sid = session_id.clone();
            let sid2 = session_id.clone();
            // F3-V1 R-20261118-010 多显示器优化：fullscreen 模式远程桌面
            // 尺寸取目标显示器/虚拟屏实际几何（与 ffi.rs settings 同源函数）；
            // 窗口定位 rect 同显示器枚举（fullscreen_window_rect），缺省
            // （None）走既有居中路径，行为不回归。
            let is_fullscreen =
                profile.resolution == easyrdp_core::profile::Resolution::Fullscreen;
            let (w0, h0) = if is_fullscreen {
                easyrdp_rdp::fullscreen_resolution(profile.monitor, profile.multi_monitor)
            } else {
                profile.resolution.dims()
            };
            let fs_rect = if is_fullscreen {
                crate::monitors::fullscreen_window_rect(profile.monitor, profile.multi_monitor)
            } else {
                None
            };
            easyrdp_rdp::debug_log(&format!(
                "[multimon] session={sid} monitor={:?} span={} rect={} remote={w0}x{h0}",
                profile.monitor,
                profile.multi_monitor,
                fs_rect
                    .map(|rc| format!(
                        "({},{})-({}x{})",
                        rc.left,
                        rc.top,
                        rc.right - rc.left,
                        rc.bottom - rc.top
                    ))
                    .unwrap_or_else(|| "none".into())
            ));
            std::thread::spawn(move || {
                // 该线程非主线程 → run_on_main_thread 无死锁
                let ok = crate::native_view::attach_for_session(
                    &app2, &sid, w0, h0, fs_rect, input_tx, close_tx, clip_tx,
                );
                if !ok {
                    let _ = app2.emit(&format!("rdp-native-view-failed-{sid}"), ());
                    easyrdp_rdp::debug_log("[native-view] attach 失败（事件已通知前端）");
                    return;
                }
                let _ = app2.emit(&format!("rdp-native-view-open-{sid}"), ());
                // **R-20261116-007 多会话并行**：主界面（Tauri 窗口）不再隐藏
                // ——多会话窗口并存时连接中心保持可见（用户可随时双击追加
                // 新连接）；原生窗口 attach 时已 TOPMOST + SetForegroundWindow
                // + SetFocus（抢前台），输入仍由原生窗口承载。
                easyrdp_rdp::debug_log("[native-view] 主界面保持可见（R-007 多会话并行：不再 hide）");
                // **hide 后输入修复（fix30 会话实证根因）**：webview 持有鼠标
                // 捕获 → 所有鼠标消息被路由到 webview 窗口——主线程把捕获
                // 抢占回原生窗口 + 前台/焦点。
                crate::native_view::recapture_to_native(&app2, &sid);
                // 输入转发线程（通道关闭自动退出）：转发计数每 100 直记（不依赖定时器）
                let app2b = app2.clone();
                let sid_b = sid.clone();
                std::thread::spawn(move || {
                    let sessions = app2b.state::<ActiveSessions>();
                    let mut fwd: u64 = 0;
                    for ev in input_rx {
                        fwd += 1;
                        if fwd % 100 == 1 {
                            easyrdp_rdp::debug_log(&format!(
                                "[native-view] input 5s: 转发到引擎={fwd}"
                            ));
                        }
                        if let Some(h) = sessions.0.lock().unwrap().get_mut(&sid_b) {
                            let _ = h.session.lock().unwrap().send_input(&ev);
                        }
                    }
                });
                // CLIPSYNC 转发线程：本地剪贴板载荷 → 引擎公告（文本/文件）
                let app2c = app2.clone();
                let sid_c = sid.clone();
                std::thread::spawn(move || {
                    let sessions = app2c.state::<ActiveSessions>();
                    for payload in clip_rx {
                        match payload {
                            ClipPayload::Files(files) => {
                                easyrdp_rdp::debug_log(&format!(
                                    "[cliprdr] 转发本地文件 {} 个 —— send_clipboard_files",
                                    files.len()
                                ));
                                if let Some(h) = sessions.0.lock().unwrap().get_mut(&sid_c) {
                                    if let Err(e) = h
                                        .session
                                        .lock()
                                        .unwrap()
                                        .send_clipboard_files(&files)
                                    {
                                        easyrdp_rdp::debug_log(&format!(
                                            "[cliprdr] send_clipboard_files 失败: {}",
                                            e.0
                                        ));
                                    }
                                }
                            }
                            ClipPayload::Text(text) => {
                                easyrdp_rdp::debug_log(&format!(
                                    "[cliprdr] 转发本地文本 chars={} —— send_clipboard_text",
                                    text.chars().count()
                                ));
                                if let Some(h) = sessions.0.lock().unwrap().get_mut(&sid_c) {
                                    if let Err(e) =
                                        h.session.lock().unwrap().send_clipboard_text(&text)
                                    {
                                        easyrdp_rdp::debug_log(&format!(
                                            "[cliprdr] send_clipboard_text 失败: {}",
                                            e.0
                                        ));
                                    }
                                }
                            }
                        }
                    }
                });
                // 关闭转发线程（WM_CLOSE → 停会话 → 回写主界面列表状态）。
                // R-20261116-007：主界面不再被 hide，单会话关闭只停对应会话、
                // 其余会话窗口不受影响；disconnected 事件照常回前端刷新列表。
                let sessions = app3.state::<ActiveSessions>();
                for mut sid in close_rx {
                    if sid.is_empty() {
                        sid = sid2.clone();
                    }
                    easyrdp_rdp::debug_log(&format!(
                        "[native-view] 收到原生窗口关闭 sid={sid}——停会话（R-007：仅关对应窗口）"
                    ));
                    crate::native_view::detach(&sid);
                    if let Some(mut h) = sessions.0.lock().unwrap().remove(&sid) {
                        h.shutdown(); // 与 session_stop 同逻辑：Drop 兜底停引擎
                    }
                    let _ = app3.emit(
                        &format!("rdp-session-{sid}"),
                        SessionEventPayload {
                            session_id: sid.clone(),
                            kind: "disconnected".into(),
                            x: None,
                            y: None,
                            reason: Some("window-closed".into()),
                            text: None,
                        },
                    );
                }
            });
        }
    }
    let pump = spawn_event_pump(
        app,
        session_id.clone(),
        &session,
        demo,
        Arc::clone(&stop),
        profile.resolution.dims(),
        Arc::clone(&frame_slot),
        Arc::clone(&cursor_slot),
    );
    // F1-V2：快照线程停止信号（与 SessionHandle.stop 同源克隆）
    let snapshot_stop = Arc::clone(&stop);

    let handle = SessionHandle {
        session: Mutex::new(session),
        stop,
        pump: Some(pump),
        demo,
        frame_slot,
        cursor_slot,
    };
    sessions.0.lock().unwrap().insert(session_id.clone(), handle);

    // F1-V2 R-20261118-013 会话快照预览：独立捕获线程（仅 Windows 原生会话；
    // demo 无原生窗口不捕获）。stop 与会话同源——session_stop/shutdown 置位
    // 后线程 1s 内自行退出。隐私开关 autoSnapshotPreview 线程内每轮读取。
    #[cfg(windows)]
    if !demo {
        crate::snapshot::spawn_capture(app_for_thread.clone(), session_id.clone(), snapshot_stop);
    }

    // 画面初始尺寸：native 会话的 FitWindow 用主显示器实际分辨率
    let (w, h) = if !demo && profile.resolution == easyrdp_core::profile::Resolution::FitWindow {
        easyrdp_rdp::fit_window_resolution()
    } else {
        profile.resolution.dims()
    };
    Ok(SessionStartView {
        session_id,
        width: w,
        height: h,
        demo,
        native_ready: native_ready(),
        native_view: native_view_attached,
        resolution_kind: match profile.resolution {
            easyrdp_core::profile::Resolution::Custom { .. } => "custom",
            easyrdp_core::profile::Resolution::Fullscreen => "fullscreen",
            easyrdp_core::profile::Resolution::FitWindow => "fit_window",
        },
    })
}

/// 会话启动结果视图。
#[derive(Serialize)]
pub struct SessionStartView {
    pub session_id: String,
    pub width: u32,
    pub height: u32,
    /// true = stub 演示帧（非真实远程画面）。
    pub demo: bool,
    pub native_ready: bool,
    /// 分辨率策略（"fit_window"|"fullscreen"|"custom"）：前端据此决定
    /// 是否把窗口调整为协商尺寸（custom → 窗口自适应 1:1 显示）。
    pub resolution_kind: &'static str,
    /// true = 原生 Win32 渲染（StretchDIBits 直绘 + 原生输入直通）：
    /// 前端隐藏 webview 画布、停止帧/光标轮询。
    pub native_view: bool,
}

/// 输入转发完整路径（`session_input` command 与单测共用——GUI 无头验证点）：
/// InputPayload -> InputEvent（键盘 W3C code -> VK 映射）-> `send_input`。
/// 每次转发写入 %APPDATA%\EasyRDP\debug.log（GUI 无 stderr，文件是唯一观测通道）。
pub(crate) fn forward_input(
    handle: &mut SessionHandle,
    input: &InputPayload,
) -> Result<(), String> {
    let raw_code = input_key_code(input);
    // "VK:0xNN" 直通（内部调用如 Win 按钮已用 VK 格式，语义确定）；
    // W3C e.code 走 vk_from_code 映射。映射失败（None）时：
    // Key 载荷静默忽略（前端未识别按键），非 Key 载荷原样透传。
    let event: InputEvent = if raw_code.starts_with("VK:") {
        let down = matches!(input, InputPayload::Key { down: true, .. });
        InputEvent::Key { code: raw_code.clone(), down }
    } else {
        match (&input, vk_from_code(&raw_code)) {
            (InputPayload::Key { down, .. }, Some(vk)) => InputEvent::Key {
                code: format!("VK:{vk:#06X}"),
                down: *down,
            },
            (InputPayload::Key { .. }, None) => return Ok(()), // 未识别按键：忽略
            _ => input.into(),
        }
    };
    easyrdp_rdp::debug_log(&format!("[gui-input] {event:?}"));
    handle
        .session
        .lock()
        .unwrap()
        .send_input(&event)
        .map_err(|e| {
            easyrdp_rdp::debug_log(&format!("[gui-input] 发送失败: {}", e.0));
            e.0
        })
}

/// 会话输入 command（2026-11 输入排查加固）：`input` 接收原始 JSON 再自行
/// 反序列化——此前由 Tauri 层反序列化失败时 invoke 在命令执行**之前**被拒绝
/// 且无任何日志。现在每个失败点都落盘 %APPDATA%\EasyRDP\debug.log。
#[tauri::command]
pub fn session_input(
    session_id: String,
    input: serde_json::Value,
    sessions: State<ActiveSessions>,
) -> Result<(), String> {
    let parsed = serde_json::from_value::<InputPayload>(input.clone());
    let input = match parsed {
        Ok(p) => p,
        Err(e) => {
            easyrdp_rdp::debug_log(&format!(
                "[gui-input] 载荷反序列化失败: {e}; raw={input}"
            ));
            return Err(format!("input 载荷非法: {e}"));
        }
    };
    let mut map = sessions.0.lock().unwrap();
    let Some(handle) = map.get_mut(&session_id) else {
        easyrdp_rdp::debug_log(&format!(
            "[gui-input] 会话不存在: '{session_id}'（当前活跃会话 {} 个）",
            map.len()
        ));
        return Err(format!("会话不存在或已结束: {session_id}"));
    };
    let result = forward_input(handle, &input);
    drop(map);
    result
}

fn input_key_code(input: &InputPayload) -> String {
    match input {
        InputPayload::Key { code, .. } => code.clone(),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// 键盘 LED 同步 + Win 键（工具条按钮）
// ---------------------------------------------------------------------------

/// 发送 Win 键（本地系统会截获物理 Win 键，webview 收不到 keydown；
/// 此 command 由工具条「⊞ Win」按钮触发：MetaLeft down → 100ms → up）。
#[tauri::command]
pub fn send_win_key(session_id: String, sessions: State<ActiveSessions>) -> Result<(), String> {
    // 锁持锁跨 100ms 间隔：该操作低频，简化生命周期
    let mut map = sessions.0.lock().unwrap();
    let handle = map
        .get_mut(&session_id)
        .ok_or_else(|| format!("会话不存在或已结束: {session_id}"))?;
    forward_input(handle, &InputPayload::Key { code: "VK:0x5B".into(), down: true })?;
    // 按住 150ms 再释放（模拟真实按键时长；过短的服务端可能不识别为 Win）
    std::thread::sleep(Duration::from_millis(150));
    forward_input(handle, &InputPayload::Key { code: "VK:0x5B".into(), down: false })
}

/// 键盘 LED 同步：读本地 NumLock/CapsLock/ScrollLock toggle → RDP sync flags
/// → 引擎 SynchronizeEvent。前端在 NumLock/CapsLock keydown 时调用。
#[tauri::command]
pub fn sync_keyboard_leds(
    session_id: String,
    sessions: State<ActiveSessions>,
) -> Result<(), String> {
    #[cfg(windows)]
    let (num, caps, scroll) = local_led_toggles();
    #[cfg(not(windows))]
    let (num, caps, scroll) = (false, false, false);
    let flags = build_led_flags(num, caps, scroll, false);
    let mut map = sessions.0.lock().unwrap();
    let handle = map
        .get_mut(&session_id)
        .ok_or_else(|| format!("会话不存在或已结束: {session_id}"))?;
    // 先绑定结果再返回：MutexGuard 临时值必须先于 map 释放（借用期）
    let result = handle
        .session
        .lock()
        .unwrap()
        .sync_keyboard_leds(flags)
        .map_err(|e| e.0);
    result
}

/// 读本地 LED toggle（Windows GetKeyState 低位 = toggle 状态）。
#[cfg(windows)]
fn local_led_toggles() -> (bool, bool, bool) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, VK_CAPITAL, VK_NUMLOCK, VK_SCROLL,
    };
    // SAFETY：user32 标准查询，无副作用（不切换键盘状态）。
    unsafe {
        (
            GetKeyState(VK_NUMLOCK as i32) & 1 != 0,
            GetKeyState(VK_CAPITAL as i32) & 1 != 0,
            GetKeyState(VK_SCROLL as i32) & 1 != 0,
        )
    }
}

/// 断开并移除会话。
#[tauri::command]
pub fn session_stop(
    session_id: String,
    sessions: State<ActiveSessions>,
    app: AppHandle,
) -> Result<(), String> {
    // 原生视图先销毁（Windows；否则 webview pull 停止后画面冻结在子窗口上）
    #[cfg(windows)]
    crate::native_view::detach(&session_id);
    stop_session_handle(&session_id, &sessions);
    // 恢复 Tauri 窗口（native_view 模式会话期曾隐藏让位原生窗口）
    #[cfg(windows)]
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
    Ok(())
}

/// 当前活跃会话 id 列表（R-20261116-007：主界面活跃会话数与快捷断开入口）。
#[tauri::command]
pub fn session_active_ids(sessions: State<ActiveSessions>) -> Vec<String> {
    sessions.0.lock().unwrap().keys().cloned().collect()
}

/// 本地 -> 远端剪贴板文本同步（α：CF_UNICODETEXT）。
#[tauri::command]
pub fn session_clipboard_set(
    session_id: String,
    text: String,
    sessions: State<ActiveSessions>,
) -> Result<(), String> {
    let map = sessions.0.lock().unwrap();
    let handle = map
        .get(&session_id)
        .ok_or_else(|| format!("会话不存在或已结束: {session_id}"))?;
    let result = handle
        .session
        .lock()
        .unwrap()
        .send_clipboard_text(&text)
        .map_err(|e| e.0);
    drop(map);
    result
}

fn stop_session_handle(session_id: &str, sessions: &State<ActiveSessions>) {
    if let Some(mut handle) = sessions.0.lock().unwrap().remove(session_id) {
        handle.shutdown(); // Drop 兜底
    }
}

// ---------------------------------------------------------------------------
// 事件泵线程
// ---------------------------------------------------------------------------

/// 启动事件泵线程：native 泵引擎 channel——增量包 16ms 内合并即时入槽，
/// 全帧（JPEG，首帧/兜底）即时入槽；demo 本地生成测试图案后走 JPEG 编码 +
/// 100ms 节流。帧统一写入帧槽（前端 session_pull_frame 轮询拉取，2026-11
/// 传输重构：emit/Channel 大 payload 在部分 webview 静默丢失，invoke 拉取
/// 走 fetch-based IPC raw 二进制可靠）。
fn spawn_event_pump(
    app: AppHandle,
    session_id: String,
    session: &Box<dyn RdpSession>,
    demo: bool,
    stop: Arc<AtomicBool>,
    dims: (u32, u32),
    frame_slot: Arc<Mutex<VecDeque<Vec<u8>>>>,
    cursor_slot: Arc<Mutex<Option<CursorSlotData>>>,
    // 原生视图输入通道（Windows 原生渲染模式）：WndProc 鼠标/键盘 →
    // pump 转发 freerdp input（不经过 webview）。
) -> JoinHandle<()> {
    let receiver = session.events().map(|r| r.clone_receiver());
    let (width, height) = dims;
    let mut tick: u32 = 0;
    std::thread::spawn(move || {
        let mut slot = LatestFrameSlot::new();
        let mut last_emit: Option<Instant> = None;
        let mut disconnected: Option<String> = None;
        let mut delta_batch: Option<DeltaBatch> = None;
        let mut seq: u64 = 0;
        let mut push_count: u64 = 0;
        let mut push_delta: u64 = 0;
        let mut push_summary_at = std::time::Instant::now();

        // 入槽辅助：组帧后覆盖帧槽（前端拉走即清）
        let mut push = |kind: u8, w: u32, h: u32, rects: &[RectPayload], payload: &[u8]| {
            seq += 1;
            if kind == 1 {
                push_delta += 1;
            } else {
                push_count += 1;
            }
            let msg = build_frame_message(kind, seq, w, h, rects, payload);
            PUMP_PUSH.fetch_add(1, Ordering::Relaxed);
            let mut q = frame_slot.lock().unwrap();
            if q.len() >= 8 { q.pop_front(); }
            q.push_back(msg);
            // 5s 汇总（发射停滞排查：引擎→槽是否持续产出；full/delta 分列）
            if push_summary_at.elapsed() >= Duration::from_secs(5) {
                easyrdp_rdp::debug_log(&format!(
                    "[frame-stats] pump 5s: full={} delta={}",
                    push_count, push_delta
                ));
                push_count = 0;
                push_delta = 0;
                push_summary_at = std::time::Instant::now();
            }
        };
        loop {
            if stop.load(Ordering::SeqCst) {
                return;
            }

            // 消息队列探针（native 模式）：pump 每 2s PostMessage WM_USER+1，
            // WndProc 回执日志证明消息派发链存活（输入零事件定位）
            #[cfg(windows)]
            crate::native_view::probe_tick(&session_id);
            // -- 取事件（native：drain 全部待处理事件，避免积压延迟）--
            if let Some(rx) = &receiver {
                loop {
                    let Some(ev) = rx.try_recv() else { break };
                    match ev {
                        // 全帧（首帧/兜底）：先冲刷积压增量保持时序，再入槽。
                        // （pixels 为 JPEG 编码字节——仅 webview 槽路径使用；
                        //   原生直绘走 NativeFrame 的 raw BGRA，见下）
                        FramebufferEvent::Framebuffer { width: w, height: h, pixels, .. } => {
                            if let Some(b) = delta_batch.take() {
                                let rects: Vec<RectPayload> = b
                                    .rects
                                    .iter()
                                    .map(|r| RectPayload { x: r.x, y: r.y, w: r.w, h: r.h })
                                    .collect();
                                push(1, b.width, b.height, &rects, &b.pixels);
                            }
                            push(0, w, h, &[], &pixels);
                        }
                        // 原生直绘帧：raw BGRA（非 JPEG）→ StretchDIBits 直绘子窗口
                        FramebufferEvent::NativeFrame { width: w, height: h, pixels } => {
                            // PRESENT-AUDIT：引擎全帧到达时间轴（与呈现层对齐）
                            if crate::native_view::audit_on_pub() {
                                let hsh = crate::native_view::audit_hash(&pixels);
                                crate::native_view::audit_engine_event(
                                    "nativeframe", w, h, hsh, 0, pixels.len(),
                                );
                            }
                            #[cfg(windows)]
                            {
                                // GHOSTFIX-R020/021/022（2026-11）：**先冲刷 pending
                                // Delta 批再上全帧**。根因：Delta 进 16ms 合并批期间
                                // NativeFrame（更新内容的全帧快照）先 present 上屏，
                                // 随后到期 flush 的旧 Delta 批经 present_delta 把旧
                                // 像素写回 FRAME_STORE 并重绘脏矩形 → 旧画面闪回
                                //（载入文字残影/拖动边缘残影/按钮旧态闪现同源）。
                                // 先应用旧 Delta（顺序语义）再全帧覆盖，恢复严格
                                // 事件序；webview 槽同步入队保持两条路径时序一致。
                                if let Some(b) = delta_batch.take() {
                                    let rects: Vec<RectPayload> = b
                                        .rects
                                        .iter()
                                        .map(|r| RectPayload { x: r.x, y: r.y, w: r.w, h: r.h })
                                        .collect();
                                    push(1, b.width, b.height, &rects, &b.pixels);
                                    crate::native_view::present_delta(
                                        &session_id, &b.rects, &b.pixels, b.width, b.height,
                                    );
                                }
                                crate::native_view::present(&session_id, &pixels, w, h);
                            }
                            #[cfg(not(windows))]
                            {
                                let _ = (w, h, pixels);
                            }
                        }
                        // 增量：16ms 内合并批入槽（不再 100ms 节流）。
                        FramebufferEvent::Delta { width, height, rects, pixels } => {
                            // PRESENT-AUDIT：引擎增量到达时间轴（合并前逐包记录）
                            if crate::native_view::audit_on_pub() {
                                let hsh = crate::native_view::audit_hash(&pixels);
                                crate::native_view::audit_engine_event(
                                    "delta-pkt", width, height, hsh, rects.len(), pixels.len(),
                                );
                            }
                            let batch = delta_batch.get_or_insert(DeltaBatch {
                                width,
                                height,
                                rects: Vec::new(),
                                pixels: Vec::new(),
                                first_at: Instant::now(),
                            });
                            batch.push(&rects, &pixels);
                            if batch.due() {
                                let b = delta_batch.take().expect("batch 刚插入");
                                let rects: Vec<RectPayload> = b
                                    .rects
                                    .iter()
                                    .map(|r| RectPayload { x: r.x, y: r.y, w: r.w, h: r.h })
                                    .collect();
                                push(1, b.width, b.height, &rects, &b.pixels);
                                // DELTA-20261115 传输优化：原生窗口直绘增量
                                //（此前原生模式丢弃 Delta，order 更新只能等
                                // 100ms 全帧节流——拖拽/输入延迟被钳制）
                                #[cfg(windows)]
                                crate::native_view::present_delta(
                                    &session_id, &b.rects, &b.pixels, b.width, b.height,
                                );
                            }
                        }
                        FramebufferEvent::Pointer { x, y } => {
                            let _ = app.emit(
                                &format!("rdp-session-{session_id}"),
                                SessionEventPayload {
                                    session_id: session_id.clone(),
                                    kind: "pointer".into(),
                                    x: Some(x),
                                    y: Some(y),
                                    reason: None,
                                    text: None,
                                },
                            );
                        }
                        // 远端光标形状（引擎已把 xor/and 掩码转成 RGBA）：
                        // 写光标槽（前端 session_pull_cursor 轮询拉取，
                        // 2026-11 修复：emit 事件通道与大 payload 帧同类不可靠）
                        FramebufferEvent::Cursor { hotspot_x, hotspot_y, width, height, rgba } => {
                            *cursor_slot.lock().unwrap() = Some(CursorSlotData {
                                hotspot_x,
                                hotspot_y,
                                width,
                                height,
                                rgba: rgba.clone(),
                            });
                        }
                        // 远端 -> 本地文本：引擎已写本地系统剪贴板；通知前端展示。
                        FramebufferEvent::ClipboardText(text) => {
                            let _ = app.emit(
                                &format!("rdp-session-{session_id}"),
                                SessionEventPayload {
                                    session_id: session_id.clone(),
                                    kind: "clipboard".into(),
                                    x: None,
                                    y: None,
                                    reason: None,
                                    text: Some(text),
                                },
                            );
                        }
                        FramebufferEvent::Disconnected(reason) => {
                            disconnected = Some(reason);
                        }
                    }
                }
            } else if demo {
                slot.offer(FramebufferEvent::Framebuffer {
                    x: 0,
                    y: 0,
                    width,
                    height,
                    pixels: test_pattern_bgra(width, height, tick),
                });
                tick = tick.wrapping_add(1);
            } else {
                return; // 无事件源也无演示：直接退出
            }
            // -- demo 定时发射最新帧（100ms 节流，JPEG 编码同一管线）--
            if demo {
                let now = Instant::now();
                if should_emit(last_emit, now, FRAME_EMIT_INTERVAL) {
                    if let Some(FramebufferEvent::Framebuffer { width: w, height: h, pixels, .. }) =
                        slot.take()
                    {
                        if let Ok(bytes) = easyrdp_rdp::frame_jpeg::encode_frame_jpeg(w, h, &pixels)
                        {
                            push(0, w, h, &[], &bytes);
                        }
                        last_emit = Some(now);
                    }
                }
            }
            // -- 断开处理：把最后一帧发完再退出 --
            if let Some(reason) = disconnected {
                // R-20261121-001 远端重启/服务端断开黑屏修复：原生窗口不会自己
                // 关闭（WM_CLOSE 只由用户点击触发）——远端重启后引擎断链，窗口
                // 残留最后一帧（远端黑屏画面）。这里主动 detach：PostMessage
                // WM_CLOSE → WndProc 自毁窗口 → notify_close 回 close_rx 停会话。
                // detach 对无原生窗口会话（webview 路径）为 no-op。
                #[cfg(windows)]
                {
                    if crate::native_view::has_view(&session_id) {
                        easyrdp_rdp::debug_log(&format!(
                            "[native-view] 引擎断链（{reason}）——自动关闭原生窗口（R-20261121-001）"
                        ));
                        crate::native_view::detach(&session_id);
                    }
                }
                let _ = app.emit(
                    &format!("rdp-session-{session_id}"),
                    SessionEventPayload {
                        session_id: session_id.clone(),
                        kind: "disconnected".into(),
                        x: None,
                        y: None,
                        reason: Some(reason),
                        text: None,
                    },
                );
                return;
            }
            // native：2ms 轮询增量通道；demo 100ms 节流。
            std::thread::sleep(Duration::from_millis(if demo { 100 } else { 2 }));
        }
    })
}

// ---------------------------------------------------------------------------
// 单测（纯函数，不依赖 Tauri App）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use easyrdp_core::profile::Resolution;

    // ---- 坐标换算 ----

    #[test]
    fn map_point_identity_when_canvas_matches_remote() {
        assert_eq!(map_point(100.0, 50.0, 1280.0, 720.0, 1280, 720), (100, 50));
    }

    #[test]
    fn map_point_scales_uniform_down() {
        assert_eq!(map_point(320.0, 160.0, 640.0, 360.0, 1280, 720), (640, 320));
    }

    #[test]
    fn map_point_handles_letterbox_horizontal_bars() {
        assert_eq!(map_point(400.0, 75.0, 800.0, 600.0, 1280, 720), (640, 0));
        assert_eq!(map_point(400.0, 525.0, 800.0, 600.0, 1280, 720), (640, 719));
        assert_eq!(map_point(400.0, 10.0, 800.0, 600.0, 1280, 720), (640, 0));
    }

    #[test]
    fn map_point_clamps_out_of_range_to_bounds() {
        assert_eq!(map_point(-50.0, -1.0, 1280.0, 720.0, 1280, 720), (0, 0));
        assert_eq!(map_point(2000.0, 900.0, 1280.0, 720.0, 1280, 720), (1279, 719));
    }

    #[test]
    fn map_point_degenerate_inputs_return_origin() {
        assert_eq!(map_point(10.0, 10.0, 0.0, 720.0, 1280, 720), (0, 0));
        assert_eq!(map_point(10.0, 10.0, 1280.0, 720.0, 0, 0), (0, 0));
    }

    /// 窗口自适应场景（2026-11）：custom 会话窗口客户区被校准为
    /// 「画面 + chrome」，画布盒 == 远端原始尺寸（无 letterbox）→
    /// 全画布任意点击必须 1:1 无偏移（含四角/中心）。
    #[test]
    fn map_point_no_offset_when_canvas_box_equals_remote() {
        // 中心
        assert_eq!(map_point(960.0, 540.0, 1920.0, 1080.0, 1920, 1080), (960, 540));
        // 四角（含小数像素，floor 后钳制）
        assert_eq!(map_point(0.0, 0.0, 1920.0, 1080.0, 1920, 1080), (0, 0));
        assert_eq!(map_point(1919.5, 1079.5, 1920.0, 1080.0, 1920, 1080), (1919, 1079));
        // 纵向中点（此前「纵向偏上」报告的对照场景）
        assert_eq!(map_point(500.0, 540.0, 1920.0, 1080.0, 1920, 1080), (500, 540));
    }

    // ---- VK 映射 ----

    #[test]
    fn vk_maps_letters_digits_and_common_keys() {
        assert_eq!(vk_from_code("KeyA"), Some(0x41));
        assert_eq!(vk_from_code("KeyZ"), Some(0x5A));
        assert_eq!(vk_from_code("Digit0"), Some(0x30));
        assert_eq!(vk_from_code("Digit9"), Some(0x39));
        assert_eq!(vk_from_code("Enter"), Some(0x0D));
        assert_eq!(vk_from_code("Escape"), Some(0x1B));
        assert_eq!(vk_from_code("Space"), Some(0x20));
        assert_eq!(vk_from_code("Backspace"), Some(0x08));
        assert_eq!(vk_from_code("ShiftLeft"), Some(0x10));
        assert_eq!(vk_from_code("ControlRight"), Some(0x11));
        assert_eq!(vk_from_code("ArrowUp"), Some(0x26));
        assert_eq!(vk_from_code("ArrowLeft"), Some(0x25));
        assert_eq!(vk_from_code("Delete"), Some(0x2E));
        assert_eq!(vk_from_code("F1"), Some(0x70));
        assert_eq!(vk_from_code("F12"), Some(0x7B));
    }

    #[test]
    fn vk_returns_none_for_unknown_codes() {
        assert_eq!(vk_from_code("Key"), None);
        assert_eq!(vk_from_code("MediaPlay"), None);
        assert_eq!(vk_from_code("F25"), None); // 超出 F1-F24 全表
        assert_eq!(vk_from_code(""), None);
        // 2026-11 补：NumpadEnter 显式映射（与主 Enter 同 VK_RETURN）
        assert_eq!(vk_from_code("NumpadEnter"), Some(0x0D));
    }

    /// 2026-11 补全的全量 VK 映射：修饰键/编辑键/锁键/Numpad/标点。
    #[test]
    fn vk_maps_extended_keys_numpad_and_locks() {
        assert_eq!(vk_from_code("ControlLeft"), Some(0x11));
        assert_eq!(vk_from_code("ShiftRight"), Some(0x10));
        assert_eq!(vk_from_code("AltLeft"), Some(0x12));
        assert_eq!(vk_from_code("MetaLeft"), Some(0x5B));
        assert_eq!(vk_from_code("MetaRight"), Some(0x5C));
        assert_eq!(vk_from_code("ContextMenu"), Some(0x5D));
        assert_eq!(vk_from_code("Insert"), Some(0x2D));
        assert_eq!(vk_from_code("PageUp"), Some(0x21));
        assert_eq!(vk_from_code("PageDown"), Some(0x22));
        assert_eq!(vk_from_code("PrintScreen"), Some(0x2C));
        assert_eq!(vk_from_code("Pause"), Some(0x13));
        assert_eq!(vk_from_code("NumLock"), Some(0x90));
        assert_eq!(vk_from_code("ScrollLock"), Some(0x91));
        assert_eq!(vk_from_code("CapsLock"), Some(0x14));
        assert_eq!(vk_from_code("Numpad0"), Some(0x60));
        assert_eq!(vk_from_code("Numpad9"), Some(0x69));
        assert_eq!(vk_from_code("NumpadMultiply"), Some(0x6A));
        assert_eq!(vk_from_code("NumpadAdd"), Some(0x6B));
        assert_eq!(vk_from_code("NumpadSubtract"), Some(0x6D));
        assert_eq!(vk_from_code("NumpadDecimal"), Some(0x6E));
        assert_eq!(vk_from_code("NumpadDivide"), Some(0x6F));
        assert_eq!(vk_from_code("Minus"), Some(0xBD));
        assert_eq!(vk_from_code("Quote"), Some(0xDE));
    }

    // ---- 帧节流 ----

    fn frame(tag: u32) -> FramebufferEvent {
        FramebufferEvent::Framebuffer {
            x: 0,
            y: 0,
            width: tag,
            height: 1,
            pixels: vec![tag as u8],
        }
    }

    #[test]
    fn latest_frame_slot_keeps_only_newest_frame() {
        let mut slot = LatestFrameSlot::new();
        assert!(slot.is_empty());
        slot.offer(frame(1));
        slot.offer(frame(2));
        slot.offer(frame(3));
        match slot.take() {
            Some(FramebufferEvent::Framebuffer { width, .. }) => assert_eq!(width, 3),
            other => panic!("期望 Framebuffer，实际 {other:?}"),
        }
        assert!(slot.is_empty());
        assert!(slot.take().is_none());
    }

    #[test]
    fn should_emit_respects_interval_and_first_call() {
        let now = Instant::now();
        assert!(should_emit(None, now, FRAME_EMIT_INTERVAL));
        let past = now - Duration::from_millis(150);
        assert!(should_emit(Some(past), now, FRAME_EMIT_INTERVAL));
        let recent = now - Duration::from_millis(50);
        assert!(!should_emit(Some(recent), now, FRAME_EMIT_INTERVAL));
    }

    // ---- 测试图案 / BMP 编码 ----

    #[test]
    fn test_pattern_has_exact_bgra_size_and_frames_differ() {
        let f1 = test_pattern_bgra(8, 4, 0);
        let f2 = test_pattern_bgra(8, 4, 1);
        assert_eq!(f1.len(), 8 * 4 * 4);
        assert_ne!(f1, f2);
    }

    #[test]
    fn bgra_to_bmp_header_and_size_are_correct() {
        use base64::Engine as _;
        let px = test_pattern_bgra(4, 2, 0);
        let b64 = bgra_to_bmp_base64(4, 2, &px).unwrap();
        let raw = base64::engine::general_purpose::STANDARD.decode(&b64).unwrap();
        assert_eq!(raw.len(), 54 + 4 * 2 * 4);
        assert_eq!(&raw[0..2], b"BM");
        assert_eq!(u32::from_le_bytes(raw[10..14].try_into().unwrap()), 54);
        assert_eq!(u16::from_le_bytes(raw[28..30].try_into().unwrap()), 32);
        let src_row0 = &px[0..16];
        let bmp_last_row = &raw[54 + 16..54 + 32];
        assert_eq!(src_row0, bmp_last_row);
    }

    #[test]
    fn bgra_to_bmp_rejects_size_mismatch() {
        assert!(bgra_to_bmp_base64(4, 2, &[0u8; 3]).is_err());
    }

    // ---- 输入载荷 -> 引擎事件 ----

    /// 构造带 StubSession 的 SessionHandle（与 GUI 相同的转发结构）。
    fn stub_handle() -> (SessionHandle, Arc<Mutex<Vec<InputEvent>>>) {
        let stub = easyrdp_rdp::stub::StubSession::new();
        let recorder = stub.recorder();
        let handle = SessionHandle {
            session: Mutex::new(Box::new(stub)),
            stop: Arc::new(AtomicBool::new(false)),
            pump: None,
            demo: true,
            frame_slot: Arc::new(Mutex::new(VecDeque::new())),
            cursor_slot: Arc::new(Mutex::new(None)),
        };
        (handle, recorder)
    }

    /// GUI 输入链路无头验证：forward_input（session_input 完整转发逻辑）。
    #[test]
    fn forward_input_delivers_full_gui_path_to_engine() {
        let (mut handle, recorder) = stub_handle();
        forward_input(&mut handle, &InputPayload::MouseMove { x: 100, y: 200 }).unwrap();
        forward_input(
            &mut handle,
            &InputPayload::MouseButton { button: "left".into(), down: true, x: 3, y: 4 },
        )
        .unwrap();
        forward_input(&mut handle, &InputPayload::Wheel { delta: -240 }).unwrap();
        forward_input(&mut handle, &InputPayload::Key { code: "KeyA".into(), down: true }).unwrap();
        forward_input(&mut handle, &InputPayload::Key { code: "Enter".into(), down: false }).unwrap();

        let rec = recorder.lock().unwrap();
        assert_eq!(rec[0], InputEvent::MouseMove { x: 100, y: 200 });
        assert_eq!(
            rec[1],
            InputEvent::MouseButton { button: "left".into(), down: true, x: 3, y: 4 }
        );
        assert_eq!(rec[2], InputEvent::Wheel { delta: -240 });
        assert_eq!(rec[3], InputEvent::Key { code: "VK:0x0041".into(), down: true });
        assert_eq!(rec[4], InputEvent::Key { code: "VK:0x000D".into(), down: false });
        assert_eq!(rec.len(), 5);
    }

    #[test]
    fn forward_input_ignores_unknown_key_codes() {
        let (mut handle, recorder) = stub_handle();
        forward_input(&mut handle, &InputPayload::Key { code: "MediaPlay".into(), down: true })
            .unwrap();
        assert!(recorder.lock().unwrap().is_empty());
    }

    /// Tauri 层原始 JSON（前端 invoke 实际形状）必须能反序列化为 InputPayload
    ///（旧实现由 Tauri 层反序列化，失败时静默拒绝且零日志——回归锚点）。
    #[test]
    fn frontend_json_payloads_deserialize_to_input_payload() {
        use serde_json::json;
        let cases = vec![
            json!({"type": "MouseMove", "x": 100, "y": 200}),
            json!({"type": "MouseButton", "button": "left", "down": true, "x": 3, "y": 4}),
            json!({"type": "Wheel", "delta": -120}),
            json!({"type": "WheelHorizontal", "delta": 60}),
            json!({"type": "Key", "code": "KeyA", "down": true}),
        ];
        for c in cases {
            let p: InputPayload = serde_json::from_value(c.clone())
                .unwrap_or_else(|e| panic!("载荷 {c} 反序列化失败: {e}"));
            let (mut handle, recorder) = stub_handle();
            forward_input(&mut handle, &p).unwrap();
            assert_eq!(recorder.lock().unwrap().len(), 1, "载荷 {c} 应到达引擎");
        }
    }

    #[test]
    fn input_payload_converts_to_engine_event() {
        let p = InputPayload::MouseButton {
            button: "right".into(),
            down: true,
            x: 5,
            y: 6,
        };
        assert_eq!(
            InputEvent::from(&p),
            InputEvent::MouseButton { button: "right".into(), down: true, x: 5, y: 6 }
        );
        let p = InputPayload::Wheel { delta: -120 };
        assert_eq!(InputEvent::from(&p), InputEvent::Wheel { delta: -120 });
        let p = InputPayload::WheelHorizontal { delta: 60 };
        assert_eq!(InputEvent::from(&p), InputEvent::WheelHorizontal { delta: 60 });
    }

    // ---- LED flags ----

    /// LED toggle 组合 -> RDP sync flags 映射（2026-11 Numpad 修复）。
    #[test]
    fn build_led_flags_maps_toggle_combinations() {
        assert_eq!(build_led_flags(false, false, false, false), 0x0);
        assert_eq!(build_led_flags(true, false, false, false), 0x1); // 仅 NumLock
        assert_eq!(build_led_flags(false, true, false, false), 0x2); // 仅 CapsLock
        assert_eq!(build_led_flags(false, false, true, false), 0x4); // 仅 ScrollLock
        assert_eq!(build_led_flags(true, true, false, false), 0x3); // Num+Caps
        assert_eq!(build_led_flags(true, true, true, true), 0xF); // 全开
    }

    // ---- 帧二进制协议 ----

    /// 帧槽语义（pull 队列模式）：写入多条 → pull 排空取全部；cap 8 丢最旧。
    #[test]
    fn frame_slot_put_take_semantics() {
        let slot = Arc::new(Mutex::new(VecDeque::new()));
        assert!(slot.lock().unwrap().is_empty()); // 初始无帧
        let m1 = build_frame_message(0, 1, 640, 480, &[], &[1, 2, 3]);
        let m2 = build_frame_message(1, 2, 100, 50, &[], &[9]);
        slot.lock().unwrap().push_back(m1.clone());
        slot.lock().unwrap().push_back(m2.clone());
        // pull 排空：按序取全部
        let drained: Vec<Vec<u8>> = slot.lock().unwrap().drain(..).collect();
        assert_eq!(drained, vec![m1, m2]);
        assert!(slot.lock().unwrap().is_empty()); // 排空后清空
        // cap 8：第 9 条入队丢最旧
        for i in 0..9u64 {
            let msg = build_frame_message(0, i, 10, 10, &[], &[i as u8]);
            let mut q = slot.lock().unwrap();
            if q.len() >= 8 {
                q.pop_front();
            }
            q.push_back(msg);
        }
        assert_eq!(slot.lock().unwrap().len(), 8);
        // 最旧（seq0）已被挤出；front 是 seq1
        let front = slot.lock().unwrap().front().cloned().unwrap();
        assert_eq!(&front[9..13], &1u32.to_le_bytes());
    }

    /// 二进制帧消息编码：头字段（含 payloadLen）+ rects + payload 逐字节核对。
    #[test]
    fn build_frame_message_layout_is_correct() {
        let rects = vec![RectPayload { x: 1, y: 2, w: 3, h: 4 }];
        let m = build_frame_message(1, 0x01020304, 1920, 1080, &rects, &[0xAA, 0xBB]);
        assert_eq!(m[0], 1); // kind=delta
        assert_eq!(&m[1..5], &1920u32.to_le_bytes());
        assert_eq!(&m[5..9], &1080u32.to_le_bytes());
        assert_eq!(&m[9..13], &0x01020304u32.to_le_bytes());
        assert_eq!(&m[13..15], &1u16.to_le_bytes());
        assert_eq!(&m[15..19], &2u32.to_le_bytes()); // payloadLen
        assert_eq!(&m[19..23], &1u32.to_le_bytes());
        assert_eq!(&m[23..27], &2u32.to_le_bytes());
        assert_eq!(&m[27..31], &3u32.to_le_bytes());
        assert_eq!(&m[31..35], &4u32.to_le_bytes());
        assert_eq!(&m[35..37], &[0xAA, 0xBB]);
        assert_eq!(m.len(), 19 + 16 + 2);
        // full：无 rects
        let f = build_frame_message(0, 1, 10, 10, &[], &[1, 2, 3]);
        assert_eq!(f[0], 0);
        assert_eq!(&f[13..15], &0u16.to_le_bytes());
        assert_eq!(&f[15..19], &3u32.to_le_bytes());
        assert_eq!(f.len(), 19 + 3);
    }

    /// 多帧拼接消息往返镜像测试（镜像前端 parseBinaryFrames 的偏移逻辑）：
    /// 构造 full+delta 两条消息拼接 → 逐帧解析（与前端同款字段偏移）→
    /// 断言与输入一致——证明组帧/解析对称（花屏排查锚点）。
    #[test]
    fn multi_frame_concat_roundtrip_mirrors_frontend_parse() {
        let f = build_frame_message(0, 1, 1366, 768, &[], &[0x11, 0x22]);
        let d = build_frame_message(
            1,
            2,
            1366,
            768,
            &[RectPayload { x: 10, y: 20, w: 30, h: 40 }],
            &vec![0x33u8; 30 * 40 * 4],
        );
        let mut msg = Vec::new();
        msg.extend_from_slice(&f);
        msg.extend_from_slice(&d);
        // —— 以下为前端 parseBinaryFrames 的 Rust 镜像（逐字段偏移一致）——
        let mut off = 0usize;
        let mut parsed = Vec::new();
        while off + 19 <= msg.len() {
            let kind = msg[off];
            let width = u32::from_le_bytes(msg[off + 1..off + 5].try_into().unwrap());
            let height = u32::from_le_bytes(msg[off + 5..off + 9].try_into().unwrap());
            let seq = u32::from_le_bytes(msg[off + 9..off + 13].try_into().unwrap());
            let n_rects = u16::from_le_bytes(msg[off + 13..off + 15].try_into().unwrap());
            let payload_len = u32::from_le_bytes(msg[off + 15..off + 19].try_into().unwrap());
            let mut p = off + 19;
            let mut rects = Vec::new();
            for _ in 0..n_rects {
                rects.push((
                    u32::from_le_bytes(msg[p..p + 4].try_into().unwrap()),
                    u32::from_le_bytes(msg[p + 4..p + 8].try_into().unwrap()),
                    u32::from_le_bytes(msg[p + 8..p + 12].try_into().unwrap()),
                    u32::from_le_bytes(msg[p + 12..p + 16].try_into().unwrap()),
                ));
                p += 16;
            }
            let payload = msg[p..p + payload_len as usize].to_vec();
            parsed.push((kind, width, height, seq, rects, payload));
            off = p + payload_len as usize;
        }
        assert_eq!(parsed.len(), 2, "恰好解析出两条帧");
        // 帧 0：full（JPEG payload 2 字节）
        assert_eq!(parsed[0].0, 0);
        assert_eq!(parsed[0].1, 1366);
        assert_eq!(parsed[0].2, 768);
        assert!(parsed[0].4.is_empty());
        assert_eq!(parsed[0].5, vec![0x11, 0x22]);
        // 帧 1：delta（1 矩形 30×40×4=4800 字节）
        assert_eq!(parsed[1].0, 1);
        assert_eq!(parsed[1].1, 1366);
        assert_eq!(parsed[1].2, 768);
        assert_eq!(parsed[1].4.len(), 1);
        assert_eq!(parsed[1].5.len(), 30 * 40 * 4);
    }

    // ---- SessionProfile 分辨率 ----

    #[test]
    fn resolution_dims_cover_all_variants() {
        assert_eq!(Resolution::FitWindow.dims(), (1280, 720));
        assert_eq!(Resolution::Custom { w: 800, h: 600 }.dims(), (800, 600));
    }
}
