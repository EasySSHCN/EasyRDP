//! Stub 会话：`native` feature 未启用时的默认实现。
//!
//! `connect` 返回 "engine not compiled" 错误（上层 session 桥据此进入
//! 本地演示帧模式），但 `send_input` 会把输入事件**记录到内部缓冲**，
//! 供单元测试断言完整输入管线的前半段。

use std::sync::{Arc, Mutex};

use easyrdp_core::profile::SessionProfile;

use crate::{InputEvent, RdpError, RdpSession};

pub const NOT_COMPILED: &str = "engine not compiled";

/// 无操作占位会话（带输入记录器）。
pub struct StubSession {
    recorded: Arc<Mutex<Vec<InputEvent>>>,
}

impl StubSession {
    pub fn new() -> Self {
        Self {
            recorded: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 读取已记录的输入事件快照（供测试断言）。
    pub fn recorded_inputs(&self) -> Vec<InputEvent> {
        self.recorded.lock().unwrap().clone()
    }

    /// 共享的记录器句柄（供上层把 stub 会话包装进 SessionHandle 后仍可断言）。
    pub fn recorder(&self) -> Arc<Mutex<Vec<InputEvent>>> {
        Arc::clone(&self.recorded)
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

    fn send_input(&mut self, event: &InputEvent) -> Result<(), RdpError> {
        self.recorded.lock().unwrap().push(event.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_records_mouse_inputs() {
        let mut s = StubSession::new();
        s.send_input(&InputEvent::MouseMove { x: 10, y: 20 }).unwrap();
        s.send_input(&InputEvent::MouseButton {
            button: "left".into(),
            down: true,
            x: 10,
            y: 20,
        })
        .unwrap();
        s.send_input(&InputEvent::MouseButton {
            button: "left".into(),
            down: false,
            x: 10,
            y: 20,
        })
        .unwrap();
        let rec = s.recorded_inputs();
        assert_eq!(rec.len(), 3);
        assert_eq!(rec[0], InputEvent::MouseMove { x: 10, y: 20 });
        assert_eq!(
            rec[1],
            InputEvent::MouseButton { button: "left".into(), down: true, x: 10, y: 20 }
        );
    }

    #[test]
    fn stub_records_keyboard_and_wheel_inputs() {
        let mut s = StubSession::new();
        s.send_input(&InputEvent::Key { code: "KeyA".into(), down: true }).unwrap();
        s.send_input(&InputEvent::Key { code: "KeyA".into(), down: false }).unwrap();
        s.send_input(&InputEvent::Wheel { delta: -120 }).unwrap();
        s.send_input(&InputEvent::Key { code: "Enter".into(), down: true }).unwrap();
        let rec = s.recorded_inputs();
        assert_eq!(rec.len(), 4);
        assert_eq!(rec[2], InputEvent::Wheel { delta: -120 });
        assert_eq!(rec[3], InputEvent::Key { code: "Enter".into(), down: true });
    }

    #[test]
    fn stub_recorder_shared_and_connect_still_fails() {
        let mut s = StubSession::new();
        let rec = s.recorder();
        s.send_input(&InputEvent::MouseMove { x: 1, y: 2 }).unwrap();
        assert_eq!(rec.lock().unwrap().len(), 1);
        let mut p = SessionProfile::default();
        p.host = "h".into();
        assert_eq!(s.connect(&p).unwrap_err().0, NOT_COMPILED);
    }
}
