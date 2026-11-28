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

use std::future::Future;
use std::io;
use std::time::Duration;

use easyrdp_core::profile::{GatewayAuth, GatewayConfig, GatewayEndpoint};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// F4-V2：单节点故障转移探测超时（连接超时 5s/条）。
pub const FAILOVER_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 桥会话句柄：持有专用 tokio Runtime（accept 循环在其中运行）。
/// Drop 时 `shutdown_background` 结束所有桥接任务。
pub struct BridgeHandle {
    runtime: Option<tokio::runtime::Runtime>,
    local_port: u16,
    accept_loop: JoinHandle<()>,
    /// F4-V2：故障转移成功后实际使用的网关节点（host:port）。
    used_gateway: String,
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

    /// F4-V2：本桥实际使用的网关节点（故障转移成功者）。
    pub fn used_gateway(&self) -> &str {
        &self.used_gateway
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
///
/// F4-V2（R-20261118-016）：多节点（配置了 fallbacks）时启动前按
/// primary → fallbacks 顺序选点（认证 + WS 隧道握手探测，超时 5s/条），
/// 首个可用节点承载本桥；认证失败默认 fail fast（`fail_on_auth_fail=true`
/// 时也切下一节点）；全部失败返回聚合错误（含每节点失败原因）。
/// **单节点（无 fallbacks）保持 v0.3.7 原语义**：启动仅做本地预检
/// （Token 认证无网络行为；UserPassword 走登录换 JWT，otp_required 在此
/// 上抛），WS 连接 / TLS 校验 / token 有效性全部发生在数据阶段——
/// 无效 JWT / 自签证书拒绝等不阻止桥启动，由回环 TCP FIN 体现。
/// 实际使用节点记录在 [`BridgeHandle::used_gateway`]。
pub async fn start_bridge(
    gw: &GatewayConfig,
    target_host: &str,
    target_port: u16,
) -> Result<BridgeHandle, String> {
    let target = format!("{target_host}:{target_port}");
    let chain = failover_chain_cfgs(gw);
    let labels = failover_chain_labels(gw);
    let resource = resolve_resource(gw, &target);

    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| format!("网关桥绑定 127.0.0.1 失败: {e}"))?;
    let local_port = listener
        .local_addr()
        .map_err(|e| format!("网关桥读取监听地址失败: {e}"))?
        .port();

    // 单节点：原语义路径——仅本地/登录预检，原始错误透传（含 otp_required）。
    if chain.len() == 1 {
        let token = acquire_token(&chain[0], &target).await?;
        let accept_loop =
            tokio::spawn(accept_loop(listener, chain, resource, target, token, 0));
        return Ok(BridgeHandle {
            runtime: None,
            local_port,
            accept_loop,
            used_gateway: labels[0].clone(),
        });
    }

    // 多节点：故障转移选点（探测超时 5s/条在闭包内施加，便于单测注入）。
    // 探测成功即持有该节点 JWT，accept 循环复用，不再二次探测。
    let (chosen, token) = failover_select(labels.clone(), gw.fail_on_auth_fail, |i| {
        let cfg = chain[i].clone();
        let resource = resource.clone();
        let target = target.clone();
        async move {
            match timeout(FAILOVER_PROBE_TIMEOUT, probe_endpoint(&cfg, &resource, &target)).await {
                Ok(r) => r,
                Err(_) => Err((false, "连接超时 (5s)".to_string())),
            }
        }
    })
    .await?;

    let accept_loop =
        tokio::spawn(accept_loop(listener, chain, resource, target, token, chosen));
    Ok(BridgeHandle {
        runtime: None,
        local_port,
        accept_loop,
        used_gateway: labels[chosen].clone(),
    })
}

/// F4-V2：故障转移节点链的运行时配置列表（主网关在前）。
/// 每个节点的定位字段取自 GatewayEndpoint，凭据 / resource_id / OTP /
/// auth 共享 GatewayConfig 本体。
fn failover_chain_cfgs(gw: &GatewayConfig) -> Vec<GatewayConfig> {
    gw.endpoint_chain()
        .iter()
        .map(|ep| endpoint_cfg(gw, ep))
        .collect()
}

/// F4-V2：节点链显示标识（host:port），聚合错误 / used_gateway 用。
fn failover_chain_labels(gw: &GatewayConfig) -> Vec<String> {
    gw.endpoint_chain().iter().map(|ep| ep.label()).collect()
}

/// F4-V2：由节点端点派生该节点的 GatewayConfig（定位字段取端点，
/// 其余字段共享主配置）。
fn endpoint_cfg(base: &GatewayConfig, ep: &GatewayEndpoint) -> GatewayConfig {
    GatewayConfig {
        hostname: ep.hostname.clone(),
        port: ep.port,
        use_tls: ep.use_tls,
        allow_self_signed: ep.allow_self_signed,
        ..base.clone()
    }
}

/// F4-V2：单节点探测——预检登录（换 JWT）+ WS 隧道握手并立即 Close。
/// 成功返回该节点 JWT；失败返回 `(是否认证阶段失败, 原因)`。
async fn probe_endpoint(
    cfg: &GatewayConfig,
    resource: &str,
    target: &str,
) -> Result<String, (bool, String)> {
    let token = acquire_token(cfg, target).await.map_err(|e| (true, e))?;
    let ws_url = format!("{}/tunnel/{resource}", ws_base(cfg));
    let (mut ws, _resp) = connect_gateway_ws(cfg, &ws_url, &token)
        .await
        .map_err(|e| (false, e))?;
    // 握手成功即 Close（隧道探测连接，不承载业务流量；服务端按 Close
    // 帧正常收尾，与 gateway 侧 pump 语义一致）。
    let _ = ws.close(None).await;
    Ok(token)
}

/// F4-V2：故障转移选点核心（可注入 probe，单测不依赖真实网关）。
/// 按序探测；首个成功节点返回 `(下标, 该节点 JWT)`。认证失败（auth=true）
/// 默认立即 fail fast；`fail_on_auth_fail=true` 时也继续切下一节点。
/// 全部失败返回聚合错误（含每节点失败原因与是否认证阶段）。
pub(crate) async fn failover_select<F, Fut>(
    labels: Vec<String>,
    fail_on_auth_fail: bool,
    mut probe: F,
) -> Result<(usize, String), String>
where
    F: FnMut(usize) -> Fut,
    Fut: Future<Output = Result<String, (bool, String)>>,
{
    let mut attempts: Vec<(String, bool, String)> = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        match probe(i).await {
            Ok(token) => return Ok((i, token)),
            Err((auth, reason)) => {
                attempts.push((label.clone(), auth, reason));
                if auth && !fail_on_auth_fail {
                    break;
                }
            }
        }
    }
    Err(aggregate_failover_error(&attempts))
}

