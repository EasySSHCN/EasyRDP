//! F1-V2 R-20261118-013 会话快照预览（Session Snapshot Preview）。
//!
//! 职责：
//! - 每会话捕获线程（Windows 原生会话）：读 rdp_view_final 的合成帧
//!   （FRAME_STORE，present/present_delta 写入的 BGRA 背板数据源），
//!   按帧世代（FRAME_GEN）判定 dirty——仅画面有变化且距上次捕获
//!   ≥ [`SNAPSHOT_INTERVAL`]（默认 10s）时落盘；
//! - 缩略帧：等比缩放进 320x180 盒（box-average 缩放，[`downscale_bgra`]），
//!   PNG 编码（纯 Rust：CRC32 + flate2 zlib，[`encode_png_bgra`]）；
//!   ≤200KB/张：超限依次回退 0.75x/0.5x 缩放重编码，仍超则跳过本次；
//! - 存储：`%APPDATA%\EasyRDP\snapshots\<profile_id>.png` 覆盖写；
//!   目录总大小上限 20MB，超限按 mtime 最旧优先清理（[`enforce_quota`]）；
//! - 命令：`get_snapshot(profile_id)`（base64 PNG + 捕获时间戳，前端卡片
//!   hover 500ms 防抖浮层用）；连接删除时由 delete_connection 联动删文件；
//! - 隐私开关：settings.json `autoSnapshotPreview`（默认开）——关时
//!   捕获线程跳过、前端不请求。

use base64::Engine as _;
use serde::Serialize;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 缩略帧目标盒（等比缩进该盒内）。
pub const THUMB_BOX_W: u32 = 320;
pub const THUMB_BOX_H: u32 = 180;
/// 单张快照大小上限。
pub const MAX_SNAPSHOT_BYTES: usize = 200 * 1024;
/// 快照目录总大小上限。
pub const QUOTA_BYTES: u64 = 20 * 1024 * 1024;
/// 捕获间隔（dirty 时才真正捕获）。
pub const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(10);
/// 捕获线程轮询间隔。
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// 隐私开关设置键。
pub const SETTING_KEY: &str = "autoSnapshotPreview";

// ---------------------------------------------------------------------------
// 目录与文件
// ---------------------------------------------------------------------------

/// 快照目录（测试可用 EASYRDP_SNAPSHOT_DIR 覆盖；默认 %APPDATA%\EasyRDP\snapshots）。
pub fn snapshots_dir() -> PathBuf {
    if let Ok(d) = std::env::var("EASYRDP_SNAPSHOT_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    match std::env::var("APPDATA") {
        Ok(base) => PathBuf::from(base).join("EasyRDP").join("snapshots"),
        Err(_) => PathBuf::from("EasyRDP-snapshots"),
    }
}

