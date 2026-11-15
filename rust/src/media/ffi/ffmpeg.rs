//! FFmpeg 7.x (avcodec-61, avutil-59) — só o que o Reflex usa: codificar e
//! decodificar vídeo. Porte de `NoOBS/rust/src/ffi/ffmpeg.rs`; os offsets de
//! estruturas não-ABI-estáveis foram medidos lá contra estas mesmas DLLs.

use std::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;

use crate::dll_api;

pub const LIB_AVCODEC: &str = "avcodec-61.dll";
pub const LIB_AVUTIL: &str = "avutil-59.dll";

pub const AVMEDIA_TYPE_VIDEO: i32 = 0;

pub const AV_PIX_FMT_YUV420P: c_int = 0;
pub const AV_PIX_FMT_YUV444P: c_int = 5;
pub const AV_PIX_FMT_NV12: c_int = 23;
pub const AV_PIX_FMT_D3D11: c_int = 171;
pub const AV_HWDEVICE_TYPE_D3D11VA: c_int = 7;

pub const AVERROR_EAGAIN: c_int = -11;
pub const AVERROR_EOF: c_int = -541_478_725;
pub const AV_PKT_FLAG_KEY: c_int = 0x0001;

/// `AVCodecContext.hw_frames_ctx` / `hw_device_ctx` / `extra_hw_frames` (avcodec-61, medidos no NoOBS).
pub const OFFS_CODECCTX_HW_FRAMES_CTX: usize = 552;
pub const OFFS_CODECCTX_HW_DEVICE_CTX: usize = 560;
pub const OFFS_CODECCTX_EXTRA_HW_FRAMES: usize = 572;
pub const OFFS_BUFREF_DATA: usize = 8;
pub const OFFS_HWDEV_TYPE: usize = 8;
pub const OFFS_HWDEV_HWCTX: usize = 16;
pub const OFFS_HWFR_DEVICE_REF: usize = 8;
pub const OFFS_HWFR_HWCTX: usize = 24;
pub const OFFS_HWFR_POOL_SIZE: usize = 56;
pub const OFFS_HWFR_FORMAT: usize = 60;
pub const OFFS_HWFR_SW_FORMAT: usize = 64;
pub const OFFS_HWFR_WIDTH: usize = 68;
pub const OFFS_HWFR_HEIGHT: usize = 72;
pub const OFFS_D3D11FR_BIND_FLAGS: usize = 8;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct AVRational {
    pub num: c_int,
    pub den: c_int,
}

#[repr(C)]
pub struct AVChannelLayout {
    pub order: c_int,
    pub nb_channels: c_int,
    pub u: u64,
    pub opaque: *mut c_void,
}

/// `AVCodecParameters` — ABI estável.
#[repr(C)]
pub struct AVCodecParameters {
    pub codec_type: c_int,
    pub codec_id: c_int,
    pub codec_tag: u32,
    pub extradata: *mut u8,
    pub extradata_size: c_int,
    pub coded_side_data: *mut c_void,
    pub nb_coded_side_data: c_int,
    pub format: c_int,
    pub bit_rate: i64,
    pub bits_per_coded_sample: c_int,
    pub bits_per_raw_sample: c_int,
    pub profile: c_int,
    pub level: c_int,
    pub width: c_int,
    pub height: c_int,
    pub sample_aspect_ratio: AVRational,
    pub framerate: AVRational,
    pub field_order: c_int,
    pub color_range: c_int,
    pub color_primaries: c_int,
    pub color_trc: c_int,
    pub color_space: c_int,
    pub chroma_location: c_int,
    pub video_delay: c_int,
    pub ch_layout: AVChannelLayout,
    pub sample_rate: c_int,
    pub block_align: c_int,
    pub frame_size: c_int,
    pub initial_padding: c_int,
    pub trailing_padding: c_int,
    pub seek_preroll: c_int,
}

#[repr(C)]
pub struct AVDictionaryEntry {
    pub key: *const c_char,
    pub value: *const c_char,
}

/// `AVPacket` — ABI estável desde o FFmpeg 5.0.
#[repr(C)]
pub struct AVPacket {
    pub buf: *mut c_void,
    pub pts: i64,
    pub dts: i64,
    pub data: *mut u8,
    pub size: c_int,
    pub stream_index: c_int,
    pub flags: c_int,
    pub side_data: *mut c_void,
    pub side_data_elems: c_int,
    pub duration: i64,
    pub pos: i64,
    pub opaque: *mut c_void,
    pub opaque_ref: *mut c_void,
    pub time_base: AVRational,
}

