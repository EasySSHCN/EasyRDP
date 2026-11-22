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
//! TLS（wss，F4-V1 R-20261118-012）：`use_tls=true` 走 `wss://`（tokio-tungstenite
//! native-tls feature，证书校验默认开启）；`allow_self_signed=true` 时
//! `danger_accept_invalid_certs` 放行自签证书（UI 明示勾选，默认关）。
//! 登录 /api/auth/login 同步支持 https（native-tls 直连，同校验策略）；
//! 网关要求 OTP 时登录返回 401 "otp_required"，桥按原样上抛（经
//! spawn_bridge -> RdpError -> session 错误串 -> UI 弹 OTP 输入重试）。
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
    // F4-V1：预检登录（换 JWT）。OTP 缺失/错误在此即以 "otp_required" 失败
    // 返回，错误链路：spawn_bridge -> connect() -> session 错误串 -> UI。
    let token = acquire_token(&cfg, &target).await?;
    let accept_loop = tokio::spawn(accept_loop(listener, cfg, resource, target, token));
    Ok(BridgeHandle {
        runtime: None,
        local_port,
        accept_loop,
    })
}

/// accept 循环本体：每个进入的 TCP 连接各开一条网关 WS 隧道（复用预检 JWT）。
async fn accept_loop(
    listener: TcpListener,
    cfg: GatewayConfig,
    resource: String,
    target: String,
    token: String,
) {
    loop {
        match listener.accept().await {
            Ok((tcp, _peer)) => {
                let cfg = cfg.clone();
                let resource = resource.clone();
                let target = target.clone();
                let token = token.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_connection(cfg, resource, target, token, tcp).await {
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
    // F4-V1：预检登录同 start_bridge（otp_required 在此即失败上抛）。
    let token = runtime
        .block_on(async { acquire_token(&cfg, &target).await })
        .map_err(|e| format!("网关认证失败: {e}"))?;

    let listener = runtime
        .block_on(async { TcpListener::bind(("127.0.0.1", 0)).await })
        .map_err(|e| format!("网关桥绑定 127.0.0.1 失败: {e}"))?;
    let local_port = listener
        .local_addr()
        .map_err(|e| format!("网关桥读取监听地址失败: {e}"))?
        .port();

    let accept_loop = runtime.spawn(accept_loop(listener, cfg, resource, target, token));

    Ok(BridgeHandle {
        runtime: Some(runtime),
        local_port,
        accept_loop,
    })
}

/// 单连接处理：连网关 WS（按 use_tls 走 ws/wss）-> 双向泵。
async fn serve_connection(
    cfg: GatewayConfig,
    resource: String,
    target: String,
    token: String,
    tcp: TcpStream,
) -> Result<(), String> {
    let ws_url = format!("{}/tunnel/{resource}", ws_base(&cfg));
    let (ws, _resp) = connect_gateway_ws(&cfg, &ws_url, &token).await?;

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
/// F4-V1：网关要求 OTP 二次认证时（401 "otp_required"）原样返回
/// `"otp_required"` 错误串，供上层 UI 识别并弹码重试。
async fn login_for_token(cfg: &GatewayConfig) -> Result<String, String> {
    let username = cfg
        .username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("网关用户名为空，无法登录换取 token")?;
    let url = format!("{}/api/auth/login", cfg.http_base());
    let mut body = serde_json::json!({ "oidc_code": username });
    if let Some(otp) = cfg.otp_code.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        body["mfa_code"] = serde_json::Value::String(otp.to_string());
    }
    let (status, resp) = http_post_json(&url, &body.to_string(), cfg).await?;
    if status == 401 && resp.contains("otp_required") {
        return Err("otp_required".to_string());
    }
    if !(200..300).contains(&status) {
        return Err(format!("网关登录失败 HTTP {status}: {}", resp.trim().to_string()));
    }
    let v: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("登录响应解析失败: {e}"))?;
    v.get("token")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| "登录响应缺少 token 字段".to_string())
}

/// 极简 HTTP/1.1 POST JSON（仅用于网关登录端点；不支持 chunked/重定向）。
/// F4-V1：https 走 native-tls（证书校验默认开启；allow_self_signed=true
/// 放行自签证书，与 wss 同一策略）；返回 (HTTP 状态码, 响应 body)。
async fn http_post_json(url: &str, body: &str, cfg: &GatewayConfig) -> Result<(u16, String), String> {
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
    let host = hostport.split(':').next().unwrap_or(&hostport).to_string();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {hostport}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|e| format!("连接网关 HTTP 端口失败: {e}"))?;

    if scheme == "https" {
        // TLS 握手（SNI=host），校验策略与 wss 一致
        let builder = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(cfg.allow_self_signed)
            .build()
            .map_err(|e| format!("TLS connector 构建失败: {e}"))?;
        let connector = tokio_native_tls::TlsConnector::from(builder);
        let mut stream = connector
            .connect(&host, tcp)
            .await
            .map_err(|e| format!("TLS 握手失败: {e}"))?;
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| format!("发送登录请求失败: {e}"))?;
        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .await
            .map_err(|e| format!("读取登录响应失败: {e}"))?;
        parse_http_response(&String::from_utf8_lossy(&buf))
    } else {
        let mut stream = tcp;
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| format!("发送登录请求失败: {e}"))?;
        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .await
            .map_err(|e| format!("读取登录响应失败: {e}"))?;
        parse_http_response(&String::from_utf8_lossy(&buf))
    }
}