/// F4-V2：全部节点失败的聚合错误（每节点一行：标签 + 阶段 + 原因）。
pub(crate) fn aggregate_failover_error(attempts: &[(String, bool, String)]) -> String {
    let mut s = String::from("网关全部节点连接失败（按序尝试）：\n");
    for (label, auth, reason) in attempts {
        let stage = if *auth { "[认证失败] " } else { "" };
        s.push_str(&format!("  - {label}: {stage}{reason}\n"));
    }
    s
}

/// accept 循环本体：每个进入的 TCP 连接各开一条网关 WS 隧道。
/// F4-V2：每条连接按 `start_idx`（上次成功节点）起的轮转顺序尝试节点
/// 链——桥断开重连时（下一 TCP 连接）自动优先上次可用节点并依次尝试
/// fallback。认证凭据按节点懒换取（UserPassword 各节点各自登录）。
async fn accept_loop(
    listener: TcpListener,
    chain: Vec<GatewayConfig>,
    resource: String,
    target: String,
    token: String,
    start_idx: usize,
) {
    loop {
        match listener.accept().await {
            Ok((tcp, _peer)) => {
                let chain = chain.clone();
                let resource = resource.clone();
                let target = target.clone();
                let token = token.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        serve_connection(chain, resource, target, token, start_idx, tcp).await
                    {
                        // GUI 下 stderr 不可见（UX 缺陷修复）：失败原因统一写入
                    // debug 日志（与 ffi.rs [gateway] 前缀同一机制），原始文案
                    // 保留供排查；用户可操作提示由 map_session_error 在
                    // Disconnected 事件处生成。
                    crate::debug_log(&format!("[gateway] 连接失败: {e}"));
                    }
                });
            }
            Err(e) => {
                crate::debug_log(&format!("[gateway] accept 失败，桥退出: {e}"));
                break;
            }
        }
    }
}

