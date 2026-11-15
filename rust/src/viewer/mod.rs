//! Lado que controla (docs/levantamento.md §5.2, §10.1).
//!
//! - [`hosts`]: a lista de hosts salvos e a confiança TOFU (impressão digital
//!   do certificado), persistida; pura e testada;
//! - [`invite_link`]: montar/ler o link `reflex://…` do acesso único (§11.4);
//! - [`client`]: conectar, handshake e os laços de decode/apresentação/input.

pub mod client;
pub mod hosts;
pub mod invite_link;

#[derive(Debug, thiserror::Error)]
pub enum ViewerError {
    #[error("rede: {0}")]
    Net(#[from] std::io::Error),
    #[error("protocolo: {0}")]
    Frame(#[from] crate::proto::frame::FrameError),
    #[error("mídia: {0}")]
    Media(#[from] crate::media::MediaError),
    #[error("o host recusou: {0}")]
    Denied(String),
    #[error("o certificado do host não confere (impressão digital mudou)")]
    UntrustedHost,
}
