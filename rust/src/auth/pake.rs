//! Prova do código de convite por SPAKE2 (§11.4).
//!
//! O código nunca trafega: cada lado manda uma mensagem SPAKE2 e os dois
//! chegam à mesma chave só se digitaram o mesmo código. A confirmação mistura
//! a chave com o *channel binding* do TLS (exporter), então um intermediário
//! que tenha duas conexões TLS separadas não consegue repassar as provas.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::auth::invite::normalize_code;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Viewer,
    Host,
}

impl Role {
    fn tag(self) -> &'static [u8] {
        match self {
            Role::Viewer => b"viewer",
            Role::Host => b"host",
        }
    }

    fn other(self) -> Role {
        match self {
            Role::Viewer => Role::Host,
            Role::Host => Role::Viewer,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PakeError {
    #[error("mensagem SPAKE2 inválida")]
    BadMessage,
}

/// Rótulo do exporter TLS usado como channel binding.
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-reflex-invite";

/// 1º passo: guarda o estado e devolve a mensagem a mandar.
pub struct Pake {
    state: Spake2<Ed25519Group>,
    role: Role,
}

impl Pake {
    pub fn start(code: &str, role: Role) -> (Self, Vec<u8>) {
        let pw = Password::new(normalize_code(code).as_bytes());
        let (state, msg) = Spake2::<Ed25519Group>::start_symmetric(&pw, &Identity::new(b"reflex-invite"));
        (Self { state, role }, msg)
    }

    /// 2º passo: com a mensagem do outro lado e o channel binding.
    pub fn finish(self, peer_msg: &[u8], channel_binding: &[u8]) -> Result<Confirmed, PakeError> {
        let key = self.state.finish(peer_msg).map_err(|_| PakeError::BadMessage)?;
        Ok(Confirmed { key, role: self.role, binding: channel_binding.to_vec() })
    }
}

/// Chave combinada; gera e confere as provas.
pub struct Confirmed {
    key: Vec<u8>,
    role: Role,
    binding: Vec<u8>,
}

impl Confirmed {
    fn mac_for(&self, role: Role) -> Hmac<Sha256> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC aceita qualquer chave");
        mac.update(b"reflex-confirm");
        mac.update(role.tag());
        mac.update(&self.binding);
        mac
    }

    /// Prova a mandar pro outro lado.
    pub fn my_proof(&self) -> [u8; 32] {
        self.mac_for(self.role).finalize().into_bytes().into()
    }

    /// Confere a prova do outro lado (comparação em tempo constante).
    pub fn verify_peer(&self, proof: &[u8; 32]) -> bool {
        self.mac_for(self.role.other()).verify_slice(proof).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(viewer_code: &str, host_code: &str, vb: &[u8], hb: &[u8]) -> (bool, bool) {
        let (v, vmsg) = Pake::start(viewer_code, Role::Viewer);
        let (h, hmsg) = Pake::start(host_code, Role::Host);
        let v = v.finish(&hmsg, vb).unwrap();
        let h = h.finish(&vmsg, hb).unwrap();
        (h.verify_peer(&v.my_proof()), v.verify_peer(&h.my_proof()))
    }

    #[test]
    fn same_code_same_channel_confirms_both_ways() {
        assert_eq!(run("482-K7Q-91X", "482K7Q91X", b"tls", b"tls"), (true, true));
    }

    #[test]
    fn wrong_code_fails() {
        assert_eq!(run("482-K7Q-91X", "482-K7Q-91Y", b"tls", b"tls"), (false, false));
    }

    #[test]
    fn different_tls_channels_fail() {
        // Intermediário com duas conexões TLS: bindings diferentes.
        assert_eq!(run("AAA-BBB-CCC", "AAA-BBB-CCC", b"canal-1", b"canal-2"), (false, false));
    }

    #[test]
    fn proof_cannot_be_reflected() {
        let (v, vmsg) = Pake::start("ABC", Role::Viewer);
        let (h, hmsg) = Pake::start("ABC", Role::Host);
        let v = v.finish(&hmsg, b"x").unwrap();
        let _h = h.finish(&vmsg, b"x").unwrap();
        // Devolver a prova do próprio viewer pra ele não passa.
        assert!(!v.verify_peer(&v.my_proof()));
    }

    #[test]
    fn garbage_message_is_rejected() {
        let (v, _) = Pake::start("ABC", Role::Viewer);
        assert!(v.finish(b"lixo", b"x").is_err());
    }
}