pub type AVCodecContext = c_void;

/// `AVFrame` — só o prefixo público.
#[repr(C)]
pub struct AVFrame {
    pub data: [*mut u8; 8],
    pub linesize: [c_int; 8],
    pub extended_data: *mut *mut u8,
    pub width: c_int,
    pub height: c_int,
    pub nb_samples: c_int,
    pub format: c_int,
    pub key_frame: c_int,
    pub pict_type: c_int,
    pub sample_aspect_ratio: AVRational,
    pub pts: i64,
    pub pkt_dts: i64,
    pub time_base: AVRational,
}

/// `AVCodec` — prefixo público.
#[repr(C)]
pub struct AVCodec {
    pub name: *const c_char,
    pub long_name: *const c_char,
    pub codec_type: c_int,
    pub id: c_int,
}

dll_api! {
    pub struct AvCodecApi;
    fn av_packet_alloc() -> *mut AVPacket;
    fn av_packet_free(pkt: *mut *mut AVPacket);
    fn av_packet_unref(pkt: *mut AVPacket);
    fn av_new_packet(pkt: *mut AVPacket, size: c_int) -> c_int;
    fn avcodec_find_encoder_by_name(name: *const c_char) -> *const AVCodec;
    fn avcodec_find_decoder_by_name(name: *const c_char) -> *const AVCodec;
    fn avcodec_alloc_context3(codec: *const AVCodec) -> *mut AVCodecContext;
    fn avcodec_free_context(avctx: *mut *mut AVCodecContext);
    fn avcodec_parameters_to_context(codec: *mut AVCodecContext, par: *const AVCodecParameters) -> c_int;
    fn avcodec_parameters_alloc() -> *mut AVCodecParameters;
    fn avcodec_parameters_free(par: *mut *mut AVCodecParameters);
    fn avcodec_open2(avctx: *mut AVCodecContext, codec: *const AVCodec, options: *mut *mut c_void) -> c_int;
    fn avcodec_send_packet(avctx: *mut AVCodecContext, avpkt: *const AVPacket) -> c_int;
    fn avcodec_receive_frame(avctx: *mut AVCodecContext, frame: *mut AVFrame) -> c_int;
    fn avcodec_send_frame(avctx: *mut AVCodecContext, frame: *const AVFrame) -> c_int;
    fn avcodec_receive_packet(avctx: *mut AVCodecContext, avpkt: *mut AVPacket) -> c_int;
    fn avcodec_flush_buffers(avctx: *mut AVCodecContext);
}

dll_api! {
    pub struct AvUtilApi;
    fn av_dict_get(m: *const c_void, key: *const c_char, prev: *const AVDictionaryEntry, flags: c_int) -> *mut AVDictionaryEntry;
    fn av_dict_set(pm: *mut *mut c_void, key: *const c_char, value: *const c_char, flags: c_int) -> c_int;
    fn av_dict_free(pm: *mut *mut c_void);
    fn av_strerror(errnum: c_int, buf: *mut c_char, size: usize) -> c_int;
    fn av_frame_alloc() -> *mut AVFrame;
    fn av_frame_free(frame: *mut *mut AVFrame);
    fn av_frame_unref(frame: *mut AVFrame);
    fn av_frame_get_buffer(frame: *mut AVFrame, align: c_int) -> c_int;
    fn av_frame_make_writable(frame: *mut AVFrame) -> c_int;
    fn av_opt_set_int(obj: *mut c_void, name: *const c_char, val: i64, flags: c_int) -> c_int;
    fn av_opt_set_q(obj: *mut c_void, name: *const c_char, val: AVRational, flags: c_int) -> c_int;
    fn av_opt_set(obj: *mut c_void, name: *const c_char, val: *const c_char, flags: c_int) -> c_int;
    fn av_hwdevice_ctx_create(device_ctx: *mut *mut c_void, device_type: c_int, device: *const c_char, opts: *mut c_void, flags: c_int) -> c_int;
    fn av_hwframe_transfer_data(dst: *mut AVFrame, src: *const AVFrame, flags: c_int) -> c_int;
    fn av_buffer_ref(buf: *const c_void) -> *mut c_void;
    fn av_buffer_unref(buf: *mut *mut c_void);
    fn av_hwdevice_ctx_alloc(device_type: c_int) -> *mut c_void;
    fn av_hwdevice_ctx_init(device_ref: *mut c_void) -> c_int;
    fn av_hwframe_ctx_alloc(device_ref: *mut c_void) -> *mut c_void;
    fn av_hwframe_ctx_init(frames_ref: *mut c_void) -> c_int;
    fn av_hwframe_get_buffer(frames_ref: *mut c_void, frame: *mut AVFrame, flags: c_int) -> c_int;
    fn av_log_set_level(level: c_int);
}

