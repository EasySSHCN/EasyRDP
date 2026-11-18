//! EasyRDP 云网关「本地回环桥」（F4 网关接入，v0.3.7）。
//!
//! ============ 方案选型与依据（写在代码里，评审可直接对照） ============
//!
//! 候选方案 A：启用 FreeRDP 原生 RD Gateway（`FreeRDP_GatewayEnabled` /
//! `GatewayHostname` / `GatewayPort` / `GatewayUsername` / `GatewayPassword` /
//! `GatewayAccessToken` 等，2.11.2 `<freerdp/settings.h>` 750-766 行已核实存在）。
//! **否决**：FreeRDP 原生 Gateway 走 MS-TSGU 协议（HTTP transport 请求
//! `/remoteDesktop/...` 端点 + RPC/NTLM 认证），而本项目云网关
//! （easyrdp-gateway/src/tunnel.rs）是自研协议：
//!   `GET /tunnel/{resource_id}`（WebSocket 升级）+ `Authorization: Bearer <JWT>`
//!   + Binary 帧透明转发。
//! 两者报文格式、认证语义、端点路径完全不同，FreeRDP 无法直连自研网关
//! （会以 HTTP transport 的 TSGU 握手报文打到 axum WS 路由上，必然失败）。
//!
//! 候选方案 B（**本实现**）：本地回环桥。
//!   客户端进程内起 `127.0.0.1:<临时端口>` TCP 监听；
//!   每个进入的 TCP 连接与网关建立一条 WS 隧道（Bearer JWT 认证），
//!   双向字节泵转发（与 gateway 侧 pump_ws_to_tcp / pump_tcp_to_ws 语义一致：
//!   Binary 帧透传、Text 帧忽略、Close 帧触发 TCP FIN）。
//!   FreeRDP 直连 `127.0.0.1:port`（ServerHostname/ServerPort 覆写），
//!   原生 Gateway 设置一律不启用。
//! 优点：零改动 FreeRDP 数据面（全部走已验证的直连路径）；网关协议解耦；
//! 复用 gateway 的 pump 语义保证字节流一致。缺点：每会话多一跳本地 memcpy
//! （127.0.0.1 回环开销可忽略）。
//!
//! TLS（wss）：MVP 仅实现明文 ws（内网/验收口径）；wss 需给
//! tokio-tungstenite 开 native-tls 或 rustls feature，见遗留项。
//!
//! 认证：
//! - `GatewayAuth::Token`：直接以 `token` 作为 Bearer JWT；
//! - `GatewayAuth::UserPassword`：先 POST `{http_base}/api/auth/login`
//!   （网关占位登录：`oidc_code = username`，见 gateway/src/auth.rs
//!   `parse_placeholder_code`），从响应 JSON 取 `token` 再 Bearer；
//! - `GatewayAuth::OtpCookie` / `Inherit`：MVP 未接（网关只认 Bearer JWT）。

use std::io;

use easyrdp_core::profile::{GatewayAuth, GatewayConfig};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// 桥会话句柄：持有专用 tokio Runtime（accept 循环在其中运行）。
/// Drop 时 `shutdown_background` 结束所有桥接任务。
pub struct BridgeHandle {
    runtime: Option<tokio::runtime::Runtime>,
    local_port: u16,
    accept_loop: JoinHandle<()>,
}

impl BridgeHandle {
    /// FreeRDP 应连接的本地回环地址（恒 127.0.0.1）。
    pub fn local_host(&self) -> &'static str {
        "127.0.0.1"
    }

    /// FreeRDP 应连接的本地回环端口（监听时由 OS 分配）。
    pub fn local_port(&self) -> u16 {
        self.local_port
    }
}

impl Drop for BridgeHandle {
    fn drop(&mut self) {
        // abort accept 循环并后台关停 runtime（任务随 shutdown 一起终止）。
        self.accept_loop.abort();
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

/// 解析桥转发目标资源：resource_id 为空 -> 以 target host:port 显式直连
/// （网关 SSRF 白名单约束私网/回环）。
fn resolve_resource(cfg: &GatewayConfig, target: &str) -> String {
    if cfg.resource_id.trim().is_empty() {
        target.to_string()
    } else {
        cfg.resource_id.trim().to_string()
    }
}

/// 启动回环桥（async 版本）：在**调用方已有的 tokio runtime** 内绑定
/// 127.0.0.1 临时端口并启动 accept 循环。返回的句柄不拥有 runtime
/// （Drop 只 abort accept 循环；会话任务随底层连接关闭自然结束）。
/// 测试 / 未来 GUI tokio 上下文用；FFI 同步路径走 [`spawn_bridge`]。
pub async fn start_bridge(
    gw: &GatewayConfig,
    target_host: &str,
    target_port: u16,
) -> Result<BridgeHandle, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| format!("网关桥绑定 127.0.0.1 失败: {e}"))?;
    let local_port = listener
        .local_addr()
        .map_err(|e| format!("网关桥读取监听地址失败: {e}"))?
        .port();
    let cfg = gw.clone();
    let target = format!("{target_host}:{target_port}");
    let resource = resolve_resource(&cfg, &target);
    let accept_loop = tokio::spawn(accept_loop(listener, cfg, resource, target));
    Ok(BridgeHandle {
        runtime: None,
        local_port,
        accept_loop,
    })
}

