//! TCP + TLS 1.3 + [`Mux`]: a reserva quando o UDP está bloqueado.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::{TcpListener as TokioListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::conn::{other_err, BoxFuture, BoxRead, BoxWrite, Connection, Transport};
use crate::mux::Mux;
use crate::tls::{client_config, server_config, server_name, LocalCert, ServerCheck};
use crate::{BINDING_LABEL, BINDING_LEN};

pub struct TcpConnection {
    mux: Mux,
    peer_cert: Vec<u8>,
    binding: [u8; BINDING_LEN],
    remote: SocketAddr,
}

/// Lê o que interessa do TLS antes de entregar o fluxo ao multiplexador.
fn tls_facts(conn: &rustls::CommonState, exporter: impl FnOnce(&mut [u8]) -> Result<(), rustls::Error>) -> io::Result<(Vec<u8>, [u8; BINDING_LEN])> {
    let cert = conn.peer_certificates().and_then(|c| c.first()).map(|c| c.to_vec()).ok_or_else(|| other_err("o outro lado não apresentou certificado"))?;
    let mut binding = [0u8; BINDING_LEN];
    exporter(&mut binding).map_err(other_err)?;
    Ok((cert, binding))
}

impl Connection for TcpConnection {
    fn open_bi(&self) -> BoxFuture<'_, io::Result<(BoxWrite, BoxRead)>> {
        Box::pin(self.mux.open_bi())
    }

    fn accept_bi(&self) -> BoxFuture<'_, io::Result<(BoxWrite, BoxRead)>> {
        Box::pin(self.mux.accept_bi())
    }

    fn open_uni(&self) -> BoxFuture<'_, io::Result<BoxWrite>> {
        Box::pin(self.mux.open_uni())
    }

    fn accept_uni(&self) -> BoxFuture<'_, io::Result<BoxRead>> {
        Box::pin(self.mux.accept_uni())
    }

    fn send_datagram(&self, data: Vec<u8>) -> io::Result<()> {
        self.mux.send_datagram(data)
    }

    fn read_datagram(&self) -> BoxFuture<'_, io::Result<Vec<u8>>> {
        Box::pin(self.mux.read_datagram())
    }

    fn peer_cert(&self) -> Vec<u8> {
        self.peer_cert.clone()
    }

    fn channel_binding(&self) -> [u8; BINDING_LEN] {
        self.binding
    }

    fn remote_addr(&self) -> SocketAddr {
        self.remote
    }

    fn transport(&self) -> Transport {
        Transport::Tcp
    }

    fn close(&self, _reason: &str) {
        self.mux.close();
    }
}

pub struct TcpListener {
    listener: TokioListener,
    acceptor: TlsAcceptor,
}

impl TcpListener {
    pub async fn bind(addr: SocketAddr, local: &LocalCert) -> io::Result<Self> {
        let acceptor = TlsAcceptor::from(Arc::new(server_config(local).map_err(other_err)?));
        Ok(Self { listener: TokioListener::bind(addr).await?, acceptor })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Aceita o TCP; o handshake TLS fica no futuro devolvido (não segura o laço de accept).
    pub async fn accept(&self) -> io::Result<impl std::future::Future<Output = io::Result<TcpConnection>>> {
        let (tcp, remote) = self.listener.accept().await?;
        let acceptor = self.acceptor.clone();
        Ok(async move {
            tcp.set_nodelay(true)?;
            let tls = acceptor.accept(tcp).await?;
            let (cert, binding) = {
                let (_, s) = tls.get_ref();
                tls_facts(s, |out| s.export_keying_material(out, BINDING_LABEL, None).map(|_| ()))?
            };
            let (r, w) = tokio::io::split(tls);
            Ok(TcpConnection { mux: Mux::new(r, w, false), peer_cert: cert, binding, remote })
        })
    }
}

pub async fn connect(addr: SocketAddr, local: &LocalCert, check: ServerCheck) -> io::Result<TcpConnection> {
    let connector = TlsConnector::from(Arc::new(client_config(local, check).map_err(other_err)?));
    let tcp = TcpStream::connect(addr).await?;
    tcp.set_nodelay(true)?;
    let tls = connector.connect(server_name(), tcp).await?;
    let (cert, binding) = {
        let (_, c) = tls.get_ref();
        tls_facts(c, |out| c.export_keying_material(out, BINDING_LABEL, None).map(|_| ()))?
    };
    let (r, w) = tokio::io::split(tls);
    Ok(TcpConnection { mux: Mux::new(r, w, true), peer_cert: cert, binding, remote: addr })
}
