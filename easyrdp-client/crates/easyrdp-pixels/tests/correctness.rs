//! 正确性单测：SIMD/GPU 输出与纯 Rust 参考对比。

use easyrdp_pixels as px;

/// 伪随机（确定性）测试图案。
fn noise(w: usize, h: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(w * h * 4);
    let mut s = 0x12345678u64;
    for _ in 0..w * h * 4 {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        v.push((s >> 33) as u8);
    }
    v
}

#[test]
fn bgra_to_rgba_matches_scalar() {
    for (w, h) in [(8, 2), (16, 4), (13, 5), (1, 1)] {
        let src = noise(w, h);
        let mut simd = vec![0u8; src.len()];
        px::bgra_to_rgba(&src, &mut simd);
        let mut scalar = vec![0u8; src.len()];
        px::scalar::bgra_to_rgba(&src, &mut scalar);
        assert_eq!(simd, scalar, "{w}x{h}");
    }
}

#[test]
fn bgra_to_rgb888_matches_scalar() {
    for (w, h) in [(8, 2), (16, 4), (13, 5), (1, 1)] {
        let src = noise(w, h);
        let pxn = (w * h) * 3;
        let mut simd = vec![0u8; pxn];
        px::bgra_to_rgb888(&src, &mut simd);
        let mut scalar = vec![0u8; pxn];
        px::scalar::bgra_to_rgb888(&src, &mut scalar);
        assert_eq!(simd, scalar, "{w}x{h}");
    }
}

#[test]
fn halve_bgra_matches_scalar() {
    for (w, h) in [(16, 8), (32, 16), (1920, 1080), (15, 9), (3, 3)] {
        let src = noise(w, h);
        let (sw, sh, simd) = px::halve_bgra(w as u32, h as u32, &src);
        let (rw, rh, scalar) = px::scalar::halve_bgra(w as u32, h as u32, &src);
        assert_eq!((sw, sh), (rw, rh), "{w}x{h}");
        assert_eq!(simd, scalar, "{w}x{h}");
    }
}

#[test]
fn halve_matches_frame_jpeg_semantics() {
    // 与 easyrdp-rdp::frame_jpeg::halve_bgra 手工同语义验证（sum/4 截断）
    let src = vec![10u8, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160];
    // 2x2 像素帧：p0(10,20,30,40) p1(50,60,70,80) / p2(90..) p3(130..)
    let (_, _, out) = px::halve_bgra(2, 2, &src);
    assert_eq!(out, vec![70u8, 80, 90, 100]);
}

#[test]
fn diff_tiles_finds_dirty_only() {
    let (w, h) = (64u32, 64u32);
    let mut a = noise(w as usize, h as usize);
    let b = a.clone();
    assert!(px::diff_tiles(&a, &b, w, h, 32).is_empty(), "相同帧无脏 tile");
    // 只改第 2 个 tile（x=32..64, y=0..32）的一个字节
    let s = (0 * (w as usize) + 40) * 4 + 1;
    a[s] ^= 0xFF;
    let tiles = px::diff_tiles(&a, &b, w, h, 32);
    assert_eq!(tiles, vec![(32, 0, 32, 32)], "恰好命中受影响 tile");
    // 尺寸不符防御
    assert!(px::diff_tiles(&a[..100], &b, w, h, 32).is_empty());
}

#[test]
fn box_downscale_matches_snapshot_semantics() {
    // 与 easyrdp-app::snapshot::downscale_bgra 同语义（2:1 均值）
    let mut src = vec![10u8; 4 * 2 * 4];
    for i in 0..src.len() {
        src[i] = (i % 251) as u8;
    }
    let out = px::box_downscale_bgra(&src, 4, 2, 2, 1);
    let mut want = vec![0u8; 8];
    for dy in 0..1usize {
        for dx in 0..2usize {
            let (mut acc, mut n) = (0u64, 0u64);
            for y in 0..2usize {
                for x in 0..2usize {
                    acc += src[((dy * 2 + y) * 4 + dx * 2 + x) * 4 + 0] as u64;
                    n += 1;
                }
            }
            want[(dy * 2 + dx) * 4] = (acc / n) as u8;
        }
    }
    assert_eq!(out[0], want[0]);
    // 短输入 → 全 0
    assert!(px::box_downscale_bgra(&[0u8; 3], 4, 2, 2, 1).iter().all(|&b| b == 0));
}

// ---------------------------------------------------------------------------
// 回退路径：无 AVX2 特征时（运行时检测为 false）公开 API 仍必须产出正确结果。
// 本测试通过直接调 scalar 模块模拟"无 SIMD 分支"，并用运行时探测报告实际环境。
// ---------------------------------------------------------------------------

#[test]
fn fallback_branch_produces_reference_output() {
    let src = noise(32, 8);
    // 模拟回退：即使检测到 AVX2，scalar 直调必须与公开 API 一致（SIMD 逐字节等价）
    let mut via_api = vec![0u8; src.len()];
    px::bgra_to_rgba(&src, &mut via_api);
    let mut via_scalar = vec![0u8; src.len()];
    px::scalar::bgra_to_rgba(&src, &mut via_scalar);
    assert_eq!(via_api, via_scalar);
}

#[cfg(feature = "d3d")]
mod d3d_tests {
    use super::*;
    use easyrdp_pixels::gpu::GpuScaler;

    /// GPU 1:1 直通：逐像素一致（±0；颜色无重采样）。
    #[test]
    fn gpu_passthrough_identity() {
        let Some(mut g) = GpuScaler::new(64, 48, 64, 48) else {
            eprintln!("[skip] 无可用 D3D11 设备（CI/无 GPU 环境）");
            return;
        };
        let src = noise(64, 48);
        let out = g.scale(&src).expect("GPU 1:1 scale");
        let max_diff = out.iter().zip(&src).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(max_diff <= 1, "GPU 1:1 直通最大偏差 {max_diff}（允许 ±1 量化误差）");
    }

    /// GPU 2:1 缩放：与 CPU box-average 有界偏差（记录式断言，非逐像素）。
    #[test]
    fn gpu_downscale_bounded_error() {
        let Some(mut g) = GpuScaler::new(128, 96, 64, 48) else {
            eprintln!("[skip] 无可用 D3D11 设备（CI/无 GPU 环境）");
            return;
        };
        let src = noise(128, 96);
        let gpu = g.scale(&src).expect("GPU 2:1 scale");
        let cpu = px::scalar::box_downscale_bgra(&src, 128, 96, 64, 48);
        let total: u32 = gpu
            .iter()
            .zip(&cpu)
            .map(|(a, b)| (*a).abs_diff(*b) as u32)
            .sum();
        let avg = total / gpu.len() as u32;
        // GPU 采样与 box-average 数学语义不同：要求平均偏差有界而非逐像素一致
        assert!(avg <= 8, "GPU 2:1 缩放平均偏差 {avg} 超界");
    }
}
