//! Decodificação no viewer (§2.3). O quadro sai em BGRA pro desenho.
//!
//! Os decoders: o sem perda ([`crate::media::lossless_codec`]) em todos os
//! sistemas; no Windows, o libavcodec ([`crate::media::av_decode`]).

use crate::media::Result;
use crate::proto::codec::Codec;

#[cfg(windows)]
pub use crate::media::av_decode::AvDecoder;

/// Quadro pronto pra desenhar: BGRA compacto (`width*4` por linha).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgraFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

pub trait VideoDecoder: Send {
    fn codec(&self) -> Codec;
    fn is_hardware(&self) -> bool;
    /// Entrega um pacote; devolve o quadro mais novo que saiu, se saiu.
    fn decode(&mut self, packet: &[u8]) -> Result<Option<BgraFrame>>;
}

/// O decoder de `codec` (pela placa se der).
pub fn open(codec: Codec) -> Result<Box<dyn VideoDecoder>> {
    match codec {
        Codec::Lossless => Ok(Box::new(crate::media::lossless_codec::LosslessDecoder::default())),
        #[cfg(windows)]
        codec => Ok(Box::new(AvDecoder::open(codec, true)?)),
        #[cfg(not(windows))]
        codec => Err(format!("{} ainda não decodifica neste sistema", codec.label()).into()),
    }
}
