//! Codificação (§2). O host fala com o trait [`VideoEncoder`]; a
//! implementação é o libavcodec (porte de `NoOBS/rust/src/capture/encode.rs`)
//! com afinação de **tempo real**: sem B-frames, CBR com buffer de ~1 quadro,
//! keyframe sob demanda e SPS/PPS repetidos em todo keyframe (o viewer pode
//! começar a decodificar em qualquer keyframe).
//!
//! Dois caminhos na mesma implementação:
//! - **GPU** (`*_amf`, `*_nvenc`): o NV12 é copiado pra um quadro do pool D3D11
//!   do FFmpeg montado sobre o NOSSO dispositivo — não sai da placa;
//! - **memória** (`libx264`, `*_mf`): o NV12 desce por uma textura de staging,
//!   ou o BGRA já lido vira NV12 na CPU ([`VideoEncoder::encode_bgra`]) — o
//!   melhor quando a "GPU" é o WARP (VM, servidor), onde shader também é CPU.

use std::ffi::c_void;
use std::time::Instant;

use reflex_proto::codec::Codec;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ};

use crate::ffi::ffmpeg::*;
use crate::gpu::Device;
use crate::damage::Region;
use crate::yuv::{bgra_to_nv12, bgra_to_nv12_par, bgra_to_yuv444_par, Nv12Planes, Yuv444Planes};
use crate::{MediaError, Result};

/// Um pacote pronto pra rede.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub key: bool,
}

pub trait VideoEncoder: Send {
    fn name(&self) -> &str;
    fn codec(&self) -> Codec;
    fn size(&self) -> (u32, u32);
    /// Codifica o quadro NV12. `force_key` = este quadro TEM que ser keyframe.
    fn encode(&mut self, dev: &Device, nv12: &ID3D11Texture2D, force_key: bool) -> Result<Vec<EncodedPacket>>;
    /// Aceita BGRA da memória ([`VideoEncoder::encode_bgra`])?
    fn accepts_bgra(&self) -> bool {
        false
    }
    /// Codifica um quadro BGRA (`stride` bytes por linha) do tamanho do
    /// encoder, convertendo pra NV12 na CPU.
    /// `regions`: só isso mudou desde o quadro anterior por este caminho
    /// (`None` = tudo); o resto do quadro convertido antes continua valendo.
    fn encode_bgra(&mut self, _bgra: &[u8], _stride: usize, _regions: Option<&[Region]>, _force_key: bool) -> Result<Vec<EncodedPacket>> {
        Err("este encoder não aceita BGRA".into())
    }
    /// Quanto o último `encode` gastou em cada etapa (pra medir o gargalo).
    fn last_timing(&self) -> EncodeTiming {
        EncodeTiming::default()
    }
}

/// Tempo de um `encode`, em µs: subir o quadro (cópia GPU → encoder, ou →
/// memória esperando a GPU terminar a conversão, ou BGRA → NV12 na CPU) e
/// codificar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EncodeTiming {
    pub upload_us: u32,
    pub codec_us: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderConfig {
    pub codec: Codec,
    /// Nome no libavcodec (`h264_amf`, `libx264`…).
    pub backend: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    /// Folga de banda em quadros (1 = cada quadro só a sua cota; mais =
    /// keyframe e refino com mais qualidade). Ver `Refine`.
    pub vbv_frames: u32,
    /// Cor completa (4:4:4, `Chroma::Yuv444`): só encoder de memória e só
    /// pelo caminho BGRA ([`VideoEncoder::encode_bgra`]).
    pub chroma444: bool,
}

/// Como o quadro chega ao encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upload {
    Gpu,
    Memory,
}

pub fn upload_of(backend: &str) -> Upload {
    if backend.ends_with("_amf") || backend.ends_with("_nvenc") {
        Upload::Gpu
    } else {
        Upload::Memory
    }
}

/// Encoder de hardware (GPU/mídia do sistema) ou CPU.
pub fn is_hardware(backend: &str) -> bool {
    !backend.starts_with("lib")
}

/// Encoder obedece a um quadro marcado como I? Os outros reabrem (sempre começam por IDR).
pub fn can_force_key(backend: &str) -> bool {
    backend == "libx264" || backend.ends_with("_nvenc")
}

