//! 真实 WS 通道：tokio-tungstenite 0.21（rustls + webpki-roots，与网关同栈）
//! 的 FrameIo 适配器 + 断线重连建连逻辑。
//!
//! 网关路由：`GET /agent/connect?resource_id=...`，鉴权
//! `Authorization: Bearer <AgentToken>`（easyrdp-gateway agent.rs auth_guard）。
//! Binary 帧承载 agent-wire/1；Text 帧忽略（与网关泵语义一致）。

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{header::AUTHORIZATION, HeaderValue};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

use crate::wire::{Frame, FrameIo};

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// WS 帧适配器：实现 FrameIo；内部维护跨 WS 消息的粘包解码缓冲。
pub struct WsFrameIo {
    ws: WsStream,
    buf: Vec<u8>,
}

impl WsFrameIo {
    /// 建连：gateway URL（https/wss/http/ws 自动解析，见 config::connect_ws_url）
    /// + Bearer AgentToken。任何失败返回错误摘要（重连循环退避重试）。
    pub async fn connect(ws_url: &str, token: &str) -> Result<WsFrameIo, String> {
        let mut request = ws_url
            .into_client_request()
            .map_err(|e| format!("WS 请求构造失败: {e}"))?;
        request.headers_mut().insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|e| format!("Authorization 头非法: {e}"))?,
        );
        let (ws, _resp) = connect_async(request)
            .await
            .map_err(|e| format!("WS 连接失败: {e}"))?;
        Ok(WsFrameIo { ws, buf: Vec::new() })
    }
}

impl FrameIo for WsFrameIo {
    async fn send_frame(&mut self, frame: Frame) -> Result<(), String> {
        self.ws
            .send(Message::Binary(frame.encode()))
            .await
            .map_err(|e| format!("WS 发送失败: {e}"))
    }

    async fn recv_frame(&mut self) -> Option<Frame> {
        loop {
            // 先消费缓冲中已有的完整帧
            match Frame::decode(&self.buf) {
                Ok(Some((frame, consumed))) => {
                    self.buf.drain(..consumed);
                    return Some(frame);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(%e, len = self.buf.len(), "agent frame decode failed, closing");
                    return None;
                }
            }
            // 缓冲不足：继续读下一条 WS 消息
            let msg = self.ws.next().await?;
            match msg {
                Ok(Message::Binary(data)) => self.buf.extend_from_slice(&data),
                Ok(Message::Text(_)) => { /* 与网关一致：文本帧忽略 */ }
                Ok(Message::Close(_)) => return None,
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                Err(e) => {
                    tracing::warn!(%e, "WS 读失败");
                    return None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::duplex_channel;
    use crate::wire::{Frame, FrameKind};

    /// FrameIo trait 契约：经 trait 对象/泛型调用 mpsc 实现收发一致。
    #[tokio::test]
    async fn mpsc_frame_io_send_recv_via_trait() {
        let mut ch = duplex_channel(8);
        ch.a.send_frame(Frame::new(FrameKind::Data, 3, b"via-trait".to_vec()))
            .await
            .expect("send");
        let f = ch.b.recv_frame().await.expect("recv");
        assert_eq!(f.kind, FrameKind::Data);
        assert_eq!(f.session_id, 3);
        assert_eq!(f.payload, b"via-trait".to_vec());
    }

    #[tokio::test]
    async fn mpsc_frame_io_recv_none_when_closed() {
        let ch = duplex_channel(4);
        let a = ch.a;
        let mut b = ch.b;
        drop(a);
        assert!(b.recv_frame().await.is_none(), "对端 drop 后 recv 应返回 None");
    }

    /// 粘包解码缓冲语义在 wire 层单测覆盖；此处验证 WsFrameIo 构造错误路径
    /// 的错误摘要可读（非法 URL）。
    #[tokio::test]
    async fn ws_connect_bad_url_reports_error() {
        let res = WsFrameIo::connect("not-a-url", "token").await;
        assert!(res.is_err(), "非法 URL 应失败");
        assert!(!res.err().unwrap_or_default().is_empty());
        // https 主机不可达 → 连接失败摘要
        let res = WsFrameIo::connect("wss://127.0.0.1:1/agent/connect", "t").await;
        assert!(res.is_err(), "不可达应失败");
        let err = res.err().unwrap_or_default();
        assert!(err.contains("失败") || err.contains("错误"), "err: {err}");
    }
}
