//! 帧管线 JPEG 编码（2026-11 性能修复）：
//!
//! 此前帧以 32bpp BMP + base64 传输（2560x1440 ≈ 14MB BMP → base64 ≈ 19MB
//! 字符串/帧），15FPS 下 ≈285MB/s 垃圾灌入 webview，是内存暴涨与 CPU 居高的
//! 根因。现在合成缓冲在引擎侧直接编码 JPEG（质量 75、4:2:0 色度子采样），
//! 事件携带的是 **JPEG 字节**，前端 `img.src = data:image/jpeg`。
//!
//! 依赖选择：`jpeg-encoder`（纯 Rust、零传递依赖、内置 4:2:0 子采样）——
//! 对比 `image` crate（default 启用全部 codec，依赖树大一个数量级），
//! jpeg-encoder 依赖树最小。

use std::sync::Mutex;

use jpeg_encoder::{ColorType, Encoder, SamplingFactor};

/// JPEG 编码质量（1..=100）。实测 2560x1440 桌面内容 q75+4:2:0 单帧 ≤500KB。
pub const JPEG_QUALITY: u8 = 75;
/// 运动期降载质量（2026-11：画面高速变化时自适应降级，编码耗时/体积双降）。
pub const JPEG_QUALITY_MOTION: u8 = 50;

/// BGRA32 top-down 像素 -> JPEG 字节（质量 [`JPEG_QUALITY`]，4:2:0）。
///
/// 纯函数；返回错误表示长度不符或编码器失败。像素长度必须 = width*height*4。
pub fn encode_frame_jpeg(
    width: u32,
    height: u32,
    bgra_top_down: &[u8],
) -> Result<Vec<u8>, String> {
    encode_frame_jpeg_q(width, height, bgra_top_down, JPEG_QUALITY)
}

/// 指定质量的编码变体（运动期降载用 q50）。
pub fn encode_frame_jpeg_q(
    width: u32,
    height: u32,
    bgra_top_down: &[u8],
    quality: u8,
) -> Result<Vec<u8>, String> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| "尺寸溢出".to_string())?;
    if bgra_top_down.len() != expected {
        return Err(format!(
            "像素长度不符: 期望 {expected} 字节，实际 {}",
            bgra_top_down.len()
        ));
    }
    let mut out = Vec::with_capacity(expected / 16); // 经验容量：约 1/16 原始大小
    let mut encoder = Encoder::new(&mut out, quality.clamp(1, 100));
    // 桌面画面大面积平坦区域：F_2_2（即 4:2:0）子采样把色度分辨率减半，
    // 体积显著下降（相对 4:4:4 通常省 20~30%），q75 下画质损失在远程桌面
    // 场景不可感知。
    encoder.set_sampling_factor(SamplingFactor::F_2_2);
    encoder
        .encode(bgra_top_down, width as u16, height as u16, ColorType::Bgra)
        .map_err(|e| format!("JPEG 编码失败: {e}"))?;
    Ok(out)
}

/// BGRA32 缓冲 2x2 盒式降采样（宽高各减半，奇数尾行列丢弃）。
/// 运动期自适应降载：全帧编码分辨率减半 → 编码耗时 ~4 倍降、
/// JPEG 数据 ~4 倍少；前端 drawImage 放大显示（rAF 缩放天然支持）。
pub fn halve_bgra(width: u32, height: u32, bgra: &[u8]) -> (u32, u32, Vec<u8>) {
    let w = width as usize;
    let h = height as usize;
    let (w2, h2) = (w / 2, h / 2);
    if w2 == 0 || h2 == 0 {
        return (width, height, bgra.to_vec());
    }
    let mut out = vec![0u8; w2 * h2 * 4];
    for y in 0..h2 {
        let y0 = y * 2;
        for x in 0..w2 {
            let x0 = x * 2;
            for c in 0..4 {
                // 2x2 均值
                let sum = u32::from(bgra[(y0 * w + x0) * 4 + c])
                    + u32::from(bgra[(y0 * w + x0 + 1) * 4 + c])
                    + u32::from(bgra[((y0 + 1) * w + x0) * 4 + c])
                    + u32::from(bgra[((y0 + 1) * w + x0 + 1) * 4 + c]);
                out[(y * w2 + x) * 4 + c] = (sum / 4) as u8;
            }
        }
    }
    (w2 as u32, h2 as u32, out)
}

// ---------------------------------------------------------------------------
// 编码统计（release 实测用；仅 native ffi 路径记录，demo 路径不污染统计）
// ---------------------------------------------------------------------------

