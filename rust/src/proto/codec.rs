//! Codecs e a escolha do codec da sessão (§10.6).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
    /// Imagem sem perda: só os retângulos que mudaram, em zstd
    /// (`docs/hibrido.md`). CPU pura: todo host e todo viewer fazem.
    Lossless,
}

impl Codec {
    /// Os codecs de vídeo na preferência do modo Auto: o mais eficiente primeiro.
    pub const BY_EFFICIENCY: [Codec; 3] = [Codec::Av1, Codec::Hevc, Codec::H264];

    pub fn label(self) -> &'static str {
        match self {
            Codec::H264 => "H.264",
            Codec::Hevc => "HEVC",
            Codec::Av1 => "AV1",
            Codec::Lossless => "sem perda",
        }
    }
}

/// O que o usuário escolheu no viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CodecChoice {
    #[default]
    Auto,
    Fixed(Codec),
}

/// Um codec que um lado sabe codificar (host) ou decodificar (viewer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodecCap {
    pub codec: Codec,
    /// Por hardware (GPU) ou por software (CPU).
    pub hw: bool,
}

/// Resultado da escolha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub codec: Codec,
    pub hw_encode: bool,
    pub hw_decode: bool,
    /// A escolha manual não era possível e caiu no Auto.
    pub fell_back: bool,
}

fn best_cap(caps: &[CodecCap], codec: Codec) -> Option<CodecCap> {
    let mut found = None;
    for c in caps.iter().filter(|c| c.codec == codec) {
        if c.hw {
            return Some(*c);
        }
        found = Some(*c);
    }
    found
}

fn pair(encoders: &[CodecCap], decoders: &[CodecCap], codec: Codec) -> Option<(CodecCap, CodecCap)> {
    Some((best_cap(encoders, codec)?, best_cap(decoders, codec)?))
}

/// Escolhe o codec da sessão.
///
/// - `Fixed`: usa o codec pedido se o host codifica e o viewer decodifica;
///   senão cai no Auto (`fell_back`).
/// - `Auto`: o mais eficiente com hardware dos dois lados (AV1 > HEVC > H.264);
///   host sem encoder de placa (VM, servidor): sem perda (de 3 a 20 vezes
///   menos CPU que o H.264, medido); sem nada disso, H.264; por fim qualquer
///   codec em comum.
pub fn select_codec(choice: CodecChoice, encoders: &[CodecCap], decoders: &[CodecCap]) -> Option<Selection> {
    let make = |(e, d): (CodecCap, CodecCap), fell_back| Selection { codec: e.codec, hw_encode: e.hw, hw_decode: d.hw, fell_back };

    if let CodecChoice::Fixed(codec) = choice
        && let Some(p) = pair(encoders, decoders, codec)
    {
        return Some(make(p, false));
    }
    let fell_back = matches!(choice, CodecChoice::Fixed(_));

    let both_hw = Codec::BY_EFFICIENCY.into_iter().filter_map(|c| pair(encoders, decoders, c)).find(|(e, d)| e.hw && d.hw);
    if let Some(p) = both_hw {
        return Some(make(p, fell_back));
    }
    if !encoders.iter().any(|e| e.hw)
        && let Some(p) = pair(encoders, decoders, Codec::Lossless)
    {
        return Some(make(p, fell_back));
    }
    if let Some(p) = pair(encoders, decoders, Codec::H264) {
        return Some(make(p, fell_back));
    }
    Codec::BY_EFFICIENCY.into_iter().find_map(|c| pair(encoders, decoders, c)).map(|p| make(p, fell_back))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(c: Codec) -> CodecCap {
        CodecCap { codec: c, hw: true }
    }
    fn sw(c: Codec) -> CodecCap {
        CodecCap { codec: c, hw: false }
    }

    #[test]
    fn auto_prefers_av1_when_both_sides_have_hardware() {
        let enc = [hw(Codec::H264), hw(Codec::Hevc), hw(Codec::Av1)];
        let dec = [hw(Codec::H264), hw(Codec::Hevc), hw(Codec::Av1)];
        let s = select_codec(CodecChoice::Auto, &enc, &dec).unwrap();
        assert_eq!(s.codec, Codec::Av1);
        assert!(s.hw_encode && s.hw_decode && !s.fell_back);
    }

    #[test]
    fn auto_skips_codec_without_hardware_decoder() {
        let enc = [hw(Codec::H264), hw(Codec::Hevc), hw(Codec::Av1)];
        let dec = [hw(Codec::H264), hw(Codec::Hevc), sw(Codec::Av1)];
        assert_eq!(select_codec(CodecChoice::Auto, &enc, &dec).unwrap().codec, Codec::Hevc);
    }

    #[test]
    fn auto_without_hardware_pair_uses_h264() {
        let enc = [sw(Codec::H264)];
        let dec = [sw(Codec::H264), sw(Codec::Av1), hw(Codec::Hevc)];
        let s = select_codec(CodecChoice::Auto, &enc, &dec).unwrap();
        assert_eq!(s.codec, Codec::H264);
        assert!(!s.hw_encode);
    }

    #[test]
    fn host_without_hardware_encoder_sends_lossless() {
        let enc = [sw(Codec::H264), sw(Codec::Lossless)];
        let dec = [hw(Codec::H264), sw(Codec::Lossless)];
        let s = select_codec(CodecChoice::Auto, &enc, &dec).unwrap();
        assert_eq!(s.codec, Codec::Lossless);
        assert!(!s.hw_encode && !s.fell_back);
    }

    #[test]
    fn host_with_hardware_encoder_keeps_video_even_if_the_viewer_decodes_in_software() {
        let enc = [hw(Codec::H264), sw(Codec::Lossless)];
        let dec = [sw(Codec::H264), sw(Codec::Lossless)];
        assert_eq!(select_codec(CodecChoice::Auto, &enc, &dec).unwrap().codec, Codec::H264);
    }

    #[test]
    fn lossless_can_be_asked_for_explicitly() {
        let enc = [hw(Codec::H264), sw(Codec::Lossless)];
        let dec = [hw(Codec::H264), sw(Codec::Lossless)];
        assert_eq!(select_codec(CodecChoice::Fixed(Codec::Lossless), &enc, &dec).unwrap().codec, Codec::Lossless);
    }

    #[test]
    fn fixed_choice_is_respected_when_possible() {
        let enc = [hw(Codec::H264), hw(Codec::Av1)];
        let dec = [hw(Codec::H264), sw(Codec::Av1)];
        let s = select_codec(CodecChoice::Fixed(Codec::Av1), &enc, &dec).unwrap();
        assert_eq!(s.codec, Codec::Av1);
        assert!(!s.hw_decode && !s.fell_back);
    }

    #[test]
    fn impossible_fixed_choice_falls_back_to_auto() {
        let enc = [hw(Codec::H264)];
        let dec = [hw(Codec::H264), hw(Codec::Av1)];
        let s = select_codec(CodecChoice::Fixed(Codec::Av1), &enc, &dec).unwrap();
        assert_eq!(s.codec, Codec::H264);
        assert!(s.fell_back);
    }

    #[test]
    fn nothing_in_common_is_none() {
        assert!(select_codec(CodecChoice::Auto, &[hw(Codec::Av1)], &[hw(Codec::H264)]).is_none());
    }

    #[test]
    fn prefers_hardware_entry_when_listed_twice() {
        let enc = [sw(Codec::H264), hw(Codec::H264)];
        let s = select_codec(CodecChoice::Auto, &enc, &[hw(Codec::H264)]).unwrap();
        assert!(s.hw_encode);
    }
}
