//! GHOST-DUMP 取证探针 v2（R-20261116-004）：复刻用户残影场景。
//! 动作序列：baseline → Win 开始菜单 → 键入 notepad 启动 → 拖动窗口 →
//! 双击标题栏最大化 → 各时刻快照 unique_tile 指标 + BMP。
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use easyrdp_core::profile::SessionProfile;
use easyrdp_rdp::{create_session, event_channel::FramebufferEvent, native_ready, InputEvent, RectUpdate, RdpSession};

fn load_first_connection() -> Result<(SessionProfile, String), String> {
    let path = std::env::var("APPDATA")
        .map(|d| PathBuf::from(d).join("cn.easyrdp.client\\connections.json"))
        .map_err(|_| "APPDATA 不存在".to_string())?;
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("读取失败: {e}"))?;
    #[derive(serde::Deserialize)]
    struct Saved {
        #[serde(default)]
        profile: SessionProfile,
        #[serde(default)]
        password: String,
    }
    let saved: Saved = match serde_json::from_str::<Vec<Saved>>(&raw) {
        Ok(list) => list.into_iter().next().ok_or("空")?,
        Err(_) => serde_json::from_str::<Saved>(&raw).map_err(|e| format!("{e}"))?,
    };
    Ok((saved.profile, saved.password))
}

struct Compositor { width: u32, height: u32, buf: Vec<u8> }
// 事件驱动平铺检测统计（每个全帧都检，一帧不漏）
static SUSPECT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static TOTAL_FULL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Compositor {
    fn ensure(&mut self, w: u32, h: u32) {
        if self.width != w || self.height != h {
            self.width = w; self.height = h;
            self.buf = vec![0u8; (w as usize) * (h as usize) * 4];
        }
    }
    /// 全帧应用 + 平铺检测：unique<40% 且非黑帧 → 存 suspect-N.bmp
    fn apply_full_bgra_checked(&mut self, w: u32, h: u32, bgra: &[u8], dump_dir: &std::path::Path, tag: &str) {
        let need = (w as usize) * (h as usize) * 4;
        let valid = bgra.len() >= need;
        // 平均亮度（排除未初始化黑帧误报）
        let mut sum = 0u64;
        if valid {
            for i in (0..need).step_by(4 * 97) { // 隔 97 像素采样
                let p = &bgra[i..i + 3];
                sum += (p[0] as u64 + p[1] as u64 + p[2] as u64) / 3;
            }
        }
        let samples = if valid { (need / (4 * 97)).max(1) } else { 1 };
        let avg = sum / samples as u64;
        let mut flat = false;
        if valid && avg > 10 {
            TOTAL_FULL.fetch_add(1, Ordering::SeqCst);
            self.ensure(w, h);
            let (u, t) = unique_tile_ratio(bgra, w, h, 64);
            let pct = if t > 0 { u as f64 / t as f64 * 100.0 } else { 0.0 };
            if pct < 40.0 {
                let n = SUSPECT.fetch_add(1, Ordering::SeqCst) + 1;
                flat = true;
                let p = dump_dir.join(format!("suspect-{tag}-{n:03}.bmp"));
                let _ = write_bmp(&p, w, h, bgra);
                println!("[ghost] ⚠ 疑似平铺帧捕获 {tag}#{n}: unique={u}/{t}={pct:.1}% 已存盘");
            }
        }
        if valid {
            self.ensure(w, h);
            self.buf.copy_from_slice(&bgra[..need]);
        }
        let _ = flat;
    }
    fn apply_delta(&mut self, rects: &[RectUpdate], pixels: &[u8]) {
        let stride = self.width as usize * 4;
        let mut off = 0usize;
        for r in rects {
            let bytes = (r.w as usize) * (r.h as usize) * 4;
            if off + bytes > pixels.len() { break; }
            for row in 0..r.h as usize {
                let src = &pixels[off + row * r.w as usize * 4..off + (row + 1) * r.w as usize * 4];
                let dst = ((r.y as usize + row) * stride + r.x as usize * 4) as usize;
                if dst + src.len() <= self.buf.len() {
                    self.buf[dst..dst + src.len()].copy_from_slice(src);
                }
            }
            off += bytes;
        }
    }
}

fn unique_tile_ratio(buf: &[u8], w: u32, h: u32, tile: usize) -> (usize, usize) {
    let stride = w as usize * 4;
    let mut set = HashSet::new();
    let mut total = 0usize;
    for ty in 0..(h as usize / tile) {
        for tx in 0..(w as usize / tile) {
            let mut hin = Vec::with_capacity(tile * 8 * 4);
            for r in 0..8usize {
                let y = ty * tile + r;
                let x0 = tx * tile * 4;
                hin.extend_from_slice(&buf[y * stride + x0..y * stride + x0 + tile * 4]);
            }
            set.insert(hin);
            total += 1;
        }
    }
    (set.len(), total)
}

