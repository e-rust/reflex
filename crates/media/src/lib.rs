//! Mídia do Reflex (docs/levantamento.md §1, §2, §4.1, §4.2).
//!
//! Reaproveita o motor do NoOBS (`rust/src/capture`), com afinação de tempo
//! real em vez de gravação:
//! - [`gpu`]: dispositivo D3D11 e conversor BGRA → NV12 (com escala);
//! - [`capture`]: DXGI Desktop Duplication (funciona na tela de login) e a
//!   lista de monitores;
//! - [`encode`] / [`decode`]: traits + libavcodec (AMF/NVENC por GPU, x264 e
//!   Media Foundation por memória; decode D3D11VA ou software);
//! - [`yuv`]: NV12/I420 → BGRA (BT.709 faixa limitada) pro viewer;
//! - [`cursor`]: formas de cursor do Windows → BGRA;
//! - [`pacing`]: quando codificar (nunca enfileirar quadro, §10.4);
//! - [`caps`]: o que esta máquina codifica/decodifica;
//! - [`clock`]: relógio QPC.

pub mod caps;
pub mod capture;
pub mod clock;
pub mod damage;
pub mod cursor;
pub mod decode;
pub mod encode;
pub mod ffi;
pub mod gpu;
pub mod pacing;
pub mod yuv;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("{0}")]
    Msg(String),
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
