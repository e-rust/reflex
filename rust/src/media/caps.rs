//! O que esta máquina codifica e decodifica (vai no `Hello`).
//!
//! Encoders: abre cada candidato da placa e codifica 1 quadro de verdade
//! (o mesmo critério do `caps.rs` do NoOBS: driver instalado não basta).
//! Decoders: perfis do D3D11 Video da placa + software do libavcodec.

use std::sync::OnceLock;

use crate::proto::codec::{Codec, CodecCap};
use windows::core::{Interface, GUID};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11VideoDevice, D3D11_BIND_RENDER_TARGET, D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0, D3D11_DECODER_PROFILE_H264_VLD_NOFGT, D3D11_DECODER_PROFILE_HEVC_VLD_MAIN,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12;

use crate::media::encode::{candidates, is_hardware, open_encoder, EncoderConfig, FULL_CHROMA_BACKEND};
use crate::media::gpu::Device;

/// Um encoder que funcionou no teste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderChoice {
    pub cap: CodecCap,
    pub backend: &'static str,
}

/// Tamanho do teste: o HEVC/AV1 do AMF recusam abaixo de ~320 px (medido: 256x256 falha).
const PROBE_W: u32 = 640;
const PROBE_H: u32 = 360;

fn works(dev: &Device, codec: Codec, backend: &str) -> bool {
    let cfg = EncoderConfig { codec, backend: backend.to_string(), width: PROBE_W, height: PROBE_H, fps: 30, bitrate_kbps: 1000, vbv_frames: 1, chroma444: false };
    let Ok(tex) = dev.texture(PROBE_W, PROBE_H, DXGI_FORMAT_NV12, D3D11_BIND_RENDER_TARGET) else { return false };
    match open_encoder(dev, &cfg).and_then(|mut e| e.encode(dev, &tex, true)) {
        Ok(_) => true,
        Err(e) => {
            log::info!("encoder {backend} indisponível: {e}");
            false
        }
    }
}

/// Testa os candidatos na placa de `dev`: o 1º de hardware e o de CPU, por codec.
pub fn probe_encoders(dev: &Device) -> Vec<EncoderChoice> {
    let vendor = dev.vendor_id();
    let mut out = Vec::new();
    for codec in [Codec::H264, Codec::Hevc, Codec::Av1] {
        let mut have_hw = false;
        for backend in candidates(codec, vendor) {
            let hw = is_hardware(backend);
            if hw && have_hw {
                continue;
            }
            if works(dev, codec, backend) {
                have_hw |= hw;
                out.push(EncoderChoice { cap: CodecCap { codec, hw }, backend });
            }
        }
    }
    out
}

/// Encoders da placa padrão (testados uma vez por processo).
pub fn encoders() -> &'static [EncoderChoice] {
    static CACHE: OnceLock<Vec<EncoderChoice>> = OnceLock::new();
    CACHE.get_or_init(|| Device::create().map(|d| probe_encoders(&d)).unwrap_or_default())
}

/// Dá pra mandar cor completa (4:4:4)? Só com o `libx264` na build do FFmpeg.
pub fn has_full_chroma(choices: &[EncoderChoice]) -> bool {
    choices.iter().any(|c| c.backend == FULL_CHROMA_BACKEND)
}

/// O backend pra um codec (preferindo o de hardware se `hw`).
pub fn backend_for(choices: &[EncoderChoice], codec: Codec, hw: bool) -> Option<&'static str> {
    choices.iter().filter(|c| c.cap.codec == codec).find(|c| c.cap.hw == hw).or_else(|| choices.iter().find(|c| c.cap.codec == codec)).map(|c| c.backend)
}

fn hw_decode_profiles(dev: &Device) -> Vec<GUID> {
    let Ok(video) = dev.device.cast::<ID3D11VideoDevice>() else { return Vec::new() };
    let n = unsafe { video.GetVideoDecoderProfileCount() };
    (0..n).filter_map(|i| unsafe { video.GetVideoDecoderProfile(i) }.ok()).collect()
}

/// Decoders: hardware pelos perfis da placa; software pro H.264/HEVC.
pub fn decoders() -> Vec<CodecCap> {
    let profiles = Device::create().map(|d| hw_decode_profiles(&d)).unwrap_or_default();
    decoders_from_profiles(&profiles)
}

pub fn decoders_from_profiles(profiles: &[GUID]) -> Vec<CodecCap> {
    let mut out = Vec::new();
    for (codec, guid) in [
        (Codec::H264, D3D11_DECODER_PROFILE_H264_VLD_NOFGT),
        (Codec::Hevc, D3D11_DECODER_PROFILE_HEVC_VLD_MAIN),
        (Codec::Av1, D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0),
    ] {
        if profiles.contains(&guid) {
            out.push(CodecCap { codec, hw: true });
        }
    }
    // Software: esta build tem h264 e hevc (AV1 só por hardware).
    out.push(CodecCap { codec: Codec::H264, hw: false });
    out.push(CodecCap { codec: Codec::Hevc, hw: false });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_decoders_always_listed() {
        let d = decoders_from_profiles(&[]);
        assert!(d.contains(&CodecCap { codec: Codec::H264, hw: false }));
        assert!(!d.iter().any(|c| c.codec == Codec::Av1));
    }

    #[test]
    fn av1_needs_hardware_profile() {
        let d = decoders_from_profiles(&[D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0]);
        assert!(d.contains(&CodecCap { codec: Codec::Av1, hw: true }));
    }

    #[test]
    fn backend_prefers_requested_kind() {
        let choices = [
            EncoderChoice { cap: CodecCap { codec: Codec::H264, hw: true }, backend: "h264_amf" },
            EncoderChoice { cap: CodecCap { codec: Codec::H264, hw: false }, backend: "openh264" },
            EncoderChoice { cap: CodecCap { codec: Codec::H264, hw: false }, backend: "libx264" },
        ];
        // CPU: o openh264 vem antes (o x264 fica pra cor completa).
        assert_eq!(backend_for(&choices, Codec::H264, false), Some("openh264"));
        assert!(has_full_chroma(&choices));
        assert!(!has_full_chroma(&choices[..2]));
        assert_eq!(backend_for(&choices, Codec::H264, true), Some("h264_amf"));
        assert_eq!(backend_for(&choices, Codec::Av1, true), None);
    }

    #[test]
    #[ignore = "precisa de GPU e das DLLs do FFmpeg"]
    fn probe_finds_some_h264_encoder() {
        let dev = Device::create().unwrap();
        let found = probe_encoders(&dev);
        println!("{found:?}");
        assert!(found.iter().any(|c| c.cap.codec == Codec::H264));
    }
}
