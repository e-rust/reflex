//! Ponto de escuta (host: QUIC e TCP na mesma porta) e conexão (viewer:
//! tenta QUIC, cai pro TCP).

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::conn::Connection;
use crate::quic::QuicListener;
use crate::tcp::TcpListener;
use crate::tls::{LocalCert, ServerCheck};

/// Quanto o viewer espera o QUIC antes de tentar o TCP.
pub const QUIC_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransportMode {
    #[default]
    Auto,
    Quic,
    Tcp,
}

pub type SharedConn = Arc<dyn Connection>;

/// Escuta UDP (QUIC) e TCP na mesma porta e entrega as conexões prontas.
pub struct Listener {
    rx: mpsc::Receiver<io::Result<SharedConn>>,
    port: u16,
}

/// Porta reservada (WSAEACCES 10013) ou já usada no TCP: vale tentar outra. Pura.
fn retryable(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::PermissionDenied | io::ErrorKind::AddrInUse)
}

impl Listener {
    /// QUIC (UDP) e TCP no mesmo número de porta. Com porta 0 (escolhida pelo
    /// sistema), o número que o UDP ganhou pode estar numa faixa que o Windows
    /// reservou só pro TCP (Hyper-V/WinNAT reservam faixas novas a cada
    /// reinício: `netsh int ipv4 show excludedportrange protocol=tcp`) — aí
    /// tenta outro. Porta fixa (47800) falha direto, com o motivo.
    async fn bind_pair(addr: SocketAddr, local: &LocalCert) -> io::Result<(QuicListener, TcpListener)> {
        const ATTEMPTS: usize = 20;
        let mut last = None;
        for _ in 0..ATTEMPTS {
            let quic = QuicListener::bind(addr, local)?;
            let port = quic.local_addr()?.port();
            match TcpListener::bind(SocketAddr::new(addr.ip(), port), local).await {
                Ok(tcp) => return Ok((quic, tcp)),
                Err(e) if addr.port() == 0 && retryable(&e) => last = Some(e),
                Err(e) => return Err(io::Error::new(e.kind(), format!("TCP na porta {port}: {e} (reservada pelo Windows? veja `netsh int ipv4 show excludedportrange protocol=tcp`)"))),
            }
        }
        Err(last.unwrap_or_else(|| io::Error::other("sem porta livre pra QUIC e TCP juntos")))
    }

