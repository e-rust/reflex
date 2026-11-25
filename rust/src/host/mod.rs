//! Lado controlado (docs/levantamento.md §4, §10, §11).
//!
//! - [`handshake`]: Hello → autenticação (chave do viewer, ou convite por
//!   SPAKE2) → política → `AuthResult`. Testável sobre um par de streams;
//! - [`engine`]: a captura+codificação numa thread, atrás de um canal de
//!   pacotes (o resto do host não sabe de D3D11 nem FFmpeg);
//! - [`session`]: uma conexão autenticada — vídeo, input, cursor, controle;
//! - [`server`]: o aceitador, com o [`crate::auth::store`] compartilhado.

pub mod engine;
pub mod handshake;
pub mod hub;
pub mod hybrid;
pub mod screen;
pub mod server;
pub mod session;

// A ponte com o processo do usuário (clipboard, arquivos) é do Windows: no
// Linux o agente já roda como o usuário (e a ponte não tem valor).
#[cfg(windows)]
pub mod user_bridge;
#[cfg(not(windows))]
#[path = "user_bridge_none.rs"]
pub mod user_bridge;

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("rede: {0}")]
    Net(#[from] std::io::Error),
    #[error("protocolo: {0}")]
    Frame(#[from] crate::proto::frame::FrameError),
    #[error("mídia: {0}")]
    Media(#[from] crate::media::MediaError),
    #[error("recusado: {0}")]
    Denied(String),
}