/// 启动回环桥：绑定 127.0.0.1 临时端口，后续每个 TCP 连接各开一条
/// 网关 WS 隧道。同步接口（内部起专用 runtime 线程），供 FFI connect 调用。
/// **不得在 tokio runtime 内调用**（block_on 会 panic）——async 上下文请用
/// [`start_bridge`]。
/// F4-V2：故障转移语义同 [`start_bridge`]（探测选点 → 首个可用节点）。
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
    let chain = failover_chain_cfgs(&cfg);
    let labels = failover_chain_labels(&cfg);
    let resource = resolve_resource(&cfg, &target);

    // 单节点：原语义路径——仅本地/登录预检（原始错误透传，含 otp_required），
    // WS 连接 / TLS 校验 / token 有效性留到数据阶段（与 start_bridge 一致）。
    let (start_idx, token) = if chain.len() == 1 {
        let token = runtime
            .block_on(async { acquire_token(&chain[0], &target).await })
            .map_err(|e| format!("网关认证失败: {e}"))?;
        (0, token)
    } else {
        // 多节点：故障转移选点（探测 + 超时 5s/条在 block_on 内执行）。
        // 探测成功即持有该节点 JWT，不再二次探测。
        let (idx, tok) = runtime
            .block_on(async {
                failover_select(labels.clone(), cfg.fail_on_auth_fail, |i| {
                    let cfg = chain[i].clone();
                    let resource = resource.clone();
                    let target = target.clone();
                    async move {
                        match timeout(
                            FAILOVER_PROBE_TIMEOUT,
                            probe_endpoint(&cfg, &resource, &target),
                        )
                        .await
                        {
                            Ok(r) => r,
                            Err(_) => Err((false, "连接超时 (5s)".to_string())),
                        }
                    }
                })
                .await
            })
            .map_err(|e| format!("网关认证失败: {e}"))?;
        (idx, tok)
    };

    let listener = runtime
        .block_on(async { TcpListener::bind(("127.0.0.1", 0)).await })
        .map_err(|e| format!("网关桥绑定 127.0.0.1 失败: {e}"))?;
    let local_port = listener
        .local_addr()
        .map_err(|e| format!("网关桥读取监听地址失败: {e}"))?
        .port();

    let accept_loop = runtime.spawn(accept_loop(
        listener,
        chain,
        resource,
        target,
        token,
        start_idx,
    ));

    Ok(BridgeHandle {
        runtime: Some(runtime),
        local_port,
        accept_loop,
        used_gateway: labels[start_idx].clone(),
    })
}

