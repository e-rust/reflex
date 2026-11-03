//! YUV 4:2:0 ↔ BGRA, BT.709 de faixa limitada (o mesmo do conversor da GPU).
//! Feito à mão em vez do swscale: o swscale usa BT.601 por padrão e as cores
//! sairiam levemente erradas.
//!
//! Volta (×256): R = 1,164(Y−16) + 1,793(V−128);
//! G = 1,164(Y−16) − 0,213(U−128) − 0,533(V−128); B = 1,164(Y−16) + 2,112(U−128).
//!
//! Ida (×65536, ver [`bgra_to_nv12`]): Y = 16 + 219/255·(0,2126R + 0,7152G + 0,0722B);
//! U e V = 128 + 224/255·(…) sobre a média de cada bloco 2×2 (o que a amostragem
//! bilinear do shader faz).

#[inline]
fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

#[inline]
fn pixel(y: u8, u: u8, v: u8) -> [u8; 4] {
    let c = (y as i32 - 16) * 298 + 128;
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    [clamp((c + 541 * d) >> 8), clamp((c - 55 * d - 136 * e) >> 8), clamp((c + 459 * e) >> 8), 255]
}

/// NV12 (Y + UV intercalado) → BGRA compacto (`w*4` por linha).
#[allow(clippy::too_many_arguments)]
pub fn nv12_to_bgra(y: &[u8], y_stride: usize, uv: &[u8], uv_stride: usize, w: usize, h: usize, out: &mut [u8]) {
    for row in 0..h {
        let yr = &y[row * y_stride..];
        let uvr = &uv[(row / 2) * uv_stride..];
        let o = &mut out[row * w * 4..(row + 1) * w * 4];
        for col in 0..w {
            let ci = (col / 2) * 2;
            o[col * 4..col * 4 + 4].copy_from_slice(&pixel(yr[col], uvr[ci], uvr[ci + 1]));
        }
    }
}

/// I420 (Y, U, V separados) → BGRA compacto.
#[allow(clippy::too_many_arguments)]
pub fn i420_to_bgra(y: &[u8], y_stride: usize, u: &[u8], u_stride: usize, v: &[u8], v_stride: usize, w: usize, h: usize, out: &mut [u8]) {
    for row in 0..h {
        let yr = &y[row * y_stride..];
        let ur = &u[(row / 2) * u_stride..];
        let vr = &v[(row / 2) * v_stride..];
        let o = &mut out[row * w * 4..(row + 1) * w * 4];
        for col in 0..w {
            o[col * 4..col * 4 + 4].copy_from_slice(&pixel(yr[col], ur[col / 2], vr[col / 2]));
        }
    }
}

// Coeficientes da ida, ×65536. Cada linha de U e V soma zero (cinza → 128).
const YR: i32 = 11966;
const YG: i32 = 40254;
const YB: i32 = 4064;
const UR: i32 = -6597;
const UG: i32 = -22187;
const UB: i32 = 28784;
const VR: i32 = 28784;
const VG: i32 = -26148;
const VB: i32 = -2636;

#[inline]
fn luma(p: &[u8]) -> u8 {
    let (b, g, r) = (p[0] as i32, p[1] as i32, p[2] as i32);
    (16 + ((YR * r + YG * g + YB * b + 32768) >> 16)) as u8
}

/// Um plano NV12 de destino: Y e UV intercalado, com seus strides.
pub struct Nv12Planes<'a> {
    pub y: &'a mut [u8],
    pub y_stride: usize,
    pub uv: &'a mut [u8],
    pub uv_stride: usize,
}

/// BGRA (`src_stride` bytes por linha) → NV12, `w`×`h` pares. Pura.
///
/// Com SSSE3 (qualquer x86-64 dos últimos ~15 anos), 4 pixels por vez com a
/// MESMA aritmética inteira — resultado idêntico ao laço simples, ~6× mais
/// rápido (numa VM de 1 núcleo, 2560×1600 levava 12 ms por quadro).
pub fn bgra_to_nv12(src: &[u8], src_stride: usize, w: usize, h: usize, out: Nv12Planes) {
    let Nv12Planes { y, y_stride, uv, uv_stride } = out;
    let simd = simd_available();
    for pair in 0..h / 2 {
        let (r0, r1) = (&src[2 * pair * src_stride..], &src[(2 * pair + 1) * src_stride..]);
        let rows = &mut y[2 * pair * y_stride..];
        let (y0, y1) = rows.split_at_mut(y_stride.min(rows.len()));
        let uvr = &mut uv[pair * uv_stride..];
        let done = if simd { nv12_pair_simd(r0, r1, y0, y1, uvr, w) } else { 0 };
        nv12_pair_scalar(r0, r1, y0, y1, uvr, done / 2, w);
    }
}