/// profile_id -> 文件名安全片段（id 为 conn-<nanos>，防御性清洗非安全字符）。
fn safe_name(profile_id: &str) -> String {
    profile_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// `<profile_id>.png` 完整路径。
pub fn snapshot_path(dir: &Path, profile_id: &str) -> PathBuf {
    dir.join(format!("{}.png", safe_name(profile_id)))
}

/// 覆盖写一张快照（目录不存在则创建）。
pub fn write_snapshot(dir: &Path, profile_id: &str, png: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建快照目录失败: {e}"))?;
    let path = snapshot_path(dir, profile_id);
    std::fs::write(&path, png).map_err(|e| format!("写入快照失败: {e}"))?;
    Ok(path)
}

/// 删除一张快照（不存在视为成功——幂等，连接删除联动用）。
pub fn delete_snapshot_file(dir: &Path, profile_id: &str) {
    let _ = std::fs::remove_file(snapshot_path(dir, profile_id));
}

/// 读取快照（字节, 捕获时刻 unix ms）；文件缺失/损坏返回 None。
pub fn read_snapshot(dir: &Path, profile_id: &str) -> Option<(Vec<u8>, u128)> {
    let path = snapshot_path(dir, profile_id);
    let bytes = std::fs::read(&path).ok()?;
    let ms = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Some((bytes, ms))
}

// ---------------------------------------------------------------------------
// 纯函数：dirty 判定 / 缩放 / PNG 编码 / 配额清理
// ---------------------------------------------------------------------------

/// 是否到达捕获时机：从未捕获过 → 立即；距上次 ≥ 间隔且画面有变化。
pub fn should_snapshot(
    last: Option<Instant>,
    now: Instant,
    interval: Duration,
    dirty: bool,
) -> bool {
    if !dirty {
        return false;
    }
    match last {
        None => true,
        Some(t) => now.duration_since(t) >= interval,
    }
}

/// 远端尺寸 -> 缩略尺寸：等比缩进 320x180 盒（小帧不放大），最小 1x1。
pub fn fit_thumb(sw: u32, sh: u32) -> (u32, u32) {
    if sw == 0 || sh == 0 {
        return (1, 1);
    }
    let sx = THUMB_BOX_W as f64 / sw as f64;
    let sy = THUMB_BOX_H as f64 / sh as f64;
    let s = sx.min(sy);
    if s >= 1.0 {
        return (sw, sh);
    }
    (
        ((sw as f64 * s).round() as u32).max(1),
        ((sh as f64 * s).round() as u32).max(1),
    )
}

/// 等比缩放系数回退序列（超 200KB 时逐级重编码）。
pub fn retry_scales() -> [f64; 3] {
    [1.0, 0.75, 0.5]
}

/// BGRA(top-down) box-average 缩放到 dw x dh（每目标像素取源盒平均）。
/// 输入长度必须 = sw*sh*4。
pub fn downscale_bgra(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
    let expected = sw * sh * 4;
    if src.len() < expected || sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return vec![0u8; dw.max(1) * dh.max(1) * 4];
    }
    let mut out = vec![0u8; dw * dh * 4];
    for dy in 0..dh {
        // 目标行对应的源行区间 [y0, y1)
        let y0 = dy * sh / dh;
        let y1 = ((dy + 1) * sh / dh).max(y0 + 1).min(sh);
        for dx in 0..dw {
            let x0 = dx * sw / dw;
            let x1 = ((dx + 1) * sw / dw).max(x0 + 1).min(sw);
            let (mut r, mut g, mut b, mut a) = (0u64, 0u64, 0u64, 0u64);
            let mut n = 0u64;
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = (y * sw + x) * 4;
                    b += src[p] as u64;
                    g += src[p + 1] as u64;
                    r += src[p + 2] as u64;
                    a += src[p + 3] as u64;
                    n += 1;
                }
            }
            let q = (dy * dw + dx) * 4;
            out[q] = (b / n) as u8;
            out[q + 1] = (g / n) as u8;
            out[q + 2] = (r / n) as u8;
            out[q + 3] = (a / n) as u8;
        }
    }
    out
}

/// CRC-32（IEEE，PNG chunk 用；查表法）。
pub fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, e) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *e = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

fn png_chunk(tag: &[u8; 4], body: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + body.len());
    crc_input.extend_from_slice(tag);
    crc_input.extend_from_slice(body);
    out.extend_from_slice(tag);
    out.extend_from_slice(body);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// BGRA(top-down) -> PNG（8-bit RGBA，filter 0，zlib deflate via flate2）。
/// 输出恒为合法 PNG（header 校验：宽高往返可解码语义由单测覆盖）。
pub fn encode_png_bgra(bgra: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let expected = (w as usize)
        .checked_mul(h as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| "尺寸溢出".to_string())?;
    if bgra.len() < expected {
        return Err(format!("像素长度不符: 期望 {expected}，实际 {}", bgra.len()));
    }
    // 原始扫描线：filter byte 0 + RGBA
    let stride = w as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0u8); // filter type 0 (None)
        let row = &bgra[y * stride..(y + 1) * stride];
        for px in row.chunks_exact(4) {
            raw.push(px[2]); // R
            raw.push(px[1]); // G
            raw.push(px[0]); // B
            raw.push(px[3]); // A
        }
    }
    let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    zlib.write_all(&raw).map_err(|e| e.to_string())?;
    let idat = zlib.finish().map_err(|e| e.to_string())?;

    let mut out = Vec::with_capacity(57 + idat.len());
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // bit depth 8, color type 6 (RGBA)
    png_chunk(b"IHDR", &ihdr, &mut out);
    png_chunk(b"IDAT", &idat, &mut out);
    png_chunk(b"IEND", &[], &mut out);
    Ok(out)
}

/// 目录总大小超出配额时按 mtime 最旧优先删除，返回删除张数。
pub fn enforce_quota(dir: &Path, quota: u64) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut files: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("png") {
            continue;
        }
        if let Ok(md) = e.metadata() {
            if md.is_file() {
                let modified = md.modified().unwrap_or(UNIX_EPOCH);
                files.push((p, md.len(), modified));
            }
        }
    }
    let total: u64 = files.iter().map(|f| f.1).sum();
    if total <= quota {
        return 0;
    }
    // 最旧优先删除
    files.sort_by_key(|f| f.2);
    let mut removed = 0usize;
    let mut total = total;
    for (p, len, _) in files {
        if total <= quota {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total = total.saturating_sub(len);
            removed += 1;
        }
    }
    removed
}

