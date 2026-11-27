//! Agent 会话核心：帧收发循环 + DIAL→本地 TCP + DATA 双向泵 + 心跳。
//!
//! 与网关解耦：`FrameIo` trait 抽象帧收发（真实实现 = tokio-tungstenite WS，
//! 测试实现 = mpsc 内存回环），会话状态机逻辑零网络依赖可单测（方案 §5 行为）。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::wire::{DialPayload, Frame, FrameIo, FrameKind};

/// 每条 session 下行 DATA 的切帧粒度（方案 §3：与网关 16KiB 缓冲一致）。
pub const PUMP_BUF_SIZE: usize = 16 * 1024;

/// 会话结束原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    /// 对端正常关闭（WS Close / 流结束）。
    PeerClosed,
    /// 静默超过判死窗口（心跳 3 周期无任何帧），主动断开重连。
    SilenceTimeout,
    /// 发送失败（连接已死）。
    SendFailed(String),
}

pub struct SessionParams {
    /// Agent 唯一允许 DIAL 的本地后端（配置内 local_backend，防跳板滥用）。
    pub local_backend: String,
    /// 本地后端 TCP connect 超时（方案 §5：5s）。
    pub dial_timeout: Duration,
    /// 应用层 PING 周期（Agent 侧主动保活）。
    pub heartbeat: Duration,
    /// 判死静默窗口（心跳 × 3）。
    pub silence_deadline: Duration,
}

impl SessionParams {
    pub fn from_config(cfg: &crate::config::AgentConfig) -> Self {
        Self {
            local_backend: cfg.tunnel.local_backend.clone(),
            dial_timeout: cfg.dial_timeout(),
            heartbeat: cfg.heartbeat(),
            silence_deadline: cfg.silence_deadline(),
        }
    }
}

/// 运行一个 Agent 会话直到连接结束。返回结束原因（重连循环据此退避）。
pub async fn run_session<I: FrameIo>(mut io: I, params: &SessionParams) -> Result<SessionEnd, SessionEnd> {
    let (out_tx, mut out_rx) = mpsc::channel::<Frame>(64);
    // session_id → 写半边入站帧通道（io 收到的该 session 帧转交写泵写进 TCP）。
    let mut sessions: HashMap<u64, mpsc::Sender<Frame>> = HashMap::new();
    // 会话任务句柄（返回时统一 abort，防 TCP 半边悬挂）。
    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let mut ping_tick = tokio::time::interval(params.heartbeat.max(Duration::from_millis(50)));
    ping_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let _ = ping_tick.tick().await; // 首个 tick 立即返回，跳过
    // 判死只看“对端入帧”静默：自己的保活 PING 不刷新该计时。
    let mut last_inbound = Instant::now();

    let result = loop {
        enum Event {
            /// 对端下行帧。
            Down(Frame),
            /// 本地任务上行帧（DATA 泵/OPEN_OK/OPEN_ERR/CLOSE）→ 转发给 io。
            Up(Frame),
            /// 周期 PING 触发。
            PingTick,
        }
        let event = tokio::select! {
            // 对端入帧（带判死静默窗口：剩余静默额度 = deadline − 距上次入帧）
            frame = tokio::time::timeout(
                params.silence_deadline.saturating_sub(last_inbound.elapsed()),
                io.recv_frame(),
            ) => {
                match frame {
                    Err(_) => break Err(SessionEnd::SilenceTimeout),
                    Ok(None) => break Ok(SessionEnd::PeerClosed),
                    Ok(Some(f)) => {
                        last_inbound = Instant::now();
                        Event::Down(f)
                    }
                }
            }
            // 本地任务产出（DATA 泵、OPEN_OK/OPEN_ERR、后端 EOF CLOSE）
            out = out_rx.recv() => {
                match out {
                    Some(f) => Event::Up(f),
                    None => break Ok(SessionEnd::PeerClosed), // 全部任务已退出
                }
            }
            // Agent 主动应用层保活 PING（session 0；网关侧 PONG 应答刷新静默窗口）
            _ = ping_tick.tick() => Event::PingTick,
        };

        match event {
            Event::Up(frame) => {
                if io.send_frame(frame).await.is_err() {
                    break Err(SessionEnd::SendFailed("forward up-frame failed".into()));
                }
            }
            Event::PingTick => {
                if io
                    .send_frame(Frame::new(FrameKind::Ping, 0, Vec::new()))
                    .await
                    .is_err()
                {
                    break Err(SessionEnd::SendFailed("send keepalive PING failed".into()));
                }
            }
            Event::Down(next) => match next.kind {
                // 应用层心跳：PING → PONG（同 session_id）。
                FrameKind::Ping => {
                    if io.send_frame(Frame::new(FrameKind::Pong, next.session_id, Vec::new())).await.is_err() {
                        break Err(SessionEnd::SendFailed("send PONG failed".into()));
                    }
                }
                FrameKind::Pong => { /* 心跳应答：刷新静默窗口即可 */ }
                FrameKind::Dial => {
                    let end = handle_dial(next, params, &out_tx, &mut sessions, &mut tasks).await;
                    if let Some(end) = end {
                        break Err(end);
                    }
                }
                // 路由到 session 写半边；CLOSE 后摘除。
                FrameKind::Data | FrameKind::Close => {
                    let removed = match sessions.get(&next.session_id) {
                        Some(tx) => tx.send(next.clone()).await.is_err(),
                        None => true,
                    };
                    if removed || next.kind == FrameKind::Close {
                        sessions.remove(&next.session_id);
                    }
                }
                // Agent→GW 方向帧不允许由网关下行伪造：忽略。
                FrameKind::OpenOk | FrameKind::OpenErr => {
                    tracing::warn!(sid = next.session_id, kind = ?next.kind, "unexpected downlink frame, ignored");
                }
            },
        }
    };

    for t in tasks {
        t.abort();
    }
    let _ = io.send_frame(Frame::new(FrameKind::Close, 0, Vec::new())).await;
    result
}

