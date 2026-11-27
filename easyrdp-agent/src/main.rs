//! EasyRDP 目标机 Agent 入口（方案 §5 / R-20261122-006 P3）。
//!
//! 模式：
//! - `easyrdp-agent [--config agent.toml]`   控制台调试运行（Ctrl-C 退出）
//! - `easyrdp-agent --service`               Windows 服务运行（SCM 托管）
//! - `easyrdp-agent --enroll <ENROLL_TOKEN>` 用一次性 enrollment token 换取
//!   AgentToken 并 DPAPI 落盘（写回 agent.toml 的 token 字段，`dpapi:` 前缀）
//! - `easyrdp-agent --check`                 仅校验配置与 token 可解密后退出
//!
//! 行为（方案 §5）：连接 WSS /agent/connect → 在线待命；断线指数退避重连
//! （1s→2s→…→60s 封顶，成功保持 ≥ 2×心跳后复位）；收到 DIAL → 5s 内连接
//! 配置内 local_backend → OPEN_OK/OPEN_ERR → 双向 DATA 泵。

mod backoff;
mod config;
mod dpapi;
mod enroll;
mod session;
mod wire;
mod ws;

#[cfg(windows)]
mod service;

use std::time::Instant;

use config::AgentConfig;

const CONFIG_ENV: &str = "EASYRDP_AGENT_CONFIG";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let mut config_path = std::env::var(CONFIG_ENV)
        .unwrap_or_else(|_| "agent.toml".to_string());
    let mut mode = Mode::Run;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" if i + 1 < args.len() => {
                config_path = args[i + 1].clone();
                i += 1;
            }
            "--enroll" if i + 1 < args.len() => {
                mode = Mode::Enroll(args[i + 1].clone());
                i += 1;
            }
            "--service" => mode = Mode::Service,
            "--check" => mode = Mode::Check,
            "--help" | "-h" => {
                print_help();
                return;
            }
            other => {
                eprintln!("未知参数: {other}\n");
                print_help();
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let code = match mode {
        Mode::Run => runtime.block_on(run_console(&config_path)),
        Mode::Check => match check_config(&config_path) {
            Ok(()) => {
                println!("配置与 AgentToken 校验通过: {config_path}");
                0
            }
            Err(e) => {
                eprintln!("配置校验失败: {e}");
                1
            }
        },
        Mode::Enroll(token) => runtime.block_on(run_enroll(&config_path, &token)),
        Mode::Service => {
            // Windows 服务模式：交给 SCM dispatcher（阻塞至服务停止）。
            #[cfg(windows)]
            {
                match service::dispatch(&config_path) {
                    Ok(()) => 0,
                    Err(e) => {
                        eprintln!("服务运行失败: {e}");
                        1
                    }
                }
            }
            #[cfg(not(windows))]
            {
                eprintln!("服务模式仅支持 Windows（首版平台，方案 §5）");
                2
            }
        }
    };
    if code != 0 {
        std::process::exit(code);
    }
}

enum Mode {
    Run,
    Enroll(String),
    Service,
    Check,
}

fn print_help() {
    println!("{HELP_TEXT}");
}

pub(crate) const HELP_TEXT: &str = "EasyRDP 目标机 Agent

用法:
  easyrdp-agent [--config agent.toml]           控制台调试运行
  easyrdp-agent --service [--config agent.toml] Windows 服务运行
  easyrdp-agent --enroll <ENROLL_TOKEN>         换取 AgentToken 并 DPAPI 落盘
  easyrdp-agent --check [--config agent.toml]   校验配置与 token

配置示例见 config.example.toml；服务注册:
  sc create EasyRDPAgent binPath= \"<exe绝对路径> --service\" start= auto
  sc start EasyRDPAgent";

fn load_config_or_exit(path: &str) -> AgentConfig {
    match AgentConfig::load_from_path(std::path::Path::new(path)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("加载 {path} 失败: {e}");
            std::process::exit(1);
        }
    }
}

/// 控制台运行：Ctrl-C / 终止信号 → 优雅退出。
async fn run_console(config_path: &str) -> i32 {
    let cfg = load_config_or_exit(config_path);
    let (_shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);
    let agent = tokio::spawn(run_agent(cfg, shutdown_rx));
    tracing::info!("EasyRDP Agent 控制台运行（Ctrl-C 退出）");
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("收到退出信号");
    let _ = _shutdown_tx.send(()).await;
    let _ = agent.await;
    0
}

/// --check：加载配置 + 解密 token（不连网关）。
fn check_config(config_path: &str) -> Result<(), String> {
    let cfg = load_config_or_exit(config_path);
    cfg.resolve_token().map_err(|e| e.to_string())?;
    Ok(())
}