/// Um par de linhas a partir do bloco 2×2 `from_cx`.
fn nv12_pair_scalar(r0: &[u8], r1: &[u8], y0: &mut [u8], y1: &mut [u8], uvr: &mut [u8], from_cx: usize, w: usize) {
    for cx in from_cx..w / 2 {
        let (a, b) = (&r0[cx * 8..cx * 8 + 8], &r1[cx * 8..cx * 8 + 8]);
        y0[2 * cx] = luma(&a[..4]);
        y0[2 * cx + 1] = luma(&a[4..]);
        y1[2 * cx] = luma(&b[..4]);
        y1[2 * cx + 1] = luma(&b[4..]);
        let sb = a[0] as i32 + a[4] as i32 + b[0] as i32 + b[4] as i32;
        let sg = a[1] as i32 + a[5] as i32 + b[1] as i32 + b[5] as i32;
        let sr = a[2] as i32 + a[6] as i32 + b[2] as i32 + b[6] as i32;
        // Soma de 4 pixels: divide por 4 junto com o ×65536 (>> 18).
        uvr[2 * cx] = (128 + ((UR * sr + UG * sg + UB * sb + 131072) >> 18)) as u8;
        uvr[2 * cx + 1] = (128 + ((VR * sr + VG * sg + VB * sb + 131072) >> 18)) as u8;
    }
}

#[cfg(target_arch = "x86_64")]
fn simd_available() -> bool {
    std::arch::is_x86_feature_detected!("ssse3")
}

#[cfg(not(target_arch = "x86_64"))]
fn simd_available() -> bool {
    false
}

/// Um par de linhas em blocos de 4 pixels; devolve quantos pixels fez.
#[cfg(target_arch = "x86_64")]
fn nv12_pair_simd(r0: &[u8], r1: &[u8], y0: &mut [u8], y1: &mut [u8], uvr: &mut [u8], w: usize) -> usize {
    let n = w / 4 * 4;
    if r0.len() < n * 4 || r1.len() < n * 4 || y0.len() < n || y1.len() < n || uvr.len() < n {
        return 0;
    }
    // SAFETY: SSSE3 conferido em `simd_available`; os tamanhos acima cobrem
    // todas as leituras e escritas.
    unsafe { nv12_pair_ssse3(r0, r1, y0, y1, uvr, n) };
    n
}

#[cfg(not(target_arch = "x86_64"))]
fn nv12_pair_simd(_: &[u8], _: &[u8], _: &mut [u8], _: &mut [u8], _: &mut [u8], _: usize) -> usize {
    0
}

