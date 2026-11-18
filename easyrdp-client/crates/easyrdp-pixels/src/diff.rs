//! 脏矩形（tile）diff：两帧 BGRA 逐 tile 比较找差异区域。
//!
//! 这是"帧差计算"热点：全帧比较 1080p ≈ 2M 像素，纯 Rust 逐字节
//! `!=` 循环在 `gpu-accel` 下由 AVX2 `pcmpeqb`+`movemask` 每 32 字节
//! 一批替代（SIMD 与纯 Rust 判定结果完全一致：都是严格逐字节相等）。

/// 逐字节相等（SIMD 分发点在启用 gpu-accel 时覆盖）。
pub(crate) fn bytes_equal(a: &[u8], b: &[u8]) -> bool {
    #[cfg(all(feature = "gpu-accel", target_arch = "x86_64"))]
    {
        if crate::has_avx2() {
            return crate::simd::bytes_equal(a, b);
        }
    }
    a == b
}

/// 两帧 BGRA（w×h×4）脏 tile 检测：按 tile 网格扫描，返回差异 tile 的
/// `(x, y, tw, th)`（右/下边界裁边后的实际尺寸），扫描序（先 y 后 x）。
pub fn diff_tiles(a: &[u8], b: &[u8], w: u32, h: u32, tile: u32) -> Vec<(u32, u32, u32, u32)> {
    let (w, h) = (w as usize, h as usize);
    let tile = tile.max(1) as usize;
    if w == 0 || h == 0 || a.len() < w * h * 4 || b.len() < w * h * 4 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut ty = 0usize;
    while ty < h {
        let th = tile.min(h - ty);
        let mut tx = 0usize;
        while tx < w {
            let tw = tile.min(w - tx);
            let mut dirty = false;
            'rows: for y in ty..ty + th {
                let s = (y * w + tx) * 4;
                let seg = tw * 4;
                if !bytes_equal(&a[s..s + seg], &b[s..s + seg]) {
                    dirty = true;
                    break 'rows;
                }
            }
            if dirty {
                out.push((tx as u32, ty as u32, tw as u32, th as u32));
            }
            tx += tile;
        }
        ty += tile;
    }
    out
}
