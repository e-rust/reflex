//! Medida do híbrido imagem + vídeo (`docs/pendencias.md`): quanto os
//! retângulos alterados de cada quadro ocupariam mandados como imagem sem
//! perda, pra comparar com o vídeo nos mesmos quadros. É diagnóstico: o motor
//! só mede quando o viewer pede (`Control::MeasureLossless`).
//!
//! Por quadro, os retângulos vão juntos num buffer (como iriam pela rede) e
//! são compactados de três jeitos: lz4, zstd e zstd do XOR com o quadro
//! anterior — dentro de um retângulo alterado, o que não mudou vira zero.

use std::time::Instant;

use crate::media::damage::Region;

/// Nível do zstd: o mais rápido que ainda compacta bem conteúdo de tela.
const ZSTD_LEVEL: i32 = 1;

/// O que um intervalo de quadros mediu (somas).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LosslessStats {
    pub frames: u32,
    /// Pixels alterados, em bytes BGRA (sem compactar).
    pub raw_bytes: u64,
    pub lz4_bytes: u64,
    pub zstd_bytes: u64,
    /// zstd do XOR com o quadro anterior.
    pub delta_bytes: u64,
    pub lz4_us: u64,
    pub zstd_us: u64,
    pub delta_us: u64,
    /// Da área alterada, quanto só mudou de lugar (rolagem, arrasto): no
    /// híbrido vira "copie daqui pra lá", sem pixel.
    pub changed_px: u64,
    pub moved_px: u64,
}

impl std::ops::AddAssign for LosslessStats {
    fn add_assign(&mut self, o: Self) {
        self.frames += o.frames;
        self.raw_bytes += o.raw_bytes;
        self.lz4_bytes += o.lz4_bytes;
        self.zstd_bytes += o.zstd_bytes;
        self.delta_bytes += o.delta_bytes;
        self.lz4_us += o.lz4_us;
        self.zstd_us += o.zstd_us;
        self.delta_us += o.delta_us;
        self.changed_px += o.changed_px;
        self.moved_px += o.moved_px;
    }
}

impl LosslessStats {
    /// Uma linha pro relatório do motor (`secs`: a duração do intervalo).
    pub fn summary(&self, secs: f64) -> String {
        let kbs = |b: u64| (b as f64 / secs.max(0.001) / 1024.0) as u64;
        let ms = |us: u64| us as f64 / 1000.0 / f64::from(self.frames.max(1));
        let moved = (self.moved_px * 100).checked_div(self.changed_px).unwrap_or(0);
        format!(
            "sem perda ({} quadros): cru {} KB/s → lz4 {} KB/s ({:.1} ms), zstd {} KB/s ({:.1} ms), zstd do delta {} KB/s ({:.1} ms); {moved}% da área só se moveu",
            self.frames,
            kbs(self.raw_bytes),
            kbs(self.lz4_bytes),
            ms(self.lz4_us),
            kbs(self.zstd_bytes),
            ms(self.zstd_us),
            kbs(self.delta_bytes),
            ms(self.delta_us),
        )
    }
}

/// O medidor: guarda o quadro anterior (pro delta) e os buffers de trabalho.
#[derive(Default)]
pub struct LosslessMeter {
    // (Debug à mão abaixo: os buffers têm megabytes.)
    width: u32,
    height: u32,
    /// O quadro anterior, BGRA sem sobra no fim das linhas.
    previous: Vec<u8>,
    joined: Vec<u8>,
    delta: Vec<u8>,
}

impl std::fmt::Debug for LosslessMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LosslessMeter({}x{})", self.width, self.height)
    }
}