/// Y de 4 pixels BGRA (i32 por pista). Exato: `madd` soma produtos de 16
/// bits em 32. O verde vai duas vezes com metade do peso (40254 não cabe em
/// i16), no lugar do alfa.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
fn luma4(v: std::arch::x86_64::__m128i) -> std::arch::x86_64::__m128i {
    use std::arch::x86_64::*;
    let lo = _mm_setr_epi8(0, -128, 1, -128, 2, -128, 1, -128, 4, -128, 5, -128, 6, -128, 5, -128);
    let hi = _mm_setr_epi8(8, -128, 9, -128, 10, -128, 9, -128, 12, -128, 13, -128, 14, -128, 13, -128);
    let k = _mm_setr_epi16(YB as i16, (YG / 2) as i16, YR as i16, (YG / 2) as i16, YB as i16, (YG / 2) as i16, YR as i16, (YG / 2) as i16);
    let sum = _mm_hadd_epi32(_mm_madd_epi16(_mm_shuffle_epi8(v, lo), k), _mm_madd_epi16(_mm_shuffle_epi8(v, hi), k));
    _mm_add_epi32(_mm_srai_epi32(_mm_add_epi32(sum, _mm_set1_epi32(32768)), 16), _mm_set1_epi32(16))
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
unsafe fn nv12_pair_ssse3(r0: &[u8], r1: &[u8], y0: &mut [u8], y1: &mut [u8], uvr: &mut [u8], n: usize) {
    use std::arch::x86_64::*;
    let zero = _mm_setzero_si128();
    let ku = _mm_setr_epi16(UB as i16, UG as i16, UR as i16, 0, UB as i16, UG as i16, UR as i16, 0);
    let kv = _mm_setr_epi16(VB as i16, VG as i16, VR as i16, 0, VB as i16, VG as i16, VR as i16, 0);
    let mut x = 0;
    while x < n {
        let a = _mm_loadu_si128(r0.as_ptr().add(x * 4) as *const __m128i);
        let b = _mm_loadu_si128(r1.as_ptr().add(x * 4) as *const __m128i);
        // Y: 4 pixels de cada linha → 8 bytes (linha de cima, linha de baixo).
        let ys = _mm_packus_epi16(_mm_packs_epi32(luma4(a), luma4(b)), zero);
        let ys = (_mm_cvtsi128_si64(ys) as u64).to_le_bytes();
        std::ptr::copy_nonoverlapping(ys.as_ptr(), y0.as_mut_ptr().add(x), 4);
        std::ptr::copy_nonoverlapping(ys.as_ptr().add(4), y1.as_mut_ptr().add(x), 4);
        // UV: soma de cada bloco 2×2 (b, g, r, a em 16 bits) e os pesos.
        let left = _mm_add_epi16(_mm_unpacklo_epi8(a, zero), _mm_unpacklo_epi8(b, zero));
        let right = _mm_add_epi16(_mm_unpackhi_epi8(a, zero), _mm_unpackhi_epi8(b, zero));
        let blocks = _mm_unpacklo_epi64(_mm_add_epi16(left, _mm_srli_si128(left, 8)), _mm_add_epi16(right, _mm_srli_si128(right, 8)));
        // [U0, U1, V0, V1] → [U0, V0, U1, V1].
        let uv = _mm_hadd_epi32(_mm_madd_epi16(blocks, ku), _mm_madd_epi16(blocks, kv));
        let uv = _mm_add_epi32(_mm_srai_epi32(_mm_add_epi32(uv, _mm_set1_epi32(131072)), 18), _mm_set1_epi32(128));
        let uv = _mm_shuffle_epi32(uv, 0b11_01_10_00);
        let uv = (_mm_cvtsi128_si32(_mm_packus_epi16(_mm_packs_epi32(uv, zero), zero)) as u32).to_le_bytes();
        std::ptr::copy_nonoverlapping(uv.as_ptr(), uvr.as_mut_ptr().add(x), 4);
        x += 4;
    }
}

/// Três planos de 4:4:4 (Y, U, V do tamanho da imagem), com seus strides.
pub struct Yuv444Planes<'a> {
    pub y: &'a mut [u8],
    pub u: &'a mut [u8],
    pub v: &'a mut [u8],
    pub stride: usize,
}

/// BGRA → YUV 4:4:4 planar (cor em resolução cheia), `w`×`h`. Pura.
pub fn bgra_to_yuv444(src: &[u8], src_stride: usize, w: usize, h: usize, out: Yuv444Planes) {
    let Yuv444Planes { y, u, v, stride } = out;
    for row in 0..h {
        let s = &src[row * src_stride..];
        let (yr, ur, vr) = (&mut y[row * stride..], &mut u[row * stride..], &mut v[row * stride..]);
        for col in 0..w {
            let p = &s[col * 4..col * 4 + 4];
            let (b, g, r) = (p[0] as i32, p[1] as i32, p[2] as i32);
            yr[col] = luma(p);
            ur[col] = (128 + ((UR * r + UG * g + UB * b + 32768) >> 16)) as u8;
            vr[col] = (128 + ((VR * r + VG * g + VB * b + 32768) >> 16)) as u8;
        }
    }
}