pub const VENDOR_AMD: u32 = 0x1002;
pub const VENDOR_NVIDIA: u32 = 0x10DE;
pub const VENDOR_INTEL: u32 = 0x8086;

/// Candidatos por codec, na ordem de preferência, pra placa `vendor`.
pub fn candidates(codec: Codec, vendor: u32) -> Vec<&'static str> {
    let hw: &[&str] = match (codec, vendor) {
        (Codec::H264, VENDOR_AMD) => &["h264_amf"],
        (Codec::H264, VENDOR_NVIDIA) => &["h264_nvenc"],
        (Codec::H264, _) => &["h264_mf"],
        (Codec::Hevc, VENDOR_AMD) => &["hevc_amf"],
        (Codec::Hevc, VENDOR_NVIDIA) => &["hevc_nvenc"],
        (Codec::Hevc, _) => &["hevc_mf"],
        (Codec::Av1, VENDOR_AMD) => &["av1_amf"],
        (Codec::Av1, VENDOR_NVIDIA) => &["av1_nvenc"],
        (Codec::Av1, _) => &[],
    };
    let mut v = hw.to_vec();
    // CPU: só H.264 é viável em tempo real (§4.2).
    if codec == Codec::H264 {
        v.push("libx264");
    }
    v
}

/// Opções de baixa latência por encoder (sem B-frames, CBR, buffer de 1 quadro).
/// Preset do x264 pela CPU: com 1–2 núcleos o `ultrafast` (cerca de metade
/// do trabalho do `superfast`, um pouco mais de banda) — numa VM de 1 núcleo,
/// sem GPU, o mesmo núcleo ainda desenha a tela. Pura.
pub fn x264_preset(cores: usize) -> &'static str {
    if cores <= 2 {
        "ultrafast"
    } else {
        "superfast"
    }
}

/// Núcleos lógicos da máquina.
pub fn cpu_cores() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

pub fn low_latency_options(backend: &str, bitrate_kbps: u32, fps: u32, cores: usize, vbv_frames: u32) -> Vec<(&'static str, String)> {
    let bps = bitrate_kbps.max(100) as u64 * 1000;
    let fps = fps.max(1) as u64;
    let mut o: Vec<(&'static str, String)> = vec![
        ("b", bps.to_string()),
        ("maxrate", bps.to_string()),
        ("bufsize", ((bps / fps).max(bps / 30) * vbv_frames.max(1) as u64).to_string()),
        ("bf", "0".into()),
        // Keyframe de segurança a cada 10 s; o resto é sob demanda.
        ("g", (fps * 10).to_string()),
    ];
    if backend == "libx264" {
        o.extend([("preset", x264_preset(cores).into()), ("tune", "zerolatency".into()), ("forced-idr", "1".into())]);
    } else if backend.ends_with("_nvenc") {
        o.extend([("preset", "p2".into()), ("tune", "ull".into()), ("zerolatency", "1".into()), ("delay", "0".into()), ("rc", "cbr".into()), ("forced-idr", "1".into())]);
    } else if backend.ends_with("_amf") {
        o.extend([("usage", "ultralowlatency".into()), ("rc", "cbr".into()), ("quality", "speed".into())]);
    } else if backend.ends_with("_mf") {
        o.extend([("hw_encoding", "1".into()), ("rate_control", "cbr".into()), ("scenario", "display_remoting".into())]);
    }
    o
}

/// Começa com código de início Annex-B (SPS/PPS repetíveis na frente do keyframe).
fn is_annex_b(data: &[u8]) -> bool {
    data.starts_with(&[0, 0, 0, 1]) || data.starts_with(&[0, 0, 1])
}

/// Põe os parâmetros do stream na frente de um keyframe H.264/HEVC, se ainda não estiverem.
pub fn with_headers(codec: Codec, headers: &[u8], packet: Vec<u8>, key: bool) -> Vec<u8> {
    if !key || codec == Codec::Av1 || headers.is_empty() || !is_annex_b(headers) || packet.starts_with(headers) {
        return packet;
    }
    let mut out = Vec::with_capacity(headers.len() + packet.len());
    out.extend_from_slice(headers);
    out.extend_from_slice(&packet);
    out
}

