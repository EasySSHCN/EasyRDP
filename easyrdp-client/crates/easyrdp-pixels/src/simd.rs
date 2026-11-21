//! AVX2/SSSE3 SIMD 快速通道（`gpu-accel` feature，x86_64）。
//!
//! 安全约定：`#[target_feature]` 内部函数只能在 [`crate::has_avx2()`] 为 true
//! 时调用（公开包装函数已强制检查；CPU 无 AVX2 时由 lib.rs 分发层走纯 Rust）。
//! 所有 SIMD 输出与 [`crate::scalar`] 参考实现**逐字节一致**（无浮点、无舍入差异：
//! halve 用 u16 整数和 + 右移 2 位，与 sum/4 截断完全等价）。

#![allow(clippy::missing_safety_doc)]

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// BGRA32 → RGBA32（AVX2 shuffle_epi8，每批 8 像素）。
pub fn bgra_to_rgba(src: &[u8], out: &mut [u8]) {
    let px = src.len() / 4;
    if out.len() < src.len() {
        return;
    }
    let mut i = 0usize;
    if crate::has_avx2() {
        // SAFETY: has_avx2() 为 true。
        unsafe {
            i = bgra_to_rgba_avx2(src, out, px);
        }
    }
    scalar_tail_rgba(src, out, i, px);
}

/// BGRA32 → RGB888 紧凑（SSSE3 shuffle_epi8，每批 4 像素）。
pub fn bgra_to_rgb888(src: &[u8], out: &mut [u8]) {
    let px = src.len() / 4;
    if out.len() < px * 3 {
        return;
    }
    let mut i = 0usize;
    if crate::has_avx2() {
        // SAFETY: has_avx2() 为 true。
        unsafe {
            i = bgra_to_rgb888_avx2(src, out, px);
        }
    }
    for p in i..px {
        let s = p * 4;
        let d = p * 3;
        out[d] = src[s];
        out[d + 1] = src[s + 1];
        out[d + 2] = src[s + 2];
    }
}