/// 处理 GW→Agent DIAL：校验 backend ∈ {local_backend} → dial_timeout 内 connect
/// 本地后端 → OPEN_OK/OPEN_ERR → 注册 session 并起双向泵。
async fn handle_dial(
    frame: Frame,
    params: &SessionParams,
    out_tx: &mpsc::Sender<Frame>,
    sessions: &mut HashMap<u64, mpsc::Sender<Frame>>,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) -> Option<SessionEnd> {
    let sid = frame.session_id;
    let backend: String = match serde_json::from_slice::<DialPayload>(&frame.payload) {
        Ok(p) => p.backend,
        Err(e) => {
            let msg = format!("DIAL payload 非法: {e}");
            tracing::warn!(sid, "{msg}");
            let _ = out_tx.send(Frame::new(FrameKind::OpenErr, sid, msg.into_bytes())).await;
            return None;
        }
    };
    if backend.trim() != params.local_backend {
        // 方案 §7：Agent 只能 DIAL 配置内 local_backend，不可作内网任意端口跳板。
        let msg = format!("backend {backend} 不在允许列表内（仅 {}）", params.local_backend);
        tracing::warn!(sid, "{msg}");
        let _ = out_tx.send(Frame::new(FrameKind::OpenErr, sid, msg.into_bytes())).await;
        return None;
    }

    let connect = tokio::time::timeout(params.dial_timeout, TcpStream::connect(backend.as_str())).await;
    let tcp = match connect {
        Err(_) => {
            let msg = format!(
                "连接本地后端 {backend} 超时（{}s）",
                params.dial_timeout.as_secs()
            );
            tracing::warn!(sid, "{msg}");
            let _ = out_tx.send(Frame::new(FrameKind::OpenErr, sid, msg.into_bytes())).await;
            return None;
        }
        Ok(Err(e)) => {
            let msg = format!("连接本地后端 {backend} 失败: {e}");
            tracing::warn!(sid, "{msg}");
            let _ = out_tx.send(Frame::new(FrameKind::OpenErr, sid, msg.into_bytes())).await;
            return None;
        }
        Ok(Ok(tcp)) => tcp,
    };
    tracing::info!(sid, backend = %backend, "local backend connected");

    let (in_tx, mut in_rx) = mpsc::channel::<Frame>(16);
    sessions.insert(sid, in_tx);
    let _ = out_tx.send(Frame::new(FrameKind::OpenOk, sid, Vec::new())).await;

    let (mut rd, mut wr) = tokio::io::split(tcp);
    let out_up = out_tx.clone();
    // 上行泵：本地后端 → GW（16KiB 切帧；EOF/错误 → CLOSE）。
    tasks.push(tokio::spawn(async move {
        let mut buf = vec![0u8; PUMP_BUF_SIZE];
        loop {
            match rd.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out_up
                        .send(Frame::new(FrameKind::Data, sid, buf[..n].to_vec()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        let _ = out_up.send(Frame::new(FrameKind::Close, sid, Vec::new())).await;
    }));
    // 下行泵：GW → 本地后端（DATA 写入；CLOSE → 半关写半边）。
    tasks.push(tokio::spawn(async move {
        while let Some(frame) = in_rx.recv().await {
            match frame.kind {
                FrameKind::Data => {
                    if wr.write_all(&frame.payload).await.is_err() {
                        break;
                    }
                }
                FrameKind::Close => {
                    let _ = wr.shutdown().await;
                    break;
                }
                _ => {}
            }
        }
    }));
    None
}

// ---------------------------------------------------------------------
// mpsc 内存帧通道（测试回环）
// ---------------------------------------------------------------------

/// 一对互连的 FrameIo 端点（经 mpsc；测试模拟网关与 Agent 直连）。
#[allow(dead_code)]
pub struct DuplexChannel {
    pub a: MpscFrameIo,
    pub b: MpscFrameIo,
}

#[allow(dead_code)]
pub struct MpscFrameIo {
    tx: mpsc::Sender<Frame>,
    rx: mpsc::Receiver<Frame>,
}

impl MpscFrameIo {
    #[allow(dead_code)]
    fn pair(capacity: usize) -> (MpscFrameIo, MpscFrameIo) {
        let (a_tx, b_rx) = mpsc::channel(capacity);
        let (b_tx, a_rx) = mpsc::channel(capacity);
        (
            MpscFrameIo { tx: a_tx, rx: a_rx },
            MpscFrameIo { tx: b_tx, rx: b_rx },
        )
    }
}

impl FrameIo for MpscFrameIo {
    async fn send_frame(&mut self, frame: Frame) -> Result<(), String> {
        self.tx.send(frame).await.map_err(|_| "channel closed".to_string())
    }

    async fn recv_frame(&mut self) -> Option<Frame> {
        self.rx.recv().await
    }
}

/// mpsc 内存帧通道（测试回环；bin 构建下为测试专用，allow dead_code）。
#[allow(dead_code)]
pub fn duplex_channel(capacity: usize) -> DuplexChannel {
    let (a, b) = MpscFrameIo::pair(capacity);
    DuplexChannel { a, b }
}

// ---------------------------------------------------------------------
// 单元测试：DIAL/DATA/CLOSE/心跳/禁限 backend 全语义（内存回环 + 本地 echo 后端）
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// 本地 TCP echo 后端（收什么回什么，EOF 关闭）。
    async fn spawn_echo_backend() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    loop {
                        match sock.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => {
                                if sock.write_all(&buf[..n]).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                });
            }
        });
        addr
    }

    fn params(backend: &str) -> SessionParams {
        SessionParams {
            local_backend: backend.to_string(),
            dial_timeout: Duration::from_secs(2),
            heartbeat: Duration::from_secs(3600), // 测试中用显式 PING 触发
            silence_deadline: Duration::from_secs(3600),
        }
    }

    async fn recv_kind(io: &mut MpscFrameIo, kind: FrameKind, sid: u64) -> Frame {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(Instant::now() < deadline, "等待 {kind:?} sid={sid} 超时");
            match tokio::time::timeout(Duration::from_millis(500), io.recv_frame()).await {
                Err(_) => continue, // 窗口内无帧，继续等到总 deadline
                Ok(None) => panic!("channel closed"),
                Ok(Some(frame)) => {
                    if frame.kind == kind && (sid == u64::MAX || frame.session_id == sid) {
                        return frame;
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn ping_replies_pong_same_session() {
        let mut ch = duplex_channel(16);
        let backend = spawn_echo_backend().await;
        let p = params(&backend);
        tokio::spawn(async move {
            let _ = run_session(ch.b, &p).await;
        });
        ch.a.send_frame(Frame::new(FrameKind::Ping, 42, Vec::new())).await.expect("send");
        let pong = recv_kind(&mut ch.a, FrameKind::Pong, 42).await;
        assert_eq!(pong.payload, Vec::<u8>::new());
    }

    #[tokio::test]
    async fn dial_opens_and_pumps_bidirectional_bytes() {
        let mut ch = duplex_channel(64);
        let backend = spawn_echo_backend().await;
        let p = params(&backend);
        tokio::spawn(async move {
            let _ = run_session(ch.b, &p).await;
        });
        // DIAL → OPEN_OK
        let dial_payload = format!(r#"{{"backend":"{backend}"}}"#);
        ch.a
            .send_frame(Frame::new(FrameKind::Dial, 7, dial_payload.into_bytes()))
            .await
            .expect("send dial");
        let ok = recv_kind(&mut ch.a, FrameKind::OpenOk, 7).await;
        assert!(ok.payload.is_empty());
        // 客户端方向 → DATA echo 回来
        ch.a
            .send_frame(Frame::new(FrameKind::Data, 7, b"hello-rdp".to_vec()))
            .await
            .expect("send data");
        let echoed = recv_kind(&mut ch.a, FrameKind::Data, 7).await;
        assert_eq!(echoed.payload, b"hello-rdp".to_vec());
        // 关闭 → Agent 回 CLOSE
        ch.a.send_frame(Frame::new(FrameKind::Close, 7, Vec::new())).await.expect("send close");
        let closed = recv_kind(&mut ch.a, FrameKind::Close, 7).await;
        assert!(closed.payload.is_empty());
    }

    #[tokio::test]
    async fn dial_disallowed_backend_returns_open_err() {
        let mut ch = duplex_channel(16);
        let backend = spawn_echo_backend().await;
        let p = params(&backend);
        tokio::spawn(async move {
            let _ = run_session(ch.b, &p).await;
        });
        ch.a
            .send_frame(Frame::new(FrameKind::Dial, 3, br#"{"backend":"10.9.9.9:445"}"#.to_vec()))
            .await
            .expect("send");
        let err = recv_kind(&mut ch.a, FrameKind::OpenErr, 3).await;
        let msg = String::from_utf8(err.payload).expect("utf8");
        assert!(msg.contains("不在允许列表内"), "msg: {msg}");
    }

    #[tokio::test]
    async fn dial_refused_backend_returns_open_err() {
        // 占住一个端口后立刻 drop listener：connect 应被拒绝
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        drop(listener);
        let mut ch = duplex_channel(16);
        let p = params(&addr);
        tokio::spawn(async move {
            let _ = run_session(ch.b, &p).await;
        });
        ch.a
            .send_frame(
                Frame::new(FrameKind::Dial, 5, format!(r#"{{"backend":"{addr}"}}"#).into_bytes()),
            )
            .await
            .expect("send");
        let err = recv_kind(&mut ch.a, FrameKind::OpenErr, 5).await;
        let msg = String::from_utf8(err.payload).expect("utf8");
        assert!(msg.contains("失败") || msg.contains("超时"), "msg: {msg}");
    }

    #[tokio::test]
    async fn dial_bad_payload_returns_open_err() {
        let mut ch = duplex_channel(16);
        let backend = spawn_echo_backend().await;
        let p = params(&backend);
        tokio::spawn(async move {
            let _ = run_session(ch.b, &p).await;
        });
        ch.a.send_frame(Frame::new(FrameKind::Dial, 9, b"not-json".to_vec())).await.expect("send");
        let err = recv_kind(&mut ch.a, FrameKind::OpenErr, 9).await;
        let msg = String::from_utf8(err.payload).expect("utf8");
        assert!(msg.contains("非法"), "msg: {msg}");
    }

    #[tokio::test]
    async fn backend_eof_sends_close_and_late_data_unrouted() {
        // 后端立即关闭（listener accept 后立刻 drop 连接）→ Agent 回 CLOSE
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.expect("accept");
            drop(sock);
        });
        let mut ch = duplex_channel(16);
        let p = params(&addr);
        tokio::spawn(async move {
            let _ = run_session(ch.b, &p).await;
        });
        ch.a
            .send_frame(Frame::new(FrameKind::Dial, 11, format!(r#"{{"backend":"{addr}"}}"#).into_bytes()))
            .await
            .expect("send");
        let ok = recv_kind(&mut ch.a, FrameKind::OpenOk, 11).await;
        assert!(ok.payload.is_empty());
        let close = recv_kind(&mut ch.a, FrameKind::Close, 11).await;
        assert!(close.payload.is_empty());
    }

    #[tokio::test]
    async fn silence_timeout_ends_session() {
        let mut ch = duplex_channel(16);
        let backend = spawn_echo_backend().await;
        let p = SessionParams {
            local_backend: backend,
            dial_timeout: Duration::from_secs(1),
            heartbeat: Duration::from_millis(50),
            silence_deadline: Duration::from_millis(120),
        };
        let io = ch.b;
        tokio::spawn(async move {
            let _ = run_session(io, &p).await;
        });
        // 静默窗口内不喂任何帧：会话应先收到周期 PING，随后判死结束
        let ping = recv_kind(&mut ch.a, FrameKind::Ping, u64::MAX).await;
        assert_eq!(ping.session_id, 0, "Agent 主动保活 PING 用 session 0");
        // 之后通道关闭（run_session 返回、io drop）或收到收尾 CLOSE
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(Instant::now() < deadline, "等待通道关闭超时");
            match tokio::time::timeout(Duration::from_millis(300), ch.a.recv_frame()).await {
                Err(_) => continue,
                Ok(None) => break,
                Ok(Some(Frame { kind: FrameKind::Close, .. })) => break,
                Ok(Some(_)) => continue,
            }
        }
    }

    #[tokio::test]
    async fn peer_close_ends_session_normally() {
        let ch = duplex_channel(16);
        let backend = spawn_echo_backend().await;
        let p = params(&backend);
        let io = ch.b;
        let handle = tokio::spawn(async move {
            run_session(io, &p).await
        });
        // 直接断开（drop 网关侧端点）
        drop(ch.a);
        let end = tokio::time::timeout(Duration::from_secs(3), handle)
            .await
            .expect("session end timeout")
            .expect("join");
        assert_eq!(end.expect("end"), SessionEnd::PeerClosed);
    }

    #[tokio::test]
    async fn large_payload_downlink_forwarded_whole() {
        let mut ch = duplex_channel(128);
        // 记录型后端：收全量后原样回一个 64KiB 块
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        let payload = vec![0xA5u8; 64 * 1024];
        let down_for_sock = payload.clone();
        let up_for_sock = payload.clone();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut got = vec![0u8; down_for_sock.len()];
            sock.read_exact(&mut got).await.expect("read");
            assert_eq!(got, down_for_sock);
            sock.write_all(&up_for_sock).await.expect("write");
            tokio::time::sleep(Duration::from_millis(50)).await;
        });
        let p = params(&addr);
        tokio::spawn(async move {
            let _ = run_session(ch.b, &p).await;
        });
        ch.a
            .send_frame(Frame::new(FrameKind::Dial, 21, format!(r#"{{"backend":"{addr}"}}"#).into_bytes()))
            .await
            .expect("send");
        let _ = recv_kind(&mut ch.a, FrameKind::OpenOk, 21).await;
        ch.a.send_frame(Frame::new(FrameKind::Data, 21, payload)).await.expect("send");
        // 上行可能多帧：聚合到 64KiB
        let total = 64 * 1024;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while got.len() < total {
            assert!(Instant::now() < deadline, "聚合上行超时");
            let f = recv_kind(&mut ch.a, FrameKind::Data, 21).await;
            got.extend_from_slice(&f.payload);
        }
        assert_eq!(got.len(), total);
        assert!(got.iter().all(|&b| b == 0xA5));
    }
}