// ---------------------------------------------------------------------------
// 捕获线程（Windows 原生会话）
// ---------------------------------------------------------------------------

/// 单张快照产出：缩放 + 编码 + 200KB 回退。返回 None = 全部回退仍超限。
pub fn build_snapshot_png(bgra: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let (tw, th) = fit_thumb(w, h);
    for s in retry_scales() {
        let (dw, dh) = (
            ((tw as f64 * s).round() as u32).max(1),
            ((th as f64 * s).round() as u32).max(1),
        );
        let thumb = downscale_bgra(bgra, w, h, dw, dh);
        if let Ok(png) = encode_png_bgra(&thumb, dw, dh) {
            if png.len() <= MAX_SNAPSHOT_BYTES {
                return Some(png);
            }
        }
    }
    None
}

/// 启动每会话捕获线程（仅 Windows 原生会话；demo 无原生窗口不捕获）。
/// 退出条件：stop 置位（与 SessionHandle.stop 同源，会话结束自动停）。
#[cfg(windows)]
pub fn spawn_capture(app: tauri::AppHandle, session_id: String, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name(format!("snapshot-{session_id}"))
        .spawn(move || {
            let dir = snapshots_dir();
            let mut last_cap: Option<Instant> = None;
            let mut last_written_gen: u64 = 0;
            let mut capture_count: u64 = 0;
            loop {
                std::thread::sleep(POLL_INTERVAL);
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                // 隐私开关（settings.json）：关 = 跳过捕获（不清已有快照）
                if !crate::settings::get_bool_setting(&app, SETTING_KEY) {
                    continue;
                }
                let Some((bgra, w, h, gen)) =
                    crate::native_view::snapshot_frame_bgra(&session_id)
                else {
                    continue; // 原生窗口尚未 attach / 已 detach
                };
                // 省电：画面无变化（帧世代未推进）不捕获
                let dirty = last_written_gen == 0 || gen != last_written_gen;
                if !should_snapshot(last_cap, Instant::now(), SNAPSHOT_INTERVAL, dirty) {
                    continue;
                }
                let Some(png) = build_snapshot_png(&bgra, w, h) else {
                    continue; // 全部回退仍超 200KB：跳过本次，等下一代
                };
                match write_snapshot(&dir, &session_id, &png) {
                    Ok(_) => {
                        last_cap = Some(Instant::now());
                        last_written_gen = gen;
                        capture_count += 1;
                        if capture_count == 1 || capture_count % 10 == 0 {
                            easyrdp_rdp::debug_log(&format!(
                                "[snapshot] session={session_id} 第{capture_count}张 {}x{} -> {}x{} {}B",
                                w, h,
                                fit_thumb(w, h).0,
                                fit_thumb(w, h).1,
                                png.len()
                            ));
                        }
                    }
                    Err(e) => {
                        easyrdp_rdp::debug_log(&format!("[snapshot] 写入失败: {e}"));
                    }
                }
                enforce_quota(&dir, QUOTA_BYTES);
            }
        })
        .ok();
}

#[cfg(not(windows))]
pub fn spawn_capture(_app: tauri::AppHandle, _session_id: String, _stop: Arc<AtomicBool>) {}

// ---------------------------------------------------------------------------
// Tauri 命令
// ---------------------------------------------------------------------------

/// get_snapshot 返回载荷：base64 PNG + 捕获时刻（unix ms，0 = 未知）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotView {
    pub png_base64: String,
    pub captured_at_ms: u128,
}