/// BGRA32 2×2 盒式降采样（AVX2：shuffle 去交错 + unpacklo u16 整数求和，
/// 每批 4 个输出像素=8 个源像素；尾部纯 Rust 补齐）。
/// sum/4 截断语义与纯 Rust 一致（右移 2 位 == 除 4 截断）。
pub fn halve_bgra(width: u32, height: u32, bgra: &[u8]) -> (u32, u32, Vec<u8>) {
    let w = width as usize;
    let h = height as usize;
    let (w2, h2) = (w / 2, h / 2);
    if w2 == 0 || h2 == 0 || bgra.len() < w * h * 4 {
        return (width, height, bgra[..bgra.len().min(w * h * 4)].to_vec());
    }
    let mut out = vec![0u8; w2 * h2 * 4];
    let mut simd_cols = 0usize;
    if crate::has_avx2() {
        // SAFETY: has_avx2() 为 true。
        unsafe {
            simd_cols = halve_rows_avx2(w, h2, w2, bgra, &mut out);
        }
    }
    // 尾部（每行 simd_cols 之后的输出像素）纯 Rust 补齐
    for y in 0..h2 {
        let y0 = y * 2;
        for x in simd_cols..w2 {
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

/// 两段字节严格相等（AVX2 pcmpeqb + movemask，每批 32 字节）。
pub fn bytes_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0usize;
    if crate::has_avx2() && a.len() >= 32 {
        // SAFETY: has_avx2() 为 true。
        unsafe {
            i = bytes_equal_avx2(a, b);
        }
    }
    a[i..] == b[i..]
}

// ---------------------------------------------------------------------------
// 标量尾部
// ---------------------------------------------------------------------------

fn scalar_tail_rgba(src: &[u8], out: &mut [u8], from: usize, px: usize) {
    for p in from..px {
        let s = p * 4;
        let (b, g, r, a) = (src[s], src[s + 1], src[s + 2], src[s + 3]);
        out[s] = r;
        out[s + 1] = g;
        out[s + 2] = b;
        out[s + 3] = a;
    }
}

// ---------------------------------------------------------------------------
// AVX2 内核（仅 has_avx2() == true 时调用）
// ---------------------------------------------------------------------------

#[target_feature(enable = "avx2,ssse3")]
unsafe fn bgra_to_rgba_avx2(src: &[u8], out: &mut [u8], px: usize) -> usize {
    // 每 128 位 lane：4 像素 BGRA→RGBA = shuffle [2,1,0,3, 6,5,4,7, 10,9,8,11, 14,13,12,15]
    let mask = _mm256_setr_epi8(
        2, 1, 0, 3, 6, 5, 4, 7, 10, 9, 8, 11, 14, 13, 12, 15, 2, 1, 0, 3, 6, 5, 4, 7, 10, 9, 8,
        11, 14, 13, 12, 15,
    );
    let mut p = 0usize;
    while p + 8 <= px {
        let v = _mm256_loadu_si256(src.as_ptr().add(p * 4) as *const __m256i);
        let r = _mm256_shuffle_epi8(v, mask);
        _mm256_storeu_si256(out.as_mut_ptr().add(p * 4) as *mut __m256i, r);
        p += 8;
    }
    p
}

#[target_feature(enable = "avx2,ssse3")]
unsafe fn bgra_to_rgb888_avx2(src: &[u8], out: &mut [u8], px: usize) -> usize {
    // 每 4 像素 16B → 12B：shuffle [0,1,2, 4,5,6, 8,9,10, 12,13,14, pad×4]
    let mask = _mm_setr_epi8(0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14, -1, -1, -1, -1);
    let groups = px / 4;
    for g in 0..groups {
        let v = _mm_loadu_si128(src.as_ptr().add(g * 16) as *const __m128i);
        let r = _mm_shuffle_epi8(v, mask);
        let dst = out.as_mut_ptr().add(g * 12);
        if g + 1 < groups {
            // 非末组：16B store 溢出的 4 字节会被下一组覆写
            _mm_storeu_si128(dst as *mut __m128i, r);
        } else {
            // 末组：栈缓冲再精确拷贝 12B，避免越界
            let mut tmp = [0u8; 16];
            _mm_storeu_si128(tmp.as_mut_ptr() as *mut __m128i, r);
            std::ptr::copy_nonoverlapping(tmp.as_ptr(), dst, 12);
        }
    }
    groups * 4
}

/// 2×2 halve AVX2 批处理（所有输出行）；返回每行已批处理的输出像素数（4 的倍数）。
/// 每批：每源行 32B（8 源像素）→ 4 个输出像素（16B）。
#[target_feature(enable = "avx2,ssse3")]
unsafe fn halve_rows_avx2(w: usize, h2: usize, w2: usize, bgra: &[u8], out: &mut [u8]) -> usize {
    // 通道去交错掩码（每 128 位 lane 4 个源像素）：取同通道 4 个字节。
    // 填充必须 0x80（shuffle 符号位置 0）。
    let mb = _mm256_setr_epi8(
        0, 4, 8, 12, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 4, 8, 12, -1, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1,
    );
    let mg = _mm256_setr_epi8(
        1, 5, 9, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 1, 5, 9, 13, -1, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1,
    );
    let mr = _mm256_setr_epi8(
        2, 6, 10, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 2, 6, 10, 14, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1, -1,
    );
    let ma = _mm256_setr_epi8(
        3, 7, 11, 15, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 3, 7, 11, 15, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1, -1,
    );
    // 跨列配对：a=[u16 0,2→位置0,1]、b=[u16 1,3→位置0,1]，p=a+b=[列对0和, 列对1和]
    let mu_a = _mm256_setr_epi8(
        0, 1, 4, 5, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 1, 4, 5, -1, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1,
    );
    let mq = _mm256_setr_epi8(
        2, 3, 6, 7, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 2, 3, 6, 7, -1, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1,
    );
    // u16 idx 0,1（= 两列对的部分和）→ 单字节
    let pack = _mm256_setr_epi8(
        0, 2, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 2, -1, -1, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1, -1,
    );
    let zero = _mm256_setzero_si256();
    // 每批 4 个输出像素 = 8 个源像素 = 32B/源行；输出 16B
    let batches = w2 / 4;
    for y in 0..h2 {
        let y0 = y * 2;
        let r0 = bgra.as_ptr().add(y0 * w * 4);
        let r1 = r0.add(w * 4);
        let dst = out.as_mut_ptr().add(y * w2 * 4);
        for b in 0..batches {
            let off = b * 32;
            let v0 = _mm256_loadu_si256(r0.add(off) as *const __m256i);
            let v1 = _mm256_loadu_si256(r1.add(off) as *const __m256i);
            // 逐通道：取 4 列 → u16 零扩展 → 行内跨列配对 + 行间相加 → >>2 → u8
            let ch = |m: __m256i| -> __m256i {
                // 位于 #[target_feature(enable = "avx2")] 函数内，无需额外 unsafe 块。
                let s0 = _mm256_unpacklo_epi8(_mm256_shuffle_epi8(v0, m), zero);
                let s1 = _mm256_unpacklo_epi8(_mm256_shuffle_epi8(v1, m), zero);
                // 列对部分和：[s[0]+s[1], s[2]+s[3]]（列对 0、列对 1 行内跨列）
                let p0 = _mm256_add_epi16(
                    _mm256_shuffle_epi8(s0, mu_a),
                    _mm256_shuffle_epi8(s0, mq),
                );
                let p1 = _mm256_add_epi16(
                    _mm256_shuffle_epi8(s1, mu_a),
                    _mm256_shuffle_epi8(s1, mq),
                );
                // 行间相加：idx0 = 列对0 全和（≤1020），idx1 = 列对1 全和
                let sum = _mm256_add_epi16(p0, p1);
                let mean = _mm256_srli_epi16(sum, 2); // == sum/4 截断
                _mm256_shuffle_epi8(mean, pack) // lane 字节 0..1 = 两个输出像素的通道值
            };
            let pb = ch(mb);
            let pg = ch(mg);
            let pr = ch(mr);
            let pa = ch(ma);
            // 交错回 BGRA：bg=[b g b g...]、ra=[r a r a...] → unpacklo_epi16 块交错
            // 得 [b g r a | b g r a | ...]（每 lane 2 个输出像素）
            let bg = _mm256_unpacklo_epi8(pb, pg);
            let ra = _mm256_unpacklo_epi8(pr, pa);
            let px = _mm256_unpacklo_epi16(bg, ra);
            let e0 = _mm256_castsi256_si128(px);
            let e1 = _mm256_extracti128_si256::<1>(px);
            _mm_storel_epi64(dst.add(b * 16) as *mut __m128i, e0);
            _mm_storel_epi64(dst.add(b * 16 + 8) as *mut __m128i, e1);
        }
    }
    batches * 4
}

#[target_feature(enable = "avx2")]
unsafe fn bytes_equal_avx2(a: &[u8], b: &[u8]) -> usize {
    let n = a.len() / 32 * 32;
    let mut i = 0usize;
    while i < n {
        let va = _mm256_loadu_si256(a.as_ptr().add(i) as *const __m256i);
        let vb = _mm256_loadu_si256(b.as_ptr().add(i) as *const __m256i);
        let eq = _mm256_cmpeq_epi8(va, vb);
        if _mm256_movemask_epi8(eq) != -1 {
            return i; // 不等：返回分歧块起点（尾部标量可继续细分，相等语义由调用方 a[..]==b[..] 兜底）
        }
        i += 32;
    }
    n
}
