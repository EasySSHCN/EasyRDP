//! Windows 服务形态（方案 §5：`sc create` 注册、开机自启）。
//!
//! 服务名 `EasyRDPAgent`；SCM STOP/SHUTDOWN → 通知主循环优雅退出。
//! 调试直接用控制台模式（`easyrdp-agent`），服务模式入口 `--service`。

#[cfg(windows)]
pub mod imp {
    use std::ffi::OsString;

    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::{define_windows_service, service_dispatcher};

    pub const SERVICE_NAME: &str = "EasyRDPAgent";

    define_windows_service!(ffi_service_main, service_main);

    fn service_main(arguments: Vec<OsString>) {
        let config_path = arguments
            .iter()
            .position(|a| a == "--config")
            .and_then(|i| arguments.get(i + 1))
            .map(|s| s.to_string_lossy().into_owned());
        let config_path = config_path.unwrap_or_else(|| {
            // 服务无工作目录概念：默认 alongside exe 的 agent.toml，其次系统盘 ProgramData。
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                .unwrap_or_default();
            exe_dir.join("agent.toml").display().to_string()
        });
        if let Err(e) = run_service(config_path) {
            tracing::error!(%e, "service run failed");
        }
    }

    fn run_service(config_path: String) -> Result<(), String> {
        use std::sync::mpsc as std_mpsc;

        let (stop_tx, stop_rx) = std_mpsc::channel::<()>();
        let event_handler = move |event: ServiceControl| -> ServiceControlHandlerResult {
            match event {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    let _ = stop_tx.send(());
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };
        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)
            .map_err(|e| format!("注册服务控制处理器失败: {e}"))?;

        let set_status = |state: ServiceState, accept: ServiceControlAccept| -> Result<(), String> {
            status_handle
                .set_service_status(ServiceStatus {
                    service_type: ServiceType::OWN_PROCESS,
                    current_state: state,
                    controls_accepted: accept,
                    exit_code: ServiceExitCode::Win32(0),
                    checkpoint: 0,
                    wait_hint: std::time::Duration::from_secs(10),
                    process_id: None,
                })
                .map_err(|e| format!("set_service_status 失败: {e}"))
        };
        set_status(ServiceState::Running, ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN)?;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("tokio runtime 失败: {e}"))?;
        let cfg = crate::config::AgentConfig::load_from_path(std::path::Path::new(&config_path))
            .map_err(|e| e.to_string())?;
        let (tx, rx) = tokio::sync::mpsc::channel::<()>(1);
        // SCM STOP 事件（std channel）→ 桥接到异步 shutdown。
        let bridge = tokio::spawn(async move {
            let _ = tokio::task::spawn_blocking(move || {
                let _ = stop_rx.recv();
            })
            .await;
            let _ = tx.send(()).await;
        });
        runtime.block_on(crate::run_agent(cfg, rx));
        bridge.abort();

        set_status(ServiceState::Stopped, ServiceControlAccept::empty())?;
        Ok(())
    }

    /// 服务入口（`--service`）：交 SCM dispatcher，阻塞至服务停止。
    pub fn dispatch(_config_path: &str) -> Result<(), String> {
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
            .map_err(|e| format!("service dispatcher 启动失败: {e}"))
    }
}

#[cfg(windows)]
pub use imp::dispatch;

#[cfg(not(windows))]
pub fn dispatch(_config_path: &str) -> Result<(), String> {
    Err("服务模式仅支持 Windows（首版平台）".into())
}
