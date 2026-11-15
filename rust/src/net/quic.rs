//! QUIC (`quinn`): a implementação principal de [`Connection`].

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::pki_types::CertificateDer;

use crate::net::conn::{other_err, BoxFuture, BoxRead, BoxWrite, Connection, Transport};
use crate::net::tls::{client_config, server_config, LocalCert, ServerCheck};
use crate::net::{BINDING_LABEL, BINDING_LEN};

fn transport_config() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(5)));
    // Viewer que sumiu sem avisar (processo morto, rede caiu) é solto em 10 s.
    t.max_idle_timeout(Some(Duration::from_secs(10).try_into().expect("10 s cabe")));
    t.max_concurrent_bidi_streams(256u32.into());
    t.max_concurrent_uni_streams(64u32.into());
    Arc::new(t)
}

pub struct QuicConnection {
    conn: quinn::Connection,
    peer_cert: Vec<u8>,
    binding: [u8; BINDING_LEN],
}

impl QuicConnection {
    fn wrap(conn: quinn::Connection) -> io::Result<Self> {
        let peer_cert = conn
            .peer_identity()
            .and_then(|any| any.downcast::<Vec<CertificateDer<'static>>>().ok())
            .and_then(|certs| certs.first().map(|c| c.to_vec()))
            .ok_or_else(|| other_err("o outro lado não apresentou certificado"))?;
        let mut binding = [0u8; BINDING_LEN];
        conn.export_keying_material(&mut binding, BINDING_LABEL, b"").map_err(|_| other_err("exporter TLS indisponível"))?;
        Ok(Self { conn, peer_cert, binding })
    }
}

impl Connection for QuicConnection {
    fn open_bi(&self) -> BoxFuture<'_, io::Result<(BoxWrite, BoxRead)>> {
        Box::pin(async move {
            let (s, r) = self.conn.open_bi().await.map_err(other_err)?;
            Ok((Box::new(s) as BoxWrite, Box::new(r) as BoxRead))
        })
    }

    fn accept_bi(&self) -> BoxFuture<'_, io::Result<(BoxWrite, BoxRead)>> {
        Box::pin(async move {
            let (s, r) = self.conn.accept_bi().await.map_err(other_err)?;
            Ok((Box::new(s) as BoxWrite, Box::new(r) as BoxRead))
        })
    }

    fn open_uni(&self) -> BoxFuture<'_, io::Result<BoxWrite>> {
        Box::pin(async move { Ok(Box::new(self.conn.open_uni().await.map_err(other_err)?) as BoxWrite) })
    }

    fn accept_uni(&self) -> BoxFuture<'_, io::Result<BoxRead>> {
        Box::pin(async move { Ok(Box::new(self.conn.accept_uni().await.map_err(other_err)?) as BoxRead) })
    }

    fn send_datagram(&self, data: Vec<u8>) -> io::Result<()> {
        self.conn.send_datagram(data.into()).map_err(other_err)
    }

    fn read_datagram(&self) -> BoxFuture<'_, io::Result<Vec<u8>>> {
        Box::pin(async move { Ok(self.conn.read_datagram().await.map_err(other_err)?.to_vec()) })
    }

    fn peer_cert(&self) -> Vec<u8> {
        self.peer_cert.clone()
    }

    fn channel_binding(&self) -> [u8; BINDING_LEN] {
        self.binding
    }

    fn remote_addr(&self) -> SocketAddr {
        self.conn.remote_address()
    }

    fn transport(&self) -> Transport {
        Transport::Quic
    }

    fn close(&self, reason: &str) {
        self.conn.close(0u32.into(), reason.as_bytes());
    }
}

/// Ponto de escuta QUIC do host.
pub struct QuicListener {
    endpoint: quinn::Endpoint,
}

impl QuicListener {
    pub fn bind(addr: SocketAddr, local: &LocalCert) -> io::Result<Self> {
        let crypto = QuicServerConfig::try_from(server_config(local).map_err(other_err)?).map_err(other_err)?;
        let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(crypto));
        cfg.transport_config(transport_config());
        Ok(Self { endpoint: quinn::Endpoint::server(cfg, addr)? })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.endpoint.local_addr()
    }

    /// Próxima conexão com handshake completo. `None` = endpoint fechado.
    pub async fn accept(&self) -> Option<io::Result<QuicConnection>> {
        let incoming = self.endpoint.accept().await?;
        Some(async move { QuicConnection::wrap(incoming.await.map_err(other_err)?) }.await)
    }
}

/// Conecta como viewer.
pub async fn connect(addr: SocketAddr, local: &LocalCert, check: ServerCheck) -> io::Result<QuicConnection> {
    let crypto = QuicClientConfig::try_from(client_config(local, check).map_err(other_err)?).map_err(other_err)?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(crypto));
    cfg.transport_config(transport_config());
    let bind: SocketAddr = if addr.is_ipv6() { "[::]:0".parse().expect("fixo") } else { "0.0.0.0:0".parse().expect("fixo") };
    let mut endpoint = quinn::Endpoint::client(bind)?;
    endpoint.set_default_client_config(cfg);
    let conn = endpoint.connect(addr, "reflex").map_err(other_err)?.await.map_err(other_err)?;
    QuicConnection::wrap(conn)
}