/// [`bgra_to_yuv444`] em faixas horizontais, uma por thread.
pub fn bgra_to_yuv444_par(src: &[u8], src_stride: usize, w: usize, h: usize, out: Yuv444Planes, threads: usize) {
    let per = h.div_ceil(threads.max(1)).max(1);
    let Yuv444Planes { mut y, mut u, mut v, stride } = out;
    std::thread::scope(|s| {
        let mut first = 0;
        while first < h {
            let n = per.min(h - first);
            let len = if first + n == h { usize::MAX } else { n * stride };
            let (yb, ub, vb) = (take_band(&mut y, len), take_band(&mut u, len), take_band(&mut v, len));
            let src_band = &src[first * src_stride..];
            s.spawn(move || bgra_to_yuv444(src_band, src_stride, w, n, Yuv444Planes { y: yb, u: ub, v: vb, stride }));
            first += n;
        }
    });
}

/// Tira os primeiros `len` bytes de `plane` (todos, se `len` passar do fim).
fn take_band<'a>(plane: &mut &'a mut [u8], len: usize) -> &'a mut [u8] {
    let p = std::mem::take(plane);
    let (band, rest) = p.split_at_mut(len.min(p.len()));
    *plane = rest;
    band
}

/// YUV 4:4:4 planar → BGRA compacto.
#[allow(clippy::too_many_arguments)]
pub fn yuv444_to_bgra(y: &[u8], y_stride: usize, u: &[u8], u_stride: usize, v: &[u8], v_stride: usize, w: usize, h: usize, out: &mut [u8]) {
    for row in 0..h {
        let (yr, ur, vr) = (&y[row * y_stride..], &u[row * u_stride..], &v[row * v_stride..]);
        let o = &mut out[row * w * 4..(row + 1) * w * 4];
        for col in 0..w {
            o[col * 4..col * 4 + 4].copy_from_slice(&pixel(yr[col], ur[col], vr[col]));
        }
    }
}

