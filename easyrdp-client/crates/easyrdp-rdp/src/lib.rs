//! EasyRDP 远程桌面引擎层（FreeRDP 集成骨架）。
//!
//! 设计要点：
//! - `default` feature 不依赖 FreeRDP，任何环境都能编译（用于 CI / 单测）。
//! - `native` feature 才链接 `libfreerdp-sys`，启用 `ffi.rs` 中的真实实现路径。
//! - 上层（easyrdp-app / 前端）只面向 [`RdpSession`] trait 与 [`create_session`] 工厂，
//!   引擎未编译时返回 [`StubSession`]，其 `connect` 返回 "engine not compiled" 错误。

use std::fmt;

use easyrdp_core::profile::SessionProfile;

/// 帧缓冲/会话事件。
///
/// native 引擎的 FFI 回调（`rdpUpdate.BitmapUpdate`、`rdpPointerUpdate.PointerPosition`
/// 等，见 `ffi.rs`）把帧数据/指针更新推入 mpsc channel，由上层消费：
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FramebufferEvent {
    /// 服务端推来的位图脏矩形更新（对应 FreeRDP `BITMAP_UPDATE`/`BITMAP_DATA`）。
    /// `pixels` 为原始位图字节（α 版按服务端格式透传，未做 BGRA->RGBA 转换）。
    Framebuffer {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
    /// 远端指针位置更新（对应 FreeRDP `POINTER_POSITION_UPDATE`）。
    Pointer { x: u32, y: u32 },
    /// 会话断开（含原因描述）。
    Disconnected(String),
}

/// 一次 RDP 会话的抽象。
pub trait RdpSession: Send {
    /// 建立到 `profile.host:profile.port` 的 RDP 会话。
    /// 引擎未编译（stub）时返回 Err("engine not compiled")。
    fn connect(&mut self, profile: &SessionProfile) -> Result<(), RdpError>;

    /// 断开当前会话（幂等）。
    fn disconnect(&mut self) -> Result<(), RdpError>;

    /// 帧缓冲事件接收端（native 引擎实现；stub 返回 None）。
    fn events(&self) -> Option<&event_channel::Receiver> {
        None
    }
}

/// 事件通道：native FFI 回调（C 线程）经 [`event_channel::Sender`] 推送，
/// 上层经 [`event_channel::Receiver`] 消费。
pub mod event_channel {
    // 公开再导出：ffi.rs 经 `crate::event_channel::FramebufferEvent` 引用。
    pub use super::FramebufferEvent;

    /// 发送端（被 FFI 回调侧共享，要求 `Send + Clone`）。
    pub type Sender = std::sync::mpsc::Sender<FramebufferEvent>;

    /// 接收端包装。`try_recv` 非阻塞，适合 UI 轮询/重绘循环。
    pub struct Receiver(std::sync::mpsc::Receiver<FramebufferEvent>);

    impl Receiver {
        pub fn try_recv(&self) -> Option<FramebufferEvent> {
            self.0.try_recv().ok()
        }
    }

    /// 创建一对事件通道。
    pub fn channel() -> (Sender, Receiver) {
        let (tx, rx) = std::sync::mpsc::channel();
        (tx, Receiver(rx))
    }
}

/// 引擎层统一错误类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RdpError(pub String);

impl fmt::Display for RdpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for RdpError {}

/// 会话工厂：native 编译时返回 FFI 实现，否则返回 StubSession。
pub fn create_session() -> Box<dyn RdpSession> {
    #[cfg(feature = "native")]
    {
        Box::new(crate::ffi::FreerdpSession::new())
    }
    #[cfg(not(feature = "native"))]
    {
        Box::new(crate::stub::StubSession::new())
    }
}

/// 引擎是否具备真实 RDP 能力（`native` feature 已编译时为 true）。
pub fn native_ready() -> bool {
    #[cfg(feature = "native")]
    {
        true
    }
    #[cfg(not(feature = "native"))]
    {
        false
    }
}

pub mod ffi;
pub mod stub;

#[cfg(test)]
mod tests {
    use super::*;
    use easyrdp_core::profile::SessionProfile;

    #[test]
    fn factory_returns_stub_when_not_native() {
        if native_ready() {
            return; // native 构建下跳过
        }
        assert!(!native_ready());
        let s = create_session();
        // stub 会话没有事件通道
        assert!(s.events().is_none());
    }

    #[test]
    fn stub_connect_reports_engine_not_compiled() {
        let mut s = create_session();
        let mut p = SessionProfile::default();
        p.host = "10.0.0.8".into();
        p.port = 3389;
        let err = s.connect(&p).unwrap_err();
        assert_eq!(err.0, "engine not compiled");
    }

    #[test]
    fn stub_disconnect_is_idempotent_ok() {
        let mut s = create_session();
        s.disconnect().unwrap();
        s.disconnect().unwrap(); // 二次调用仍 Ok
    }

    #[test]
    fn stub_connect_fails_for_any_profile_shape() {
        let mut s = create_session();
        let mut p = SessionProfile::default();
        p.host = "example.com".into();
        p.resolution = easyrdp_core::profile::Resolution::Custom { w: 1920, h: 1080 };
        assert!(s.connect(&p).is_err());
    }
}
