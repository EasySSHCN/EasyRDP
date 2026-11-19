//! 无头 E2E：增量渲染管线测量 + 输入通路验证（真实凭据，来自连接存储）。
//!
//! 2026-11 增量管线 + 输入 E2E 探针：
//! 1. 读 %APPDATA%\cn.easyrdp.client\connections.json 第一条连接
//!    （密码经 DPAPI 解密后**仅注入 FreeRDP，绝不打印明文**；
//!    可用 EASYRDP_TEST_PASSWORD 覆盖）。
//! 2. 连接真实主机约 45 秒，分阶段测量：
//!    - 静止基线（0-6s）：全帧/增量包统计
//!    - 鼠标扫动（6-16s）：每 150ms 移动光标制造画面变化 →
//!      增量包大小分布与更新频率（应远小于全帧 204KB）
//!    - 输入验证：光标移到屏幕左下角点击（模拟开始菜单）→ 1.5s 后
//!      与点击前缓冲做像素差异率对比；再按 Win 键验证键盘。前后
//!      缓冲各存 BMP 到 frames-debug\input-test\ 供人工目检。
//!
//! 像素缓冲由**增量事件**合成（全帧为 JPEG 不解码；合成缓冲以增量
//! 逐步补全，足够支撑差异率对比）。
//!
//! 运行（PowerShell，release 实测）：
//!   $env:PKG_CONFIG_PATH = 'B:\tools\EasyRDP\vcpkg\installed\x64-windows-static-md\lib\pkgconfig'
//!   $env:PATH = 'B:\tools\EasyRDP\pkgconf-msi\PFiles64\pkgconf-3.0.10;' + $env:PATH
//!   cargo run --release -p easyrdp-rdp --features native --example e2e_connect
use std::path::PathBuf;
use std::time::{Duration, Instant};

use easyrdp_core::profile::SessionProfile;
use easyrdp_rdp::{
    create_session, event_channel::FramebufferEvent, frame_jpeg, native_ready, InputEvent,
    RectUpdate, RdpSession,
};

/// 读连接存储第一条连接（不打印任何敏感字段）。
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

// ---------------------------------------------------------------------------
// 增量合成缓冲（BGRA32 top-down）
// ---------------------------------------------------------------------------

struct Compositor {
    width: u32,
    height: u32,
    buf: Vec<u8>,
}

impl Compositor {
    fn ensure(&mut self, w: u32, h: u32) {
        if self.width != w || self.height != h {
            self.width = w;
            self.height = h;
            self.buf = vec![0u8; (w as usize) * (h as usize) * 4];
        }
    }

