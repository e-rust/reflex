//! Identidade de uma instalação (§11.1): par de chaves Ed25519 + certificado
//! autoassinado, gerados uma vez e guardados. O [`AccessId`] é a impressão
//! digital do certificado — o que se passa pra quem vai cadastrar, e o que o
//! viewer guarda pra reconhecer o host (TOFU).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("gerar certificado: {0}")]
    Generate(#[from] rcgen::Error),
}

/// Certificado e chave privada (PKCS#8), em DER.
#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Nunca imprimir a chave privada.
        f.debug_struct("Identity").field("access_id", &self.access_id()).finish()
    }
}

impl Identity {
    /// Gera uma identidade nova.
    pub fn generate(name: &str) -> Result<Self, IdentityError> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519)?;
        let mut params = rcgen::CertificateParams::new(vec!["reflex".to_string()])?;
        params.distinguished_name.push(rcgen::DnType::CommonName, name);
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2124, 1, 1);
        let cert = params.self_signed(&key)?;
        Ok(Self { cert_der: cert.der().to_vec(), key_der: key.serialize_der() })
    }

    pub fn access_id(&self) -> AccessId {
        AccessId::of_cert(&self.cert_der)
    }
}

/// Impressão digital de um certificado: 80 bits do SHA-256 em base32,
/// `XXXX-XXXX-XXXX-XXXX`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AccessId(String);

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const ID_CHARS: usize = 16;

impl AccessId {
    pub fn of_cert(cert_der: &[u8]) -> Self {
        let digest = Sha256::digest(cert_der);
        AccessId(base32(&digest[..10]))
    }

    /// Aceita com ou sem hífens/espaços, maiúsculas ou minúsculas.
    pub fn parse(text: &str) -> Option<Self> {
        let clean: String = text.chars().filter(|c| !matches!(c, '-' | ' ')).map(|c| c.to_ascii_uppercase()).collect();
        (clean.len() == ID_CHARS && clean.bytes().all(|b| B32.contains(&b))).then_some(AccessId(clean))
    }

    /// Sem hífens (forma canônica guardada).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AccessId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let groups: Vec<&str> = (0..self.0.len()).step_by(4).map(|i| &self.0[i..(i + 4).min(self.0.len())]).collect();
        f.write_str(&groups.join("-"))
    }
}

fn base32(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &b in bytes {
        buffer = (buffer << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(B32[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(B32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_identity_has_stable_access_id() {
        let id = Identity::generate("teste").unwrap();
        assert_eq!(id.access_id(), id.access_id());
        assert_eq!(id.access_id().as_str().len(), 16);
    }

    #[test]
    fn two_identities_differ() {
        let a = Identity::generate("a").unwrap();
        let b = Identity::generate("b").unwrap();
        assert_ne!(a.access_id(), b.access_id());
    }

    #[test]
    fn display_groups_and_parse_back() {
        let id = AccessId::of_cert(b"certificado");
        let shown = id.to_string();
        assert_eq!(shown.len(), 19);
        assert_eq!(shown.matches('-').count(), 3);
        assert_eq!(AccessId::parse(&shown), Some(id.clone()));
        assert_eq!(AccessId::parse(&shown.to_lowercase().replace('-', " ")), Some(id));
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(AccessId::parse("ABC").is_none());
        assert!(AccessId::parse("0000-0000-0000-0000").is_none()); // 0 não é base32
    }

    #[test]
    fn base32_known_vector() {
        // RFC 4648: "foobar" → MZXW6YTBOI
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
    }

    #[test]
    fn debug_hides_private_key() {
        let id = Identity::generate("x").unwrap();
        assert!(!format!("{id:?}").contains("key_der"));
    }
}
