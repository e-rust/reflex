//! O decoder do libavcodec (Windows), com D3D11VA quando a placa suporta
//! (obrigatório pro AV1: esta build não tem decoder AV1 por software) e
//! software no resto. O quadro sai em BGRA pro desenho.

use std::ffi::c_void;

use crate::proto::codec::Codec;

use crate::media::decode::{BgraFrame, VideoDecoder};
use crate::media::ffi::ffmpeg::*;
use crate::media::yuv::{i420_to_bgra, nv12_to_bgra, yuv444_to_bgra};
use crate::media::{MediaError, Result};

/// `AV_PIX_FMT_YUVJ420P` (faixa cheia; mesma disposição do I420).
const AV_PIX_FMT_YUVJ420P: i32 = 12;
/// `AV_PIX_FMT_YUVJ444P` (faixa cheia do 4:4:4).
const AV_PIX_FMT_YUVJ444P: i32 = 14;

/// O decoder do libavcodec (o sem perda não passa pelo FFmpeg).
fn decoder_name(codec: Codec) -> Option<&'static str> {
    match codec {
        Codec::H264 => Some("h264"),
        Codec::Hevc => Some("hevc"),
        Codec::Av1 => Some("av1"),
        Codec::Lossless => None,
    }
}

pub struct AvDecoder {
    codec: Codec,
    ctx: CodecCtx,
    hw: bool,
    _hw_dev: Option<BufRef>,
    frame: Frame,
    sw_frame: Frame,
    pkt: Packet,
}

// SAFETY: usado por uma thread (a de decodificação do viewer).
unsafe impl Send for AvDecoder {}

impl AvDecoder {
    /// `hardware`: tenta D3D11VA (cai pro software se falhar, menos no AV1).
    pub fn open(codec: Codec, hardware: bool) -> Result<Self> {
        match Self::open_with(codec, hardware) {
            Ok(d) => Ok(d),
            Err(e) if hardware && codec != Codec::Av1 => {
                log::warn!("decode por hardware indisponível ({e}); usando software");
                Self::open_with(codec, false)
            }
            Err(e) => Err(e),
        }
    }

    fn open_with(codec: Codec, hardware: bool) -> Result<Self> {
        let a = api()?;
        if codec == Codec::Av1 && !hardware {
            return Err("AV1 só decodifica por hardware nesta build do FFmpeg".into());
        }
        let name = decoder_name(codec).ok_or_else(|| MediaError::Msg(format!("{} não decodifica pelo FFmpeg", codec.label())))?;
        let dec = unsafe { (a.codec.avcodec_find_decoder_by_name)(to_cstring(name).as_ptr()) };
        if dec.is_null() {
            return Err(MediaError::Msg(format!("decoder \"{name}\" não existe")));
        }
        let ctx = CodecCtx::alloc(dec).ok_or("avcodec_alloc_context3")?;
        // Sai quadro assim que possível (sem reordenação: o host não usa B-frames).
        ctx.set_str(c"flags", c"+low_delay");
        let mut hw_dev = None;
        if hardware {
            let mut dev: *mut c_void = std::ptr::null_mut();
            let r = unsafe { (a.util.av_hwdevice_ctx_create)(&mut dev, AV_HWDEVICE_TYPE_D3D11VA, std::ptr::null(), std::ptr::null_mut(), 0) };
            if r < 0 {
                return Err(MediaError::Msg(format!("D3D11VA: {}", av_err_str(r))));
            }
            let dev = BufRef(dev);
            // O get_format padrão escolhe o formato de hardware quando
            // hw_device_ctx está preenchido (campo sem setter: vai por offset).
            unsafe { wr(ctx.ptr(), OFFS_CODECCTX_HW_DEVICE_CTX, (a.util.av_buffer_ref)(dev.0)) };
            hw_dev = Some(dev);
        } else {
            ctx.set_int(c"threads", 0);
            ctx.set_str(c"thread_type", c"slice");
        }
        let rc = unsafe { (a.codec.avcodec_open2)(ctx.ptr(), dec, std::ptr::null_mut()) };
        if rc < 0 {
            return Err(MediaError::Msg(format!("avcodec_open2(\"{name}\"): {}", av_err_str(rc))));
        }
        Ok(Self {
            codec,
            ctx,
            hw: hardware,
            _hw_dev: hw_dev,
            frame: Frame::alloc().ok_or("av_frame_alloc")?,
            sw_frame: Frame::alloc().ok_or("av_frame_alloc")?,
            pkt: Packet::alloc().ok_or("av_packet_alloc")?,
        })
    }

