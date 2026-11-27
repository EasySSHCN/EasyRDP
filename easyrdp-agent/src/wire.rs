//! agent-wire/1 帧协议（easyrdp-agent 侧独立实现）。
//!
//! 与 easyrdp-gateway `src/agent.rs`、EasySSH 侧共享同一 spec 文本，代码独立实现
//! （方案 §3 / §10）。Binary 帧统一头部：
//!
//! ```text
//! 偏移  长度  字段
//! 0     1    type
//! 1     8    session_id (u64 BE)
//! 9     4    payload_len (u32 BE)
//! 13    N    payload
//! ```
//!
//! 指令：0x01 DIAL(GW→Agent) / 0x02 OPEN_OK(Agent→GW) / 0x03 OPEN_ERR(Agent→GW) /
//! 0x04 DATA(双向) / 0x05 CLOSE(双向) / 0x06 PING(双向) / 0x07 PONG(双向)。

/// Binary 帧头长度：1(type) + 8(session_id BE) + 4(len BE)。
pub const FRAME_HEADER_LEN: usize = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// GW→Agent：payload = JSON {"backend":"host:port"}，请求 Agent 建立本地 TCP。
    Dial,
    /// Agent→GW：本地 TCP 已建立，session 开始。
    OpenOk,
    /// Agent→GW：payload = 错误摘要。
    OpenErr,
    /// 双向：payload = 透传字节。
    Data,
    /// 双向：关闭该 session。
    Close,
    /// 双向：应用层心跳。
    Ping,
    /// 双向：心跳应答。
    Pong,
}

impl FrameKind {
    pub fn from_u8(b: u8) -> Option<FrameKind> {
        match b {
            0x01 => Some(FrameKind::Dial),
            0x02 => Some(FrameKind::OpenOk),
            0x03 => Some(FrameKind::OpenErr),
            0x04 => Some(FrameKind::Data),
            0x05 => Some(FrameKind::Close),
            0x06 => Some(FrameKind::Ping),
            0x07 => Some(FrameKind::Pong),
            _ => None,
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            FrameKind::Dial => 0x01,
            FrameKind::OpenOk => 0x02,
            FrameKind::OpenErr => 0x03,
            FrameKind::Data => 0x04,
            FrameKind::Close => 0x05,
            FrameKind::Ping => 0x06,
            FrameKind::Pong => 0x07,
        }
    }
}

/// 一条 agent-wire/1 帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameKind,
    pub session_id: u64,
    pub payload: Vec<u8>,
}

/// 单帧 payload 上限（与网关侧一致，防声明攻击）。
pub const MAX_PAYLOAD_LEN: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// 非法帧 type。
    BadType(u8),
    /// payload 长度超出上限。
    PayloadTooLarge(usize),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::BadType(b) => write!(f, "illegal frame type 0x{b:02X}"),
            FrameError::PayloadTooLarge(n) => write!(f, "payload too large: {n}"),
        }
    }
}

impl Frame {
    pub fn new(kind: FrameKind, session_id: u64, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            kind,
            session_id,
            payload: payload.into(),
        }
    }

    /// 编码为完整 Binary 帧（header + payload）。
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(FRAME_HEADER_LEN + self.payload.len());
        out.push(self.kind.to_u8());
        out.extend_from_slice(&self.session_id.to_be_bytes());
        out.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// 从缓冲解码一条完整帧。
    /// 返回 `Ok(Some((frame, consumed)))`：解码成功；
    /// 返回 `Ok(None)`：缓冲不足一条完整帧（调用方继续累积）；
    /// 返回 `Err`：type 非法或 payload_len 超过上限。
    pub fn decode(buf: &[u8]) -> Result<Option<(Frame, usize)>, FrameError> {
        if buf.len() < FRAME_HEADER_LEN {
            return Ok(None);
        }
        let kind = FrameKind::from_u8(buf[0]).ok_or(FrameError::BadType(buf[0]))?;
        let session_id = u64::from_be_bytes(buf[1..9].try_into().expect("8 bytes"));
        let payload_len = u32::from_be_bytes(buf[9..13].try_into().expect("4 bytes")) as usize;
        if payload_len > MAX_PAYLOAD_LEN {
            return Err(FrameError::PayloadTooLarge(payload_len));
        }
        let total = FRAME_HEADER_LEN + payload_len;
        if buf.len() < total {
            return Ok(None);
        }
        let payload = buf[FRAME_HEADER_LEN..total].to_vec();
        Ok(Some((Frame::new(kind, session_id, payload), total)))
    }
}