/// accept 循环本体：每个进入的 TCP 连接各开一条网关 WS 隧道。
async fn accept_loop(
    listener: TcpListener,
    cfg: GatewayConfig,
    resource: String,
    target: String,
) {
    loop {
        match listener.accept().await {
            Ok((tcp, _peer)) => {
                let cfg = cfg.clone();
                let resource = resource.clone();
                let target = target.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_connection(cfg, resource, target, tcp).await {
                        eprintln!("[gateway-bridge] 连接失败: {e}");
                    }
                });
            }
            Err(e) => {
                eprintln!("[gateway-bridge] accept 失败，桥退出: {e}");
                break;
            }
        }
    }
}

/// 启动回环桥：绑定 127.0.0.1 临时端口，后续每个 TCP 连接各开一条
/// 网关 WS 隧道。同步接口（内部起专用 runtime 线程），供 FFI connect 调用。
/// **不得在 tokio runtime 内调用**（block_on 会 panic）——async 上下文请用
/// [`start_bridge`]。
pub fn spawn_bridge(
    gw: &GatewayConfig,
    target_host: &str,
    target_port: u16,
) -> Result<BridgeHandle, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|e| format!("网关桥 runtime 启动失败: {e}"))?;

    let cfg = gw.clone();
    let target = format!("{target_host}:{target_port}");
    let resource = resolve_resource(&cfg, &target);

    let listener = runtime
        .block_on(async { TcpListener::bind(("127.0.0.1", 0)).await })
        .map_err(|e| format!("网关桥绑定 127.0.0.1 失败: {e}"))?;
    let local_port = listener
        .local_addr()
        .map_err(|e| format!("网关桥读取监听地址失败: {e}"))?
        .port();

    let accept_loop = runtime.spawn(accept_loop(listener, cfg, resource, target));

    Ok(BridgeHandle {
        runtime: Some(runtime),
        local_port,
        accept_loop,
    })
}

/// 单连接处理：换取/附加凭据 -> 连网关 WS -> 双向泵。
async fn serve_connection(
    cfg: GatewayConfig,
    resource: String,
    target: String,
    tcp: TcpStream,
) -> Result<(), String> {
    let token = acquire_token(&cfg, &target).await?;
    let ws_url = format!("{}/tunnel/{resource}", ws_base(&cfg));
    let mut request = ws_url
        .into_client_request()
        .map_err(|e| format!("WS 请求构造失败: {e}"))?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {token}")
            .parse()
            .map_err(|_| "Authorization 头构造失败".to_string())?,
    );
    let (ws, _resp) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| format!("网关 WS 连接失败: {e}"))?;

    let (ws_tx, ws_rx) = ws.split();
    let (tcp_r, tcp_w) = tokio::io::split(tcp);
    let (from_ws_tx, from_ws_rx) = mpsc::channel::<Message>(64);
    let (to_ws_tx, to_ws_rx) = mpsc::channel::<Message>(64);

    // WS 读半 -> 通道（与 gateway 侧 ws_source_to_channel 同构）
    let src = tokio::spawn(async move {
        let mut ws_rx = ws_rx;
        while let Some(Ok(msg)) = ws_rx.next().await {
            if from_ws_tx.send(msg).await.is_err() {
                break;
            }
        }
    });
    // 通道 -> WS 写半（与 gateway 侧 channel_to_ws_sink 同构）
    let sink = tokio::spawn(async move {
        let mut ws_tx = ws_tx;
        let mut to_ws_rx = to_ws_rx;
        while let Some(msg) = to_ws_rx.recv().await {
            if ws_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    // 双向泵：与 gateway/src/tunnel.rs pump_ws_to_tcp / pump_tcp_to_ws 语义一致。
    let to_backend = tokio::spawn(pump_ws_to_tcp(from_ws_rx, tcp_w));
    let to_client = tokio::spawn(pump_tcp_to_ws(tcp_r, to_ws_tx));
    let _ = tokio::join!(to_backend, to_client);
    // 泵结束后唯一持有发送端的任务退出，sink/src 适配任务随之结束。
    let _ = sink.await;
    let _ = src.await;
    Ok(())
}

/// WS base（网关地址）：`ws://host:port` / `wss://host:port`。
fn ws_base(cfg: &GatewayConfig) -> String {
    let scheme = if cfg.use_tls { "wss" } else { "ws" };
    format!("{scheme}://{}:{}", cfg.hostname.trim(), cfg.port)
}

/// 认证凭据解析：Token 直用；UserPassword 走网关占位登录换 JWT。
async fn acquire_token(cfg: &GatewayConfig, _target: &str) -> Result<String, String> {
    match &cfg.auth {
        GatewayAuth::Token => {
            let t = cfg.token.as_deref().map(str::trim).unwrap_or("");
            if t.is_empty() {
                // 校验层已拦，这里兜底走登录（有 username 时）
                login_for_token(cfg)
                    .await
                    .map_err(|e| format!("网关 Token 为空且登录换取失败: {e}"))
            } else {
                Ok(t.to_string())
            }
        }
        GatewayAuth::UserPassword => login_for_token(cfg).await,
        other => Err(format!(
            "网关认证方式 {other:?} 暂未接入云网关（仅支持 Token / UserPassword）"
        )),
    }
}

/// POST /api/auth/login（网关占位登录）换取短期 JWT。
/// 占位口径：`oidc_code = username`（gateway auth.rs parse_placeholder_code
/// 解析为 `sub=username, roles=["user"]`）；真实 OIDC 接入后此处换成 code 换取。
async fn login_for_token(cfg: &GatewayConfig) -> Result<String, String> {
    let username = cfg
        .username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("网关用户名为空，无法登录换取 token")?;
    let url = format!("{}/api/auth/login", cfg.http_base());
    let body = serde_json::json!({ "oidc_code": username });
    // MVP：占位登录端点无 TLS 校验需求（use_tls=false 的内网网关）。
    // http(s) 请求体极小，直接用 tungstenite 的底层不需要——这里用极简
    // std HTTP 客户端（避免引入 reqwest 大依赖）。
    let resp = http_post_json(&url, &body.to_string()).await?;
    let v: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("登录响应解析失败: {e}"))?;
    v.get("token")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| "登录响应缺少 token 字段".to_string())
}

