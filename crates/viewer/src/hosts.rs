//! Hosts salvos e confiança TOFU (§10.1, §5.2). O viewer guarda, por host, o
//! apelido, o endereço, o codec preferido e a **impressão digital** do
//! certificado aceito. Na conexão:
//! - host novo → pergunta ao usuário (mostra a impressão digital);
//! - impressão igual → conecta direto;
//! - impressão diferente → recusa e alerta (possível ataque).
//!
//! A impressão digital reusa o `AccessId` do `reflex-auth` (SHA-256 do
//! certificado).

use reflex_auth::identity::AccessId;
use reflex_proto::codec::CodecChoice;
use reflex_proto::msg::{Chroma, Refine, RemoteSize};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedHost {
    pub nickname: String,
    /// `ip:porta` ou `nome:porta`.
    pub address: String,
    /// Impressão digital do certificado aceito (TOFU).
    pub fingerprint: String,
    #[serde(default)]
    pub codec: CodecChoice,
    #[serde(default)]
    pub last_seen: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Hosts {
    pub hosts: Vec<SavedHost>,
    /// Último endereço que conectou (a aba Conectar abre com ele).
    #[serde(default)]
    pub last_address: Option<String>,
    /// Nitidez com a tela parada escolhida na aba Conectar.
    #[serde(default)]
    pub refine: Refine,
    /// Opções de imagem da aba Conectar.
    #[serde(default)]
    pub scale: ViewScale,
    #[serde(default)]
    pub chroma: Chroma,
    #[serde(default)]
    pub remote_size: RemoteSize,
    #[serde(default)]
    pub cursor: CursorMode,
    #[serde(default)]
    pub clipboard: ClipboardMode,
}

/// Em que sentido o clipboard é compartilhado (§10.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ClipboardMode {
    #[default]
    Both,
    /// O que se copia lá chega aqui; o daqui não vai.
    FromRemote,
    /// O que se copia aqui vai pra lá; o de lá não vem.
    ToRemote,
    Off,
}

impl ClipboardMode {
    pub const ALL: [ClipboardMode; 4] = [ClipboardMode::Both, ClipboardMode::FromRemote, ClipboardMode::ToRemote, ClipboardMode::Off];

    pub fn label(self) -> &'static str {
        match self {
            ClipboardMode::Both => "Nos dois sentidos",
            ClipboardMode::FromRemote => "Só da máquina remota pra cá",
            ClipboardMode::ToRemote => "Só daqui pra máquina remota",
            ClipboardMode::Off => "Desligado",
        }
    }

    /// Manda pra máquina remota o que se copia aqui.
    pub fn sends(self) -> bool {
        matches!(self, ClipboardMode::Both | ClipboardMode::ToRemote)
    }

    /// Aceita o que se copia lá.
    pub fn receives(self) -> bool {
        matches!(self, ClipboardMode::Both | ClipboardMode::FromRemote)
    }
}

/// Que cursor aparece sobre a imagem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CursorMode {
    /// O da máquina remota (forma e posição de lá); o daqui some sobre a imagem.
    #[default]
    Remote,
    /// Os dois.
    Both,
    /// Só o daqui (o remoto não é desenhado).
    Local,
}

impl CursorMode {
    pub const ALL: [CursorMode; 3] = [CursorMode::Remote, CursorMode::Both, CursorMode::Local];

    pub fn label(self) -> &'static str {
        match self {
            CursorMode::Remote => "Só o da máquina remota",
            CursorMode::Both => "Os dois",
            CursorMode::Local => "Só o deste PC",
        }
    }

    pub fn draws_remote(self) -> bool {
        self != CursorMode::Local
    }

    pub fn hides_local(self) -> bool {
        self == CursorMode::Remote
    }
}

/// Como a janela de vídeo mostra a tela remota.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ViewScale {
    /// Na escala do Windows deste monitor (150% amplia 1,5x): o tamanho que o
    /// console do Hyper-V mostra, mas fator não inteiro borra linhas finas.
    #[default]
    Windows,
    /// Um pixel remoto = um pixel aqui: nítido, menor.
    Native,
}

impl ViewScale {
    pub const ALL: [ViewScale; 2] = [ViewScale::Windows, ViewScale::Native];

    pub fn label(self) -> &'static str {
        match self {
            ViewScale::Windows => "Escala do Windows (maior)",
            ViewScale::Native => "Pixels reais (100%, nítido)",
        }
    }
}

/// O que fazer com o certificado que o host apresentou.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustDecision {
    /// Conhecido e confere: segue.
    Trusted,
    /// Primeira vez: o usuário confirma esta impressão digital.
    FirstTime { fingerprint: String },
    /// Mudou: recusar (alerta).
    Changed { known: String, got: String },
}

impl Hosts {
    pub fn get(&self, address: &str) -> Option<&SavedHost> {
        self.hosts.iter().find(|h| h.address.eq_ignore_ascii_case(address))
    }

