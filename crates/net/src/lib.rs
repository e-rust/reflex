//! Transporte (docs/levantamento.md §10.1, §10.2, §10.5).
//!
//! O resto do Reflex só enxerga [`Connection`]: streams bidirecionais e
//! unidirecionais, datagramas, o certificado do outro lado e o *channel
//! binding* do TLS. Duas implementações:
//! - [`quic`]: QUIC (`quinn`) — a principal;
//! - [`tcp`]: TCP + TLS com um multiplexador próprio ([`mux`]) — reserva pra
//!   rede que bloqueia UDP.
//!
//! [`tls`] monta o TLS 1.3 mútuo com certificados autoassinados: quem decide
//! se confia no outro lado é a camada de cima (usuários no host, TOFU no viewer).

pub mod conn;
pub mod endpoint;
pub mod mux;
pub mod quic;
pub mod tcp;
pub mod tls;

pub use conn::{BoxRead, BoxWrite, Connection, Transport};
pub use endpoint::{connect, Listener, TransportMode};
pub use tls::{LocalCert, ServerCheck};

/// Rótulo do exporter TLS que vira o channel binding (SPAKE2 do convite).
pub const BINDING_LABEL: &[u8] = b"EXPORTER-reflex-binding";
pub const BINDING_LEN: usize = 32;