pub struct FfmpegApi {
    pub codec: AvCodecApi,
    pub util: AvUtilApi,
}

fn open(name: &str) -> Result<libloading::Library, String> {
    // Com o caminho completo + LOAD_WITH_ALTERED_SEARCH_PATH, as dependências
    // da DLL (avutil, x264…) são procuradas na pasta DELA.
    const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x0000_0008;
    let path = super::dll_dir().map(|d| d.join(name)).unwrap_or_else(|| name.into());
    unsafe { libloading::os::windows::Library::load_with_flags(&path, LOAD_WITH_ALTERED_SEARCH_PATH) }
        .map(Into::into)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// As DLLs, carregadas uma vez. `Err` = alguma faltou.
pub fn api() -> Result<&'static FfmpegApi, String> {
    static API: OnceLock<Result<FfmpegApi, String>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        // avutil antes: o avcodec depende dela.
        let util = AvUtilApi::load(open(LIB_AVUTIL)?).map_err(|e| format!("{LIB_AVUTIL}: {e}"))?;
        let codec = AvCodecApi::load(open(LIB_AVCODEC)?).map_err(|e| format!("{LIB_AVCODEC}: {e}"))?;
        // Só erros no log do FFmpeg (AV_LOG_ERROR = 16).
        (util.av_log_set_level)(16);
        Ok(FfmpegApi { codec, util })
    })
    .as_ref()
    .map_err(Clone::clone)
}

