//! A tela que o motor captura, com a mesma interface em todos os sistemas:
//! abrir um monitor, esperar imagem nova ([`Screen::acquire`]), ler o BGRA
//! das áreas alteradas ([`Screen::read_bgra`]) e, no Windows com placa,
//! codificar direto da placa ([`Screen::encode_gpu`]).
//!
//! - Windows: DXGI + D3D11 (`screen_windows.rs`);
//! - Linux: PipeWire do Mutter (`screen_linux.rs`).

#[cfg(windows)]
#[path = "screen_windows.rs"]
mod sys;

#[cfg(target_os = "linux")]
#[path = "screen_linux.rs"]
mod sys;

pub use sys::{monitors, Screen};