/// Device D3D11 do FFmpeg sobre o NOSSO `ID3D11Device` + pool de quadros NV12
/// na GPU. Cada offset é conferido contra um valor conhecido antes de escrever.
struct HwFrames {
    _device: BufRef,
    frames: BufRef,
}

impl HwFrames {
    fn new(device: &ID3D11Device, w: u32, h: u32) -> Result<Self> {
        let a = api()?;
        unsafe {
            let dev_ref = BufRef((a.util.av_hwdevice_ctx_alloc)(AV_HWDEVICE_TYPE_D3D11VA));
            if dev_ref.0.is_null() {
                return Err("av_hwdevice_ctx_alloc(d3d11va) falhou".into());
            }
            let devctx = dev_ref.data();
            let ty: i32 = rd(devctx, OFFS_HWDEV_TYPE);
            if ty != AV_HWDEVICE_TYPE_D3D11VA {
                return Err(MediaError::Msg(format!("AVHWDeviceContext.type = {ty}: offsets do avutil mudaram")));
            }
            let hwctx: *mut c_void = rd(devctx, OFFS_HWDEV_HWCTX);
            if hwctx.is_null() {
                return Err("AVHWDeviceContext.hwctx nulo".into());
            }
            // O FFmpeg dá Release no fim: a referência entregue é nossa (clone = AddRef).
            wr(hwctx, 0, device.clone().into_raw());
            let rc = (a.util.av_hwdevice_ctx_init)(dev_ref.0);
            if rc < 0 {
                return Err(MediaError::Msg(format!("av_hwdevice_ctx_init: {}", av_err_str(rc))));
            }
            let frames = BufRef((a.util.av_hwframe_ctx_alloc)(dev_ref.0));
            if frames.0.is_null() {
                return Err("av_hwframe_ctx_alloc falhou".into());
            }
            let fr = frames.data();
            let fr_dev: *mut c_void = rd(fr, OFFS_HWFR_DEVICE_REF);
            if fr_dev.is_null() || rd::<*mut c_void>(fr_dev, OFFS_BUFREF_DATA) != devctx {
                return Err("AVHWFramesContext.device_ref não confere: offsets do avutil mudaram".into());
            }
            wr(fr, OFFS_HWFR_FORMAT, AV_PIX_FMT_D3D11);
            wr(fr, OFFS_HWFR_SW_FORMAT, AV_PIX_FMT_NV12);
            wr(fr, OFFS_HWFR_WIDTH, w as i32);
            wr(fr, OFFS_HWFR_HEIGHT, h as i32);
            wr(fr, OFFS_HWFR_POOL_SIZE, 8i32);
            let d3dfr: *mut c_void = rd(fr, OFFS_HWFR_HWCTX);
            if !d3dfr.is_null() {
                wr(d3dfr, OFFS_D3D11FR_BIND_FLAGS, D3D11_BIND_RENDER_TARGET.0 as u32);
            }
            let rc = (a.util.av_hwframe_ctx_init)(frames.0);
            if rc < 0 {
                return Err(MediaError::Msg(format!("av_hwframe_ctx_init {w}x{h}: {}", av_err_str(rc))));
            }
            Ok(Self { _device: dev_ref, frames })
        }
    }
}

/// `AVCodecContext.hw_frames_ctx` por offset, conferido pela âncora `extra_hw_frames`.
unsafe fn set_hw_frames_ctx(ctx: *mut AVCodecContext, frames: *mut c_void) -> Result<()> {
    let a = api()?;
    const MARK: i32 = 0x2468;
    (a.util.av_opt_set_int)(ctx, c"extra_hw_frames".as_ptr(), MARK as i64, 0);
    let got: i32 = rd(ctx, OFFS_CODECCTX_EXTRA_HW_FRAMES);
    (a.util.av_opt_set_int)(ctx, c"extra_hw_frames".as_ptr(), -1, 0);
    let cur: *mut c_void = rd(ctx, OFFS_CODECCTX_HW_FRAMES_CTX);
    if got != MARK || !cur.is_null() {
        return Err("AVCodecContext: offsets do avcodec mudaram (hw_frames_ctx)".into());
    }
    let r = (a.util.av_buffer_ref)(frames);
    if r.is_null() {
        return Err("av_buffer_ref(frames)".into());
    }
    wr(ctx, OFFS_CODECCTX_HW_FRAMES_CTX, r);
    Ok(())
}