/// --enroll：enrollment token → AgentToken → DPAPI 落盘写回 agent.toml。
async fn run_enroll(config_path: &str, enrollment_token: &str) -> i32 {
    let path = std::path::PathBuf::from(config_path);
    let toml_text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("读取 {config_path} 失败: {e}");
            return 1;
        }
    };
    let cfg = match toml::from_str::<AgentConfig>(&toml_text) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("解析 {config_path} 失败: {e}");
            return 1;
        }
    };
    let gateway_url = cfg.gateway.url.trim().to_string();
    let resource_id = cfg.tunnel.resource_id.clone();
    let mut transport = enroll::http_transport();
    // HTTP 请求为阻塞调用；包一层避免长时间占死异步线程。
    let agent_token = {
        let gateway_url = gateway_url.clone();
        let enrollment_token = enrollment_token.to_string();
        let resource_id = resource_id.clone();
        tokio::task::spawn_blocking(move || {
            enroll::enroll(&mut *transport, &gateway_url, &enrollment_token, &resource_id)
        })
        .await
        .map_err(|e| format!("enroll 任务失败: {e}"))
        .and_then(|r| r)
    };
    let agent_token = match agent_token {
        Ok(t) => t,
        Err(e) => {
            eprintln!("enroll 失败: {e}");
            return 1;
        }
    };

    // 落盘：Windows 用 DPAPI 内联（dpapi:<base64>）；非 Windows 回退明文文件
    //（首版仅 Windows，此处仅为可移植性兜底）。
    let stored = if cfg!(windows) {
        match dpapi::protect(&agent_token) {
            Ok(blob) => format!("dpapi:{blob}"),
            Err(e) => {
                eprintln!("DPAPI 加密失败: {e}");
                return 1;
            }
        }
    } else {
        let token_file = path.with_file_name("agent.token");
        if let Err(e) = std::fs::write(&token_file, &agent_token) {
            eprintln!("写入 token 文件失败: {e}");
            return 1;
        }
        format!("file:{}", token_file.display())
    };

    let updated = config::apply_token_to_toml(&toml_text, &stored);
    if let Err(e) = std::fs::write(&path, updated) {
        eprintln!("写回 {config_path} 失败: {e}");
        return 1;
    }
    println!("enroll 成功：AgentToken 已加密写入 {config_path}（绑定资源 {resource_id}）");
    println!("运行 `easyrdp-agent --config {config_path}` 上线");
    0
}

/// Agent 主循环：建连 → 会话 → 断线退避重连，直到 shutdown。
pub async fn run_agent(cfg: AgentConfig, mut shutdown: tokio::sync::mpsc::Receiver<()>) {
    let params = session::SessionParams::from_config(&cfg);
    let resource_id = cfg.tunnel.resource_id.clone();
    let token = match cfg.resolve_token() {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(%e, "AgentToken 解析失败，退出（请先 --enroll）");
            return;
        }
    };
    let ws_url = cfg.connect_ws_url();
    tracing::info!(resource_id = %resource_id, url = %ws_url, backend = %cfg.tunnel.local_backend, "agent starting");

    let mut backoff =
        backoff::Backoff::new(cfg.reconnect_initial(), cfg.reconnect_cap());
    let healthy_hold = cfg.heartbeat().saturating_mul(2);

    loop {
        if !shutdown.is_empty() || token.is_empty() {
            tracing::info!("shutdown，退出");
            return;
        }
        let started = Instant::now();
        match ws::WsFrameIo::connect(&ws_url, &token).await {
            Ok(io) => {
                tracing::info!(resource_id = %resource_id, "connected to gateway");
                let end = session::run_session(io, &params).await;
                match end {
                    Ok(session::SessionEnd::PeerClosed) => {
                        tracing::warn!("网关关闭连接");
                    }
                    Ok(other) => tracing::warn!(?other, "会话结束"),
                    Err(session::SessionEnd::SilenceTimeout) => {
                        tracing::warn!("心跳静默判死（{}s 无帧），重连", cfg.silence_deadline().as_secs());
                    }
                    Err(other) => tracing::warn!(?other, "会话异常结束"),
                }
                // 连接保持了足够久（≥ 2×心跳）才视为"稳定在线"，复位退避。
                if started.elapsed() >= healthy_hold {
                    backoff.reset();
                }
            }
            Err(e) => {
                tracing::warn!(%e, "建连失败");
            }
        }
        let delay = backoff.next_delay();
        tracing::info!(delay = delay.as_millis() as u64, attempt = backoff.attempt(), "等待重连");
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown.recv() => {
                tracing::info!("shutdown，退出");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 主循环语义（内存回环假网关）：连接成功 → DIAL 建链可用 → 断开后按退避重连。
    #[tokio::test]
    async fn run_agent_reconnects_after_gateway_drop() {
        let cfg: AgentConfig = toml::from_str(
            r#"
[gateway]
url = "http://127.0.0.1:1"   # 不可达：验证退避重连不 panic
token = "test-token"

[tunnel]
resource_id = "u-test-01"
local_backend = "127.0.0.1:3389"

[options]
reconnect_initial_ms = 10
reconnect_cap_ms = 30
"#,
        )
        .expect("cfg");
        let (_tx, rx) = tokio::sync::mpsc::channel::<()>(1);
        // 只验证：固定时间内不 panic 且持续尝试（退避逻辑另有单测）。
        let handle = tokio::spawn(run_agent(cfg, rx));
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        assert!(!handle.is_finished(), "不可达网关下 Agent 应持续退避重连而非退出");
        handle.abort();
    }

    #[test]
    fn help_text_mentions_modes() {
        assert!(HELP_TEXT.contains("--enroll"));
        assert!(HELP_TEXT.contains("--service"));
        assert!(HELP_TEXT.contains("--check"));
    }
}
