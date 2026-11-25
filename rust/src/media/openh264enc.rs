//! H.264 pela CPU com o **openh264** da Cisco (BSD) — o encoder das máquinas
//! sem placa de vídeo (VMs, servidores). Troca o `libx264`, que é GPL (§4.2).
//!
//! A biblioteca é o binário que a Cisco distribui: só ele tem a licença de
//! patente do H.264 paga pela Cisco, e só vale se for baixado da Cisco por
//! quem usa — o `reflex --install` baixa ([`download`]), e o pacote do Reflex
//! não leva a DLL. O hash é conferido antes de carregar (versões conhecidas).
//!
//! Entra I420; o quadro fica guardado entre um envio e outro e, no caminho
//! BGRA, só o que mudou na tela é convertido de novo (como no encoder do
//! libavcodec).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use openh264::encoder::{self as oh, BitRate, Complexity, FrameRate, FrameType, IntraFramePeriod, RateControlMode, UsageType, VuiConfig};
use openh264::formats::YUVSlices;
use openh264::OpenH264API;
use crate::proto::codec::Codec;

use crate::media::damage::Region;
use crate::media::encode::{conversion_threads, parameter_sets, with_headers, EncodeTiming, EncodedPacket, EncoderConfig, VideoEncoder};
use crate::media::yuv::{bgra_to_i420, bgra_to_i420_par, I420Planes};
use crate::media::{MediaError, Result};

// O NV12 da placa (Windows com placa, mas sem encoder de hardware).
#[cfg(windows)]
use crate::media::gpu::Device;
#[cfg(windows)]
use crate::media::yuv::nv12_to_i420;
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::{ID3D11Texture2D, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ};

/// Nome do encoder (como os do libavcodec: `h264_amf`, `libx264`…).
pub const BACKEND: &str = "openh264";

/// O binário da Cisco que o Reflex usa (o crate `openh264` confere o hash).
pub const DLL: &str = "openh264-2.7.0-win64.dll";

/// De onde baixar: o servidor da Cisco (a licença de patente exige que venha de lá).
pub const DOWNLOAD_URL: &str = "http://ciscobinary.openh264.org/openh264-2.7.0-win64.dll.bz2";