/// Encoder do libavcodec.
pub struct AvEncoder {
    cfg: EncoderConfig,
    ctx: CodecCtx,
    hw: Option<HwFrames>,
    staging: Option<ID3D11Texture2D>,
    frame: Frame,
    pkt: Packet,
    headers: Vec<u8>,
    pts: i64,
    sent: u64,
    timing: EncodeTiming,
    /// O quadro em memória guarda a tela convertida (caminho BGRA): fica entre
    /// um envio e outro e só o que mudou é convertido de novo.
    kept: bool,
}

// SAFETY: contexto e quadros usados por uma thread só (a de captura).
unsafe impl Send for AvEncoder {}

/// Separa os NALs de parâmetros (SPS/PPS/VPS) do início de um keyframe Annex-B.
pub fn parameter_sets(codec: Codec, packet: &[u8]) -> Vec<u8> {
    let is_param = |nal: &[u8]| -> bool {
        let Some(&h) = nal.first() else { return false };
        match codec {
            Codec::H264 => matches!(h & 0x1F, 7 | 8),
            Codec::Hevc => matches!((h >> 1) & 0x3F, 32..=34),
            Codec::Av1 => false,
        }
    };
    let mut out = Vec::new();
    for (start, end) in nal_units(packet) {
        let body = &packet[start..end];
        // `start` aponta logo após o código de início; volta pra incluí-lo.
        let sc = if start >= 4 && packet[start - 4..start] == [0, 0, 0, 1] { 4 } else { 3 };
        if is_param(body) {
            out.extend_from_slice(&packet[start - sc..end]);
        } else {
            break;
        }
    }
    out
}

/// Intervalos `(início_do_nal, fim)` de um fluxo Annex-B.
fn nal_units(data: &[u8]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for (k, &s) in starts.iter().enumerate() {
        let mut e = starts.get(k + 1).map(|&n| n - 3).unwrap_or(data.len());
        // Código de 4 bytes: o zero extra pertence ao próximo.
        if e > s && starts.get(k + 1).is_some() && data[e - 1] == 0 {
            e -= 1;
        }
        out.push((s, e));
    }
    out
}

impl AvEncoder {
    fn open_ctx(cfg: &EncoderConfig, hw: Option<&HwFrames>) -> Result<CodecCtx> {
        let a = api()?;
        let codec = unsafe { (a.codec.avcodec_find_encoder_by_name)(to_cstring(&cfg.backend).as_ptr()) };
        if codec.is_null() {
            return Err(MediaError::Msg(format!("encoder \"{}\" não existe nesta build do FFmpeg", cfg.backend)));
        }
        let ctx = CodecCtx::alloc(codec).ok_or("avcodec_alloc_context3 falhou")?;
        unsafe {
            let mut par = (a.codec.avcodec_parameters_alloc)();
            if par.is_null() {
                return Err("avcodec_parameters_alloc".into());
            }
            (*par).codec_type = AVMEDIA_TYPE_VIDEO;
            (*par).codec_id = (*codec).id;
            (*par).width = cfg.width as i32;
            (*par).height = cfg.height as i32;
            (*par).format = match (hw.is_some(), cfg.chroma444) {
                (true, _) => AV_PIX_FMT_D3D11,
                (false, true) => AV_PIX_FMT_YUV444P,
                (false, false) => AV_PIX_FMT_NV12,
            };
            let r = (a.codec.avcodec_parameters_to_context)(ctx.ptr(), par);
            (a.codec.avcodec_parameters_free)(&mut par);
            if r < 0 {
                return Err(MediaError::Msg(format!("avcodec_parameters_to_context: {}", av_err_str(r))));
            }
            (a.util.av_opt_set_q)(ctx.ptr(), c"time_base".as_ptr(), AVRational { num: 1, den: cfg.fps as i32 }, 0);
            (a.util.av_opt_set_q)(ctx.ptr(), c"framerate".as_ptr(), AVRational { num: cfg.fps as i32, den: 1 }, 0);
            ctx.set_str(c"color_primaries", c"bt709");
            ctx.set_str(c"color_trc", c"bt709");
            ctx.set_str(c"colorspace", c"bt709");
            ctx.set_str(c"color_range", c"tv");
            match hw {
                Some(h) => set_hw_frames_ctx(ctx.ptr(), h.frames.0)?,
                None => ctx.set_int(c"threads", 0),
            }
        }
        let mut opts = Dict::default();
        for (k, v) in low_latency_options(&cfg.backend, cfg.bitrate_kbps, cfg.fps, cpu_cores(), cfg.vbv_frames) {
            opts.set(k, &v);
        }
        let rc = unsafe { (a.codec.avcodec_open2)(ctx.ptr(), codec, opts.as_mut_ptr()) };
        let left = opts.leftover_keys();
        if !left.is_empty() {
            log::debug!("encoder {} ignorou {left:?}", cfg.backend);
        }
        if rc < 0 {
            return Err(MediaError::Msg(format!("avcodec_open2(\"{}\"): {}", cfg.backend, av_err_str(rc))));
        }
        Ok(ctx)
    }

