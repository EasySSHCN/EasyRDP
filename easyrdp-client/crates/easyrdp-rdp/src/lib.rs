//! EasyRDP 远程桌面引擎层（FreeRDP 集成骨架）。
//!
//! 设计要点：
//! - `default` feature 不依赖 FreeRDP，任何环境都能编译（用于 CI / 单测）。
//! - `native` feature 才链接 `libfreerdp-sys`，启用 `ffi.rs` 中的真实实现路径。
//! - 上层（easyrdp-app / 前端）只面向 [`RdpSession`] trait 与 [`create_session`] 工厂，
//!   引擎未编译时返回 [`StubSession`]，其 `connect` 返回 "engine not compiled" 错误。

use std::fmt;

use easyrdp_core::profile::SessionProfile;

/// 帧缓冲/会话事件（占位）。
///
/// native 引擎接入后，将由此通道推送：
/// - `Framebuffer { x, y, width, height, pixels }` 区域脏矩形更新
/// - `Pointer`、`Clipboard`、`Disconnected(reason)` 等会话事件
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FramebufferEvent {
    /// 占位变体：native 实现落地前不会产生。
    Placeholder,
}

/// 一次 RDP 会话的抽象。
pub trait RdpSession: Send {
    /// 建立到 `profile.host:profile.port` 的 RDP 会话。
    /// 引擎未编译（stub）时返回 Err("engine not compiled")。
    fn connect(&mut self, profile: &SessionProfile) -> Result<(), RdpError>;

    /// 断开当前会话（幂等）。
    fn disconnect(&mut self) -> Result<(), RdpError>;

    /// 帧缓冲事件接收端（native 引擎实现；stub 返回 None）。
    fn events(&self) -> Option<&crossbeam_like_channel::Receiver> {
        None
    }
}

/// 事件通道占位：native 接入前用最小类型撑住 trait 签名。
pub mod crossbeam_like_channel {
    /// 占位接收端。native 实现落地后替换为真实 mpsc/rtrb channel。
    #[derive(Debug)]
    pub struct Receiver;

    impl Receiver {
        pub fn try_recv(&self) -> Option<super::FramebufferEvent> {
            None
        }
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

/// 引擎是否具备真实 RDP 能力（`native` feature 已编译）。
pub fn native_ready() -> bool {
    cfg!(feature = "native")
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