/// [`bgra_to_nv12`] dividido em faixas horizontais, uma por thread.
pub fn bgra_to_nv12_par(src: &[u8], src_stride: usize, w: usize, h: usize, out: Nv12Planes, threads: usize) {
    let pairs = h / 2;
    let per = pairs.div_ceil(threads.max(1)).max(1);
    let Nv12Planes { mut y, y_stride, mut uv, uv_stride } = out;
    std::thread::scope(|s| {
        let mut first = 0;
        while first < pairs {
            let n = per.min(pairs - first);
            let last = first + n == pairs;
            let (y_band, y_rest) = if last { (std::mem::take(&mut y), &mut [][..]) } else { y.split_at_mut(2 * n * y_stride) };
            let (uv_band, uv_rest) = if last { (std::mem::take(&mut uv), &mut [][..]) } else { uv.split_at_mut(n * uv_stride) };
            y = y_rest;
            uv = uv_rest;
            let src_band = &src[2 * first * src_stride..];
            s.spawn(move || bgra_to_nv12(src_band, src_stride, w, 2 * n, Nv12Planes { y: y_band, y_stride, uv: uv_band, uv_stride }));
            first += n;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // Valores que o conversor da GPU (e o OBS) produz pra branco/preto/vermelho.
    #[test]
    fn white_black_red_roundtrip() {
        assert_eq!(pixel(235, 128, 128), [255, 255, 255, 255]);
        assert_eq!(pixel(16, 128, 128), [0, 0, 0, 255]);
        let red = pixel(63, 102, 240);
        assert!(red[2] >= 250 && red[1] <= 3 && red[0] <= 3, "{red:?}");
    }

    #[test]
    fn nv12_shares_chroma_between_2x2_pixels() {
        // 2x2: Y todos 235 (branco), UV neutro.
        let y = [235u8, 235, 235, 235];
        let uv = [128u8, 128];
        let mut out = [0u8; 16];
        nv12_to_bgra(&y, 2, &uv, 2, 2, 2, &mut out);
        assert!(out.iter().all(|&b| b == 255));
    }

    #[test]
    fn i420_matches_nv12() {
        let y = [100u8, 120, 140, 160];
        let mut a = [0u8; 16];
        let mut b = [0u8; 16];
        nv12_to_bgra(&y, 2, &[90, 170], 2, 2, 2, &mut a);
        i420_to_bgra(&y, 2, &[90], 1, &[170], 1, 2, 2, &mut b);
        assert_eq!(a, b);
    }

    fn to_nv12(bgra: &[u8], w: usize, h: usize) -> (Vec<u8>, Vec<u8>) {
        let (mut y, mut uv) = (vec![0u8; w * h], vec![0u8; w * h / 2]);
        bgra_to_nv12(bgra, w * 4, w, h, Nv12Planes { y: &mut y, y_stride: w, uv: &mut uv, uv_stride: w });
        (y, uv)
    }

    #[test]
    fn bgra_to_nv12_matches_the_gpu_reference_values() {
        // Branco, preto e vermelho: os mesmos valores que o shader produz.
        let solid = |b: u8, g: u8, r: u8| [b, g, r, 255].repeat(4);
        assert_eq!(to_nv12(&solid(255, 255, 255), 2, 2), (vec![235; 4], vec![128, 128]));
        assert_eq!(to_nv12(&solid(0, 0, 0), 2, 2), (vec![16; 4], vec![128, 128]));
        assert_eq!(to_nv12(&solid(0, 0, 255), 2, 2), (vec![63; 4], vec![102, 240]));
    }

    #[test]
    fn bgra_to_nv12_roundtrips_within_rounding() {
        let px = [40u8, 160, 220, 255];
        let (y, uv) = to_nv12(&px.repeat(4), 2, 2);
        let back = pixel(y[0], uv[0], uv[1]);
        for c in 0..3 {
            assert!((back[c] as i32 - px[c] as i32).abs() <= 2, "{back:?} vs {px:?}");
        }
    }

    #[test]
    fn chroma_is_the_average_of_the_2x2_block() {
        // Metade branca, metade preta: crominância neutra, luma de cada um.
        let mut bgra = [255u8, 255, 255, 255].repeat(2);
        bgra.extend([0u8, 0, 0, 255].repeat(2));
        let (y, uv) = to_nv12(&bgra, 2, 2);
        assert_eq!(y, vec![235, 235, 16, 16]);
        assert_eq!(uv, vec![128, 128]);
    }

    #[test]
    fn simd_equals_the_plain_loop_on_any_width() {
        for (w, h) in [(4usize, 2usize), (6, 4), (13, 6), (64, 8), (130, 6)] {
            let stride = w * 4 + 12;
            let src: Vec<u8> = (0..stride * h).map(|i| (i.wrapping_mul(2654435761) >> 7) as u8).collect();
            let (mut y, mut uv) = (vec![0u8; w * h], vec![0u8; w.div_ceil(2) * 2 * h / 2]);
            bgra_to_nv12(&src, stride, w, h, Nv12Planes { y: &mut y, y_stride: w, uv: &mut uv, uv_stride: w.div_ceil(2) * 2 });
            let (mut y2, mut uv2) = (vec![0u8; w * h], vec![0u8; w.div_ceil(2) * 2 * h / 2]);
            for pair in 0..h / 2 {
                let (r0, r1) = (&src[2 * pair * stride..], &src[(2 * pair + 1) * stride..]);
                let rows = &mut y2[2 * pair * w..];
                let (a, b) = rows.split_at_mut(w);
                nv12_pair_scalar(r0, r1, a, b, &mut uv2[pair * w.div_ceil(2) * 2..], 0, w);
            }
            assert_eq!((y, uv), (y2, uv2), "{w}x{h}");
        }
    }

    #[test]
    #[ignore = "medida de tempo"]
    fn nv12_speed() {
        let (w, h) = (2560usize, 1600usize);
        let src: Vec<u8> = (0..w * h * 4).map(|i| (i * 31 % 253) as u8).collect();
        let (mut y, mut uv) = (vec![0u8; w * h], vec![0u8; w * h / 2]);
        let t = std::time::Instant::now();
        for _ in 0..20 {
            bgra_to_nv12(&src, w * 4, w, h, Nv12Planes { y: &mut y, y_stride: w, uv: &mut uv, uv_stride: w });
        }
        let simd = t.elapsed() / 20;
        let t = std::time::Instant::now();
        for _ in 0..20 {
            for pair in 0..h / 2 {
                let (r0, r1) = (&src[2 * pair * w * 4..], &src[(2 * pair + 1) * w * 4..]);
                let rows = &mut y[2 * pair * w..];
                let (a, b) = rows.split_at_mut(w);
                nv12_pair_scalar(r0, r1, a, b, &mut uv[pair * w..], 0, w);
            }
        }
        println!("2560x1600 → NV12: {simd:?} (laço simples {:?})", t.elapsed() / 20);
    }

    #[test]
    #[ignore = "medida de tempo"]
    fn nv12_to_bgra_speed() {
        let (w, h) = (2560usize, 1600usize);
        let y: Vec<u8> = (0..w * h).map(|i| (i * 7 % 220 + 16) as u8).collect();
        let uv: Vec<u8> = (0..w * h / 2).map(|i| (i * 13 % 200 + 20) as u8).collect();
        let mut out = vec![0u8; w * h * 4];
        let t = std::time::Instant::now();
        for _ in 0..10 {
            nv12_to_bgra(&y, w, &uv, w, w, h, &mut out);
        }
        println!("NV12 → BGRA 2560x1600: {:?}", t.elapsed() / 10);
    }

    #[test]
    fn parallel_conversion_equals_serial_with_padded_strides() {
        let (w, h) = (6usize, 10usize);
        let src_stride = w * 4 + 8;
        let src: Vec<u8> = (0..src_stride * h).map(|i| (i * 37 % 251) as u8).collect();
        let run = |threads: usize| {
            let (ys, uvs) = (w + 2, w + 4);
            let (mut y, mut uv) = (vec![0u8; ys * h], vec![0u8; uvs * h / 2]);
            let planes = Nv12Planes { y: &mut y, y_stride: ys, uv: &mut uv, uv_stride: uvs };
            if threads == 0 {
                bgra_to_nv12(&src, src_stride, w, h, planes);
            } else {
                bgra_to_nv12_par(&src, src_stride, w, h, planes, threads);
            }
            (y, uv)
        };
        let serial = run(0);
        for t in [1, 2, 3, 4, 16] {
            assert_eq!(run(t), serial, "{t} threads");
        }
    }

    fn to_444(bgra: &[u8], w: usize, h: usize, threads: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let stride = w + 3;
        let (mut y, mut u, mut v) = (vec![0u8; stride * h], vec![0u8; stride * h], vec![0u8; stride * h]);
        bgra_to_yuv444_par(bgra, w * 4, w, h, Yuv444Planes { y: &mut y, u: &mut u, v: &mut v, stride }, threads);
        (y, u, v)
    }

    #[test]
    fn yuv444_keeps_the_color_of_every_pixel() {
        // Vermelho e azul lado a lado: no 4:2:0 a cor dos dois vira a média;
        // no 4:4:4 cada pixel volta com a sua.
        let mut bgra = [0u8, 0, 255, 255].to_vec();
        bgra.extend([255u8, 0, 0, 255]);
        let (y, u, v) = to_444(&bgra, 2, 1, 1);
        let stride = 2 + 3;
        let mut out = [0u8; 8];
        yuv444_to_bgra(&y, stride, &u, stride, &v, stride, 2, 1, &mut out);
        for (got, want) in [(&out[..3], [0u8, 0, 255]), (&out[4..7], [255, 0, 0])] {
            for c in 0..3 {
                assert!((got[c] as i32 - want[c] as i32).abs() <= 3, "{got:?} vs {want:?}");
            }
        }
    }

    #[test]
    fn yuv444_parallel_equals_serial() {
        let (w, h) = (5usize, 9usize);
        let src: Vec<u8> = (0..w * 4 * h).map(|i| (i * 53 % 251) as u8).collect();
        let serial = to_444(&src, w, h, 1);
        for t in [2, 3, 4, 16] {
            assert_eq!(to_444(&src, w, h, t), serial, "{t} threads");
        }
    }

    #[test]
    fn strides_are_respected() {
        // Linha com folga (stride 4 pra largura 2).
        let y = [235u8, 235, 0, 0, 16, 16, 0, 0];
        let uv = [128u8, 128, 0, 0];
        let mut out = [0u8; 16];
        nv12_to_bgra(&y, 4, &uv, 4, 2, 2, &mut out);
        assert_eq!(&out[..4], &[255, 255, 255, 255]);
        assert_eq!(&out[8..12], &[0, 0, 0, 255]);
    }
}
