//! GHOST-DUMP 取证探针 v2（R-20261116-004）：复刻用户残影场景。
//! 动作序列：baseline → Win 开始菜单 → 键入 notepad 启动 → 拖动窗口 →
//! 双击标题栏最大化 → 各时刻快照 unique_tile 指标 + BMP。
use std::collections::{HashMap, HashSet};
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
/// v0.1.5 复测扩展：最后一批 delta 原始数据（suspect 全帧到达时 dump，
/// 用于判定撕裂源：delta 内容 vs 全帧快照 vs 呈现层）。
static LAST_DELTA: std::sync::Mutex<Option<(std::time::Instant, u32, u32, Vec<RectUpdate>, Vec<u8>)>> =
    std::sync::Mutex::new(None);

// ---------------------------------------------------------------------------
// GHOSTFIX-R020/021/022 呈现层乱序模拟器（2026-11）：
// 同一事件流喂两份合成缓冲：
//   ref    = 严格事件序（delta 即时应用）——无残影基准；
//   sim_old= 复刻 v0.2.0 pump 行为（delta 进 16ms 合并批、全帧即时上屏）
//            ——旧 delta 批在更晚时刻覆盖新全帧 = 残影机制本体。
// 每个全帧应用后比对 sim_old vs ref：差异 = 该机制在真实流量中产生的
// 「旧内容闪回」暴露。统计触发全帧数与差异像素数（按动作分列）。
// ---------------------------------------------------------------------------
struct PumpSim {
    width: u32,
    height: u32,
    buf: Vec<u8>,
    pending: Vec<(Instant, Vec<RectUpdate>, Vec<u8>)>,
}
static SIM_GHOST_FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SIM_GHOST_PIXELS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SIM_PER_ACTION: std::sync::Mutex<Option<HashMap<String, (u64, u64)>>> = std::sync::Mutex::new(None);

fn sim_bump(action: &str, frames: u64, pixels: u64) {
    let mut m = SIM_PER_ACTION.lock().unwrap_or_else(|e| e.into_inner());
    let e = m.get_or_insert_with(HashMap::new).entry(action.to_string()).or_insert((0, 0));
    e.0 += frames;
    e.1 += pixels;
}

impl PumpSim {
    fn ensure(&mut self, w: u32, h: u32) {
        if self.width != w || self.height != h {
            self.width = w; self.height = h;
            self.buf = vec![0u8; (w as usize) * (h as usize) * 4];
            self.pending.clear();
        }
    }
    fn flush_due(&mut self) {
        let now = Instant::now();
        let mut due = Vec::new();
        self.pending.retain(|(at, r, p)| {
            if *at <= now { due.push((r.clone(), p.clone())); false } else { true }
        });
        for (rects, pixels) in due {
            Self::apply(&mut self.buf, self.width, self.height, &rects, &pixels);
        }
    }
    fn apply(buf: &mut [u8], w: u32, h: u32, rects: &[RectUpdate], pixels: &[u8]) {
        let stride = w as usize * 4;
        let mut off = 0usize;
        for r in rects {
            let bytes = (r.w as usize) * (r.h as usize) * 4;
            if off + bytes > pixels.len() { break; }
            for row in 0..r.h as usize {
                let src = &pixels[off + row * r.w as usize * 4..off + (row + 1) * r.w as usize * 4];
                let dst = ((r.y as usize + row) * stride + r.x as usize * 4) as usize;
                if dst + src.len() <= buf.len() {
                    buf[dst..dst + src.len()].copy_from_slice(src);
                }
            }
            off += bytes;
        }
        let _ = h;
    }
    /// v0.2.0 旧行为：delta 入 16ms 延迟批（全帧不等待）。
    fn on_delta(&mut self, w: u32, h: u32, rects: &[RectUpdate], pixels: &[u8]) {
        self.ensure(w, h);
        self.pending.push((Instant::now() + Duration::from_millis(16), rects.to_vec(), pixels.to_vec()));
    }
    /// v0.2.0 旧行为：全帧即时应用；先冲刷到期 delta 批（模拟 pump 时序），
    /// 然后比对 ref（严格序）差异 → 量化残影暴露。
    fn on_full(&mut self, action: &str, ref_buf: &[u8], w: u32, h: u32, bgra: &[u8]) {
        self.ensure(w, h);
        self.flush_due();
        let need = (w as usize) * (h as usize) * 4;
        if self.buf.len() >= need && ref_buf.len() >= need {
            let mut diff = 0u64;
            for i in (0..need).step_by(4) {
                if self.buf[i..i + 3] != ref_buf[i..i + 3] { diff += 1; }
            }
            if diff > 0 {
                SIM_GHOST_FRAMES.fetch_add(1, Ordering::SeqCst);
                SIM_GHOST_PIXELS.fetch_add(diff, Ordering::SeqCst);
                sim_bump(action, 1, diff);
            }
        }
        self.buf.copy_from_slice(&bgra[..need]);
    }
}