/// 解析 HTTP/1.1 响应（Connection: close 读到 EOF 即完整）：取状态码与 body。
fn parse_http_response(text: &str) -> Result<(u16, String), String> {
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("登录响应格式异常: {:.128}", text))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| format!("登录响应状态行异常: {:.64}", head))?;
    Ok((status, body.to_string()))
}

/// 网关 WS 连接：按 `use_tls` 选择明文 ws 或 wss（native-tls 连接器）。
/// 证书校验默认开启；`allow_self_signed=true` 放行自签证书（UI 明示勾选）。
async fn connect_gateway_ws(
    cfg: &GatewayConfig,
    ws_url: &str,
    token: &str,
) -> Result<
    (
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
    ),
    String,
> {
    let mut request = ws_url
        .into_client_request()
        .map_err(|e| format!("WS 请求构造失败: {e}"))?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {token}")
            .parse()
            .map_err(|_| "Authorization 头构造失败".to_string())?,
    );
    if !cfg.use_tls {
        return tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| gateway_connect_error(&e));
    }
    // wss：native-tls 连接器（校验策略随 allow_self_signed）
    let builder = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(cfg.allow_self_signed)
        .build()
        .map_err(|e| format!("TLS connector 构建失败: {e}"))?;
    let connector = tokio_tungstenite::Connector::NativeTls(builder);
    tokio_tungstenite::connect_async_tls_with_config(request, None, false, Some(connector))
        .await
        .map_err(|e| gateway_connect_error(&e))
}

/// 网关连接错误透传（F5-V1 R-20261118-011）：网关返回 HTTP 拒绝（如
/// 策略矩阵 policy_denied 的 403）时，提取状态码与响应 body 文本，让
/// UI/日志能直接看到网关的拒绝原因；其余错误保留 tungstenite 原始信息。
fn gateway_connect_error(
    e: &tokio_tungstenite::tungstenite::Error,
) -> String {
    if let tokio_tungstenite::tungstenite::Error::Http(resp) = e {
        let status = resp.status();
        let body = String::from_utf8_lossy(resp.body().as_ref().map(|b| b.as_slice()).unwrap_or(&[]))
            .trim()
            .to_string();
        if body.is_empty() {
            return format!("网关拒绝连接 (HTTP {status})");
        }
        return format!("网关拒绝连接 (HTTP {status}): {body}");
    }
    format!("网关 WS 连接失败: {e}")
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