pub fn to_cstring(s: &str) -> std::ffi::CString {
    std::ffi::CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// `av_strerror` legível (só pra log).
pub fn av_err_str(code: c_int) -> String {
    let Ok(a) = api() else { return code.to_string() };
    let mut buf = [0 as c_char; 256];
    unsafe {
        if (a.util.av_strerror)(code, buf.as_mut_ptr(), buf.len()) == 0 {
            return format!("{code} ({})", super::cstr_to_string(buf.as_ptr()));
        }
    }
    code.to_string()
}

/// # Safety
/// `base + off` deve apontar pra um `T` válido.
pub unsafe fn rd<T: Copy>(base: *const c_void, off: usize) -> T {
    unsafe { ((base as *const u8).add(off) as *const T).read_unaligned() }
}

/// # Safety
/// `base + off` deve ter espaço pra um `T`.
pub unsafe fn wr<T>(base: *mut c_void, off: usize, v: T) {
    unsafe { ((base as *mut u8).add(off) as *mut T).write_unaligned(v) }
}

/// `AVPacket` com free no `Drop`.
pub struct Packet(*mut AVPacket);

unsafe impl Send for Packet {}

impl Packet {
    pub fn alloc() -> Option<Self> {
        let a = api().ok()?;
        let p = unsafe { (a.codec.av_packet_alloc)() };
        (!p.is_null()).then_some(Self(p))
    }
    pub fn ptr(&self) -> *mut AVPacket {
        self.0
    }
    pub fn unref(&mut self) {
        if let Ok(a) = api() {
            unsafe { (a.codec.av_packet_unref)(self.0) }
        }
    }
    /// Os bytes do pacote.
    pub fn bytes(&self) -> &[u8] {
        unsafe {
            let p = &*self.0;
            if p.data.is_null() || p.size <= 0 {
                &[]
            } else {
                std::slice::from_raw_parts(p.data, p.size as usize)
            }
        }
    }
    pub fn is_key(&self) -> bool {
        unsafe { (*self.0).flags & AV_PKT_FLAG_KEY != 0 }
    }
}

impl Drop for Packet {
    fn drop(&mut self) {
        if let Ok(a) = api() {
            unsafe { (a.codec.av_packet_free)(&mut self.0) }
        }
    }
}

/// `AVFrame` com free no `Drop`.
pub struct Frame(*mut AVFrame);

unsafe impl Send for Frame {}

impl Frame {
    pub fn alloc() -> Option<Self> {
        let a = api().ok()?;
        let f = unsafe { (a.util.av_frame_alloc)() };
        (!f.is_null()).then_some(Self(f))
    }
    pub fn ptr(&self) -> *mut AVFrame {
        self.0
    }
    pub fn unref(&mut self) {
        if let Ok(a) = api() {
            unsafe { (a.util.av_frame_unref)(self.0) }
        }
    }
}

impl std::ops::Deref for Frame {
    type Target = AVFrame;
    fn deref(&self) -> &AVFrame {
        unsafe { &*self.0 }
    }
}

impl std::ops::DerefMut for Frame {
    fn deref_mut(&mut self) -> &mut AVFrame {
        unsafe { &mut *self.0 }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        if let Ok(a) = api() {
            unsafe { (a.util.av_frame_free)(&mut self.0) }
        }
    }
}

/// `AVDictionary` de opções.
pub struct Dict(*mut c_void);

impl Default for Dict {
    fn default() -> Self {
        Self(std::ptr::null_mut())
    }
}

impl Dict {
    pub fn set(&mut self, key: &str, value: &str) {
        if let Ok(a) = api() {
            unsafe { (a.util.av_dict_set)(&mut self.0, to_cstring(key).as_ptr(), to_cstring(value).as_ptr(), 0) };
        }
    }
    pub fn as_mut_ptr(&mut self) -> *mut *mut c_void {
        &mut self.0
    }
    /// Chaves que o codec NÃO consumiu.
    pub fn leftover_keys(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(a) = api() else { return out };
        if self.0.is_null() {
            return out;
        }
        let mut prev: *const AVDictionaryEntry = std::ptr::null();
        loop {
            let e = unsafe { (a.util.av_dict_get)(self.0, c"".as_ptr(), prev, 2) };
            if e.is_null() {
                break;
            }
            out.push(unsafe { super::cstr_to_string((*e).key) });
            prev = e;
        }
        out
    }
}

impl Drop for Dict {
    fn drop(&mut self) {
        if !self.0.is_null()
            && let Ok(a) = api()
        {
            unsafe { (a.util.av_dict_free)(&mut self.0) }
        }
    }
}

/// `AVCodecContext` com free no `Drop`.
pub struct CodecCtx(*mut AVCodecContext);

unsafe impl Send for CodecCtx {}

impl CodecCtx {
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn alloc(codec: *const AVCodec) -> Option<Self> {
        let a = api().ok()?;
        let c = unsafe { (a.codec.avcodec_alloc_context3)(codec) };
        (!c.is_null()).then_some(Self(c))
    }
    pub fn ptr(&self) -> *mut AVCodecContext {
        self.0
    }
    pub fn set_int(&self, name: &std::ffi::CStr, v: i64) {
        if let Ok(a) = api() {
            unsafe { (a.util.av_opt_set_int)(self.0, name.as_ptr(), v, 0) };
        }
    }
    pub fn set_str(&self, name: &std::ffi::CStr, v: &std::ffi::CStr) {
        if let Ok(a) = api() {
            unsafe { (a.util.av_opt_set)(self.0, name.as_ptr(), v.as_ptr(), 0) };
        }
    }
}

impl Drop for CodecCtx {
    fn drop(&mut self) {
        if let Ok(a) = api() {
            unsafe { (a.codec.avcodec_free_context)(&mut self.0) }
        }
    }
}

/// `AVBufferRef*` com unref no `Drop`.
pub struct BufRef(pub *mut c_void);

unsafe impl Send for BufRef {}

impl BufRef {
    pub fn data(&self) -> *mut c_void {
        unsafe { rd(self.0, OFFS_BUFREF_DATA) }
    }
}

impl Drop for BufRef {
    fn drop(&mut self) {
        if !self.0.is_null()
            && let Ok(a) = api()
        {
            unsafe { (a.util.av_buffer_unref)(&mut self.0) };
        }
    }
}