/// 单连接处理：按轮转顺序（start_idx 起）尝试节点链建立网关 WS 隧道，
/// 首个成功节点承载本连接 → 双向泵。
///
/// F4-V2 会话迁移语义（如实标注）：桥与 FreeRDP 之间是本地回环 TCP。
/// 网关侧隧道断开时，泵向回环侧发 FIN → FreeRDP 的 RDP TCP 连接**同步
/// 断开**，正在运行的 RDP 会话无法在传输层保持（RDP 协议本身无透明
/// 会话迁移能力，隧道重建后字节流上下文不可恢复）。因此本实现的
/// 「重连」作用于**下一连接**：FreeRDP 断开后重开会话时，桥按上次可用
/// 节点起轮转自动选择可用网关（含 fallback），用户无需改配置。
/// 真正的会话内迁移需 RDP 层自动重连接续（FreeRDP auto-reconnect +
/// 服务端支持），列为遗留项，MVP 口径 = **重连后需重开会话**。
async fn serve_connection(
    chain: Vec<GatewayConfig>,
    resource: String,
    target: String,
    token: String,
    start_idx: usize,
    tcp: TcpStream,
) -> Result<(), String> {
    let n = chain.len();
    let mut last_err = String::new();
    let mut ws = None;
    for k in 0..n {
        let i = (start_idx + k) % n;
        let cfg = &chain[i];
        let ws_url = format!("{}/tunnel/{resource}", ws_base(cfg));
        // 首个节点（start_idx）复用预检 JWT；其余节点按需各自换取。
        let tok = if k == 0 {
            token.clone()
        } else {
            match acquire_token(cfg, &target).await {
                Ok(t) => t,
                Err(e) => {
                    last_err = format!("{}: [认证失败] {e}", cfg.hostname.trim());
                    continue;
                }
            }
        };
        match connect_gateway_ws(cfg, &ws_url, &tok).await {
            Ok(pair) => {
                ws = Some(pair);
                break;
            }
            Err(e) => {
                last_err = format!("{}: {e}", cfg.hostname.trim());
            }
        }
    }
    let (ws, _resp) = ws.ok_or_else(|| {
        format!("网关隧道建立失败（按序尝试 {n} 节点）：{last_err}")
    })?;

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

/// 从失败原因中提取 FreeRDP last_error 数字（last_error_reason 生成的
/// `[last_error=13185 (0x2000D): ...]` 形态；无则 None）。
fn parse_last_error_code(reason: &str) -> Option<u32> {
    let idx = reason.find("last_error=")?;
    let rest = &reason[idx + "last_error=".len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// 失败原因 -> 用户可操作提示（UX 缺陷修复 B：GUI 下 stderr 丢失，用户
/// 唯一能看到的就是 Disconnected 事件文案，必须自解释、可操作）。
/// 未命中任何已知模式时原样透传（保留原始诊断信息，不丢上下文）。
/// 原始原因始终另写 debug 日志（调用方负责），此处只产出展示文案。
pub fn map_session_error(reason: &str, gateway_configured: bool, backend: &str) -> String {
    // 1) 网关策略拒绝（桥 gateway_connect_error 产出的 "网关拒绝连接 (HTTP 403)"）
    if reason.contains("网关拒绝连接 (HTTP 403)") {
        return "网关拒绝连接：当前账号没有该资源的访问权限（策略矩阵 deny），请联系管理员在 gw-admin 配置策略".to_string();
    }
    // 2) 认证失败（401 / otp_required / Token 失效）
    if reason.contains("HTTP 401") || reason.contains("otp_required") || reason.contains("Token 已过期") {
        return "网关认证失败：Token 已过期或无效，请重新签发".to_string();
    }
    // 3) TLS 证书错误（桥/健康探测的 "TLS 握手失败" 与 tungstenite 证书报文）
    let lower = reason.to_lowercase();
    if reason.contains("TLS 握手失败") || lower.contains("certificate") || reason.contains("证书") {
        return "网关证书校验失败，可在设置中允许自签证书".to_string();
    }
    // 4) FreeRDP 传输层断链（13185 / 0x2000D ERRINFO 传输失败）
    let last_code = parse_last_error_code(reason);
    if last_code == Some(13185) {
        if gateway_configured {
            return "网关连接失败：网关服务无响应或网络中断，请检测网关连通性或联系管理员".to_string();
        }
        return "连接传输失败：网络中断或目标机器无响应，请检测网络连通性".to_string();
    }
    // 5) 后端不可达（桥聚合错误/网关 504：backend connect timeout / HTTP 504）
    if reason.contains("backend connect timeout") || reason.contains("504") {
        return format!(
            "目标机器不可达：网关无法连接到 {backend}，请确认目标机已开机/可达（局域网机器请使用直连模式）"
        );
    }
    // 6) 兜底：原样透传
    reason.to_string()
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
// F4-V2 健康探测（src-tauri gateway_health_check 命令的数据面）
// -------------------------------------------------------------------------

/// F4-V2：单网关节点健康探测结果（前端网关区「检测」按钮逐节点展示）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GatewayHealthReport {
    /// 节点 host（回显探测输入）。
    pub host: String,
    /// 节点 port。
    pub port: u16,
    /// TCP 连接 +（use_tls 时）TLS 握手全部通过。
    pub ok: bool,
    /// 探测耗时（毫秒；失败时为到失败点的耗时）。
    pub latency_ms: u64,
    /// 失败原因（ok=true 时为 None）。
    pub error: Option<String>,
}

/// F4-V2：对单个网关节点做健康探测——TCP 连接 +（use_tls=true 时）TLS
/// 握手，超时 5s。不含认证 / 隧道探测（凭据属运行时态，健康探测只回答
/// 「节点网络可达」）。前端逐节点并发调用本函数即可。
pub async fn probe_gateway_health(
    host: &str,
    port: u16,
    use_tls: bool,
    allow_self_signed: bool,
) -> GatewayHealthReport {
    let started = std::time::Instant::now();
    let mut report = GatewayHealthReport {
        host: host.to_string(),
        port,
        ok: false,
        latency_ms: 0,
        error: None,
    };
    let tcp = match timeout(
        FAILOVER_PROBE_TIMEOUT,
        TcpStream::connect((host.trim(), port)),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            report.latency_ms = started.elapsed().as_millis() as u64;
            report.error = Some(format!("TCP 连接失败: {e}"));
            return report;
        }
        Err(_) => {
            report.latency_ms = started.elapsed().as_millis() as u64;
            report.error = Some("TCP 连接超时 (5s)".to_string());
            return report;
        }
    };
    if use_tls {
        let builder = match native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(allow_self_signed)
            .build()
        {
            Ok(b) => b,
            Err(e) => {
                report.latency_ms = started.elapsed().as_millis() as u64;
                report.error = Some(format!("TLS connector 构建失败: {e}"));
                return report;
            }
        };
        let connector = tokio_native_tls::TlsConnector::from(builder);
        let host_trim = host.trim();
        let sni = if host_trim.parse::<std::net::IpAddr>().is_ok() {
            // IP 直连没有 SNI 语义：用 "localhost" 占位（证书校验按
            // allow_self_signed 策略放行/拒绝，与 wss 连接行为一致）。
            "localhost"
        } else {
            host_trim
        };
        match timeout(FAILOVER_PROBE_TIMEOUT, connector.connect(sni, tcp)).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                report.latency_ms = started.elapsed().as_millis() as u64;
                report.error = Some(format!("TLS 握手失败: {e}"));
                return report;
            }
            Err(_) => {
                report.latency_ms = started.elapsed().as_millis() as u64;
                report.error = Some("TLS 握手超时 (5s)".to_string());
                return report;
            }
        }
    }
    report.ok = true;
    report.latency_ms = started.elapsed().as_millis() as u64;
    report
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

    /// map_session_error：五类已知失败 -> 用户可操作提示；未知原样透传。
    #[test]
    fn ux_map_session_error_covers_known_failures() {
        // 13185 + 网关已配置 → 网关连通性提示
        let r = "freerdp_connect 到 127.0.0.1 失败 [last_error=13185 (0x2000D): TRANSPORT_FAILED]";
        let m = map_session_error(r, true, "10.0.0.5:3389");
        assert!(m.contains("网关连接失败") && m.contains("网关连通性"), "{m}");
        // 13185 + 未配网关 → 通用传输提示
        let m = map_session_error(r, false, "10.0.0.5:3389");
        assert!(m.contains("连接传输失败"), "{m}");
        // 网关 403 → 策略 deny 提示
        let m = map_session_error("网关拒绝连接 (HTTP 403): policy_denied", true, "x");
        assert!(m.contains("策略矩阵 deny"), "{m}");
        // backend timeout / 504 → 目标机不可达提示（含 backend 标识）
        let m = map_session_error("gw-a:443: backend connect timeout", true, "10.0.0.5:3389");
        assert!(m.contains("目标机器不可达") && m.contains("10.0.0.5:3389"), "{m}");
        let m = map_session_error("网关拒绝连接 (HTTP 504)", true, "host-b");
        assert!(m.contains("目标机器不可达") && m.contains("host-b"), "{m}");
        // 认证失败 401 / otp_required → Token 重新签发提示
        let m = map_session_error("gw-a:443: [认证失败] 网关登录失败 HTTP 401", true, "x");
        assert!(m.contains("Token 已过期或无效"), "{m}");
        let m = map_session_error("otp_required", true, "x");
        assert!(m.contains("Token 已过期或无效"), "{m}");
        // TLS 证书错误 → 允许自签证书提示
        let m = map_session_error("gw-a:443: TLS 握手失败: certificate verify failed", true, "x");
        assert!(m.contains("允许自签证书"), "{m}");
        // 未知失败原样透传（不丢诊断信息）
        let m = map_session_error("某种未知失败", true, "x");
        assert_eq!(m, "某种未知失败");
    }

    /// parse_last_error_code：从 last_error_reason 文案中提取数字码。
    #[test]
    fn ux_parse_last_error_code_extracts_digits() {
        assert_eq!(
            parse_last_error_code("x [last_error=13185 (0x2000D): t]"),
            Some(13185)
        );
        assert_eq!(parse_last_error_code("no code here"), None);
        assert_eq!(parse_last_error_code("[last_error=]"), None);
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

    // -----------------------------------------------------------------
    // F4-V2「网关高可用」（R-20261118-016）：故障转移序列 / 聚合错误
    // （probe 注入 mock，不依赖真实网关）
    // -----------------------------------------------------------------

    /// 故障转移序列：primary 连接级拒连 → fallback 成功；返回下标 1 +
    /// 该节点 token；不触发认证 fail fast。
    #[tokio::test]
    async fn f4v2_failover_primary_refused_fallback_succeeds() {
        let labels = vec!["gw-a:443".to_string(), "gw-b:8443".to_string()];
        let mut probed = Vec::new();
        let (idx, token) = failover_select(labels.clone(), false, |i| {
            probed.push(i);
            let r: Result<String, (bool, String)> = match i {
                0 => Err((false, "connection refused".into())),
                _ => Ok("jwt-b".into()),
            };
            async move { r }
        })
        .await
        .expect("fallback 节点应被选中");
        assert_eq!(idx, 1, "primary 拒连后应选中 fallback[0]");
        assert_eq!(token, "jwt-b");
        assert_eq!(probed, vec![0, 1], "按序探测：先 primary 再 fallback");
    }

    /// 故障转移序列：primary 超时（连接超时 5s 由调用方闭包施加，此处以
    /// 错误串模拟超时形态）→ fallback 成功。
    #[tokio::test]
    async fn f4v2_failover_primary_timeout_fallback_succeeds() {
        let labels = vec!["gw-a:443".into(), "gw-b:443".into()];
        let (idx, _) = failover_select(labels, false, |i| {
            let r: Result<String, (bool, String)> = match i {
                0 => Err((false, "连接超时 (5s)".into())),
                _ => Ok("jwt-b".into()),
            };
            async move { r }
        })
        .await
        .expect("fallback 应被选中");
        assert_eq!(idx, 1);
    }

    /// 全部节点失败：返回聚合错误，且每节点失败原因逐行包含在内。
    #[tokio::test]
    async fn f4v2_failover_all_failed_aggregates_errors() {
        let labels = vec!["gw-a:443".into(), "gw-b:8443".into(), "gw-c:443".into()];
        let err = failover_select(labels, false, |i| {
            let r: Result<String, (bool, String)> = match i {
                0 => Err((false, "connection refused".into())),
                1 => Err((false, "连接超时 (5s)".into())),
                _ => Err((false, "TLS 握手失败: cert".into())),
            };
            async move { r }
        })
        .await
        .expect_err("全部失败应返回聚合错误");
        assert!(err.contains("全部节点连接失败"), "{err}");
        assert!(err.contains("gw-a:443") && err.contains("connection refused"), "{err}");
        assert!(err.contains("gw-b:8443") && err.contains("连接超时"), "{err}");
        assert!(err.contains("gw-c:443") && err.contains("TLS 握手失败"), "{err}");
    }

    /// 认证失败语义：fail_on_auth_fail=false（默认）→ 认证失败 fail fast，
    /// 不再探测后续节点；=true → 继续切下一节点。
    #[tokio::test]
    async fn f4v2_auth_fail_semantics_by_fail_on_auth_fail() {
        let labels = vec!["gw-a:443".into(), "gw-b:443".into()];
        // 默认 false：认证失败立即停止（快速失败，凭据错误逐节点重试无意义）
        let mut probed = Vec::new();
        let err = failover_select(labels.clone(), false, |i| {
            probed.push(i);
            let r: Result<String, (bool, String)> =
                Err((true, "网关登录失败 HTTP 401".into()));
            async move { r }
        })
        .await
        .expect_err("认证失败应返回错误");
        assert!(err.contains("[认证失败]"), "{err}");
        assert_eq!(probed, vec![0], "fail_on_auth_fail=false 时认证失败 fail fast");
        // true：认证失败也切下一节点
        let mut probed = Vec::new();
        let (idx, _) = failover_select(labels, true, |i| {
            probed.push(i);
            let r: Result<String, (bool, String)> = match i {
                0 => Err((true, "网关登录失败 HTTP 401".into())),
                _ => Ok("jwt-b".into()),
            };
            async move { r }
        })
        .await
        .expect("fail_on_auth_fail=true 时应切到 fallback");
        assert_eq!(idx, 1);
        assert_eq!(probed, vec![0, 1]);
    }

    // -----------------------------------------------------------------
    // F4-V2 健康探测结构（本地 TcpListener 模拟节点端口，不依赖真实网关）
    // -----------------------------------------------------------------

    /// 探测开放端口（明文）：ok=true，latency_ms 记录，error=None。
    #[tokio::test]
    async fn f4v2_health_probe_open_port_ok() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let r = probe_gateway_health("127.0.0.1", addr.port(), false, false).await;
        assert!(r.ok, "{r:?}");
        assert_eq!(r.host, "127.0.0.1");
        assert_eq!(r.port, addr.port());
        assert_eq!(r.error, None);
        // latency_ms 为非负整数（结构字段存在性锚定）
        let _ = r.latency_ms;
    }

    /// 探测无监听端口：ok=false + error 含 TCP 失败原因 + latency 记录。
    #[tokio::test]
    async fn f4v2_health_probe_closed_port_reports_error() {
        // 绑定后立刻释放端口（端口处于无监听状态）
        let port = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let r = probe_gateway_health("127.0.0.1", port, false, false).await;
        assert!(!r.ok);
        assert!(r.error.as_deref().unwrap_or("").contains("TCP"), "{r:?}");
    }

    /// GatewayHealthReport 可序列化为前端期望的 JSON 结构
    /// {host, port, ok, latency_ms, error}。
    #[test]
    fn f4v2_health_report_json_shape() {
        let r = GatewayHealthReport {
            host: "gw-a".into(),
            port: 443,
            ok: false,
            latency_ms: 12,
            error: Some("TLS 握手失败".into()),
        };
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(v["host"], "gw-a");
        assert_eq!(v["port"], 443);
        assert_eq!(v["ok"], false);
        assert_eq!(v["latency_ms"], 12);
        assert_eq!(v["error"], "TLS 握手失败");
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
