//! Link de acesso único (§11.4): `reflex://IP:PORTA/CODIGO#FINGERPRINT`.
//! Carrega tudo o que o viewer precisa pra entrar sem cadastro nem TOFU
//! manual: endereço, código e a impressão digital esperada do host.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteLink {
    pub address: String,
    pub code: String,
    pub fingerprint: String,
}

impl InviteLink {
    pub fn to_url(&self) -> String {
        format!("reflex://{}/{}#{}", self.address, self.code, self.fingerprint)
    }

    /// Lê um link; aceita o código com ou sem hífens.
    pub fn parse(url: &str) -> Option<Self> {
        let rest = url.strip_prefix("reflex://")?;
        let (before_hash, fingerprint) = rest.split_once('#')?;
        let (address, code) = before_hash.split_once('/')?;
        if address.is_empty() || code.is_empty() || fingerprint.is_empty() {
            return None;
        }
        Some(Self { address: address.to_string(), code: code.to_string(), fingerprint: fingerprint.to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let l = InviteLink { address: "192.168.0.15:47800".into(), code: "482-K7Q-91X".into(), fingerprint: "K7QF2M9X".into() };
        let url = l.to_url();
        assert_eq!(url, "reflex://192.168.0.15:47800/482-K7Q-91X#K7QF2M9X");
        assert_eq!(InviteLink::parse(&url), Some(l));
    }

    #[test]
    fn rejects_malformed() {
        assert!(InviteLink::parse("http://x").is_none());
        assert!(InviteLink::parse("reflex://addr/code").is_none()); // sem #
        assert!(InviteLink::parse("reflex:///code#fp").is_none()); // sem endereço
        assert!(InviteLink::parse("reflex://addr/#fp").is_none()); // sem código
    }
}
