//! FreeRDP FFI 集成骨架（仅在 `--features native` 下编译）。
//!
//! ============ 实现计划（native 接入时的路线图） ============
//!
//! 1. 上下文创建
//!    - `freerdp_new()` -> RDP_CONTEXT*；设置 `context->instances` 回调结构。
//!    - 注册 PreConnect / PostConnect / ReceiveChannelData 回调
//!      （`IFCALLRET` 检查；PostConnect 里创建 HCLIPRDR、RDPDR 虚拟通道）。
//! 2. 连接参数映射（SessionProfile -> FreeRDP settings）
//!    - `freerdp_settings_set_string(settings, FreeRDP_ServerHostname, profile.host)`
//!    - `FreeRDP_ServerPort` <- profile.port
//!    - `FreeRDP_Username` / `FreeRDP_Password`（经 `freerdp_settings_set_string`）
//!    - 分辨率：FitWindow -> 由窗口尺寸动态设置 `FreeRDP_DesktopWidth/Height`；
//!      Fullscreen -> `FreeRDP_Fullscreen = TRUE`；Custom -> 显式宽高。
//!    - 颜色深度 `FreeRDP_ColorDepth` <- profile.color_depth
//!    - 重定向：`FreeRDP_RedirectClipboard/Printers/Drives/Camera`
//! 3. 事件循环
//!    - 专用线程：`freerdp_connect` -> 循环 `freerdp_check_event_handles`；
//!      断开时 `freerdp_disconnect`。
//!    - `PostConnect` 中注册 `PointerUpdate`/`BitmapUpdate` 回调，把
//!      `rdpBitmap` 脏矩形转成 `FramebufferEvent::Framebuffer {..}` 推入
//!      mpsc channel（替换 lib.rs 中占位的 `crossbeam_like_channel::Receiver`）。
//! 4. 输入
//!    - 前端经 Tauri command 把键盘/鼠标事件转 `freerdp_input_send_keyboard_event`
//!      / `freerdp_input_send_mouse_event`。
//! 5. 构建矩阵
//!    - Linux: pkg-config freerdp2 (libfreerdp-dev)
//!    - Windows: vcpkg freerdp:x64-windows-static
//!    - `libfreerdp-sys` 提供 -sys 绑定；上层可再包一层 -rs 安全封装。
//! ==========================================================

#![cfg(feature = "native")]

use easyrdp_core::profile::SessionProfile;

use crate::{RdpError, RdpSession};

/// FreeRDP 上下文句柄的拥有者（native 编译时的真实会话）。
pub struct FreerdpSession {
    // TODO(native): freerdp::ContextHandle 与事件 mpsc::Receiver<(u64, FramebufferEvent)>
    connected: bool,
}

// SAFETY：FreeRDP 上下文要求单线程驱动（事件循环线程），跨线程只传递事件。
unsafe impl Send for FreerdpSession {}

impl FreerdpSession {
    pub fn new() -> Self {
        Self { connected: false }
    }
}

impl Default for FreerdpSession {
    fn default() -> Self {
        Self::new()
    }
}

impl RdpSession for FreerdpSession {
    fn connect(&mut self, profile: &SessionProfile) -> Result<(), RdpError> {
        if profile.host.trim().is_empty() {
            return Err(RdpError("host 不能为空".into()));
        }
        // TODO(native):
        // let ctx = freerdp_new();
        // apply_profile_settings(ctx, profile);
        // if unsafe { freerdp_connect(ctx) } == 0 { return Err(RdpError("freerdp_connect failed")); }
        // spawn_event_loop(ctx);   // 帧缓冲事件 -> channel
        self.connected = true;
        Err(RdpError(
            "native engine skeleton: event loop not implemented yet".into(),
        ))
    }

    fn disconnect(&mut self) -> Result<(), RdpError> {
        // TODO(native): unsafe { freerdp_disconnect(ctx) }; freerdp_free(ctx);
        self.connected = false;
        Ok(())
    }
}

/// 把 SessionProfile 翻译成 FreeRDP settings 键值（native 落地时的纯函数，便于单测）。
#[cfg(feature = "native")]
pub(crate) fn plan_settings(profile: &SessionProfile) -> Vec<(String, String)> {
    use easyrdp_core::profile::Resolution;
    let mut kv = vec![
        ("ServerHostname".to_string(), profile.host.clone()),
        ("ServerPort".to_string(), profile.port.to_string()),
        ("Username".to_string(), profile.username.clone()),
        ("ColorDepth".to_string(), profile.color_depth.to_string()),
    ];
    match profile.resolution {
        Resolution::FitWindow => kv.push(("DynamicResolution".to_string(), "true".into())),
        Resolution::Fullscreen => kv.push(("Fullscreen".to_string(), "true".into())),
        Resolution::Custom { w, h } => {
            kv.push(("DesktopWidth".to_string(), w.to_string()));
            kv.push(("DesktopHeight".to_string(), h.to_string()));
        }
    }
    kv
}
