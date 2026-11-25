//! O que esta máquina codifica e decodifica (vai no `Hello`).
//!
//! Cada sistema descobre do seu jeito: no Windows, testando a placa e o
//! libavcodec ([`crate::media::caps_windows`]); no Linux, só a CPU (o sem
//! perda e o openh264). A escolha entre eles é a mesma.

use crate::media::encode::FULL_CHROMA_BACKEND;
use crate::proto::codec::{Codec, CodecCap};

#[cfg(windows)]
pub use crate::media::caps_windows::{decoders, encoders, probe_encoders};
#[cfg(target_os = "linux")]
pub use self::linux::{decoders, encoders};

/// Um encoder que funcionou no teste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderChoice {
    pub cap: CodecCap,
    pub backend: &'static str,
}

/// Dá pra mandar cor completa (4:4:4)? Só com o `libx264` na build do FFmpeg.
pub fn has_full_chroma(choices: &[EncoderChoice]) -> bool {
    choices.iter().any(|c| c.backend == FULL_CHROMA_BACKEND)
}

/// O backend pra um codec (preferindo o de hardware se `hw`).
pub fn backend_for(choices: &[EncoderChoice], codec: Codec, hw: bool) -> Option<&'static str> {
    choices.iter().filter(|c| c.cap.codec == codec).find(|c| c.cap.hw == hw).or_else(|| choices.iter().find(|c| c.cap.codec == codec)).map(|c| c.backend)
}

/// Linux: sem placa por enquanto (VAAPI depois; docs/linux.md).
#[cfg(target_os = "linux")]
mod linux {
    use super::EncoderChoice;
    use crate::proto::codec::{Codec, CodecCap};

    /// O sem perda, sempre.
    pub fn encoders() -> &'static [EncoderChoice] {
        const CPU: [EncoderChoice; 1] = [EncoderChoice { cap: CodecCap { codec: Codec::Lossless, hw: false }, backend: crate::media::lossless_codec::BACKEND }];
        &CPU
    }

    /// O sem perda, sempre.
    pub fn decoders() -> Vec<CodecCap> {
        vec![CodecCap { codec: Codec::Lossless, hw: false }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
