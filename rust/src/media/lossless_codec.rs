//! O codec "sem perda" (`docs/hibrido.md`, etapa 1): só os retângulos que
//! mudaram, compactados com zstd — direto, ou como XOR do quadro anterior,
//! o que der menor. CPU pura, sem DLL e sem patente; o texto chega com o
//! pixel exato e a cor completa.
//!
//! - [`Packet`]: o formato (versão, flags, tamanho da tela, retângulos e um
//!   bloco zstd com os pixels deles, em ordem);
//! - [`LosslessEncoder`]: no caminho de CPU do motor (o BGRA lido só nas
//!   regiões alteradas);
//! - [`LosslessDecoder`]: guarda a tela inteira e aplica os retângulos.
//!
//! O delta só vale porque o stream de vídeo é confiável e em ordem; quem
//! entra no meio espera o keyframe (a tela inteira, sem delta).

use std::time::Instant;

use crate::media::damage::Region;
use crate::media::decode::{BgraFrame, VideoDecoder};
use crate::media::encode::{EncodeTiming, EncodedPacket, EncoderConfig, VideoEncoder};
use crate::media::gpu::Device;
use crate::media::lossless::{clamp, join_region, xor_region};
use crate::media::{MediaError, Result};
use crate::proto::codec::Codec;

/// Nome do encoder (como os do libavcodec: `h264_amf`, `openh264`…).
pub const BACKEND: &str = "lossless";

/// Versão do formato do pacote.
const VERSION: u8 = 1;
const FLAG_KEY: u8 = 1;
const FLAG_DELTA: u8 = 2;
/// Nível do zstd: o mais rápido que ainda compacta bem conteúdo de tela.
const ZSTD_LEVEL: i32 = 1;

/// Um quadro sem perda.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// A tela inteira (quem acabou de entrar pode começar daqui).
    pub key: bool,
    /// Os pixels são o XOR com o quadro anterior (senão, os próprios).
    pub delta: bool,
    pub width: u32,
    pub height: u32,
    pub rects: Vec<Region>,
    /// zstd dos pixels BGRA dos retângulos, um depois do outro.
    pub payload: Vec<u8>,
}

impl Packet {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.rects.len() * 8 + self.payload.len());
        out.push(VERSION);
        out.push(if self.key { FLAG_KEY } else { 0 } | if self.delta { FLAG_DELTA } else { 0 });
        for v in [self.width as u16, self.height as u16, self.rects.len() as u16] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for r in &self.rects {
            for v in [r.left, r.top, r.right - r.left, r.bottom - r.top] {
                out.extend_from_slice(&(v as u16).to_le_bytes());
            }
        }
        out.extend_from_slice(&self.payload);
        out
    }

    /// Lê um pacote, conferindo que os retângulos cabem na tela.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let bad = |why: &str| MediaError::Msg(format!("pacote sem perda inválido: {why}"));
        let mut words = bytes.get(2..8).ok_or_else(|| bad("curto"))?.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as u32);
        if bytes[0] != VERSION {
            return Err(bad(&format!("versão {}", bytes[0])));
        }
        let (width, height, count) = (words.next().unwrap_or(0), words.next().unwrap_or(0), words.next().unwrap_or(0) as usize);
        let table = bytes.get(8..8 + count * 8).ok_or_else(|| bad("lista de retângulos cortada"))?;
        let rects = table
            .chunks(8)
            .map(|c| {
                let v = |i: usize| u16::from_le_bytes([c[i], c[i + 1]]) as u32;
                Region { left: v(0), top: v(2), right: v(0) + v(4), bottom: v(2) + v(6) }
            })
            .collect::<Vec<_>>();
        if rects.iter().any(|r| r.right > width || r.bottom > height) {
            return Err(bad("retângulo fora da tela"));
        }
        let flags = bytes[1];
        Ok(Self { key: flags & FLAG_KEY != 0, delta: flags & FLAG_DELTA != 0, width, height, rects, payload: bytes[8 + count * 8..].to_vec() })
    }

    /// Quantos bytes de pixels os retângulos somam.
    pub fn pixel_bytes(&self) -> usize {
        self.rects.iter().map(|r| r.area() as usize * 4).sum()
    }
}

/// O encoder: guarda o quadro anterior (pro delta) e os buffers de trabalho.
pub struct LosslessEncoder {
    width: u32,
    height: u32,
    /// O quadro anterior, BGRA sem sobra no fim das linhas.
    previous: Vec<u8>,
    joined: Vec<u8>,
    delta: Vec<u8>,
    /// Já mandou um keyframe (o viewer tem a tela inteira).
    primed: bool,
    timing: EncodeTiming,
}