impl Compositor {
    fn ensure(&mut self, w: u32, h: u32) {
        if self.width != w || self.height != h {
            self.width = w; self.height = h;
            self.buf = vec![0u8; (w as usize) * (h as usize) * 4];
        }
    }
    /// 全帧应用 + 平铺检测：unique<40% 且非黑帧 → 存 suspect-N.bmp。
    /// 返回是否捕获 suspect（v0.1.5 复测扩展：供主循环联动 dump 最后一批
    /// delta 原始数据，区分撕裂帧来自引擎 delta/全帧流还是呈现层）。
    fn apply_full_bgra_checked(&mut self, w: u32, h: u32, bgra: &[u8], dump_dir: &std::path::Path, tag: &str) -> bool {
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
        flat
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
    let mut sim = PumpSim { width: 0, height: 0, buf: Vec::new(), pending: Vec::new() };
    let mut connected = false;
    let mut phase = 0u8;
    let mut phase_at = Instant::now();
    let start = Instant::now();
    let deadline = start + Duration::from_secs(200);
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
                        // 呈现层乱序模拟：全帧到达瞬间，旧行为 sim 缓冲若与
                        // 严格序 ref 缓冲有差异 = 旧 delta 覆盖新全帧的残影暴露
                        sim.on_full(action, &comp.buf, width, height, &pixels);
                        let suspect = comp.apply_full_bgra_checked(width, height, &pixels, &dump_dir, action);
                        if suspect {
                            // v0.1.5 复测扩展：联动 dump 最后一批 delta 原始数据
                            let ld = LAST_DELTA.lock().unwrap_or_else(|e| e.into_inner()).take();
                            if let Some((at, dw, dh, rects, pix)) = ld {
                                let n = SUSPECT.load(Ordering::SeqCst);
                                let age = at.elapsed().as_millis();
                                let json = format!(
                                    "{{\"age_ms_before_suspect\":{},\"width\":{},\"height\":{},\"rects\":{},\"pixels_bytes\":{},\"rect_list\":{}}}",
                                    age, dw, dh, rects.len(), pix.len(),
                                    serde_json::to_string(&rects.iter().map(|r| serde_json::json!({"x":r.x,"y":r.y,"w":r.w,"h":r.h})).collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into())
                                );
                                let _ = std::fs::write(dump_dir.join(format!("suspect-{}-{:03}-delta.json", action, n)), &json);
                                let _ = std::fs::write(dump_dir.join(format!("suspect-{}-{:03}-delta.bin", action, n)), &pix);
                                println!("[ghost] 已联动 dump 最后 delta 批（suspect 前 {}ms，rects={}）", age, rects.len());
                            } else {
                                println!("[ghost] suspect 无前置 delta（撕裂帧来自全帧快照路径）");
                            }
                        }
                        connected = true;
                    }
                    FramebufferEvent::Delta { width, height, rects, pixels } => {
                        comp.ensure(width, height);
                        comp.apply_delta(&rects, &pixels);
                        sim.on_delta(width, height, &rects, &pixels);
                        if pixels.len() <= 8 * 1024 * 1024 {
                            *LAST_DELTA.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some((Instant::now(), width, height, rects.to_vec(), pixels.to_vec()));
                        }
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
                        // R-20261116-022 场景：最大化后远端窗口右上三按钮
                        //（最小化/最大化/关闭）区域高频悬停扫掠（按钮高亮态
                        // 小面积局部重绘 = 残影高发区）。
                        action = "hover";
                        sched = vec![(800, "hover-800ms"), (2000, "hover-2000ms")];
                        done.clear(); phase = 5; phase_at = Instant::now();
                        println!("[ghost] >>> 悬停扫掠右上按钮区 (1080..1270, y=10)");
                        let _ = session.send_input(&InputEvent::MouseMove { x: 1080, y: 10 });
                        std::thread::sleep(Duration::from_millis(150));
                        for i in 0..12usize {
                            let mx = 1080 + (i % 2) * 190 + i * 4;
                            let _ = session.send_input(&InputEvent::MouseMove { x: (mx.min(1270)) as u32, y: 10 });
                            std::thread::sleep(Duration::from_millis(90));
                        }
                        for i in 0..12usize {
                            let mx = 1270 - (i % 2) * 190 - i * 4;
                            let _ = session.send_input(&InputEvent::MouseMove { x: (mx.max(1080)) as u32, y: 10 });
                            std::thread::sleep(Duration::from_millis(90));
                        }
                    }
                }
                5 => {
                    if all_done && phase_at.elapsed() >= Duration::from_millis(2600) {
                        println!("[ghost] ===== 取证完成 =====");
                        println!("[ghost] 平铺检测统计: 全帧总数={} 疑似平铺={} (比率 {:.1}%)",
                            TOTAL_FULL.load(Ordering::SeqCst), SUSPECT.load(Ordering::SeqCst),
                            if TOTAL_FULL.load(Ordering::SeqCst) > 0 { SUSPECT.load(Ordering::SeqCst) as f64 / TOTAL_FULL.load(Ordering::SeqCst) as f64 * 100.0 } else { 0.0 });
                        println!("[ghost] 判据: 任一动作快照 unique 显著低于 baseline（如 65%→<30%）= 合成缓冲损坏(上游);否则呈现层。");
                        println!("[ghost] 快照: {}", dump_dir.display());
                        // 呈现层乱序模拟统计（GHOSTFIX-R020/021/022 取证）：
                        let gf = SIM_GHOST_FRAMES.load(Ordering::SeqCst);
                        let gp = SIM_GHOST_PIXELS.load(Ordering::SeqCst);
                        println!("[ghost] [pump-sim] v0.2.0 旧行为(16ms Delta 批)残影暴露: 残影全帧={} 差异像素={}（修复后该机制被 NativeFrame 前冲刷消除）", gf, gp);
                        let m = SIM_PER_ACTION.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(map) = m.as_ref() {
                            let mut rows: Vec<_> = map.iter().collect();
                            rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
                            for (k, (f, p)) in rows {
                                println!("[ghost] [pump-sim]   {k}: 残影全帧={f} 差异像素={p}");
                            }
                        }
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