fn write_bmp(path: &std::path::Path, width: u32, height: u32, bgra: &[u8]) -> Result<(), String> {
    let expected = width as usize * height as usize * 4;
    if bgra.len() != expected { return Err(format!("尺寸不符 {expected} vs {}", bgra.len())); }
    let stride = width as usize * 4;
    let mut bmp = Vec::with_capacity(54 + expected);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((54u32 + expected as u32)).to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(width as i32).to_le_bytes());
    bmp.extend_from_slice(&(height as i32).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&(expected as u32).to_le_bytes());
    bmp.extend_from_slice(&[0u8; 16]);
    for row in (0..height as usize).rev() {
        bmp.extend_from_slice(&bgra[row * stride..(row + 1) * stride]);
    }
    std::fs::write(path, &bmp).map_err(|e| format!("{e}"))
}

fn key(s: &mut dyn RdpSession, vk: u32) {
    let code = format!("VK:0x{vk:02X}");
    let _ = s.send_input(&InputEvent::Key { code: code.clone(), down: true });
    std::thread::sleep(Duration::from_millis(40));
    let _ = s.send_input(&InputEvent::Key { code, down: false });
}

fn main() {
    assert!(native_ready(), "需要 --features native");
    let (profile, stored_password) = match load_first_connection() {
        Ok(x) => x, Err(e) => { eprintln!("[ghost] {e}"); std::process::exit(2); }
    };
    let dump_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..\\..\\frames-debug\\ghost-dump");
    std::fs::create_dir_all(&dump_dir).expect("目录");
    println!("[ghost] 连接 {}:{} '{}'", profile.host, profile.port, profile.username);
    let password = if let Ok(p) = std::env::var("EASYRDP_TEST_PASSWORD") { p }
        else { match easyrdp_core::secret::unprotect(&stored_password) { Ok(p) if !p.is_empty() => p, _ => { eprintln!("[ghost] 密码不可用"); std::process::exit(3); } } };

    let mut session = create_session();
    session.set_password(&password);
    session.connect(&profile).expect("connect");
    println!("[ghost] connect() 已返回");

    let mut comp = Compositor { width: 0, height: 0, buf: Vec::new() };
    let mut connected = false;
    let mut phase = 0u8;
    let mut phase_at = Instant::now();
    let start = Instant::now();
    let deadline = start + Duration::from_secs(170);
    let rx = session.events().map(|r| r.clone_receiver());

    macro_rules! snap {
        ($name:expr) => {{
            let p = dump_dir.join(format!("{}.bmp", $name));
            match write_bmp(&p, comp.width, comp.height, &comp.buf) {
                Ok(()) => {
                    let (u, t) = unique_tile_ratio(&comp.buf, comp.width, comp.height, 64);
                    let pct = if t > 0 { u as f64 / t as f64 * 100.0 } else { 0.0 };
                    println!("[ghost] {}.bmp  {}x{}  unique={}/{}={:.1}%", $name, comp.width, comp.height, u, t, pct);
                }
                Err(e) => println!("[ghost] {} 失败: {}", $name, e),
            }
        }};
    }
    // 已拍快照标记（防重复）
    let mut done: HashSet<&'static str> = HashSet::new();
    let mut sched: Vec<(u64, &'static str)> = Vec::new(); // (相对 phase 开始 ms, 名称)
    let mut action: &str = "";

    if let Some(rx) = &rx {
        loop {
            if Instant::now() >= deadline { println!("[ghost] 超时 phase={phase}"); break; }
            loop {
                let Some(ev) = rx.try_recv() else { break };
                match ev {
                    FramebufferEvent::NativeFrame { width, height, pixels } => {
                        comp.apply_full_bgra_checked(width, height, &pixels, &dump_dir, action);
                        connected = true;
                    }
                    FramebufferEvent::Delta { width, height, rects, pixels } => {
                        comp.ensure(width, height);
                        comp.apply_delta(&rects, &pixels);
                        connected = true;
                    }
                    FramebufferEvent::Disconnected(reason) => {
                        println!("[ghost] 断开: {reason}"); std::process::exit(4);
                    }
                    _ => {}
                }
            }
            if !connected { std::thread::sleep(Duration::from_millis(50)); continue; }

            // 快照调度执行
            for (ms, name) in &sched {
                if !done.contains(name) && phase_at.elapsed().as_millis() >= *ms as u128 {
                    snap!(name);
                    done.insert(*name);
                }
            }
            // 阶段推进（调度全部完成后进入下一阶段）
            let all_done = sched.iter().all(|(_, n)| done.contains(n));

            match phase {
                0 => {
                    if start.elapsed() >= Duration::from_secs(20) {
                        snap!("baseline");
                        action = "win-menu";
                        sched = vec![(400, "menu-400ms"), (1200, "menu-1200ms")];
                        done.clear(); phase = 1; phase_at = Instant::now();
                        println!("[ghost] >>> Win 键（开始菜单）");
                        key(&mut *session, 0x5B);
                    }
                }
                1 => {
                    if all_done && phase_at.elapsed() >= Duration::from_millis(1500) {
                        // 键入 notepad + Enter
                        for c in "notepad".chars() {
                            if c.is_ascii_lowercase() { key(&mut *session, c as u32 - 32); std::thread::sleep(Duration::from_millis(60)); }
                        }
                        key(&mut *session, 0x0D);
                        action = "notepad-open";
                        sched = vec![(800, "np-800ms"), (2000, "np-2000ms"), (3500, "np-3500ms")];
                        done.clear(); phase = 2; phase_at = Instant::now();
                        println!("[ghost] >>> 键入 notepad+Enter");
                    }
                }
                2 => {
                    if all_done && phase_at.elapsed() >= Duration::from_millis(4000) {
                        action = "drag";
                        sched = vec![(200, "drag-200ms"), (600, "drag-600ms"), (1200, "drag-1200ms"), (2000, "drag-2000ms")];
                        done.clear(); phase = 3; phase_at = Instant::now();
                        // 记事本标题栏拖动：窗口默认居中偏上，标题栏约 y=125；从 (683,125) 拖到 (400,300)
                        println!("[ghost] >>> 拖动记事本窗口 (683,125)→(400,300)");
                        let _ = session.send_input(&InputEvent::MouseMove { x: 683, y: 125 });
                        std::thread::sleep(Duration::from_millis(200));
                        let _ = session.send_input(&InputEvent::MouseButton { button: "left".into(), down: true, x: 683, y: 125 });
                        std::thread::sleep(Duration::from_millis(150));
                        for (mx, my) in [(600,180),(520,240),(450,300),(400,300)] {
                            let _ = session.send_input(&InputEvent::MouseMove { x: mx, y: my });
                            std::thread::sleep(Duration::from_millis(150));
                        }
                        let _ = session.send_input(&InputEvent::MouseButton { button: "left".into(), down: false, x: 400, y: 300 });
                    }
                }
                3 => {
                    if all_done && phase_at.elapsed() >= Duration::from_millis(1500) {
                        action = "maximize";
                        sched = vec![(300, "max-300ms"), (800, "max-800ms"), (1600, "max-1600ms")];
                        done.clear(); phase = 4; phase_at = Instant::now();
                        // 双击标题栏 = 最大化（窗口现位置未知，双击 (400,~155) 附近即拖后标题栏）
                        println!("[ghost] >>> 双击标题栏最大化 (400,155)");
                        for _ in 0..2 {
                            let _ = session.send_input(&InputEvent::MouseButton { button: "left".into(), down: true, x: 400, y: 155 });
                            std::thread::sleep(Duration::from_millis(60));
                            let _ = session.send_input(&InputEvent::MouseButton { button: "left".into(), down: false, x: 400, y: 155 });
                            std::thread::sleep(Duration::from_millis(120));
                        }
                    }
                }
                4 => {
                    if all_done && phase_at.elapsed() >= Duration::from_millis(2200) {
                        println!("[ghost] ===== 取证完成 =====");
                        println!("[ghost] 平铺检测统计: 全帧总数={} 疑似平铺={} (比率 {:.1}%)",
                            TOTAL_FULL.load(Ordering::SeqCst), SUSPECT.load(Ordering::SeqCst),
                            if TOTAL_FULL.load(Ordering::SeqCst) > 0 { SUSPECT.load(Ordering::SeqCst) as f64 / TOTAL_FULL.load(Ordering::SeqCst) as f64 * 100.0 } else { 0.0 });
                        println!("[ghost] 判据: 任一动作快照 unique 显著低于 baseline（如 65%→<30%）= 合成缓冲损坏(上游);否则呈现层。");
                        println!("[ghost] 快照: {}", dump_dir.display());
                        std::process::exit(0);
                    }
                }
                _ => {}
            }
            let _ = action;
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