    pub fn open(dev: &Device, cfg: &EncoderConfig) -> Result<Self> {
        let hw = match upload_of(&cfg.backend) {
            Upload::Gpu => Some(HwFrames::new(&dev.device, cfg.width, cfg.height)?),
            Upload::Memory => None,
        };
        let ctx = Self::open_ctx(cfg, hw.as_ref())?;
        let staging = match hw {
            Some(_) => None,
            None => {
                let probe = dev.texture(cfg.width, cfg.height, windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12, D3D11_BIND_RENDER_TARGET)?;
                Some(dev.staging_like(&probe)?)
            }
        };
        log::info!("encoder {} {}x{} @{} {} kbps ({:?})", cfg.backend, cfg.width, cfg.height, cfg.fps, cfg.bitrate_kbps, upload_of(&cfg.backend));
        Ok(Self {
            // Sem `global_header`, os encoders mandam SPS/PPS no keyframe; o 1º
            // keyframe ensina e os seguintes que vierem sem ganham uma cópia.
            headers: Vec::new(),
            cfg: cfg.clone(),
            ctx,
            hw,
            staging,
            frame: Frame::alloc().ok_or("av_frame_alloc")?,
            pkt: Packet::alloc().ok_or("av_packet_alloc")?,
            pts: 0,
            sent: 0,
            timing: EncodeTiming::default(),
            kept: false,
        })
    }

    /// Keyframe sem suporte do encoder: reabre (o 1º quadro é sempre IDR).
    fn restart(&mut self) -> Result<()> {
        self.ctx = Self::open_ctx(&self.cfg, self.hw.as_ref())?;
        self.sent = 0;
        Ok(())
    }

    fn drain(&mut self, out: &mut Vec<EncodedPacket>) -> Result<()> {
        let a = api()?;
        loop {
            let rc = unsafe { (a.codec.avcodec_receive_packet)(self.ctx.ptr(), self.pkt.ptr()) };
            if rc == AVERROR_EAGAIN || rc == AVERROR_EOF {
                return Ok(());
            }
            if rc < 0 {
                return Err(MediaError::Msg(format!("avcodec_receive_packet: {}", av_err_str(rc))));
            }
            let key = self.pkt.is_key();
            let data = self.pkt.bytes().to_vec();
            self.pkt.unref();
            if key {
                let found = parameter_sets(self.cfg.codec, &data);
                if !found.is_empty() {
                    self.headers = found;
                }
            }
            out.push(EncodedPacket { data: with_headers(self.cfg.codec, &self.headers, data, key), key });
        }
    }