    /// Decide a confiança pelo certificado (DER) que o host apresentou.
    pub fn decide(&self, address: &str, cert_der: &[u8]) -> TrustDecision {
        let got = AccessId::of_cert(cert_der).to_string();
        match self.get(address) {
            None => TrustDecision::FirstTime { fingerprint: got },
            Some(h) if h.fingerprint == got => TrustDecision::Trusted,
            Some(h) => TrustDecision::Changed { known: h.fingerprint.clone(), got },
        }
    }

    /// Salva/atualiza um host (o usuário aceitou a impressão digital).
    pub fn remember(&mut self, host: SavedHost) {
        match self.hosts.iter_mut().find(|h| h.address.eq_ignore_ascii_case(&host.address)) {
            Some(existing) => *existing = host,
            None => self.hosts.push(host),
        }
    }

    pub fn forget(&mut self, address: &str) {
        self.hosts.retain(|h| !h.address.eq_ignore_ascii_case(address));
    }

    pub fn touch(&mut self, address: &str, now: u64) {
        if let Some(h) = self.hosts.iter_mut().find(|h| h.address.eq_ignore_ascii_case(address)) {
            h.last_seen = now;
        }
    }

    /// Guarda `address` como o último que conectou.
    pub fn set_last(&mut self, address: &str) {
        self.last_address = Some(address.to_string());
    }

    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self).unwrap_or_default())
    }

    /// Verificador pro `reflex-net`: aceita o cert cuja impressão digital é `expected`.
    pub fn verifier(expected_fingerprint: String) -> reflex_net::ServerCheck {
        std::sync::Arc::new(move |der: &[u8]| AccessId::of_cert(der).to_string() == expected_fingerprint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(addr: &str, fp: &str) -> SavedHost {
        SavedHost { nickname: "n".into(), address: addr.into(), fingerprint: fp.into(), codec: CodecChoice::Auto, last_seen: 0 }
    }

    fn fp(cert: &[u8]) -> String {
        AccessId::of_cert(cert).to_string()
    }

    #[test]
    fn first_time_host_asks_for_confirmation() {
        let hosts = Hosts::default();
        assert_eq!(hosts.decide("1.2.3.4:47800", b"cert"), TrustDecision::FirstTime { fingerprint: fp(b"cert") });
    }

    #[test]
    fn known_matching_fingerprint_is_trusted() {
        let mut hosts = Hosts::default();
        hosts.remember(host("1.2.3.4:47800", &fp(b"cert")));
        assert_eq!(hosts.decide("1.2.3.4:47800", b"cert"), TrustDecision::Trusted);
        assert_eq!(hosts.decide("1.2.3.4:47800", b"CERT"), TrustDecision::Changed { known: fp(b"cert"), got: fp(b"CERT") });
    }

    #[test]
    fn address_match_is_case_insensitive() {
        let mut hosts = Hosts::default();
        hosts.remember(host("Host.Lan:47800", &fp(b"c")));
        assert_eq!(hosts.decide("host.lan:47800", b"c"), TrustDecision::Trusted);
    }

    #[test]
    fn remember_updates_in_place() {
        let mut hosts = Hosts::default();
        hosts.remember(host("a:1", &fp(b"x")));
        hosts.remember(host("a:1", &fp(b"y")));
        assert_eq!(hosts.hosts.len(), 1);
        assert_eq!(hosts.decide("a:1", b"y"), TrustDecision::Trusted);
    }

    #[test]
    fn old_file_without_last_address_still_loads() {
        let hosts: Hosts = serde_json::from_str(r#"{"hosts":[]}"#).unwrap();
        assert_eq!(hosts.last_address, None);
        assert_eq!(hosts.refine, Refine::Normal);
        assert_eq!((hosts.scale, hosts.chroma, hosts.remote_size), (ViewScale::Windows, Chroma::Yuv420, RemoteSize::Keep));
        assert_eq!(hosts.cursor, CursorMode::Remote);
        assert_eq!(hosts.clipboard, ClipboardMode::Both);
        assert!(ClipboardMode::FromRemote.receives() && !ClipboardMode::FromRemote.sends());
        assert!(ClipboardMode::ToRemote.sends() && !ClipboardMode::ToRemote.receives());
        assert!(!ClipboardMode::Off.sends() && !ClipboardMode::Off.receives());
    }

    #[test]
    fn forget_removes() {
        let mut hosts = Hosts::default();
        hosts.remember(host("a:1", &fp(b"x")));
        hosts.forget("A:1");
        assert!(hosts.hosts.is_empty());
    }

    #[test]
    fn verifier_matches_only_expected() {
        let v = Hosts::verifier(fp(b"cert"));
        assert!(v(b"cert"));
        assert!(!v(b"outro"));
    }

    #[test]
    fn roundtrip_to_disk() {
        let dir = std::env::temp_dir().join(format!("reflex-hosts-{}", std::process::id()));
        let path = dir.join("hosts.json");
        let mut hosts = Hosts::default();
        hosts.remember(host("a:1", &fp(b"x")));
        hosts.set_last("192.168.0.5:47800");
        hosts.save(&path).unwrap();
        let back = Hosts::load(&path);
        assert_eq!(back.hosts.len(), 1);
        assert_eq!(back.last_address.as_deref(), Some("192.168.0.5:47800"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