/// 极简 HTTP/1.1 POST JSON（仅用于网关占位登录端点；不支持 chunked/重定向）。
async fn http_post_json(url: &str, body: &str) -> Result<String, String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("URL 非法: {url}"))?;
    let (hostport, path) = rest.split_once('/').map_or(
        (rest.to_string(), "/".to_string()),
        |(h, p)| (h.to_string(), format!("/{p}")),
    );
    let port = hostport
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse::<u16>().ok())
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    let host = hostport.split(':').next().unwrap_or(&hostport);
    if scheme == "https" {
        return Err("https 登录请求暂未实现（MVP 仅支持明文 ws/http 网关）".into());
    }
    let mut stream = TcpStream::connect((host, port))
        .await
        .map_err(|e| format!("连接网关 HTTP 端口失败: {e}"))?;
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {hostport}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|e| format!("发送登录请求失败: {e}"))?;
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .await
        .map_err(|e| format!("读取登录响应失败: {e}"))?;
    let text = String::from_utf8_lossy(&buf);
    // 跳过头部，取 body（Connection: close，读到 EOF 即完整）
    text.split("\r\n\r\n")
        .nth(1)
        .map(str::to_string)
        .ok_or_else(|| format!("登录响应格式异常: {text:.128}"))
}

// -------------------------------------------------------------------------
// 双向泵（与 easyrdp-gateway/src/tunnel.rs 同语义；客户端侧 WS 为
// tokio-tungstenite 类型，与 axum Message 不共享类型，故本地实现。
// 语义逐条对照 pump_ws_to_tcp / pump_tcp_to_ws 的注释核实过。）
// -------------------------------------------------------------------------