    pub async fn bind(addr: SocketAddr, local: &LocalCert) -> io::Result<Self> {
        let (quic, tcp) = Self::bind_pair(addr, local).await?;
        let port = quic.local_addr()?.port();
        let (tx, rx) = mpsc::channel(16);

        let qtx = tx.clone();
        tokio::spawn(async move {
            while let Some(res) = quic.accept().await {
                let item = res.map(|c| Arc::new(c) as SharedConn);
                if qtx.send(item).await.is_err() {
                    break;
                }
            }
        });
        tokio::spawn(async move {
            loop {
                let pending = match tcp.accept().await {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("accept TCP: {e}");
                        continue;
                    }
                };
                let tx = tx.clone();
                tokio::spawn(async move {
                    let item = pending.await.map(|c| Arc::new(c) as SharedConn);
                    let _ = tx.send(item).await;
                });
            }
        });
        Ok(Self { rx, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Próxima conexão (ou erro de handshake de alguém). `None` = parou.
    pub async fn accept(&mut self) -> Option<io::Result<SharedConn>> {
        self.rx.recv().await
    }
}

/// Conecta como viewer.
pub async fn connect(addr: SocketAddr, local: &LocalCert, check: ServerCheck, mode: TransportMode) -> io::Result<SharedConn> {
    match mode {
        TransportMode::Quic => Ok(Arc::new(crate::quic::connect(addr, local, check).await?)),
        TransportMode::Tcp => Ok(Arc::new(crate::tcp::connect(addr, local, check).await?)),
        TransportMode::Auto => match tokio::time::timeout(QUIC_TIMEOUT, crate::quic::connect(addr, local, check.clone())).await {
            Ok(Ok(c)) => Ok(Arc::new(c)),
            Ok(Err(e)) => {
                log::info!("QUIC falhou ({e}); tentando TCP");
                Ok(Arc::new(crate::tcp::connect(addr, local, check).await?))
            }
            Err(_) => {
                log::info!("QUIC sem resposta em {QUIC_TIMEOUT:?}; tentando TCP");
                Ok(Arc::new(crate::tcp::connect(addr, local, check).await?))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transport;
    use reflex_auth::identity::{AccessId, Identity};
    use reflex_proto::frame::{read_msg, write_msg};
    use reflex_proto::msg::Control;

    fn cert(name: &str) -> (LocalCert, AccessId) {
        let id = Identity::generate(name).unwrap();
        let access = id.access_id();
        (LocalCert { cert_der: id.cert_der, key_der: id.key_der }, access)
    }

    #[test]
    fn reserved_or_busy_tcp_port_is_retried() {
        assert!(retryable(&io::Error::from(io::ErrorKind::PermissionDenied)));
        assert!(retryable(&io::Error::from(io::ErrorKind::AddrInUse)));
        assert!(!retryable(&io::Error::from(io::ErrorKind::InvalidInput)));
    }

    fn accept_any() -> ServerCheck {
        Arc::new(|_| true)
    }

    async fn roundtrip(mode: TransportMode) -> Transport {
        let (host, host_id) = cert("host");
        let (viewer, viewer_id) = cert("viewer");
        let mut listener = Listener::bind("127.0.0.1:0".parse().unwrap(), &host).await.unwrap();
        let addr: SocketAddr = format!("127.0.0.1:{}", listener.port()).parse().unwrap();

        let server = tokio::spawn(async move {
            let conn = listener.accept().await.unwrap().unwrap();
            // O host vê o certificado do viewer.
            assert_eq!(AccessId::of_cert(&conn.peer_cert()), viewer_id);
            let (mut w, mut r) = conn.accept_bi().await.unwrap();
            let m: Control = read_msg(&mut r).await.unwrap();
            write_msg(&mut w, &m).await.unwrap();
            let mut uni = conn.open_uni().await.unwrap();
            write_msg(&mut uni, &Control::Ping { t_ns: 9 }).await.unwrap();
            conn.send_datagram(vec![7, 7]).unwrap();
            let binding = conn.channel_binding();
            tokio::time::sleep(Duration::from_millis(300)).await;
            binding
        });

        let expected = host_id.clone();
        let check: ServerCheck = Arc::new(move |der| AccessId::of_cert(der) == expected);
        let conn = connect(addr, &viewer, check, mode).await.unwrap();
        assert_eq!(AccessId::of_cert(&conn.peer_cert()), host_id);
        let (mut w, mut r) = conn.open_bi().await.unwrap();
        write_msg(&mut w, &Control::RequestKeyframe).await.unwrap();
        let back: Control = read_msg(&mut r).await.unwrap();
        assert_eq!(back, Control::RequestKeyframe);
        let mut uni = conn.accept_uni().await.unwrap();
        let ping: Control = read_msg(&mut uni).await.unwrap();
        assert_eq!(ping, Control::Ping { t_ns: 9 });
        assert_eq!(conn.read_datagram().await.unwrap(), vec![7, 7]);
        // Mesmo canal TLS → mesmo channel binding dos dois lados.
        assert_eq!(server.await.unwrap(), conn.channel_binding());
        conn.transport()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn quic_roundtrip_with_mutual_tls() {
        assert_eq!(roundtrip(TransportMode::Quic).await, Transport::Quic);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tcp_roundtrip_with_mutual_tls() {
        assert_eq!(roundtrip(TransportMode::Tcp).await, Transport::Tcp);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn auto_prefers_quic() {
        assert_eq!(roundtrip(TransportMode::Auto).await, Transport::Quic);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wrong_host_certificate_is_refused() {
        let (host, _) = cert("host");
        let (viewer, _) = cert("viewer");
        let mut listener = Listener::bind("127.0.0.1:0".parse().unwrap(), &host).await.unwrap();
        let addr: SocketAddr = format!("127.0.0.1:{}", listener.port()).parse().unwrap();
        tokio::spawn(async move { while listener.accept().await.is_some() {} });
        let reject: ServerCheck = Arc::new(|_| false);
        assert!(connect(addr, &viewer, reject.clone(), TransportMode::Quic).await.is_err());
        assert!(connect(addr, &viewer, reject, TransportMode::Tcp).await.is_err());
        let _ = accept_any();
    }
}
