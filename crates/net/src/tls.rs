//! TLS 1.3 mútuo com certificados autoassinados.
//!
//! Os verificadores aceitam qualquer certificado **desde que** a assinatura do
//! handshake confira (o outro lado tem a chave privada daquele certificado).
//! Quem decide se aquele certificado é bem-vindo é a camada de cima:
//! - no host, a lista de usuários (ou o convite);
//! - no viewer, o [`ServerCheck`] (impressão digital conhecida/TOFU).

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};

/// Certificado e chave desta instalação.
#[derive(Clone)]
pub struct LocalCert {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

impl LocalCert {
    fn chain(&self) -> Vec<CertificateDer<'static>> {
        vec![CertificateDer::from(self.cert_der.clone())]
    }

    fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_der.clone()))
    }
}

/// Decide, pelo certificado (DER), se o servidor é quem o viewer espera.
/// Recusar aqui derruba o handshake antes de qualquer dado.
pub type ServerCheck = Arc<dyn Fn(&[u8]) -> bool + Send + Sync>;

pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn algorithms() -> WebPkiSupportedAlgorithms {
    rustls::crypto::ring::default_provider().signature_verification_algorithms
}

#[derive(Debug)]
struct AnyClientCert {
    algs: WebPkiSupportedAlgorithms,
}

impl ClientCertVerifier for AnyClientCert {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(&self, _end: &CertificateDer<'_>, _inter: &[CertificateDer<'_>], _now: UnixTime) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(&self, msg: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(msg, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(&self, msg: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(msg, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }
}

struct CheckedServerCert {
    algs: WebPkiSupportedAlgorithms,
    check: ServerCheck,
}

impl std::fmt::Debug for CheckedServerCert {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CheckedServerCert")
    }
}

impl ServerCertVerifier for CheckedServerCert {
    fn verify_server_cert(
        &self,
        end: &CertificateDer<'_>,
        _inter: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if (self.check)(end.as_ref()) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("certificado do host não é o esperado".into()))
        }
    }

    fn verify_tls12_signature(&self, msg: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(msg, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(&self, msg: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(msg, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// TLS do host: exige certificado do viewer.
pub fn server_config(local: &LocalCert) -> Result<rustls::ServerConfig, rustls::Error> {
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(AnyClientCert { algs: algorithms() }))
        .with_single_cert(local.chain(), local.key())?;
    cfg.alpn_protocols = vec![reflex_proto::ALPN.to_vec()];
    Ok(cfg)
}

/// TLS do viewer: apresenta o próprio certificado e confere o do host.
pub fn client_config(local: &LocalCert, check: ServerCheck) -> Result<rustls::ClientConfig, rustls::Error> {
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(CheckedServerCert { algs: algorithms(), check }))
        .with_client_auth_cert(local.chain(), local.key())?;
    cfg.alpn_protocols = vec![reflex_proto::ALPN.to_vec()];
    Ok(cfg)
}

/// Nome qualquer pro SNI (o certificado não é conferido por nome).
pub fn server_name() -> ServerName<'static> {
    ServerName::try_from("reflex").expect("nome fixo válido")
}