/// 前端连接卡片 hover 浮层预览：读一张快照。
/// None = 暂无快照（前端显示占位文案）。
#[tauri::command]
pub fn get_snapshot(profile_id: String) -> Result<Option<SnapshotView>, String> {
    match read_snapshot(&snapshots_dir(), &profile_id) {
        Some((bytes, ms)) => Ok(Some(SnapshotView {
            png_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            captured_at_ms: ms,
        })),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// 单测（纯函数：缩放 / PNG 编码 / 配额 / dirty 判定）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 纯蓝 BGRA 测试图案。
    fn solid_bgra(w: u32, h: u32, b: u8, g: u8, r: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            v.extend_from_slice(&[b, g, r, 0xFF]);
        }
        v
    }

    #[test]
    fn should_snapshot_requires_dirty_and_interval() {
        let now = Instant::now();
        // 首帧必捕（dirty）
        assert!(should_snapshot(None, now, SNAPSHOT_INTERVAL, true));
        // 不 dirty 永不捕
        assert!(!should_snapshot(None, now, SNAPSHOT_INTERVAL, false));
        assert!(!should_snapshot(Some(now - Duration::from_secs(60)), now, SNAPSHOT_INTERVAL, false));
        // dirty + 未到间隔 → 不捕
        assert!(!should_snapshot(Some(now - Duration::from_secs(3)), now, SNAPSHOT_INTERVAL, true));
        // dirty + 到间隔 → 捕
        assert!(should_snapshot(Some(now - Duration::from_secs(11)), now, SNAPSHOT_INTERVAL, true));
    }

    #[test]
    fn fit_thumb_scales_into_box_and_never_upscales() {
        assert_eq!(fit_thumb(1920, 1080), (320, 180));
        assert_eq!(fit_thumb(1280, 720), (320, 180));
        assert_eq!(fit_thumb(2560, 1440), (320, 180));
        // 非 16:9：等比进盒（高度顶满 180，宽度等比缩小）
        assert_eq!(fit_thumb(1024, 768), (240, 180));
        assert_eq!(fit_thumb(1600, 1200), (240, 180));
        // 小帧不放大
        assert_eq!(fit_thumb(100, 50), (100, 50));
        // 退化输入
        assert_eq!(fit_thumb(0, 0), (1, 1));
    }

    #[test]
    fn downscale_bgra_produces_exact_size_and_averages() {
        // 4x2 纯红 -> 2x1：仍是纯红
        let src = solid_bgra(4, 2, 0, 0, 255);
        let out = downscale_bgra(&src, 4, 2, 2, 1);
        assert_eq!(out.len(), 2 * 1 * 4);
        assert_eq!(out[0], 0); // B
        assert_eq!(out[1], 0); // G
        assert_eq!(out[2], 255); // R
        assert_eq!(out[3], 0xFF);
        // 混色平均：左半红右半蓝 -> 2x1 像素分别为红/蓝
        let mut mixed = Vec::new();
        for _y in 0..2u32 {
            for x in 0..4u32 {
                if x < 2 {
                    mixed.extend_from_slice(&[0, 0, 255, 0xFF]);
                } else {
                    mixed.extend_from_slice(&[255, 0, 0, 0xFF]);
                }
            }
        }
        let avg = downscale_bgra(&mixed, 4, 2, 2, 1);
        assert_eq!(&avg[0..4], &[0, 0, 255, 0xFF]);
        assert_eq!(&avg[4..8], &[255, 0, 0, 0xFF]);
        // 输入过短：输出黑帧占位（不 panic）
        let short = downscale_bgra(&[0u8; 3], 4, 2, 2, 1);
        assert_eq!(short.len(), 8);
    }

    #[test]
    fn png_roundtrip_header_pixels_and_crc() {
        let (w, h) = (4u32, 2u32);
        let src = solid_bgra(w, h, 10, 20, 30);
        let png = encode_png_bgra(&src, w, h).unwrap();
        // 签名
        assert_eq!(&png[0..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        // IHDR：长度 13 + tag + body
        assert_eq!(&png[8..12], &13u32.to_be_bytes());
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..20], &w.to_be_bytes());
        assert_eq!(&png[20..24], &h.to_be_bytes());
        assert_eq!(png[24], 8); // bit depth
        assert_eq!(png[25], 6); // RGBA
        // IDAT：IHDR chunk 共 8 + (4 len + 4 tag + 13 body + 4 crc) = 33 起
        let idat_at = 8 + 25;
        let idat_len = u32::from_be_bytes(png[idat_at..idat_at + 4].try_into().unwrap()) as usize;
        assert_eq!(&png[idat_at + 4..idat_at + 8], b"IDAT");
        let idat = &png[idat_at + 8..idat_at + 8 + idat_len];
        let mut inflater = flate2::read::ZlibDecoder::new(idat);
        let mut raw = Vec::new();
        std::io::Read::read_to_end(&mut inflater, &mut raw).unwrap();
        assert_eq!(raw.len(), (stride_of(w) + 1) * h as usize);
        // 第 0 行 filter=0，像素 RGBA = (30, 20, 10, 255)
        assert_eq!(raw[0], 0);
        assert_eq!(&raw[1..5], &[30, 20, 10, 0xFF]);
        // IEND CRC 覆盖 tag（tag+CRC 一致性：构造体内部一致即可）
        let iend_at = png.len() - 12;
        assert_eq!(&png[iend_at + 4..iend_at + 8], b"IEND");
        let crc_stored = u32::from_be_bytes(png[iend_at + 8..].try_into().unwrap());
        assert_eq!(crc_stored, crc32(b"IEND"));
    }

    fn stride_of(w: u32) -> usize {
        w as usize * 4
    }

    #[test]
    fn encode_png_rejects_short_pixel_buffer() {
        assert!(encode_png_bgra(&[0u8; 3], 4, 4).is_err());
    }

    #[test]
    fn build_snapshot_png_respects_size_cap() {
        // 大幅面噪声图案（PNG 难压缩）验证 200KB 回退
        let (w, h) = (1920u32, 1080u32);
        let mut src = Vec::with_capacity((w * h * 4) as usize);
        let mut x: u32 = 12345;
        for _ in 0..w * h {
            x = x.wrapping_mul(1103515245).wrapping_add(12345);
            src.extend_from_slice(&[(x >> 16) as u8, (x >> 8) as u8, x as u8, 0xFF]);
        }
        let png = build_snapshot_png(&src, w, h).expect("噪声图案至少有一档 ≤200KB");
        assert!(png.len() <= MAX_SNAPSHOT_BYTES, "实际 {}B", png.len());
        // 尺寸是缩略级（PNG IHDR 宽 ≤ 320）
        let w_stored = u32::from_be_bytes(png[16..20].try_into().unwrap());
        assert!(w_stored <= 320);
    }

    #[test]
    fn crc32_matches_known_vectors() {
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"IEND"), crc32(b"IEND"));
    }

    // ---- 配额清理（临时目录在 target/test-data，沙箱不写 %TEMP%）----

    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-data"))
            .join(format!("snapshot-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 写一张带指定 mtime 的快照文件。
    fn put(dir: &Path, name: &str, len: usize, age_secs: u64) {
        let p = dir.join(name);
        std::fs::write(&p, vec![0xABu8; len]).unwrap();
        let past = SystemTime::now() - Duration::from_secs(age_secs);
        let file = std::fs::File::options().write(true).open(&p).unwrap();
        file.set_modified(past).unwrap();
    }

    #[test]
    fn quota_under_limit_removes_nothing() {
        let dir = test_dir("under");
        put(&dir, "a.png", 100, 10);
        put(&dir, "b.png", 100, 20);
        assert_eq!(enforce_quota(&dir, 1000), 0);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quota_over_limit_deletes_oldest_first() {
        let dir = test_dir("over");
        put(&dir, "old.png", 100, 100);
        put(&dir, "mid.png", 100, 50);
        put(&dir, "new.png", 100, 1);
        // 配额 150B：总 300B → 删到 ≤150 → 删最旧两张
        assert_eq!(enforce_quota(&dir, 150), 2);
        assert!(!dir.join("old.png").exists(), "最旧先删");
        assert!(!dir.join("mid.png").exists());
        assert!(dir.join("new.png").exists(), "最新保留");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_read_delete_roundtrip() {
        let dir = test_dir("roundtrip");
        let png = encode_png_bgra(&solid_bgra(4, 2, 1, 2, 3), 4, 2).unwrap();
        write_snapshot(&dir, "conn-123", &png).unwrap();
        assert_eq!(snapshot_path(&dir, "conn-123"), dir.join("conn-123.png"));
        let (bytes, ms) = read_snapshot(&dir, "conn-123").unwrap();
        assert_eq!(bytes, png);
        assert!(ms > 0);
        // 非法字符清洗（"/" 与 "." 各替换为 "_"：conn + 4 个 "_" + x）
        write_snapshot(&dir, "conn/../x", &png).unwrap();
        assert!(dir.join("conn____x.png").exists());
        delete_snapshot_file(&dir, "conn-123");
        assert!(read_snapshot(&dir, "conn-123").is_none());
        // 幂等删除
        delete_snapshot_file(&dir, "conn-123");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 命令载荷结构（get_snapshot 契约锚点）：base64 可解 + 时间戳透传。
    #[test]
    fn snapshot_view_serializes_base64_and_timestamp() {
        use base64::Engine as _;
        let png = vec![1u8, 2, 3];
        let view = SnapshotView {
            png_base64: base64::engine::general_purpose::STANDARD.encode(&png),
            captured_at_ms: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("pngBase64"), "camelCase 序列化: {json}");
        assert!(json.contains("capturedAtMs"));
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&view.png_base64)
            .unwrap();
        assert_eq!(decoded, png);
    }
}