/// DIAL payload 的 JSON 形状：`{"backend":"host:port"}`（方案 §3）。
#[derive(Debug, serde::Deserialize)]
pub struct DialPayload {
    pub backend: String,
}

/// 帧收发抽象：真实实现（tokio-tungstenite WS）与测试实现（mpsc 内存回环）
/// 共用同一会话状态机（`session::run_session`），保证测试语义 = 运行语义。
pub trait FrameIo: Send {
    /// 发送一帧；失败 = 连接已死。
    fn send_frame(
        &mut self,
        frame: Frame,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;

    /// 接收下一帧；None = 连接关闭。
    fn recv_frame(&mut self) -> impl std::future::Future<Output = Option<Frame>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip_all_kinds() {
        for kind in [
            FrameKind::Dial,
            FrameKind::OpenOk,
            FrameKind::OpenErr,
            FrameKind::Data,
            FrameKind::Close,
            FrameKind::Ping,
            FrameKind::Pong,
        ] {
            let f = Frame::new(kind, 0x0102_0304_0506_0708, b"payload-bytes".to_vec());
            let bytes = f.encode();
            assert_eq!(bytes.len(), FRAME_HEADER_LEN + 13);
            let (decoded, consumed) = Frame::decode(&bytes).expect("decode ok").expect("complete");
            assert_eq!(consumed, bytes.len());
            assert_eq!(decoded, f);
        }
    }

    #[test]
    fn frame_decode_partial_returns_none() {
        let full = Frame::new(FrameKind::Data, 7, b"abc".to_vec()).encode();
        assert_eq!(Frame::decode(&full[..5]).expect("ok"), None);
        assert_eq!(Frame::decode(&full[..FRAME_HEADER_LEN + 1]).expect("ok"), None);
        let (f, n) = Frame::decode(&full).expect("ok").expect("complete");
        assert_eq!(f.session_id, 7);
        assert_eq!(n, full.len());
    }

    #[test]
    fn frame_decode_pipelined_two_frames() {
        let a = Frame::new(FrameKind::Ping, 0, Vec::new()).encode();
        let b = Frame::new(FrameKind::Data, 3, b"xyz".to_vec()).encode();
        let mut both = a.clone();
        both.extend_from_slice(&b);
        let (f1, n1) = Frame::decode(&both).expect("ok").expect("complete");
        assert_eq!(f1.kind, FrameKind::Ping);
        assert_eq!(n1, a.len());
        let (f2, n2) = Frame::decode(&both[n1..]).expect("ok").expect("complete");
        assert_eq!(f2, Frame::new(FrameKind::Data, 3, b"xyz".to_vec()));
        assert_eq!(n2, b.len());
    }

    #[test]
    fn frame_decode_rejects_bad_type_and_oversize() {
        let mut f = Frame::new(FrameKind::Data, 1, Vec::new()).encode();
        f[0] = 0xFF;
        assert_eq!(Frame::decode(&f), Err(FrameError::BadType(0xFF)));
        let mut f2 = Frame::new(FrameKind::Data, 1, Vec::new()).encode();
        f2[9..13].copy_from_slice(&(u32::MAX).to_be_bytes());
        assert_eq!(
            Frame::decode(&f2),
            Err(FrameError::PayloadTooLarge(u32::MAX as usize))
        );
    }

    #[test]
    fn kind_values_match_wire_spec() {
        assert_eq!(FrameKind::Dial.to_u8(), 0x01);
        assert_eq!(FrameKind::OpenOk.to_u8(), 0x02);
        assert_eq!(FrameKind::OpenErr.to_u8(), 0x03);
        assert_eq!(FrameKind::Data.to_u8(), 0x04);
        assert_eq!(FrameKind::Close.to_u8(), 0x05);
        assert_eq!(FrameKind::Ping.to_u8(), 0x06);
        assert_eq!(FrameKind::Pong.to_u8(), 0x07);
        for k in [
            FrameKind::Dial,
            FrameKind::OpenOk,
            FrameKind::OpenErr,
            FrameKind::Data,
            FrameKind::Close,
            FrameKind::Ping,
            FrameKind::Pong,
        ] {
            assert_eq!(FrameKind::from_u8(k.to_u8()), Some(k));
        }
        assert_eq!(FrameKind::from_u8(0x00), None);
        assert_eq!(FrameKind::from_u8(0x08), None);
    }

    #[test]
    fn dial_payload_parses_json() {
        let p: DialPayload =
            serde_json::from_slice(br#"{"backend":"127.0.0.1:3389"}"#).expect("json");
        assert_eq!(p.backend, "127.0.0.1:3389");
        assert!(serde_json::from_slice::<DialPayload>(b"{}").is_err());
    }
}
