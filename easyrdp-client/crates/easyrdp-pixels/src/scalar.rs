//! 纯 Rust 参考实现（正确性基准，任何 feature 下都可用）。

/// BGRA32 → RGBA32（逐字节通道重排）。
pub fn bgra_to_rgba(src: &[u8], out: &mut [u8]) {
    let px = src.len() / 4;
    debug_assert!(out.len() >= src.len());
    for i in 0..px {
        let s = i * 4;
        let (b, g, r, a) = (src[s], src[s + 1], src[s + 2], src[s + 3]);
        out[s] = r;
        out[s + 1] = g;
        out[s + 2] = b;
        out[s + 3] = a;
    }
}

/// BGRA32 → RGB888 紧凑（3 字节/像素）。
pub fn bgra_to_rgb888(src: &[u8], out: &mut [u8]) {
    let px = src.len() / 4;
    debug_assert!(out.len() >= px * 3);
    for i in 0..px {
        let s = i * 4;
        let d = i * 3;
        out[d] = src[s]; // B
        out[d + 1] = src[s + 1]; // G
        out[d + 2] = src[s + 2]; // R
    }
}

/// BGRA32 缓冲 2×2 盒式降采样（宽高各减半，奇数尾行列丢弃；sum/4 截断）。
pub fn halve_bgra(width: u32, height: u32, bgra: &[u8]) -> (u32, u32, Vec<u8>) {
    let w = width as usize;
    let h = height as usize;
    let (w2, h2) = (w / 2, h / 2);
    if w2 == 0 || h2 == 0 || bgra.len() < w * h * 4 {
        return (width, height, bgra[..bgra.len().min(w * h * 4)].to_vec());
    }
    let mut out = vec![0u8; w2 * h2 * 4];
    for y in 0..h2 {
        let y0 = y * 2;
        for x in 0..w2 {
            let x0 = x * 2;
            for c in 0..4 {
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

/// BGRA32(top-down) box-average 缩放到 dw×dh（每目标像素取源盒平均）。
/// 输入长度必须 = sw*sh*4（不足返回全 0，与 snapshot.rs 行为一致）。
pub fn box_downscale_bgra(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
    let expected = sw * sh * 4;
    if src.len() < expected || sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return vec![0u8; dw.max(1) * dh.max(1) * 4];
    }
    let mut out = vec![0u8; dw * dh * 4];
    for dy in 0..dh {
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