impl LosslessMeter {
    /// Mede um quadro: `bgra` com `stride` bytes por linha, `width`×`height`;
    /// `regions` são as partes alteradas (`None`: a tela inteira).
    pub fn measure(&mut self, bgra: &[u8], stride: usize, width: u32, height: u32, regions: Option<&[Region]>) -> LosslessStats {
        if (width, height) != (self.width, self.height) {
            // Resolução nova: sem quadro anterior (o delta é a própria imagem).
            (self.width, self.height) = (width, height);
            self.previous = vec![0; width as usize * height as usize * 4];
        }
        let whole = [Region { left: 0, top: 0, right: width, bottom: height }];
        let regions = regions.unwrap_or(&whole);
        self.joined.clear();
        self.delta.clear();
        for r in regions.iter().map(|r| clamp(r, width, height)).filter(|r| !r.is_empty()) {
            join_region(bgra, stride, &r, &mut self.joined);
            xor_region(&mut self.previous, width, bgra, stride, &r, &mut self.delta);
        }
        let mut s = LosslessStats { frames: 1, raw_bytes: self.joined.len() as u64, ..Default::default() };
        if self.joined.is_empty() {
            return s;
        }
        let t = Instant::now();
        s.lz4_bytes = lz4_flex::block::compress(&self.joined).len() as u64;
        s.lz4_us = t.elapsed().as_micros() as u64;
        let t = Instant::now();
        s.zstd_bytes = zstd::bulk::compress(&self.joined, ZSTD_LEVEL).map_or(s.raw_bytes, |c| c.len() as u64);
        s.zstd_us = t.elapsed().as_micros() as u64;
        let t = Instant::now();
        s.delta_bytes = zstd::bulk::compress(&self.delta, ZSTD_LEVEL).map_or(s.raw_bytes, |c| c.len() as u64);
        s.delta_us = t.elapsed().as_micros() as u64;
        s
    }
}

impl LosslessMeter {
    /// Quanto o codec sem perda mandaria por este quadro (o menor entre o
    /// zstd direto e o do delta, como ele faz). Mais barato que [`measure`]:
    /// sem o lz4 nem os tempos.
    ///
    /// [`measure`]: LosslessMeter::measure
    pub fn estimate(&mut self, bgra: &[u8], stride: usize, width: u32, height: u32, regions: Option<&[Region]>) -> usize {
        if (width, height) != (self.width, self.height) {
            (self.width, self.height) = (width, height);
            self.previous = vec![0; width as usize * height as usize * 4];
        }
        let whole = [Region { left: 0, top: 0, right: width, bottom: height }];
        self.joined.clear();
        self.delta.clear();
        for r in regions.unwrap_or(&whole).iter().map(|r| clamp(r, width, height)).filter(|r| !r.is_empty()) {
            join_region(bgra, stride, &r, &mut self.joined);
            xor_region(&mut self.previous, width, bgra, stride, &r, &mut self.delta);
        }
        if self.joined.is_empty() {
            return 0;
        }
        let size = |b: &[u8]| zstd::bulk::compress(b, ZSTD_LEVEL).map_or(b.len(), |c| c.len());
        size(&self.joined).min(size(&self.delta))
    }
}

/// O retângulo dentro da tela.
pub(crate) fn clamp(r: &Region, width: u32, height: u32) -> Region {
    Region { left: r.left.min(width), top: r.top.min(height), right: r.right.min(width), bottom: r.bottom.min(height) }
}

/// As linhas de `r` (BGRA) no fim de `out`, sem a sobra do `stride`. Pura.
pub fn join_region(bgra: &[u8], stride: usize, r: &Region, out: &mut Vec<u8>) {
    let (x0, x1) = (r.left as usize * 4, r.right as usize * 4);
    for y in r.top as usize..r.bottom as usize {
        out.extend_from_slice(&bgra[y * stride + x0..y * stride + x1]);
    }
}

