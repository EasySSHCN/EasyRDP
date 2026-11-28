use easyrdp_pixels::{halve_bgra, scalar};

fn main() {
    // 8x2 帧：像素 i = B=i*4, G=i*4+1, R=i*4+2, A=i*4+3
    let mut src = vec![0u8; 8 * 2 * 4];
    for (i, b) in src.iter_mut().enumerate() {
        *b = i as u8;
    }
    let (_, _, simd) = halve_bgra(8, 2, &src);
    let (_, _, scal) = scalar::halve_bgra(8, 2, &src);
    for p in 0..4 {
        println!(
            "px{p}: simd={:?} scalar={:?}",
            &simd[p * 4..p * 4 + 4],
            &scal[p * 4..p * 4 + 4]
        );
    }
    println!("src px0..3 row0: {:?}", &src[0..16]);
    println!("src px0..3 row1: {:?}", &src[32..48]);
}