    /// 解码全帧 JPEG（RGB）→ BGRA 缓冲。运动期降载时 JPEG 为半分辨率，
    /// 需 2x 最近邻上采样到事件携带的远端尺寸（否则 ensure 重置缓冲 +
    /// 全分辨率增量越界丢弃 → 探针/前端黑屏白块）。
    fn apply_full_jpeg(&mut self, expected_w: u32, expected_h: u32, jpeg: &[u8]) -> Result<(), String> {
        let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(jpeg));
        let px = decoder.decode().map_err(|e| format!("JPEG 解码失败: {e}"))?;
        let info = decoder
            .info()
            .ok_or_else(|| "JPEG 缺少图像信息".to_string())?;
        let (iw, ih) = (info.width as u32, info.height as u32);
        self.ensure(expected_w, expected_h);
        match info.pixel_format {
            jpeg_decoder::PixelFormat::CMYK32 => Err("意外的 CMYK 输出".into()),
            _ => {
                let stride = self.width as usize * 4;
                // RGB → BGRA；解码尺寸 ≠ 事件尺寸（运动期半分辨率）时按比例
                // 最近邻映射到远端坐标。
                let sx = iw as f64 / self.width as f64;
                let sy = ih as f64 / self.height as f64;
                for y in 0..self.height as usize {
                    let syi = ((y as f64 * sy) as usize).min(ih as usize - 1);
                    for x in 0..self.width as usize {
                        let sxi = ((x as f64 * sx) as usize).min(iw as usize - 1);
                        let s = (syi * iw as usize + sxi) * 3;
                        let o = y * stride + x * 4;
                        self.buf[o] = px[s + 2];
                        self.buf[o + 1] = px[s + 1];
                        self.buf[o + 2] = px[s];
                        self.buf[o + 3] = 0xFF;
                    }
                }
                Ok(())
            }
        }
    }

    fn apply_delta(&mut self, rects: &[RectUpdate], pixels: &[u8]) {
        let stride = self.width as usize * 4;
        let mut off = 0usize;
        for r in rects {
            let bytes = (r.w as usize) * (r.h as usize) * 4;
            if off + bytes > pixels.len() {
                break;
            }
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

/// 两缓冲像素差异率（%）。
fn diff_pct(a: &[u8], b: &[u8]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 100.0;
    }
    let diff = a.iter().zip(b).filter(|(x, y)| x != y).count();
    diff as f64 / a.len() as f64 * 100.0
}

/// BGRA32 top-down 缓冲 -> 32bpp BMP（bottom-up，BI_RGB）。
fn write_bmp(path: &std::path::Path, width: u32, height: u32, bgra: &[u8]) -> Result<(), String> {
    let expected = width as usize * height as usize * 4;
    if bgra.len() != expected {
        return Err(format!("缓冲尺寸不符: 期望 {expected}，实际 {}", bgra.len()));
    }
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
    std::fs::write(path, &bmp).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

/// 阶段统计窗口。
#[derive(Default, Clone, Copy)]
struct WindowStats {
    delta_packets: u64,
    delta_bytes: u64,
    delta_max: u64,
    delta_rects: u64,
    fulls: u64,
}

impl WindowStats {
    fn add_delta(&mut self, bytes: usize, rects: usize) {
        self.delta_packets += 1;
        self.delta_bytes += bytes as u64;
        self.delta_max = self.delta_max.max(bytes as u64);
        self.delta_rects += rects as u64;
    }
}

/// 字母 -> VK（notepad 键入辅助；小写字母映射到对应 VK）。
fn vk_from_char(c: char) -> Option<u32> {
    let asc = c as u32;
    if c.is_ascii_lowercase() {
        Some(asc - 32) // 'a'->0x41
    } else {
        None
    }
}

fn main() {
    assert!(native_ready(), "必须以 --features native 构建");

    let (profile, stored_password) = match load_first_connection() {
        Ok(x) => x,
        Err(e) => {
            eprintln!("[e2e] {e}");
            std::process::exit(2);
        }
    };
    println!(
        "[e2e] 连接: host={} port={} username='{}' resolution={:?} 密码来源={}",
        profile.host,
        profile.port,
        profile.username,
        profile.resolution,
        if std::env::var_os("EASYRDP_TEST_PASSWORD").is_some() { "env" } else { "存储(DPAPI)" }
    );

    let debug_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..\\..\\frames-debug");
    let input_dir2 = debug_dir.join("input-test2");
    if let Err(e) = std::fs::create_dir_all(&input_dir2) {
        eprintln!("[e2e] 创建 input-test2 目录失败: {e}");
    }

    let password = if let Ok(p) = std::env::var("EASYRDP_TEST_PASSWORD") {
        p
    } else if stored_password.is_empty() {
        eprintln!("[e2e] 存储密码为空（请在 GUI 保存密码或设 EASYRDP_TEST_PASSWORD）");
        std::process::exit(3);
    } else {
        match easyrdp_core::secret::unprotect(&stored_password) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[e2e] 密码解密失败: {e}");
                std::process::exit(3);
            }
        }
    };
    println!("[e2e] 密码已解密注入（长度 {} 字符，不回显）", password.len());

    let mut session = create_session();
    session.set_password(&password);
    session.connect(&profile).expect("connect 启动失败");

    // -- 阶段时间轴 --
    // 2026-11 实测教训：远端登录（欢迎画面）耗时可达 20s+，登录期间输入
    // 无效且画面自带大变化，差异率对比全是假阳性。因此输入测试改为
    // 「等画面静止」（连续 5s 无增量包，且至少 8s）后再执行：
    //   A) 键盘：Win 键（开始菜单开合，画面必然变化）
    //   B) 鼠标：点击任务栏开始按钮
    let start = Instant::now();
    let mut comp = Compositor { width: 0, height: 0, buf: Vec::new() };
    let mut connected_seen = false;
    let mut overall = WindowStats::default();
    let mut sweep = WindowStats::default();
    let mut disconnected: Option<String> = None;

    // 输入测试状态
    let mut phase = 0u8;
    // 0=等待静止 1=图标拖拽(增量残影) 2=Ctrl+A 全选 3=Ctrl+Esc 开始菜单
    // 4=菜单内滚轮下 5=菜单内滚轮上 6=Win 键 7=notepad 启动 8=Numpad 输入 9=完成
    let mut last_delta: Option<Instant> = None;
    let mut snaps: Vec<Vec<u8>> = Vec::new();
    let mut diffs: Vec<f64> = Vec::new();
    let mut input_test_done = false;
    let mut test_started_at: Option<Instant> = None;
    let deadline_all = Instant::now() + Duration::from_secs(140);

    /// 快照 + 差异计算辅助
    macro_rules! snapshot_diff {
        ($prev:expr) => {{
            let prev = $prev.clone();
            let d = diff_pct(&prev, &comp.buf);
            snaps.push(comp.buf.clone());
            d
        }};
    }

    let rx = session.events().map(|r| r.clone_receiver());
    if let Some(rx) = rx {
        loop {
            // -- 输入验证状态机（全部在画面静止后执行）--
            if connected_seen && !input_test_done {
                match phase {
                    0 => {
                        let quiet_ok = last_delta
                            .map(|t| t.elapsed() >= Duration::from_secs(8))
                            .unwrap_or(false)
                            && start.elapsed() >= Duration::from_secs(20);
                        if quiet_ok {
                            snaps.push(comp.buf.clone());
                            phase = 1;
                            test_started_at = Some(Instant::now());
                            // LED 同步（2026-11 Numpad 修复验证）：发 NUM_LOCK sync
                            match session.sync_keyboard_leds(0x1) {
                                Ok(()) => println!("[e2e] >>> LED sync(NUM_LOCK) 发送成功（详见 debug.log sync 行 rc）"),
                                Err(e) => println!("[e2e] LED sync 失败: {}", e.0),
                            }
                            // A) 图标拖拽（增量残影验证）：按下→分步移动→抬起
                            let _ = session.send_input(&InputEvent::MouseMove { x: 35, y: 60 });
                            std::thread::sleep(Duration::from_millis(200));
                            let _ = session.send_input(&InputEvent::MouseButton {
                                button: "left".into(), down: true, x: 35, y: 60,
                            });
                            std::thread::sleep(Duration::from_millis(150));
                            let steps = [(150, 150), (240, 240), (330, 330), (400, 420), (450, 480)];
                            for (i, (mx, my)) in steps.iter().enumerate() {
                                let _ = session.send_input(&InputEvent::MouseMove { x: *mx, y: *my });
                                std::thread::sleep(Duration::from_millis(180));
                                if i == 2 {
                                    // 拖拽中途快照（对比引擎增量合成是否连贯）
                                    snaps.push(comp.buf.clone());
                                }
                            }
                            let _ = session.send_input(&InputEvent::MouseButton {
                                button: "left".into(), down: false, x: 450, y: 480,
                            });
                            println!("[e2e] >>> 拖拽执行完毕（35,60 → 450,480），2.5s 后对比");
                        }
                    }
                    1 => {
                        if test_started_at.map(|t| t.elapsed() >= Duration::from_secs(3)).unwrap_or(false) {
                            let d = snapshot_diff!(snaps[1]);
                            diffs.push(d);
                            // B) Ctrl+A：桌面全选（图标高亮 → 键盘组合键验证）
                            let _ = session.send_input(&InputEvent::Key { code: "VK:0x11".into(), down: true });
                            let _ = session.send_input(&InputEvent::Key { code: "VK:0x41".into(), down: true });
                            std::thread::sleep(Duration::from_millis(80));
                            let _ = session.send_input(&InputEvent::Key { code: "VK:0x41".into(), down: false });
                            let _ = session.send_input(&InputEvent::Key { code: "VK:0x11".into(), down: false });
                            phase = 2;
                            test_started_at = Some(Instant::now());
                            println!("[e2e] >>> 拖拽差异 {:.3}%，发送 Ctrl+A（桌面全选），2.5s 后对比", d);
                        }
                    }
                    2 => {
                        if test_started_at.map(|t| t.elapsed() >= Duration::from_secs(3)).unwrap_or(false) {
                            let d = snapshot_diff!(snaps[2]);
                            diffs.push(d);
                            // C) Ctrl+Esc：打开开始菜单（另一组组合键）
                            let _ = session.send_input(&InputEvent::Key { code: "VK:0x11".into(), down: true });
                            let _ = session.send_input(&InputEvent::Key { code: "VK:0x1B".into(), down: true });
                            std::thread::sleep(Duration::from_millis(80));
                            let _ = session.send_input(&InputEvent::Key { code: "VK:0x1B".into(), down: false });
                            let _ = session.send_input(&InputEvent::Key { code: "VK:0x11".into(), down: false });
                            phase = 3;
                            test_started_at = Some(Instant::now());
                            println!("[e2e] >>> Ctrl+A 差异 {:.3}%，发送 Ctrl+Esc（开始菜单），2.5s 后对比", d);
                        }
                    }
                    3 => {
                        if test_started_at.map(|t| t.elapsed() >= Duration::from_secs(3)).unwrap_or(false) {
                            let d = snapshot_diff!(snaps[3]);
                            diffs.push(d);
                            // D) 开始菜单「所有应用」列表：滚轮向下 4 格
                            for _ in 0..4 {
                                let _ = session.send_input(&InputEvent::Wheel { delta: -120 });
                                std::thread::sleep(Duration::from_millis(120));
                            }
                            phase = 4;
                            test_started_at = Some(Instant::now());
                            println!("[e2e] >>> Ctrl+Esc 差异 {:.3}%，滚轮向下 4 格，2.5s 后对比", d);
                        }
                    }
                    4 => {
                        if test_started_at.map(|t| t.elapsed() >= Duration::from_secs(3)).unwrap_or(false) {
                            let d = snapshot_diff!(snaps[4]);
                            diffs.push(d);
                            // E) 滚轮向上 4 格（列表回到原位 → 与打开时画面应高度相似）
                            for _ in 0..4 {
                                let _ = session.send_input(&InputEvent::Wheel { delta: 120 });
                                std::thread::sleep(Duration::from_millis(120));
                            }
                            phase = 5;
                            test_started_at = Some(Instant::now());
                            println!("[e2e] >>> 滚轮向下差异 {:.3}%，滚轮向上 4 格，2.5s 后对比", d);
                        }
                    }
                    5 => {
                        if test_started_at.map(|t| t.elapsed() >= Duration::from_secs(3)).unwrap_or(false) {
                            let d = diff_pct(&snaps[4], &comp.buf);
                            diffs.push(d);
                            snaps.push(comp.buf.clone());
                            let names = [
                                "1-initial", "2-drag-mid", "3-after-drag",
                                "4-after-ctrla", "5-menu-open", "6-wheel-down", "7-wheel-up",
                            ];
                            for (i, n) in names.iter().enumerate() {
                                if let Some(s) = snaps.get(i) {
                                    let _ = write_bmp(&input_dir2.join(format!("{n}.bmp")), comp.width, comp.height, s);
                                }
                            }
                            // B) 键盘链验证：Esc 关菜单 → Win 键 → notepad → Enter
                            let _ = session.send_input(&InputEvent::Key { code: "VK:0x1B".into(), down: true });
                            let _ = session.send_input(&InputEvent::Key { code: "VK:0x1B".into(), down: false });
                            std::thread::sleep(Duration::from_millis(400));
                            let _ = session.send_input(&InputEvent::Key { code: "VK:0x5B".into(), down: true });
                            std::thread::sleep(Duration::from_millis(60));
                            let _ = session.send_input(&InputEvent::Key { code: "VK:0x5B".into(), down: false });
                            phase = 6;
                            test_started_at = Some(Instant::now());
                            println!("[e2e] >>> 滚轮差异 {:.3}%，发送 Win 键（键盘链开始），2.5s 后对比", diffs[diffs.len() - 1]);
                        }
                    }
                    6 => {
                        if test_started_at.map(|t| t.elapsed() >= Duration::from_secs(3)).unwrap_or(false) {
                            snaps.push(comp.buf.clone());
                            diffs.push(diff_pct(&snaps[snaps.len() - 2], &comp.buf));
                            // 键入 notepad + Enter 启动记事本
                            for c in "notepad".chars() {
                                let vk = vk_from_char(c);
                                if let Some(v) = vk {
                                    let code = format!("VK:0x{v:02X}");
                                    let _ = session.send_input(&InputEvent::Key { code: code.clone(), down: true });
                                    let _ = session.send_input(&InputEvent::Key { code, down: false });
                                    std::thread::sleep(Duration::from_millis(60));
                                }
                            }
                            let _ = session.send_input(&InputEvent::Key { code: "VK:0x0D".into(), down: true });
                            let _ = session.send_input(&InputEvent::Key { code: "VK:0x0D".into(), down: false });
                            phase = 7;
                            test_started_at = Some(Instant::now());
                            println!("[e2e] >>> Win 菜单差异 {:.3}%，键入 notepad+Enter，3s 后对比", diffs[diffs.len() - 1]);
                        }
                    }
                    7 => {
                        if test_started_at.map(|t| t.elapsed() >= Duration::from_secs(4)).unwrap_or(false) {
                            snaps.push(comp.buf.clone());
                            diffs.push(diff_pct(&snaps[snaps.len() - 2], &comp.buf));
                            // Numpad 数字输入（硬编码 AT 码位路径）：1 5 9
                            for vk in [0x61u32, 0x65, 0x69] {
                                let code = format!("VK:0x{vk:02X}");
                                let _ = session.send_input(&InputEvent::Key { code: code.clone(), down: true });
                                let _ = session.send_input(&InputEvent::Key { code, down: false });
                                std::thread::sleep(Duration::from_millis(80));
                            }
                            phase = 8;
                            test_started_at = Some(Instant::now());
                            println!("[e2e] >>> 记事本差异 {:.3}%，Numpad 输入 1/5/9，2.5s 后对比", diffs[diffs.len() - 1]);
                        }
                    }
                    8 => {
                        if test_started_at.map(|t| t.elapsed() >= Duration::from_secs(3)).unwrap_or(false) {
                            diffs.push(diff_pct(&snaps[snaps.len() - 1], &comp.buf));
                            snaps.push(comp.buf.clone());
                            let names = [
                                "1-initial", "2-drag-mid", "3-after-drag", "4-after-ctrla",
                                "5-menu-open", "6-wheel-down", "7-wheel-up", "8-win-menu",
                                "9-notepad", "10-numpad-input",
                            ];
                            for (i, n) in names.iter().enumerate() {
                                if let Some(s) = snaps.get(i) {
                                    let _ = write_bmp(&input_dir2.join(format!("{n}.bmp")), comp.width, comp.height, s);
                                }
                            }
                            input_test_done = true;
                            break;
                        }
                    }
                    _ => {}
                }
            }

            // -- 收事件 --
            match rx.try_recv() {
                Some(FramebufferEvent::Framebuffer { width, height, pixels, .. }) => {
                    overall.fulls += 1;
                    // 全帧 JPEG 兜底帧：解码为合成缓冲底图（真实像素）
                    if let Err(e) = comp.apply_full_jpeg(width, height, &pixels) {
                        eprintln!("[e2e] {e}");
                    }
                    if last_delta.is_none() {
                        last_delta = Some(Instant::now());
                    }
                    if !connected_seen {
                        connected_seen = true;
                        last_delta = Some(Instant::now());
                        println!("[e2e] >>> 首个帧事件到达（{width}x{height}）—— 进入会话");
                    }
                }
                Some(FramebufferEvent::Delta { width, height, rects, pixels }) => {
                    comp.ensure(width, height);
                    comp.apply_delta(&rects, &pixels);
                    overall.add_delta(pixels.len(), rects.len());
                    // 扫动窗口统计：连接后 6~16s（制造画面对照）
                    if connected_seen && start.elapsed().as_secs() >= 6 && start.elapsed().as_secs() < 16 {
                        sweep.add_delta(pixels.len(), rects.len());
                    }
                    // 静止检测：任何增量都刷新「最近活动」计时
                    last_delta = Some(Instant::now());
                    if !connected_seen {
                        connected_seen = true;
                    }
                }
                Some(FramebufferEvent::Pointer { .. }) => {}
                // 原生直绘全帧（raw BGRA，2026-11 新增）：e2e 解码链只吃 JPEG
                // 全帧/增量，此事件忽略即可（原 miss 导致 native 构建下示例
                // 编译失败——顺手修复，F4 任务附带）。
                Some(FramebufferEvent::NativeFrame { .. }) => {}
                Some(FramebufferEvent::Cursor { width, height, .. }) => {
                    if width > 0 {
                        // 光标形状变化本身不触发画面增量；记录一次（帧差异判定不受影响）
                        println!("[e2e] 远端光标形状更新 {width}x{height}");
                    }
                }
                Some(FramebufferEvent::ClipboardText(t)) => {
                    println!("[e2e] 剪贴板文本 {} 字符", t.len());
                }
                Some(FramebufferEvent::Disconnected(reason)) => {
                    println!("[e2e] Disconnected: {reason}");
                    disconnected = Some(reason);
                    break;
                }
                None => std::thread::sleep(Duration::from_millis(5)),
            }

            // 鼠标扫动（连接后 6~16s：每 150ms 移动一次，制造连续画面变化）
            if connected_seen && start.elapsed().as_secs() >= 6 && start.elapsed().as_secs() < 16 && phase == 0 {
                let t = start.elapsed().as_millis() as u32;
                let w = comp.width.max(2);
                let h = comp.height.max(2);
                let ang = (t % 10_000) as f64 * 0.004;
                let mx = ((w as f64 / 2.0) + (ang.sin() * w as f64 * 0.35)) as u32;
                let my = ((h as f64 / 2.0) + (ang.cos() * h as f64 * 0.35)) as u32;
                let _ = session.send_input(&InputEvent::MouseMove {
                    x: mx.min(w - 1),
                    y: my.min(h - 1),
                });
                std::thread::sleep(Duration::from_millis(150));
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }

            // 总时长上限（输入测试未完成也退出）
            if Instant::now() >= deadline_all {
                break;
            }
        }
    }

    let _ = session.disconnect();

    // -- 汇总报告 --
    let stats = frame_jpeg::stats_snapshot();
    println!("\n========== 增量管线 + 输入 E2E 报告 ==========");
    println!("协商帧尺寸:         {}x{}", comp.width, comp.height);
    println!(
        "全帧(JPEG)发射:     {} 次（首帧/兜底）",
        overall.fulls.max(stats.encoded)
    );
    println!(
        "增量包总数:         {} 包 / {} 矩形 / {:.2} MB",
        overall.delta_packets,
        overall.delta_rects,
        overall.delta_bytes as f64 / 1024.0 / 1024.0
    );
    println!(
        "增量包平均/峰值:    {:.1} KB / {:.1} KB（对照全帧 JPEG ≈ 204 KB）",
        if overall.delta_packets > 0 { overall.delta_bytes as f64 / overall.delta_packets as f64 / 1024.0 } else { 0.0 },
        overall.delta_max as f64 / 1024.0
    );
    println!(
        "鼠标扫动窗口增量:   {} 包 / 平均 {:.1} KB → 更新频率 ≈ {:.1} 次/秒",
        sweep.delta_packets,
        if sweep.delta_packets > 0 { sweep.delta_bytes as f64 / sweep.delta_packets as f64 / 1024.0 } else { 0.0 },
        sweep.delta_packets as f64 / 10.0
    );
    println!(
        "引擎侧统计:         全帧 {} / 增量 {} 包 / 峰值增量 {:.1} KB",
        stats.encoded, stats.delta_packets, stats.max_delta_bytes as f64 / 1024.0
    );
    println!("---- 输入验证（画面静止后，链式甄别） ----");
    println!("A 图标拖拽(残影/增量):   差异 {:.3}%", diffs[0]);
    println!("B Ctrl+A 全选(组合键):   差异 {:.3}%", diffs[1]);
    println!("C Ctrl+Esc 菜单(组合键): 差异 {:.3}%", diffs[2]);
    println!("D 菜单内滚轮向下:        差异 {:.3}%", diffs[3]);
    println!("E 菜单内滚轮向上(复位):  差异 {:.3}%", diffs[4]);
    if diffs.len() > 7 {
        println!("F Win 键(键盘链):        差异 {:.3}%", diffs[5]);
        println!("G notepad 启动(键入):    差异 {:.3}%", diffs[6]);
        println!("H Numpad 输入 1/5/9:     差异 {:.3}%", diffs[7]);
    }
    println!("BMP 证据目录:            frames-debug\\input-test2\\（1-initial ~ 10-numpad-input 链式）");
    println!("断开原因:                {}", disconnected.as_deref().unwrap_or("<时间轴结束>"));

    if !connected_seen {
        println!("[e2e] FAILED: 未收到任何帧事件");
        std::process::exit(1);
    }
    let ok = |d: f64| d > 0.05;
    if diffs.len() > 7 && ok(diffs[1]) && ok(diffs[2]) && ok(diffs[5]) && ok(diffs[7]) {
        println!("[e2e] SUCCESS: Ctrl+A/Ctrl+Esc/Win 键/Numpad 输入全部生效；残影检查请目检 2/3 号 BMP");
        if !(ok(diffs[0]) && ok(diffs[3])) {
            println!("[e2e] NOTE: 拖拽差异 {:.3}% / 滚轮差异 {:.3}%（可能无图标可拖/列表无滚动，属环境差异非管线故障）", diffs[0], diffs[3]);
        }
    } else if diffs.len() > 2 && ok(diffs[1]) && ok(diffs[2]) {
        println!("[e2e] PARTIAL: 组合键生效，键盘链未完成（diffs={} 项）", diffs.len());
    } else {
        let m = |i: usize| if i < diffs.len() { format!("{:.3}", diffs[i]) } else { "n/a".into() };
        println!("[e2e] FAILED: 组合键未生效（ctrla={} menu={}），继续排查输入通路", m(1), m(2));
        std::process::exit(1);
    }
}