/// Onde está a DLL: `REFLEX_OPENH264_DIR`, a pasta do exe (o `runtime\` do
/// projeto, ou a da instalação), ou `runtime\` num ancestral.
pub fn dll_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("REFLEX_OPENH264_DIR") {
        return Some(PathBuf::from(dir).join(DLL));
    }
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    if dir.join(DLL).exists() {
        return Some(dir.join(DLL));
    }
    for _ in 0..5 {
        let runtime = dir.join("runtime").join(DLL);
        if runtime.exists() {
            return Some(runtime);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

/// A DLL, conferida (hash de uma versão conhecida) uma vez por processo.
fn verified_dll() -> std::result::Result<&'static Path, String> {
    static CHECKED: OnceLock<std::result::Result<PathBuf, String>> = OnceLock::new();
    CHECKED
        .get_or_init(|| {
            let path = dll_path().ok_or_else(|| format!("{DLL} não encontrada (o `reflex --install` baixa da Cisco)"))?;
            OpenH264API::from_blob_path(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(path)
        })
        .as_deref()
        .map_err(Clone::clone)
}

/// Baixa a DLL da Cisco pra `dir`, confere o hash e devolve o caminho. Já
/// existindo e conferindo, não baixa de novo.
pub fn download(dir: &Path) -> std::result::Result<PathBuf, String> {
    let dest = dir.join(DLL);
    if dest.exists() && OpenH264API::from_blob_path(&dest).is_ok() {
        return Ok(dest);
    }
    let response = ureq::get(DOWNLOAD_URL).call().map_err(|e| format!("baixar {DOWNLOAD_URL}: {e}"))?;
    let compressed = response.into_body().with_config().limit(16 * 1024 * 1024).read_to_vec().map_err(|e| format!("baixar {DOWNLOAD_URL}: {e}"))?;
    let mut dll = Vec::new();
    std::io::Read::read_to_end(&mut bzip2::read::BzDecoder::new(compressed.as_slice()), &mut dll).map_err(|e| format!("descompactar {DLL}: {e}"))?;
    // Grava ao lado e só troca depois de conferir: uma DLL errada nunca fica no lugar.
    let tmp = dir.join(format!("{DLL}.baixando"));
    std::fs::write(&tmp, &dll).map_err(|e| format!("{}: {e}", tmp.display()))?;
    if let Err(e) = OpenH264API::from_blob_path(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{DLL} baixada não confere: {e}"));
    }
    std::fs::rename(&tmp, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    Ok(dest)
}

/// Threads do openh264: uma em máquina de 1–2 núcleos (o mesmo núcleo ainda
/// desenha a tela); até 4 nas outras. Pura.
pub fn threads_for(cores: usize) -> u16 {
    if cores <= 2 {
        1
    } else {
        cores.min(4) as u16
    }
}

/// Afinação de tempo real: conteúdo de tela, taxa de bits constante sem pular
/// quadro, keyframe só sob demanda (e um de segurança a cada 10 s), cor
/// BT.709 de faixa limitada (a da nossa conversão). Sem detecção de troca de
/// cena (viraria um IDR grande a cada janela trocada; o keyframe é nosso) e
/// sem quantização adaptativa nem detecção de fundo, que o modo de tela não
/// suporta (desligaria sozinho, com aviso).
fn config(cfg: &EncoderConfig, cores: usize) -> oh::EncoderConfig {
    let fps = cfg.fps.max(1);
    oh::EncoderConfig::new()
        .bitrate(BitRate::from_bps(cfg.bitrate_kbps.max(100) * 1000))
        .max_frame_rate(FrameRate::from_hz(fps as f32))
        .rate_control_mode(RateControlMode::Bitrate)
        .skip_frames(false)
        .usage_type(UsageType::ScreenContentRealTime)
        .complexity(Complexity::Low)
        .num_threads(threads_for(cores))
        .intra_frame_period(IntraFramePeriod::from_num_frames(fps * 10))
        .scene_change_detect(false)
        .adaptive_quantization(false)
        .background_detection(false)
        .vui(VuiConfig::bt709())
}

/// O quadro I420 guardado entre um envio e outro.
struct Planes {
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    w: usize,
    h: usize,
    /// Largura de U e V (metade, arredondada pra cima).
    cw: usize,
}

impl Planes {
    fn new(w: usize, h: usize) -> Self {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        Self { y: vec![16; w * h], u: vec![128; cw * ch], v: vec![128; cw * ch], w, h, cw }
    }

    fn all(&mut self) -> I420Planes<'_> {
        I420Planes { y: &mut self.y, y_stride: self.w, u: &mut self.u, v: &mut self.v, uv_stride: self.cw }
    }

    /// Os planos a partir do canto `r` (par) da imagem.
    fn at(&mut self, r: &Region) -> I420Planes<'_> {
        let (x, y) = (r.left as usize, r.top as usize);
        I420Planes { y: &mut self.y[y * self.w + x..], y_stride: self.w, u: &mut self.u[y / 2 * self.cw + x / 2..], v: &mut self.v[y / 2 * self.cw + x / 2..], uv_stride: self.cw }
    }
}

pub struct OpenH264Encoder {
    cfg: EncoderConfig,
    enc: oh::Encoder,
    planes: Planes,
    /// Os planos já têm uma tela inteira convertida (aí basta o que mudou).
    filled: bool,
    #[cfg(windows)]
    staging: Option<ID3D11Texture2D>,
    /// SPS/PPS do último keyframe (repostos se um keyframe vier sem).
    headers: Vec<u8>,
    frames: u64,
    timing: EncodeTiming,
}

// SAFETY: o encoder e a textura são usados por uma thread só (a do motor).
unsafe impl Send for OpenH264Encoder {}

impl OpenH264Encoder {
    pub fn open(cfg: &EncoderConfig) -> Result<Self> {
        if cfg.codec != Codec::H264 || cfg.chroma444 {
            return Err("o openh264 só faz H.264 4:2:0".into());
        }
        let path = verified_dll().map_err(MediaError::Msg)?;
        // SAFETY: a DLL foi conferida acima (hash de uma versão que o crate conhece).
        let api = unsafe { OpenH264API::from_blob_path_unchecked(path) }.map_err(|e| MediaError::Msg(format!("{DLL}: {e}")))?;
        let cores = crate::media::encode::cpu_cores();
        let enc = oh::Encoder::with_api_config(api, config(cfg, cores)).map_err(|e| MediaError::Msg(format!("openh264: {e}")))?;
        log::info!("encoder openh264 {}x{} @{} {} kbps ({} threads)", cfg.width, cfg.height, cfg.fps, cfg.bitrate_kbps, threads_for(cores));
        Ok(Self {
            cfg: cfg.clone(),
            enc,
            planes: Planes::new(cfg.width as usize, cfg.height as usize),
            filled: false,
            #[cfg(windows)]
            staging: None,
            headers: Vec::new(),
            frames: 0,
            timing: EncodeTiming::default(),
        })
    }

    /// Codifica o que está nos planos.
    fn submit(&mut self, force_key: bool, started: Instant) -> Result<Vec<EncodedPacket>> {
        let filled = Instant::now();
        // O 1º quadro já é IDR (e o encoder só existe depois dele).
        if force_key && self.frames > 0 {
            self.enc.force_intra_frame();
        }
        let p = &self.planes;
        let src = YUVSlices::new((&p.y, &p.u, &p.v), (p.w, p.h), (p.w, p.cw, p.cw));
        let bs = self.enc.encode(&src).map_err(|e| MediaError::Msg(format!("openh264: {e}")))?;
        let key = matches!(bs.frame_type(), FrameType::IDR | FrameType::I);
        let mut data = bs.to_vec();
        self.frames += 1;
        if key {
            let ps = parameter_sets(Codec::H264, &data);
            if ps.is_empty() {
                data = with_headers(Codec::H264, &self.headers, data, true);
            } else {
                self.headers = ps;
            }
        }
        self.timing = EncodeTiming { upload_us: (filled - started).as_micros() as u32, codec_us: filled.elapsed().as_micros() as u32 };
        Ok(if data.is_empty() { Vec::new() } else { vec![EncodedPacket { data, key }] })
    }
}

impl VideoEncoder for OpenH264Encoder {
    fn name(&self) -> &str {
        BACKEND
    }

    fn codec(&self) -> Codec {
        Codec::H264
    }

    fn size(&self) -> (u32, u32) {
        (self.cfg.width, self.cfg.height)
    }

    /// NV12 da GPU (máquina com placa, mas sem encoder de hardware): desce
    /// por uma textura de staging e vira I420.
    #[cfg(windows)]
    fn encode(&mut self, dev: &Device, nv12: &ID3D11Texture2D, force_key: bool) -> Result<Vec<EncodedPacket>> {
        let started = Instant::now();
        if self.staging.is_none() {
            self.staging = Some(dev.staging_like(nv12)?);
        }
        let staging = self.staging.clone().ok_or("sem staging")?;
        let (w, h) = (self.planes.w, self.planes.h);
        unsafe {
            dev.ctx.CopyResource(&staging, nv12);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            dev.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let pitch = m.RowPitch as usize;
            // SAFETY: NV12 mapeado: `h` linhas de Y e `h/2` de UV com o mesmo pitch, até o Unmap.
            let y = std::slice::from_raw_parts(m.pData as *const u8, pitch * h);
            let uv = std::slice::from_raw_parts((m.pData as *const u8).add(pitch * h), pitch * h.div_ceil(2));
            nv12_to_i420(y, pitch, uv, pitch, w, h, self.planes.all());
            dev.ctx.Unmap(&staging, 0);
        }
        self.filled = true;
        self.submit(force_key, started)
    }

    fn accepts_bgra(&self) -> bool {
        true
    }

    fn encode_bgra(&mut self, bgra: &[u8], stride: usize, regions: Option<&[Region]>, force_key: bool) -> Result<Vec<EncodedPacket>> {
        let started = Instant::now();
        let (w, h) = (self.planes.w, self.planes.h);
        if stride < w * 4 || bgra.len() < stride * (h - 1) + w * 4 {
            return Err(MediaError::Msg(format!("BGRA menor que {w}x{h}")));
        }
        match regions.filter(|_| self.filled) {
            None => bgra_to_i420_par(bgra, stride, w, h, self.planes.all(), conversion_threads()),
            Some(list) => {
                for r in list.iter().map(|r| r.even_within(w as u32, h as u32)).filter(|r| !r.is_empty()) {
                    let (rw, rh) = ((r.right - r.left) as usize, (r.bottom - r.top) as usize);
                    let src = &bgra[r.top as usize * stride + r.left as usize * 4..];
                    bgra_to_i420(src, stride, rw, rh, self.planes.at(&r));
                }
            }
        }
        self.filled = true;
        self.submit(force_key, started)
    }

    fn last_timing(&self) -> EncodeTiming {
        self.timing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_thread_on_weak_machines() {
        assert_eq!(threads_for(1), 1);
        assert_eq!(threads_for(2), 1);
        assert_eq!(threads_for(8), 4);
        assert_eq!(threads_for(3), 3);
    }

    #[test]
    fn region_planes_start_at_the_corner() {
        let mut p = Planes::new(8, 4);
        let r = Region { left: 2, top: 2, right: 6, bottom: 4 };
        let at = p.at(&r);
        assert_eq!(at.y.len(), 8 * 4 - (2 * 8 + 2));
        assert_eq!(at.u.len(), 4 * 2 - (4 + 1));
    }

    #[test]
    fn only_420_h264() {
        let cfg = EncoderConfig { codec: Codec::Hevc, backend: BACKEND.into(), width: 64, height: 64, fps: 30, bitrate_kbps: 1000, vbv_frames: 1, chroma444: false };
        assert!(OpenH264Encoder::open(&cfg).is_err());
        let cfg = EncoderConfig { codec: Codec::H264, chroma444: true, ..cfg };
        assert!(OpenH264Encoder::open(&cfg).is_err());
    }
}
