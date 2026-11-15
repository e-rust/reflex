//! A abstração de conexão que o host e o viewer usam.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncWrite};

pub type BoxRead = Box<dyn AsyncRead + Send + Unpin>;
pub type BoxWrite = Box<dyn AsyncWrite + Send + Unpin>;
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Quic,
    Tcp,
}

/// Uma conexão autenticada por TLS mútuo.
pub trait Connection: Send + Sync {
    fn open_bi(&self) -> BoxFuture<'_, io::Result<(BoxWrite, BoxRead)>>;
    fn accept_bi(&self) -> BoxFuture<'_, io::Result<(BoxWrite, BoxRead)>>;
    fn open_uni(&self) -> BoxFuture<'_, io::Result<BoxWrite>>;
    fn accept_uni(&self) -> BoxFuture<'_, io::Result<BoxRead>>;
    /// Não confiável: pode se perder (posição do cursor).
    fn send_datagram(&self, data: Vec<u8>) -> io::Result<()>;
    fn read_datagram(&self) -> BoxFuture<'_, io::Result<Vec<u8>>>;
    /// Certificado (DER) que o outro lado apresentou no TLS.
    fn peer_cert(&self) -> Vec<u8>;
    /// Valor do exporter TLS ([`crate::net::BINDING_LABEL`]): igual dos dois lados
    /// só se for o MESMO canal TLS.
    fn channel_binding(&self) -> [u8; crate::net::BINDING_LEN];
    fn remote_addr(&self) -> SocketAddr;
    fn transport(&self) -> Transport;
    fn close(&self, reason: &str);
}

pub(crate) fn other_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