    fn fill_gpu(&mut self, dev: &Device, nv12: &ID3D11Texture2D) -> Result<()> {
        let a = api()?;
        let h = self.hw.as_ref().ok_or("sem pool de quadros")?;
        self.frame.unref();
        let rc = unsafe { (a.util.av_hwframe_get_buffer)(h.frames.0, self.frame.ptr(), 0) };
        if rc < 0 {
            return Err(MediaError::Msg(format!("av_hwframe_get_buffer: {}", av_err_str(rc))));
        }
        let raw = self.frame.data[0] as *mut c_void;
        let index = self.frame.data[1] as usize as u32;
        let tex = unsafe { ID3D11Texture2D::from_raw_borrowed(&raw) }.ok_or("quadro D3D11 sem textura")?;
        unsafe { dev.ctx.CopySubresourceRegion(tex, index, 0, 0, 0, nv12, 0, None) };
        Ok(())
    }

    /// Quadro NV12 novo, em memória, do tamanho do encoder.
    fn alloc_memory_frame(&mut self) -> Result<()> {
        let a = api()?;
        self.kept = false;
        self.frame.unref();
        self.frame.width = self.cfg.width as i32;
        self.frame.height = self.cfg.height as i32;
        self.frame.format = if self.cfg.chroma444 { AV_PIX_FMT_YUV444P } else { AV_PIX_FMT_NV12 };
        let rc = unsafe { (a.util.av_frame_get_buffer)(self.frame.ptr(), 0) };
        if rc < 0 {
            return Err(MediaError::Msg(format!("av_frame_get_buffer: {}", av_err_str(rc))));
        }
        Ok(())
    }

    fn fill_memory(&mut self, dev: &Device, nv12: &ID3D11Texture2D) -> Result<()> {
        if self.cfg.chroma444 {
            return Err("cor 4:4:4 só pelo caminho BGRA".into());
        }
        let staging = self.staging.clone().ok_or("sem staging")?;
        let staging = &staging;
        let (w, h) = (self.cfg.width as usize, self.cfg.height as usize);
        self.alloc_memory_frame()?;
        unsafe {
            dev.ctx.CopyResource(staging, nv12);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            dev.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let src = m.pData as *const u8;
            let pitch = m.RowPitch as usize;
            let uv = src.add(pitch * h);
            let (ly, dy) = (self.frame.linesize[0] as usize, self.frame.data[0]);
            for y in 0..h {
                std::ptr::copy_nonoverlapping(src.add(y * pitch), dy.add(y * ly), w);
            }
            let (luv, duv) = (self.frame.linesize[1] as usize, self.frame.data[1]);
            for y in 0..h / 2 {
                std::ptr::copy_nonoverlapping(uv.add(y * pitch), duv.add(y * luv), w);
            }
            dev.ctx.Unmap(staging, 0);
        }
        Ok(())
    }

    fn fill_bgra(&mut self, bgra: &[u8], stride: usize, regions: Option<&[Region]>) -> Result<()> {
        let (w, h) = (self.cfg.width as usize, self.cfg.height as usize);
        if stride < w * 4 || bgra.len() < stride * (h - 1) + w * 4 {
            return Err(MediaError::Msg(format!("BGRA menor que {w}x{h}")));
        }
        // Reusa o quadro do envio anterior (o x264 copia o que recebe; se o
        // libavcodec ainda tiver uma referência, o make_writable copia).
        let regions = if self.kept && unsafe { (api()?.util.av_frame_make_writable)(self.frame.ptr()) } >= 0 {
            regions
        } else {
            self.alloc_memory_frame()?;
            None
        };
        self.kept = true;
        if self.cfg.chroma444 {
            let line = self.frame.linesize[0] as usize;
            if self.frame.linesize[1] as usize != line || self.frame.linesize[2] as usize != line {
                return Err("quadro 4:4:4 com planos de tamanhos diferentes".into());
            }
            // SAFETY: três planos de `linesize × h` (iguais no 4:4:4), só nossos até o send_frame.
            let planes = unsafe {
                Yuv444Planes {
                    y: std::slice::from_raw_parts_mut(self.frame.data[0], line * h),
                    u: std::slice::from_raw_parts_mut(self.frame.data[1], line * h),
                    v: std::slice::from_raw_parts_mut(self.frame.data[2], line * h),
                    stride: line,
                }
            };
            bgra_to_yuv444_par(bgra, stride, w, h, planes, conversion_threads());
            return Ok(());
        }
        let (ly, luv) = (self.frame.linesize[0] as usize, self.frame.linesize[1] as usize);
        // SAFETY: av_frame_get_buffer alocou `linesize × linhas` em cada plano
        // (h linhas de Y, h/2 de UV) e o quadro é só nosso até o send_frame.
        let planes = unsafe {
            Nv12Planes {
                y: std::slice::from_raw_parts_mut(self.frame.data[0], ly * h),
                y_stride: ly,
                uv: std::slice::from_raw_parts_mut(self.frame.data[1], luv * (h / 2)),
                uv_stride: luv,
            }
        };
        match regions {
            None => bgra_to_nv12_par(bgra, stride, w, h, planes, conversion_threads()),
            Some(list) => {
                let Nv12Planes { y, uv, .. } = planes;
                for r in list.iter().map(|r| r.even_within(w as u32, h as u32)).filter(|r| !r.is_empty()) {
                    let (x0, y0) = (r.left as usize, r.top as usize);
                    let (rw, rh) = ((r.right - r.left) as usize, (r.bottom - r.top) as usize);
                    let part = Nv12Planes { y: &mut y[y0 * ly + x0..], y_stride: ly, uv: &mut uv[y0 / 2 * luv + x0..], uv_stride: luv };
                    bgra_to_nv12(&bgra[y0 * stride + x0 * 4..], stride, rw, rh, part);
                }
            }
        }
        Ok(())
    }

