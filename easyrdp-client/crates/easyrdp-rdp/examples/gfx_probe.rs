//! 无头 GFX 诊断：只跑 GFX ON 一段，用于配合 WLOG_LEVEL 观察
//! drdynvc/rdpgfx 日志。凭据来自 connections.json（DPAPI），不打印明文。
use std::path::PathBuf;
use std::time::{Duration, Instant};

use easyrdp_core::profile::SessionProfile;
use easyrdp_rdp::{create_session, event_channel::FramebufferEvent, native_ready};

fn load_first_connection() -> Result<(SessionProfile, String), String> {
    let path = std::env::var("APPDATA")
        .map(|d| PathBuf::from(d).join("cn.easyrdp.client\\connections.json"))
        .map_err(|_| "APPDATA 环境变量不存在".to_string())?;
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    #[derive(serde::Deserialize)]
    struct Saved {
        #[serde(default)]
        profile: SessionProfile,
        #[serde(default)]
        password: String,
    }
    let saved: Saved = match serde_json::from_str::<Vec<Saved>>(&raw) {
        Ok(list) => list.into_iter().next().ok_or("connections.json 为空数组")?,
        Err(_) => serde_json::from_str::<Saved>(&raw)
            .map_err(|e| format!("connections.json 解析失败: {e}"))?,
    };
    Ok((saved.profile, saved.password))
}

fn main() {
    assert!(native_ready(), "必须以 --features native 构建");
    let (profile, stored_password) = load_first_connection().expect("连接存储读取失败");
    let password = easyrdp_core::secret::unprotect(&stored_password).expect("DPAPI 解密失败");
    let mut session = create_session();
    session.set_password(&password);
    session.connect(&profile).expect("connect 启动失败");
    let start = Instant::now();
    let mut frames = 0u64;
    if let Some(rx) = session.events().map(|r| r.clone_receiver()) {
        while start.elapsed() < Duration::from_secs(30) {
            match rx.try_recv() {
                Some(FramebufferEvent::Framebuffer { .. }) => frames += 1,
                Some(FramebufferEvent::Delta { .. }) => frames += 1,
                Some(FramebufferEvent::Disconnected(r)) => {
                    println!("[gfx-probe] Disconnected: {r}");
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }
    let _ = session.disconnect();
    println!("[gfx-probe] 30s 帧事件总数: {frames}");
}
