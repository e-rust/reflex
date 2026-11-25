//! Mídia do Reflex (docs/levantamento.md §1, §2, §4.1, §4.2; docs/linux.md).
//!
//! Reaproveita o motor do NoOBS (`rust/src/capture`), com afinação de tempo
//! real em vez de gravação:
//! - [`encode`] / [`decode`]: os traits dos codecs;
//! - [`lossless_codec`]: a imagem sem perda (zstd das áreas alteradas);
//! - [`openh264enc`]: H.264 pela CPU com o openh264 da Cisco;
//! - [`yuv`]: conversões BGRA ↔ NV12/I420/4:4:4 (BT.709 faixa limitada);
//! - [`cursor`]: formas de cursor → BGRA;
//! - [`pacing`]: quando codificar (nunca enfileirar quadro, §10.4);
//! - [`caps`]: o que esta máquina codifica/decodifica;
//! - [`clock`]: relógio monotônico.
//!
//! Só no Windows:
//! - `gpu`: dispositivo D3D11 e conversor BGRA → NV12 (com escala);
//! - `capture`: DXGI Desktop Duplication (funciona na tela de login) e a
//!   lista de monitores;
//! - `ffi`, `av_decode` e o encoder do libavcodec em [`encode`] (AMF/NVENC
//!   por GPU, Media Foundation por memória; decode D3D11VA ou software).

#[cfg(windows)]
pub mod av_decode;
pub mod caps;
#[cfg(windows)]
pub mod caps_windows;
#[cfg(windows)]
pub mod capture;
pub mod clock;
pub mod cursor;
pub mod damage;
pub mod decode;
pub mod encode;
#[cfg(windows)]
pub mod ffi;
pub mod frame_info;
#[cfg(windows)]
pub mod gpu;
pub mod lossless;
pub mod lossless_codec;
pub mod openh264enc;
pub mod pacing;
#[cfg(target_os = "linux")]
pub mod pipewire;
pub mod yuv;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("{0}")]
    Msg(String),
    #[cfg(windows)]
    #[error("Windows: {0}")]
    Win(#[from] windows_core::Error),
    /// A captura perdeu o acesso (troca de área de trabalho, resolução, UAC):
    /// recriar.
    #[error("captura perdeu o acesso à tela")]
    AccessLost,
}

impl From<String> for MediaError {
    fn from(s: String) -> Self {
        MediaError::Msg(s)
    }
}

impl From<&str> for MediaError {
    fn from(s: &str) -> Self {
        MediaError::Msg(s.to_string())
    }
}

pub type Result<T> = std::result::Result<T, MediaError>;