impl LosslessEncoder {
    pub fn open(cfg: &EncoderConfig) -> Result<Self> {
        if cfg.codec != Codec::Lossless {
            return Err(MediaError::Msg(format!("o encoder sem perda não faz {}", cfg.codec.label())));
        }
        if cfg.width > u16::MAX as u32 || cfg.height > u16::MAX as u32 {
            return Err(MediaError::Msg(format!("tela grande demais pro sem perda: {}x{}", cfg.width, cfg.height)));
        }
        log::info!("encoder sem perda {}x{}", cfg.width, cfg.height);
        Ok(Self {
            width: cfg.width,
            height: cfg.height,
            previous: vec![0; cfg.width as usize * cfg.height as usize * 4],
            joined: Vec::new(),
            delta: Vec::new(),
            primed: false,
            timing: EncodeTiming::default(),
        })
    }

    /// Codifica um quadro a partir do BGRA (`stride` bytes por linha).
    fn pack(&mut self, bgra: &[u8], stride: usize, regions: Option<&[Region]>, force_key: bool) -> Option<Packet> {
        let key = force_key || !self.primed;
        let whole = [Region { left: 0, top: 0, right: self.width, bottom: self.height }];
        let regions = if key { &whole[..] } else { regions.unwrap_or(&whole) };
        let rects: Vec<Region> = regions.iter().map(|r| clamp(r, self.width, self.height)).filter(|r| !r.is_empty()).collect();
        if rects.is_empty() {
            return None;
        }
        self.joined.clear();
        self.delta.clear();
        for r in &rects {
            join_region(bgra, stride, r, &mut self.joined);
            xor_region(&mut self.previous, self.width, bgra, stride, r, &mut self.delta);
        }
        let direct = zstd::bulk::compress(&self.joined, ZSTD_LEVEL).ok()?;
        // Keyframe: o viewer pode não ter o anterior; o delta não serve.
        let (delta, payload) = match (!key).then(|| zstd::bulk::compress(&self.delta, ZSTD_LEVEL).ok()).flatten() {
            Some(d) if d.len() < direct.len() => (true, d),
            _ => (false, direct),
        };
        self.primed = true;
        Some(Packet { key, delta, width: self.width, height: self.height, rects, payload })
    }
}

impl VideoEncoder for LosslessEncoder {
    fn name(&self) -> &str {
        BACKEND
    }

    fn codec(&self) -> Codec {
        Codec::Lossless
    }

    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Só pelo BGRA (caminho de CPU): o NV12 da placa já perdeu a cor completa.
    fn encode(&mut self, _dev: &Device, _nv12: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D, _force_key: bool) -> Result<Vec<EncodedPacket>> {
        Err(MediaError::Msg("o sem perda só codifica pelo BGRA (caminho de CPU)".into()))
    }

    fn accepts_bgra(&self) -> bool {
        true
    }

    fn encode_bgra(&mut self, bgra: &[u8], stride: usize, regions: Option<&[Region]>, force_key: bool) -> Result<Vec<EncodedPacket>> {
        let t = Instant::now();
        let packet = self.pack(bgra, stride, regions, force_key);
        self.timing = EncodeTiming { upload_us: 0, codec_us: t.elapsed().as_micros() as u32 };
        Ok(packet.map(|p| EncodedPacket { key: p.key, data: p.to_bytes() }).into_iter().collect())
    }

    fn last_timing(&self) -> EncodeTiming {
        self.timing
    }
}

/// O decoder: a tela inteira, atualizada a cada pacote.
#[derive(Default)]
pub struct LosslessDecoder {
    /// Largura, altura e o BGRA (só depois do 1º keyframe).
    screen: Option<(u32, u32, Vec<u8>)>,
}