    /// Antes de cada quadro: keyframe sem suporte do encoder = reabrir.
    fn begin(&mut self, force_key: bool) -> Result<()> {
        if force_key && self.sent > 0 && !can_force_key(&self.cfg.backend) {
            self.restart()?;
        }
        self.sent += 1;
        Ok(())
    }

    /// Manda o quadro já preenchido ao codec e colhe os pacotes.
    fn submit(&mut self, force_key: bool, started: Instant) -> Result<Vec<EncodedPacket>> {
        let filled = Instant::now();
        self.frame.pts = self.pts;
        self.pts += 1;
        // AV_PICTURE_TYPE_I (1) força keyframe; 0 deixa o encoder decidir.
        self.frame.pict_type = if force_key { 1 } else { 0 };
        let a = api()?;
        let rc = unsafe { (a.codec.avcodec_send_frame)(self.ctx.ptr(), self.frame.ptr()) };
        if !self.kept {
            self.frame.unref();
        }
        if rc < 0 && rc != AVERROR_EAGAIN {
            return Err(MediaError::Msg(format!("avcodec_send_frame: {}", av_err_str(rc))));
        }
        let mut out = Vec::new();
        self.drain(&mut out)?;
        self.timing = EncodeTiming { upload_us: (filled - started).as_micros() as u32, codec_us: filled.elapsed().as_micros() as u32 };
        Ok(out)
    }
}

/// Threads pra conversão na CPU: o bastante pra não ser o gargalo sem
/// disputar com as do x264.
fn conversion_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 4)
}

impl VideoEncoder for AvEncoder {
    fn name(&self) -> &str {
        &self.cfg.backend
    }

    fn codec(&self) -> Codec {
        self.cfg.codec
    }

    fn size(&self) -> (u32, u32) {
        (self.cfg.width, self.cfg.height)
    }

    fn encode(&mut self, dev: &Device, nv12: &ID3D11Texture2D, force_key: bool) -> Result<Vec<EncodedPacket>> {
        self.begin(force_key)?;
        let started = Instant::now();
        match self.hw {
            Some(_) => self.fill_gpu(dev, nv12)?,
            None => self.fill_memory(dev, nv12)?,
        }
        self.submit(force_key, started)
    }

    fn accepts_bgra(&self) -> bool {
        self.hw.is_none()
    }