    #[allow(clippy::wrong_self_convention)]
    fn to_bgra(&mut self) -> Result<BgraFrame> {
        let a = api()?;
        if self.frame.format == AV_PIX_FMT_D3D11 {
            self.sw_frame.unref();
            let r = unsafe { (a.util.av_hwframe_transfer_data)(self.sw_frame.ptr(), self.frame.ptr(), 0) };
            if r < 0 {
                return Err(MediaError::Msg(format!("cópia do quadro da placa: {}", av_err_str(r))));
            }
            return convert(&self.sw_frame);
        }
        convert(&self.frame)
    }
}

fn convert(f: &AVFrame) -> Result<BgraFrame> {
    let (w, h) = (f.width.max(0) as usize, f.height.max(0) as usize);
    let mut data = vec![0u8; w * h * 4];
    let plane = |i: usize, rows: usize| unsafe { std::slice::from_raw_parts(f.data[i], f.linesize[i] as usize * rows) };
    let l = [f.linesize[0] as usize, f.linesize[1] as usize, f.linesize[2] as usize];
    let ls = |i: usize| l[i];
    match f.format {
        AV_PIX_FMT_NV12 => {
            let (y, uv) = (plane(0, h), plane(1, h.div_ceil(2)));
            in_bands(w, h, &mut data, |first, rows, band| nv12_to_bgra(&y[first * ls(0)..], ls(0), &uv[first / 2 * ls(1)..], ls(1), w, rows, band));
        }
        AV_PIX_FMT_YUV420P | AV_PIX_FMT_YUVJ420P => {
            let (y, u, v) = (plane(0, h), plane(1, h.div_ceil(2)), plane(2, h.div_ceil(2)));
            in_bands(w, h, &mut data, |first, rows, band| {
                i420_to_bgra(&y[first * ls(0)..], ls(0), &u[first / 2 * ls(1)..], ls(1), &v[first / 2 * ls(2)..], ls(2), w, rows, band)
            });
        }
        // Cor completa (`Chroma::Yuv444`): só o decoder de software entrega.
        AV_PIX_FMT_YUV444P | AV_PIX_FMT_YUVJ444P => {
            let (y, u, v) = (plane(0, h), plane(1, h), plane(2, h));
            in_bands(w, h, &mut data, |first, rows, band| {
                yuv444_to_bgra(&y[first * ls(0)..], ls(0), &u[first * ls(1)..], ls(1), &v[first * ls(2)..], ls(2), w, rows, band)
            });
        }
        other => return Err(MediaError::Msg(format!("formato de quadro {other} não suportado"))),
    }
    Ok(BgraFrame { width: w as u32, height: h as u32, data })
}

/// Converte em faixas de linhas (começando em linha par: no 4:2:0 a cor vale
/// pra duas), uma por thread. Uma thread só gastava ~12 ms num quadro de
/// 2560×1600 — metade do tempo de decodificar.
fn in_bands(w: usize, h: usize, out: &mut [u8], convert: impl Fn(usize, usize, &mut [u8]) + Sync) {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 8);
    let per = h.div_ceil(threads).next_multiple_of(2).max(2);
    if threads == 1 || h <= per || w == 0 {
        convert(0, h, out);
        return;
    }
    std::thread::scope(|s| {
        for (i, band) in out.chunks_mut(per * w * 4).enumerate() {
            let convert = &convert;
            s.spawn(move || convert(i * per, band.len() / (w * 4), band));
        }
    });
}

impl VideoDecoder for AvDecoder {
    fn codec(&self) -> Codec {
        self.codec
    }

    fn is_hardware(&self) -> bool {
        self.hw
    }

    fn decode(&mut self, packet: &[u8]) -> Result<Option<BgraFrame>> {
        let a = api()?;
        unsafe {
            if (a.codec.av_new_packet)(self.pkt.ptr(), packet.len() as i32) < 0 {
                return Err("av_new_packet".into());
            }
            std::ptr::copy_nonoverlapping(packet.as_ptr(), (*self.pkt.ptr()).data, packet.len());
        }
        let rc = unsafe { (a.codec.avcodec_send_packet)(self.ctx.ptr(), self.pkt.ptr()) };
        self.pkt.unref();
        if rc < 0 && rc != AVERROR_EAGAIN {
            return Err(MediaError::Msg(format!("avcodec_send_packet: {}", av_err_str(rc))));
        }
        let mut latest = None;
        loop {
            let rc = unsafe { (a.codec.avcodec_receive_frame)(self.ctx.ptr(), self.frame.ptr()) };
            if rc == AVERROR_EAGAIN || rc == AVERROR_EOF {
                break;
            }
            if rc < 0 {
                return Err(MediaError::Msg(format!("avcodec_receive_frame: {}", av_err_str(rc))));
            }
            latest = Some(self.to_bgra()?);
            self.frame.unref();
        }
        Ok(latest)
    }
}
