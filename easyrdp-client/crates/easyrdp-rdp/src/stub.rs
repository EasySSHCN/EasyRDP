//! Stub 会话：`native` feature 未启用时的默认实现。
//!
//! 所有方法返回 "engine not compiled" 类错误，保证上层可在无 FreeRDP 的
//! 环境下编译、运行 UI 与连接管理流程。

use easyrdp_core::profile::SessionProfile;

use crate::{RdpError, RdpSession};

pub const NOT_COMPILED: &str = "engine not compiled";

/// 无操作占位会话。
pub struct StubSession;

impl StubSession {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StubSession {
    fn default() -> Self {
        Self::new()
    }
}

impl RdpSession for StubSession {
    fn connect(&mut self, _profile: &SessionProfile) -> Result<(), RdpError> {
        Err(RdpError(NOT_COMPILED.into()))
    }

    fn disconnect(&mut self) -> Result<(), RdpError> {
        // 幂等：stub 从未真正连接，直接成功。
        Ok(())
    }
}