impl LosslessDecoder {
    /// Aplica um pacote; `None` enquanto não chegou o keyframe.
    pub fn apply(&mut self, packet: &Packet) -> Result<Option<BgraFrame>> {
        if packet.key {
            self.screen = Some((packet.width, packet.height, vec![0; packet.width as usize * packet.height as usize * 4]));
        }
        let Some((w, h, screen)) = self.screen.as_mut().filter(|(w, h, _)| (*w, *h) == (packet.width, packet.height)) else {
            // Delta de outra tela (ou antes do keyframe): espera o próximo keyframe.
            return Ok(None);
        };
        let want = packet.pixel_bytes();
        let pixels = zstd::bulk::decompress(&packet.payload, want).map_err(|e| MediaError::Msg(format!("sem perda: {e}")))?;
        if pixels.len() != want {
            return Err(MediaError::Msg(format!("sem perda: {} bytes de pixels, esperava {want}", pixels.len())));
        }
        let row = *w as usize * 4;
        let mut at = 0;
        for r in &packet.rects {
            let (x0, x1) = (r.left as usize * 4, r.right as usize * 4);
            for y in r.top as usize..r.bottom as usize {
                let dst = &mut screen[y * row + x0..y * row + x1];
                let src = &pixels[at..at + dst.len()];
                if packet.delta {
                    dst.iter_mut().zip(src).for_each(|(d, s)| *d ^= s);
                } else {
                    dst.copy_from_slice(src);
                }
                // A captura pode vir com alfa zero; a tela é opaca.
                dst.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
                at += src.len();
            }
        }
        Ok(Some(BgraFrame { width: *w, height: *h, data: screen.clone() }))
    }
}

impl VideoDecoder for LosslessDecoder {
    fn codec(&self) -> Codec {
        Codec::Lossless
    }

    fn is_hardware(&self) -> bool {
        false
    }

