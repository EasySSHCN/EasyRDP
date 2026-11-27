//! 基准对比：1080p 帧各热点操作 CPU 基线 vs SIMD vs GPU（若实现）。
//!
//! 运行：
//!   cargo run --release -p easyrdp-pixels --example bench
//!   cargo run --release -p easyrdp-pixels --example bench --features gpu-accel
//!   cargo run --release -p easyrdp-pixels --example bench --features gpu-accel,d3d

use easyrdp_pixels as px;
use std::time::Instant;

fn noise(w: usize, h: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(w * h * 4);
    let mut s = 0x12345678u64;
    for _ in 0..w * h * 4 {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        v.push((s >> 33) as u8);
    }
    v
}

fn bench<F: FnMut()>(name: &str, iters: u32, mut f: F) -> f64 {
    f(); // 预热
    let t = Instant::now();
    for _ in 0..iters {
        f();
    }
    let us = t.elapsed().as_secs_f64() * 1e6 / iters as f64;
    println!("{name:<46} {us:>10.1} µs/帧");
    us
}

fn main() {
    let (w, h) = (1920usize, 1080usize);
    let frame = noise(w, h);
    let half = (w / 2, h / 2);
    println!("=== EasyRDP 本地帧管线热点 bench（{w}x{h}）===");
    println!(
        "feature: gpu-accel={} d3d={}",
        cfg!(feature = "gpu-accel"),
        cfg!(feature = "d3d")
    );
    println!(
        "CPU: AVX2={} SSSE3={}",
        std::arch::is_x86_feature_detected!("avx2"),
        std::arch::is_x86_feature_detected!("ssse3")
    );

    // 1) BGRA→RGBA
    let mut rgba = vec![0u8; frame.len()];
    let s = bench("BGRA→RGBA  scalar（CPU 基线）", 100, || {
        px::scalar::bgra_to_rgba(&frame, &mut rgba);
    });
    let f = if cfg!(feature = "gpu-accel") {
        bench("BGRA→RGBA  公开 API（SIMD 分发）", 100, || {
            px::bgra_to_rgba(&frame, &mut rgba);
        })
    } else {
        s
    };
    if cfg!(feature = "gpu-accel") {
        println!("           → 加速比 {:.2}x\n", s / f);
    }

    // 2) BGRA→RGB888
    let mut rgb = vec![0u8; w * h * 3];
    let s = bench("BGRA→RGB888 scalar（CPU 基线）", 100, || {
        px::scalar::bgra_to_rgb888(&frame, &mut rgb);
    });
    let f = if cfg!(feature = "gpu-accel") {
        bench("BGRA→RGB888 公开 API（SIMD 分发）", 100, || {
            px::bgra_to_rgb888(&frame, &mut rgb);
        })
    } else {
        s
    };
    if cfg!(feature = "gpu-accel") {
        println!("           → 加速比 {:.2}x\n", s / f);
    }

    // 3) 2×2 halve（JPEG 编码前置，运动期每帧执行）
    let s = bench("2×2 halve  scalar（CPU 基线）", 200, || {
        let _ = px::scalar::halve_bgra(w as u32, h as u32, &frame);
    });
    let f = if cfg!(feature = "gpu-accel") {
        bench("2×2 halve  公开 API（SIMD 分发）", 200, || {
            let _ = px::halve_bgra(w as u32, h as u32, &frame);
        })
    } else {
        s
    };
    if cfg!(feature = "gpu-accel") {
        println!("           → 加速比 {:.2}x\n", s / f);
    }

    // 4) 脏 tile diff
    let mut other = frame.clone();
    for i in (0..other.len()).step_by(4097) {
        other[i] ^= 0xAA;
    }
    let s = bench("脏 tile diff(32px) scalar", 60, || {
        let _ = px::diff::diff_tiles(&frame, &other, w as u32, h as u32, 32);
    });
    let f = if cfg!(feature = "gpu-accel") {
        bench("脏 tile diff(32px) 公开 API（SIMD）", 60, || {
            let _ = px::diff_tiles(&frame, &other, w as u32, h as u32, 32);
        })
    } else {
        s
    };
    let _ = f;

    // 5) box-average 缩放（快照缩略图路径，scalar only——非每帧热点）
    bench("box 缩放 1920x1080→320x180（CPU）", 10, || {
        let _ = px::box_downscale_bgra(&frame, w as u32, h as u32, 320, 180);
    });

    // 6) GPU 路径（d3d feature 时启用）
    #[cfg(feature = "d3d")]
    {
        if let Some(mut g) = easyrdp_pixels::gpu::GpuScaler::new(
            w as u32,
            h as u32,
            half.0 as u32,
            half.1 as u32,
        ) {
            bench("GPU 1080p→960x540（D3D11 VideoProcessor）", 100, || {
                let _ = g.scale(&frame);
            });
            let (out, src) = easyrdp_pixels::gpu::scale_with_fallback(
                &frame, w as u32, h as u32, half.0 as u32, half.1 as u32,
            );
            println!("           回退分发当前选择: {src}（{}B）", out.len());
        } else {
            println!("GPU 1080p→960x540                          [skip] 设备创建失败→回退 CPU");
        }
    }

    println!("=== 完成 ===");
}
