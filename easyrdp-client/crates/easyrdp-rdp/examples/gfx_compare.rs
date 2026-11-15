//! 无头 GFX A/B 对比实验：同一会话配置，GFX on/off 各连一次真实主机，
//! 各采样约 20 秒，统计帧事件数与增量字节。
//!
//! 运行：
//!   cargo run --release -p easyrdp-rdp --features native --example gfx_compare
//!
//! GFX 关闭通过 EASYRDP_DISABLE_GFX=1 环境变量（ffi.rs on_pre_connect 读取）。
//! 凭据来源与 e2e_connect 相同：connections.json DPAPI 解密，不打印明文。
use std::path::PathBuf;
use std::time::{Duration, Instant};

use easyrdp_core::profile::SessionProfile;
use easyrdp_rdp::{create_session, event_channel::FramebufferEvent, native_ready, RdpSession};

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

/// 采样统计。
#[derive(Default)]
struct Sample {
    fulls: u64,
    full_bytes: u64,
    deltas: u64,
    delta_rects: u64,
    delta_bytes: u64,
    first_frame: Option<Instant>,
}

fn run_once(label: &str, profile: &SessionProfile, password: &str, secs: u64) -> Sample {
    println!("\n[gfx-compare] === {label}（采样 {secs}s）===");
    let mut session = create_session();
    session.set_password(password);
    session.connect(profile).expect("connect 启动失败");
    let start = Instant::now();
    let deadline = start + Duration::from_secs(secs + 5);
    let mut s = Sample::default();
    if let Some(rx) = session.events().map(|r| r.clone_receiver()) {
        loop {
            if Instant::now() >= deadline {
                break;
            }
            match rx.try_recv() {
                Some(FramebufferEvent::Framebuffer { pixels, .. }) => {
                    s.fulls += 1;
                    s.full_bytes += pixels.len() as u64;
                    if s.first_frame.is_none() {
                        s.first_frame = Some(Instant::now());
                        println!(
                            "[gfx-compare] 首帧到达 t={:.2}s（{} bytes）",
                            s.first_frame.unwrap().duration_since(start).as_secs_f32(),
                            pixels.len()
                        );
                    }
                }
                Some(FramebufferEvent::Delta { rects, pixels, .. }) => {
                    s.deltas += 1;
                    s.delta_rects += rects.len() as u64;
                    s.delta_bytes += pixels.len() as u64;
                }
                Some(FramebufferEvent::Disconnected(reason)) => {
                    println!("[gfx-compare] Disconnected: {reason}");
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }
    let _ = session.disconnect();
    println!(
        "[gfx-compare] {label} 结果: 全帧 {} 次 / 增量 {} 包 / {} 矩形 / 增量像素 {:.2} MB",
        s.fulls,
        s.deltas,
        s.delta_rects,
        s.delta_bytes as f64 / 1024.0 / 1024.0
    );
    s
}

fn main() {
    assert!(native_ready(), "必须以 --features native 构建");
    let (profile, stored_password) = match load_first_connection() {
        Ok(x) => x,
        Err(e) => {
            eprintln!("[gfx-compare] {e}");
            std::process::exit(2);
        }
    };
    let password = easyrdp_core::secret::unprotect(&stored_password).expect("DPAPI 解密失败");
    println!(
        "[gfx-compare] host={} port={} 密码已解密注入（长度 {}，不回显）",
        profile.host,
        profile.port,
        password.len()
    );

    // A/B 顺序轮换（GFX on 先跑，随后 off），各 20s。
    let on = run_once("GFX ON", &profile, &password, 20);
    std::thread::sleep(Duration::from_secs(3));
    std::env::set_var("EASYRDP_DISABLE_GFX", "1");
    let off = run_once("GFX OFF", &profile, &password, 20);
    std::env::remove_var("EASYRDP_DISABLE_GFX");

    println!("\n========== GFX on/off 对比（各 20s 采样）==========");
    println!("指标             GFX ON          GFX OFF");
    println!(
        "全帧(JPEG)       {:<15} {}",
        on.fulls,
        off.fulls
    );
    println!(
        "全帧字节         {:.2} MB        {:.2} MB",
        on.full_bytes as f64 / 1024.0 / 1024.0,
        off.full_bytes as f64 / 1024.0 / 1024.0
    );
    println!(
        "增量包           {:<15} {}",
        on.deltas,
        off.deltas
    );
    println!(
        "增量矩形         {:<15} {}",
        on.delta_rects,
        off.delta_rects
    );
    println!(
        "增量字节         {:.2} MB        {:.2} MB",
        on.delta_bytes as f64 / 1024.0 / 1024.0,
        off.delta_bytes as f64 / 1024.0 / 1024.0
    );
    if on.deltas > 0 && off.deltas > 0 {
        println!(
            "增量均包         {:.1} KB          {:.1} KB",
            on.delta_bytes as f64 / on.deltas as f64 / 1024.0,
            off.delta_bytes as f64 / off.deltas as f64 / 1024.0
        );
    }
}
