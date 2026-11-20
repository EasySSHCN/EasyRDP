//! EasyRDP 本地帧管线像素处理（F3-V2 R-20261118-017）。
//!
//! 定位（重要）：RDP 协议图形压缩由 FreeRDP 与服务端协商（RFX/NSCodec/H.264 RemoteFX），
//! 本 crate **不**做"GPU H.264 编码 RDP 流"；它是**本地帧管线的 GPU 加速**——
//! 把每帧执行的 CPU 热点（BGRA→RGB888 / BGRA→RGBA 转换、2×2 盒式降采样、
//! 脏矩形 tile diff、box-average 缩放）换成 SIMD 或 D3D11 路径。
//!
//! 分层：
//! - [`scalar`]：纯 Rust 参考实现（始终编译，正确性基准）；
//! - `gpu-accel` feature：AVX2/SSSE3 intrinsics（[`simd`]），运行时
//!   `is_x86_feature_detected!` 检测，CPU 不支持自动回退 [`scalar`]；
//! - `d3d` feature：D3D11 VideoProcessor GPU 缩放（[`gpu`]），设备创建失败
//!   自动回退 CPU/SIMD。GPU 输出允许 ±1 量化误差（见 `gpu` 模块注释）。
//!
//! 稳定性约束：所有 SIMD/GPU 代码只在启用对应 feature 且编译进调用方后才生效；
//! 未启用 feature 时行为与纯 Rust 基线逐位一致，不影响 GDI 直绘主路径。

pub mod diff;
pub mod scalar;

#[cfg(all(feature = "gpu-accel", target_arch = "x86_64"))]
pub mod simd;

#[cfg(feature = "d3d")]
#[cfg(target_os = "windows")]
pub mod gpu;

use std::sync::atomic::{AtomicU8, Ordering};

/// 运行时 CPU 特性缓存：0=未探测 1=无 AVX2 2=有 AVX2（含 SSSE3）。
static AVX2_STATE: AtomicU8 = AtomicU8::new(0);

#[cfg(all(feature = "gpu-accel", target_arch = "x86_64"))]
pub(crate) fn has_avx2() -> bool {
    match AVX2_STATE.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => {
            let ok = std::arch::is_x86_feature_detected!("avx2");
            AVX2_STATE.store(if ok { 2 } else { 1 }, Ordering::Relaxed);
            ok
        }
    }
}

#[cfg(not(all(feature = "gpu-accel", target_arch = "x86_64")))]
pub(crate) fn has_avx2() -> bool {
    false
}

// ---------------------------------------------------------------------------
// 对外统一入口（运行时分发：SIMD 可用走 SIMD，否则纯 Rust）
// ---------------------------------------------------------------------------

/// BGRA32(top-down) → RGBA32(top-down) 通道重排。
/// `out.len()` 必须 ≥ `src.len()`。
pub fn bgra_to_rgba(src: &[u8], out: &mut [u8]) {
    #[cfg(all(feature = "gpu-accel", target_arch = "x86_64"))]
    {
        if has_avx2() {
            return simd::bgra_to_rgba(src, out);
        }
    }
    scalar::bgra_to_rgba(src, out);
}

/// BGRA32(top-down) → RGB888（紧凑 3 字节/像素）。
/// `out.len()` 必须 ≥ `src.len()/4*3`。
pub fn bgra_to_rgb888(src: &[u8], out: &mut [u8]) {
    #[cfg(all(feature = "gpu-accel", target_arch = "x86_64"))]
    {
        if has_avx2() {
            return simd::bgra_to_rgb888(src, out);
        }
    }
    scalar::bgra_to_rgb888(src, out);
}

/// BGRA32 缓冲 2×2 盒式降采样（宽高各减半，奇数尾行列丢弃）。
/// 与 `easyrdp-rdp::frame_jpeg::halve_bgra` 同语义（sum/4 截断）。
pub fn halve_bgra(width: u32, height: u32, bgra: &[u8]) -> (u32, u32, Vec<u8>) {
    #[cfg(all(feature = "gpu-accel", target_arch = "x86_64"))]
    {
        if has_avx2() {
            return simd::halve_bgra(width, height, bgra);
        }
    }
    scalar::halve_bgra(width, height, bgra)
}

/// BGRA32(top-down) box-average 缩放到 dw×dh（每目标像素取源盒平均）。
/// 与 `easyrdp-app::snapshot::downscale_bgra` 同语义。
pub fn box_downscale_bgra(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    scalar::box_downscale_bgra(src, sw, sh, dw, dh)
}

/// 两帧 BGRA 之间的脏 tile 检测（见 [`diff`]）。
pub fn diff_tiles(a: &[u8], b: &[u8], w: u32, h: u32, tile: u32) -> Vec<(u32, u32, u32, u32)> {
    diff::diff_tiles(a, b, w, h, tile)
}