/// O XOR de `r` com o quadro anterior no fim de `out`, e o anterior passa a
/// ser este (só em `r`). Pura.
pub fn xor_region(previous: &mut [u8], width: u32, bgra: &[u8], stride: usize, r: &Region, out: &mut Vec<u8>) {
    let (x0, x1) = (r.left as usize * 4, r.right as usize * 4);
    let row = width as usize * 4;
    for y in r.top as usize..r.bottom as usize {
        let now = &bgra[y * stride + x0..y * stride + x1];
        let before = &mut previous[y * row + x0..y * row + x1];
        out.extend(now.iter().zip(before.iter()).map(|(a, b)| a ^ b));
        before.copy_from_slice(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Imagem `w`×`h` com `pad` bytes de sobra por linha; o pixel (x, y) vale `f(x, y)`.
    fn image(w: u32, h: u32, pad: usize, f: impl Fn(u32, u32) -> [u8; 4]) -> (Vec<u8>, usize) {
        let stride = w as usize * 4 + pad;
        let mut v = vec![0xEE; stride * h as usize];
        for y in 0..h {
            for x in 0..w {
                let i = y as usize * stride + x as usize * 4;
                v[i..i + 4].copy_from_slice(&f(x, y));
            }
        }
        (v, stride)
    }

    #[test]
    fn regions_are_joined_without_the_row_padding() {
        let (img, stride) = image(4, 3, 8, |x, y| [x as u8, y as u8, 0, 255]);
        let mut out = Vec::new();
        join_region(&img, stride, &Region { left: 1, top: 1, right: 3, bottom: 3 }, &mut out);
        assert_eq!(out, vec![1, 1, 0, 255, 2, 1, 0, 255, 1, 2, 0, 255, 2, 2, 0, 255]);
    }

    #[test]
    fn delta_is_zero_where_nothing_changed_and_tracks_the_previous_frame() {
        let (a, stride) = image(2, 1, 0, |_, _| [10, 20, 30, 255]);
        let (b, _) = image(2, 1, 0, |x, _| if x == 0 { [10, 20, 30, 255] } else { [11, 20, 30, 255] });
        let r = Region { left: 0, top: 0, right: 2, bottom: 1 };
        let mut prev = vec![0; 8];
        let mut out = Vec::new();
        xor_region(&mut prev, 2, &a, stride, &r, &mut out);
        assert_eq!(out, a, "sem anterior, o delta é a imagem");
        out.clear();
        xor_region(&mut prev, 2, &b, stride, &r, &mut out);
        assert_eq!(out, vec![0, 0, 0, 0, 1, 0, 0, 0]);
    }

    #[test]
    fn only_the_changed_area_is_counted() {
        let (img, stride) = image(64, 32, 16, |x, y| [(x * 7) as u8, (y * 3) as u8, 0, 255]);
        let mut m = LosslessMeter::default();
        let s = m.measure(&img, stride, 64, 32, Some(&[Region { left: 0, top: 0, right: 10, bottom: 10 }]));
        assert_eq!(s.raw_bytes, 10 * 10 * 4);
        assert!(s.lz4_bytes > 0 && s.zstd_bytes > 0);
        let all = m.measure(&img, stride, 64, 32, None);
        assert_eq!(all.raw_bytes, 64 * 32 * 4);
    }

    #[test]
    fn a_repeated_frame_compresses_to_almost_nothing_as_delta() {
        let (img, stride) = image(256, 128, 0, |x, y| [(x ^ y) as u8, (x * y) as u8, (x + y) as u8, 255]);
        let mut m = LosslessMeter::default();
        let first = m.measure(&img, stride, 256, 128, None);
        let again = m.measure(&img, stride, 256, 128, None);
        assert!(again.delta_bytes * 20 < first.delta_bytes, "delta {} contra {}", again.delta_bytes, first.delta_bytes);
    }

    #[test]
    fn the_estimate_is_the_smaller_of_direct_and_delta() {
        let (img, stride) = image(128, 64, 0, |x, y| [(x ^ y) as u8, (x * y) as u8, (x + y) as u8, 255]);
        let mut m = LosslessMeter::default();
        let first = m.estimate(&img, stride, 128, 64, None);
        assert!(first > 0);
        // A mesma tela de novo: o delta é tudo zero, quase nada.
        let again = m.estimate(&img, stride, 128, 64, None);
        assert!(again * 20 < first, "{again} contra {first}");
        assert_eq!(m.estimate(&img, stride, 128, 64, Some(&[])), 0, "nada mudou");
    }

    #[test]
    fn out_of_screen_regions_are_clipped() {
        let (img, stride) = image(8, 8, 0, |_, _| [1, 2, 3, 255]);
        let s = LosslessMeter::default().measure(&img, stride, 8, 8, Some(&[Region { left: 4, top: 4, right: 99, bottom: 99 }]));
        assert_eq!(s.raw_bytes, 4 * 4 * 4);
    }

    #[test]
    fn summary_reports_rates_and_moved_share() {
        let s = LosslessStats { frames: 10, raw_bytes: 10 * 1024, lz4_bytes: 4 * 1024, zstd_bytes: 2 * 1024, delta_bytes: 1024, changed_px: 100, moved_px: 25, ..Default::default() };
        let line = s.summary(1.0);
        assert!(line.contains("cru 10 KB/s") && line.contains("lz4 4 KB/s") && line.contains("zstd 2 KB/s") && line.contains("delta 1 KB/s"), "{line}");
        assert!(line.contains("25% da área só se moveu"), "{line}");
    }
}
