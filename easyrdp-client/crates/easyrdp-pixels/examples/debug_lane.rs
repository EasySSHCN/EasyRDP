// 手动复现 halve AVX2 内核中间值（8x2 帧，逐打印）
#[cfg(all(target_arch = "x86_64", feature = "gpu-accel"))]
use std::arch::x86_64::*;

fn main() {
    #[cfg(all(target_arch = "x86_64", feature = "gpu-accel"))]
    unsafe {
        // 8x2：src[i] = i
        let mut src = vec![0u8; 64];
        for (i, b) in src.iter_mut().enumerate() {
            *b = i as u8;
        }
        let mb = _mm256_setr_epi8(
            0, 4, 8, 12, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 4, 8, 12, -1, -1, -1,
            -1, -1, -1, -1, -1, -1, -1, -1, -1,
        );
        let mq = _mm256_setr_epi8(
            2, 3, 6, 7, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 2, 3, 6, 7, -1, -1, -1,
            -1, -1, -1, -1, -1, -1, -1, -1, -1,
        );
        let pack = _mm256_setr_epi8(
            0, 2, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 2, -1, -1, -1, -1,
            -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
        );
        let zero = _mm256_setzero_si256();
        let v0 = _mm256_loadu_si256(src.as_ptr() as *const __m256i);
        let v1 = _mm256_loadu_si256(src.as_ptr().add(32) as *const __m256i);
        let sh0 = _mm256_shuffle_epi8(v0, mb);
        let sh1 = _mm256_shuffle_epi8(v1, mb);
        let mut t = [0u8; 32];
        let st = |v: __m256i, t: &mut [u8; 32]| {
            _mm256_storeu_si256(t.as_mut_ptr() as *mut __m256i, v);
        };
        st(sh0, &mut t);
        println!("sh0 lane0: {:?}", &t[0..8]);
        let s0 = _mm256_unpacklo_epi8(sh0, zero);
        let s1 = _mm256_unpacklo_epi8(sh1, zero);
        st(s0, &mut t);
        println!("s0 u16: {:?}", &t[0..16]);
        let p0 = _mm256_add_epi16(s0, _mm256_shuffle_epi8(s0, mq));
        let p1 = _mm256_add_epi16(s1, _mm256_shuffle_epi8(s1, mq));
        st(p0, &mut t);
        println!("p0 u16: {:?}", &t[0..8]);
        st(p1, &mut t);
        println!("p1 u16: {:?}", &t[0..8]);
        let sum = _mm256_add_epi16(p0, p1);
        let mean = _mm256_srli_epi16(sum, 2);
        st(mean, &mut t);
        println!("mean u16: {:?}", &t[0..8]);
        let pb = _mm256_shuffle_epi8(mean, pack);
        st(pb, &mut t);
        println!("pb bytes: {:?}", &t[0..8]);
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "gpu-accel")))]
    {
        println!("need --features gpu-accel");
    }
}