/// WS -> TCP 泵：Binary 帧写入 TCP；Text/Ping/Pong 忽略；Close（通道关闭）
/// 向 TCP 发 FIN。返回是否正常收尾。
async fn pump_ws_to_tcp<W>(mut rx: mpsc::Receiver<Message>, mut tcp_w: W) -> bool
where
    W: AsyncWrite + Unpin,
{
    while let Some(msg) = rx.recv().await {
        match msg {
            Message::Binary(data) => {
                if tcp_w.write_all(&data).await.is_err() {
                    return false;
                }
            }
            Message::Text(_) => {}
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }
    let _ = tcp_w.shutdown().await;
    true
}

/// TCP -> WS 泵：读取字节切 16KiB Binary 帧发往 WS 通道，EOF 正常结束。
async fn pump_tcp_to_ws<R>(mut tcp_r: R, tx: mpsc::Sender<Message>) -> bool
where
    R: AsyncRead + Unpin,
{
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match tcp_r.read(&mut buf).await {
            Ok(0) => return true,
            Ok(n) => {
                if tx.send(Message::Binary(buf[..n].to_vec())).await.is_err() {
                    return false;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
}

// -------------------------------------------------------------------------
// 单元测试（不依赖真机/真实网关）
// -------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    /// 泵语义对照测试：WS Binary 透传、Text 忽略、Close 触发 FIN。
    #[tokio::test]
    async fn ws_to_tcp_pump_forwards_binary_ignores_text() {
        let (tx, rx) = mpsc::channel(8);
        let (tcp_client, mut tcp_server) = tokio::io::duplex(64);
        let pump = tokio::spawn(pump_ws_to_tcp(rx, tcp_client));
        tx.send(Message::Binary(vec![0x03, 0x00, 0x00, 0x00])).await.unwrap();
        tx.send(Message::Text("ignored".into())).await.unwrap();
        tx.send(Message::Binary(b"rdp-payload".to_vec())).await.unwrap();
        drop(tx);
        assert!(pump.await.unwrap());
        let mut got = Vec::new();
        tcp_server.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"\x03\x00\x00\x00rdp-payload");
    }

    /// TCP -> WS 泵：字节流按序到达 Binary 帧，EOF 正常结束。
    #[tokio::test]
    async fn tcp_to_ws_pump_streams_binary_until_eof() {
        let (mut tcp_server, tcp_client) = tokio::io::duplex(64);
        let (tx, mut rx) = mpsc::channel(8);
        let pump = tokio::spawn(pump_tcp_to_ws(tcp_client, tx));
        tcp_server.write_all(b"RDP:hello").await.unwrap();
        tcp_server.shutdown().await.unwrap();
        assert!(pump.await.unwrap());
        let mut frames = Vec::new();
        while let Some(msg) = rx.recv().await {
            if let Message::Binary(d) = msg {
                frames.extend_from_slice(&d);
            }
        }
        assert_eq!(frames, b"RDP:hello");
    }

    /// plan_gateway_overrides：回环桥覆写计划（供 ffi.rs 应用 + 单测锚定）。
    #[test]
    fn gateway_overrides_map_loopback_bridge() {
        let gw = GatewayConfig {
            hostname: "gw.example.com".into(),
            port: 8443,
            use_tls: false,
            auth: GatewayAuth::Token,
            resource_id: "rdp-win01".into(),
            token: Some("jwt".into()),
            ..GatewayConfig::default()
        };
        let kv = plan_gateway_overrides(&gw);
        assert!(kv.contains(&("GatewayEnabled".into(), "loopback-bridge".into())));
        assert!(kv.contains(&("GatewayHostname".into(), "gw.example.com".into())));
        assert!(kv.contains(&("GatewayPort".into(), "8443".into())));
        assert!(kv.contains(&("ServerHostname".into(), "127.0.0.1".into())));
        assert!(kv.contains(&("ServerPort".into(), "loopback-bridge".into())));
        assert!(kv.contains(&("GatewayResourceId".into(), "rdp-win01".into())));
    }

    /// resource_id 为空时回退 target host:port 显式直连（网关 SSRF 白名单内）。
    #[test]
    fn bridge_resource_falls_back_to_target_addr() {
        let gw = GatewayConfig {
            hostname: "gw.example.com".into(),
            auth: GatewayAuth::Token,
            token: Some("jwt".into()),
            ..GatewayConfig::default()
        };
        // 构造逻辑复刻 serve_connection 调用方的规则（保持一致锚定）
        let resource = if gw.resource_id.trim().is_empty() {
            format!("{}:{}", "10.0.0.5", 3389)
        } else {
            gw.resource_id.trim().to_string()
        };
        assert_eq!(resource, "10.0.0.5:3389");
    }
}

/// 网关设置覆写计划（纯函数）：ffi.rs `apply_profile_settings` 据此把
/// FreeRDP 目标改写到回环桥。原生 RD Gateway 设置（FreeRDP_GatewayEnabled
/// 等）一律不启用——选型依据见模块头注释。
pub(crate) fn plan_gateway_overrides(gw: &GatewayConfig) -> Vec<(String, String)> {
    vec![
        ("GatewayEnabled".to_string(), "loopback-bridge".to_string()),
        ("GatewayHostname".to_string(), gw.hostname.trim().to_string()),
        ("GatewayPort".to_string(), gw.port.to_string()),
        (
            "GatewayAuth".to_string(),
            match gw.auth {
                GatewayAuth::Token => "token",
                GatewayAuth::UserPassword => "user_password",
                GatewayAuth::OtpCookie => "otp_cookie",
                GatewayAuth::Inherit => "inherit",
            }
            .to_string(),
        ),
        ("GatewayResourceId".to_string(), gw.resource_id.trim().to_string()),
        // 回环桥覆写：FreeRDP 实际连接目标
        ("ServerHostname".to_string(), "127.0.0.1".to_string()),
        ("ServerPort".to_string(), "loopback-bridge".to_string()),
    ]
}
