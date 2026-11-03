//! Lado controlado (docs/levantamento.md §4, §10, §11).
//!
//! - [`handshake`]: Hello → autenticação (chave do viewer, ou convite por
//!   SPAKE2) → política → `AuthResult`. Testável sobre um par de streams;
//! - [`engine`]: a captura+codificação numa thread, atrás de um canal de
//!   pacotes (o resto do host não sabe de D3D11 nem FFmpeg);
//! - [`session`]: uma conexão autenticada — vídeo, input, cursor, controle;
//! - [`server`]: o aceitador, com o [`reflex_auth::store`] compartilhado.

pub mod engine;
pub mod handshake;
pub mod hub;
pub mod server;
pub mod session;
pub mod user_bridge;

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("rede: {0}")]
    Net(#[from] std::io::Error),
    #[error("protocolo: {0}")]
    Frame(#[from] reflex_proto::frame::FrameError),
    #[error("mídia: {0}")]
    Media(#[from] reflex_media::MediaError),
    #[error("recusado: {0}")]
    Denied(String),
}