    fn decode(&mut self, packet: &[u8]) -> Result<Option<BgraFrame>> {
        self.apply(&Packet::parse(packet)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(w: u32, h: u32) -> EncoderConfig {
        EncoderConfig { codec: Codec::Lossless, backend: BACKEND.into(), width: w, height: h, fps: 30, bitrate_kbps: 1000, vbv_frames: 1, chroma444: false }
    }

    /// Tela `w`×`h` (alfa 255) com `pad` bytes de sobra por linha.
    fn screen(w: u32, h: u32, pad: usize, f: impl Fn(u32, u32) -> [u8; 3]) -> (Vec<u8>, usize) {
        let stride = w as usize * 4 + pad;
        let mut v = vec![0; stride * h as usize];
        for y in 0..h {
            for x in 0..w {
                let [b, g, r] = f(x, y);
                let i = y as usize * stride + x as usize * 4;
                v[i..i + 4].copy_from_slice(&[b, g, r, 255]);
            }
        }
        (v, stride)
    }

    fn tight(img: &[u8], stride: usize, w: u32, h: u32) -> Vec<u8> {
        (0..h as usize).flat_map(|y| img[y * stride..y * stride + w as usize * 4].to_vec()).collect()
    }

    fn roundtrip(enc: &mut LosslessEncoder, dec: &mut LosslessDecoder, img: &[u8], stride: usize, regions: Option<&[Region]>, key: bool) -> Option<BgraFrame> {
        let packets = enc.encode_bgra(img, stride, regions, key).unwrap();
        packets.iter().map(|p| dec.decode(&p.data).unwrap()).last().flatten()
    }

    #[test]
    fn packet_roundtrips() {
        let p = Packet { key: false, delta: true, width: 640, height: 480, rects: vec![Region { left: 10, top: 20, right: 30, bottom: 25 }], payload: vec![1, 2, 3] };
        assert_eq!(Packet::parse(&p.to_bytes()).unwrap(), p);
    }

    #[test]
    fn broken_packets_are_refused() {
        let p = Packet { key: true, delta: false, width: 8, height: 8, rects: vec![Region { left: 0, top: 0, right: 8, bottom: 8 }], payload: vec![] };
        let bytes = p.to_bytes();
        assert!(Packet::parse(&bytes[..5]).is_err(), "curto");
        assert!(Packet::parse(&bytes[..12]).is_err(), "lista cortada");
        let mut outside = p.clone();
        outside.rects[0].right = 9;
        assert!(Packet::parse(&outside.to_bytes()).is_err(), "fora da tela");
        let mut other = bytes.clone();
        other[0] = 99;
        assert!(Packet::parse(&other).is_err(), "versão");
    }

    #[test]
    fn first_frame_is_the_whole_screen_and_arrives_exact() {
        let (img, stride) = screen(37, 21, 12, |x, y| [x as u8, y as u8, (x * y) as u8]);
        let (mut enc, mut dec) = (LosslessEncoder::open(&cfg(37, 21)).unwrap(), LosslessDecoder::default());
        // Mesmo pedindo só um pedaço: sem keyframe ainda, vai a tela inteira.
        let got = roundtrip(&mut enc, &mut dec, &img, stride, Some(&[Region { left: 0, top: 0, right: 2, bottom: 2 }]), false).unwrap();
        assert_eq!((got.width, got.height), (37, 21));
        assert_eq!(got.data, tight(&img, stride, 37, 21));
    }

    #[test]
    fn changed_regions_update_the_screen_exactly() {
        let (a, stride) = screen(64, 48, 0, |x, y| [(x * 3) as u8, (y * 5) as u8, 7]);
        let (b, _) = screen(64, 48, 0, |x, y| if (10..30).contains(&x) && (5..15).contains(&y) { [200, (x + y) as u8, 9] } else { [(x * 3) as u8, (y * 5) as u8, 7] });
        let (mut enc, mut dec) = (LosslessEncoder::open(&cfg(64, 48)).unwrap(), LosslessDecoder::default());
        roundtrip(&mut enc, &mut dec, &a, stride, None, false).unwrap();
        let got = roundtrip(&mut enc, &mut dec, &b, stride, Some(&[Region { left: 10, top: 5, right: 30, bottom: 15 }]), false).unwrap();
        assert_eq!(got.data, tight(&b, stride, 64, 48));
    }

    #[test]
    fn a_small_change_in_a_big_region_goes_as_delta() {
        let (a, stride) = screen(256, 128, 0, |x, y| [(x ^ y) as u8, (x * y) as u8, (x + 2 * y) as u8]);
        let (b, _) = screen(256, 128, 0, |x, y| if x == 100 && y == 50 { [1, 2, 3] } else { [(x ^ y) as u8, (x * y) as u8, (x + 2 * y) as u8] });
        let mut enc = LosslessEncoder::open(&cfg(256, 128)).unwrap();
        enc.pack(&a, stride, None, false).unwrap();
        let p = enc.pack(&b, stride, None, false).unwrap();
        assert!(p.delta && !p.key, "pouca coisa mudou na área: delta");
        let mut dec = LosslessDecoder::default();
        let mut enc2 = LosslessEncoder::open(&cfg(256, 128)).unwrap();
        roundtrip(&mut enc2, &mut dec, &a, stride, None, false);
        assert_eq!(roundtrip(&mut enc2, &mut dec, &b, stride, None, false).unwrap().data, tight(&b, stride, 256, 128));
    }

    #[test]
    fn deltas_before_the_keyframe_are_ignored() {
        let (a, stride) = screen(16, 16, 0, |x, _| [x as u8, 0, 0]);
        let mut enc = LosslessEncoder::open(&cfg(16, 16)).unwrap();
        enc.encode_bgra(&a, stride, None, false).unwrap();
        let later = enc.encode_bgra(&a, stride, Some(&[Region { left: 0, top: 0, right: 4, bottom: 4 }]), false).unwrap();
        // Viewer que entrou agora: o pacote não é keyframe.
        let mut late = LosslessDecoder::default();
        assert_eq!(late.decode(&later[0].data).unwrap(), None);
        let key = enc.encode_bgra(&a, stride, None, true).unwrap();
        assert!(key[0].key);
        assert_eq!(late.decode(&key[0].data).unwrap().unwrap().data, tight(&a, stride, 16, 16));
    }

    #[test]
    fn alpha_from_the_capture_does_not_matter() {
        let (mut img, stride) = screen(4, 4, 0, |_, _| [9, 8, 7]);
        img.iter_mut().skip(3).step_by(4).for_each(|a| *a = 0);
        let (mut enc, mut dec) = (LosslessEncoder::open(&cfg(4, 4)).unwrap(), LosslessDecoder::default());
        let got = roundtrip(&mut enc, &mut dec, &img, stride, None, false).unwrap();
        assert!(got.data.chunks(4).all(|p| p == [9, 8, 7, 255]));
    }

    #[test]
    fn nothing_changed_sends_nothing() {
        let (img, stride) = screen(8, 8, 0, |_, _| [1, 1, 1]);
        let mut enc = LosslessEncoder::open(&cfg(8, 8)).unwrap();
        enc.encode_bgra(&img, stride, None, false).unwrap();
        assert!(enc.encode_bgra(&img, stride, Some(&[]), false).unwrap().is_empty());
    }

    #[test]
    fn only_lossless_is_accepted() {
        let mut c = cfg(8, 8);
        c.codec = Codec::H264;
        assert!(LosslessEncoder::open(&c).is_err());
    }
}