    fn encode_bgra(&mut self, bgra: &[u8], stride: usize, regions: Option<&[Region]>, force_key: bool) -> Result<Vec<EncodedPacket>> {
        if self.hw.is_some() {
            return Err("encoder de GPU não aceita BGRA".into());
        }
        self.begin(force_key)?;
        let started = Instant::now();
        self.fill_bgra(bgra, stride, regions)?;
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
    fn upload_path_by_backend() {
        assert_eq!(upload_of("h264_amf"), Upload::Gpu);
        assert_eq!(upload_of("av1_nvenc"), Upload::Gpu);
        assert_eq!(upload_of("libx264"), Upload::Memory);
        assert_eq!(upload_of("h264_mf"), Upload::Memory);
    }

    #[test]
    fn candidates_follow_vendor_and_keep_cpu_fallback_for_h264() {
        assert_eq!(candidates(Codec::H264, VENDOR_AMD), vec!["h264_amf", "libx264"]);
        assert_eq!(candidates(Codec::Av1, VENDOR_NVIDIA), vec!["av1_nvenc"]);
        assert_eq!(candidates(Codec::Av1, 0), Vec::<&str>::new());
        assert_eq!(candidates(Codec::Hevc, VENDOR_INTEL), vec!["hevc_mf"]);
    }

    #[test]
    fn low_latency_has_no_bframes_and_one_frame_buffer() {
        let o = low_latency_options("h264_nvenc", 6000, 60, 8, 1);
        let get = |k: &str| o.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        assert_eq!(get("bf").as_deref(), Some("0"));
        assert_eq!(get("b").as_deref(), Some("6000000"));
        assert_eq!(get("bufsize").as_deref(), Some("200000"));
        assert_eq!(get("zerolatency").as_deref(), Some("1"));
        let x = low_latency_options("libx264", 2000, 30, 8, 1);
        assert!(x.contains(&("tune", "zerolatency".into())));
        assert!(x.contains(&("preset", "superfast".into())));
    }

    #[test]
    fn weak_cpu_gets_the_fastest_x264_preset() {
        assert_eq!(x264_preset(1), "ultrafast");
        assert_eq!(x264_preset(2), "ultrafast");
        assert_eq!(x264_preset(4), "superfast");
        assert!(low_latency_options("libx264", 2000, 30, 1, 1).contains(&("preset", "ultrafast".into())));
    }

    #[test]
    fn refinement_widens_the_rate_buffer() {
        let get = |vbv: u32| low_latency_options("libx264", 6000, 30, 8, vbv).into_iter().find(|(k, _)| *k == "bufsize").map(|(_, v)| v);
        assert_eq!(get(1).as_deref(), Some("200000"));
        assert_eq!(get(4).as_deref(), Some("800000"));
        assert_eq!(get(0).as_deref(), Some("200000"));
    }

    #[test]
    fn only_x264_and_nvenc_force_keyframes() {
        assert!(can_force_key("libx264"));
        assert!(can_force_key("hevc_nvenc"));
        assert!(!can_force_key("h264_amf"));
        assert!(!can_force_key("h264_mf"));
    }

    #[test]
    fn parameter_sets_are_extracted_from_h264_keyframe() {
        // SPS (0x67), PPS (0x68), IDR (0x65)
        let pkt = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3, 0, 0, 1, 0x65, 9, 9];
        assert_eq!(parameter_sets(Codec::H264, &pkt), vec![0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3]);
    }

    #[test]
    fn hevc_parameter_sets() {
        // VPS (32<<1=0x40), SPS (0x42), PPS (0x44), IDR (19<<1=0x26)
        let pkt = [0, 0, 1, 0x40, 1, 0, 0, 1, 0x42, 1, 0, 0, 1, 0x44, 1, 0, 0, 1, 0x26, 1];
        assert_eq!(parameter_sets(Codec::Hevc, &pkt).len(), 15);
    }

    #[test]
    fn headers_are_prepended_only_when_missing() {
        let headers = vec![0, 0, 0, 1, 0x67, 1];
        let idr = vec![0, 0, 0, 1, 0x65, 7];
        let out = with_headers(Codec::H264, &headers, idr.clone(), true);
        assert_eq!(out, [headers.clone(), idr.clone()].concat());
        assert_eq!(with_headers(Codec::H264, &headers, out.clone(), true), out);
        assert_eq!(with_headers(Codec::H264, &headers, idr.clone(), false), idr);
        assert_eq!(with_headers(Codec::Av1, &headers, idr.clone(), true), idr);
    }
}