/// 帧编码累计统计（ffi.rs on_bitmap_update 写入；e2e 探针读取）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameEncodeStats {
    /// 全帧（JPEG）发射数。
    pub encoded: u64,
    /// 全帧编码累计耗时（µs）。
    pub total_encode_us: u64,
    /// 单帧全帧编码峰值耗时（µs）。
    pub max_encode_us: u64,
    /// JPEG 字节累计。
    pub total_jpeg_bytes: u64,
    /// 单帧 JPEG 峰值大小（字节）。
    pub max_jpeg_bytes: u64,
    /// 增量（脏矩形）包发射数。
    pub delta_packets: u64,
    /// 增量包 BGRA 字节累计。
    pub total_delta_bytes: u64,
    /// 单包增量峰值大小（字节）。
    pub max_delta_bytes: u64,
    /// 增量脏矩形总数。
    pub total_delta_rects: u64,
}

static FRAME_STATS: Mutex<Option<FrameEncodeStats>> = Mutex::new(None);

/// 记录一次成功编码（耗时 µs + JPEG 大小）。
pub fn record_encode(encode_us: u64, jpeg_bytes: usize) {
    let mut slot = FRAME_STATS.lock().expect("FRAME_STATS poisoned");
    let s = slot.get_or_insert_with(FrameEncodeStats::default);
    s.encoded += 1;
    s.total_encode_us += encode_us;
    s.max_encode_us = s.max_encode_us.max(encode_us);
    s.total_jpeg_bytes += jpeg_bytes as u64;
    s.max_jpeg_bytes = s.max_jpeg_bytes.max(jpeg_bytes as u64);
}

/// 记录一次增量（脏矩形）包发射。
pub fn record_delta(delta_bytes: usize, rects: usize) {
    let mut slot = FRAME_STATS.lock().expect("FRAME_STATS poisoned");
    let s = slot.get_or_insert_with(FrameEncodeStats::default);
    s.delta_packets += 1;
    s.total_delta_bytes += delta_bytes as u64;
    s.max_delta_bytes = s.max_delta_bytes.max(delta_bytes as u64);
    s.total_delta_rects += rects as u64;
}

/// 读取统计快照（不清零）。
pub fn stats_snapshot() -> FrameEncodeStats {
    FRAME_STATS
        .lock()
        .expect("FRAME_STATS poisoned")
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_rejects_size_mismatch() {
        assert!(encode_frame_jpeg(4, 2, &[0u8; 3]).is_err());
    }

    #[test]
    fn encode_produces_valid_jpeg_and_is_far_smaller_than_raw() {
        // 64x64 纯色帧：JPEG 应远小于原始 16KB，且以 SOI (FFD8) 开头。
        let px = vec![0x40u8; 64 * 64 * 4];
        let jpeg = encode_frame_jpeg(64, 64, &px).unwrap();
        assert!(jpeg.len() < 4096, "纯色 64x64 JPEG 应 <4KB，实际 {}", jpeg.len());
        assert_eq!(&jpeg[0..2], &[0xFF, 0xD8]);
        // 同输入两次编码字节完全一致（确定性，供相同帧校验/测试）。
        let jpeg2 = encode_frame_jpeg(64, 64, &px).unwrap();
        assert_eq!(jpeg, jpeg2);
    }

    #[test]
    fn stats_record_and_snapshot() {
        // 先清零避免并发测试干扰
        *FRAME_STATS.lock().unwrap() = None;
        record_encode(1500, 300_000);
        record_encode(900, 250_000);
        record_delta(4096, 2);
        record_delta(8192, 3);
        let s = stats_snapshot();
        assert_eq!(s.encoded, 2);
        assert_eq!(s.total_encode_us, 2400);
        assert_eq!(s.max_encode_us, 1500);
        assert_eq!(s.max_jpeg_bytes, 300_000);
        assert_eq!(s.delta_packets, 2);
        assert_eq!(s.total_delta_bytes, 12_288);
        assert_eq!(s.max_delta_bytes, 8192);
        assert_eq!(s.total_delta_rects, 5);
        // 收尾清零，避免影响其他用例对统计的观察
        *FRAME_STATS.lock().unwrap() = None;
        assert_eq!(stats_snapshot(), FrameEncodeStats::default());
    }

    /// 运动期降载：2x2 盒式降采样尺寸与均值正确；奇数尺寸截断。
    #[test]
    fn halve_bgra_averages_2x2_blocks() {
        // 4x2 缓冲：左半全 0，右半全 200 → 2x1 输出：左 0、右 200
        let mut buf = vec![0u8; 4 * 2 * 4];
        for y in 0..2 {
            for x in 2..4 {
                for c in 0..3 {
                    buf[(y * 4 + x) * 4 + c] = 200;
                }
                buf[(y * 4 + x) * 4 + 3] = 0xFF;
            }
        }
        let (w2, h2, out) = halve_bgra(4, 2, &buf);
        assert_eq!((w2, h2), (2, 1));
        assert_eq!(&out[0..4], &[0, 0, 0, 0]); // 左：全 0
        assert_eq!(&out[4..8], &[200, 200, 200, 0xFF]); // 右：均值 200
        // 奇数尺寸：3x3 → 1x1（取左上 2x2）
        let (w3, h3, _) = halve_bgra(3, 3, &vec![7u8; 3 * 3 * 4]);
        assert_eq!((w3, h3), (1, 1));
    }
}
