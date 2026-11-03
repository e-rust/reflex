//! Protocolo do Reflex (docs/levantamento.md §10 e §11).
//!
//! Só tipos e regras puras: nada aqui sabe de rede, Windows ou codec.
//! - [`codec`]: codecs, capacidades e a escolha do codec da sessão;
//! - [`perms`]: níveis de acesso e permissões;
//! - [`msg`]: as mensagens de cada canal;
//! - [`frame`]: enquadramento (tamanho + postcard) sobre qualquer stream.

pub mod codec;
pub mod frame;
pub mod msg;
pub mod perms;

/// Versão do protocolo, conferida no `Hello` pelos dois lados: versões
/// diferentes não conversam (o postcard não tem campo opcional).
/// 2: `SessionConfig::refine`. 3: `chroma`, `remote_size`, `desired`.
/// 4: posição do cursor com sinal; datagrama sem prefixo de tamanho.
/// 5: clipboard com todos os formatos (`ClipboardOffer::items`).
/// 6: arquivos no clipboard (`ClipboardOffer::files`, pedidos por stream).
/// 7: `Control::HostReport`.
/// 8: `Input::Locks` (Caps/Num/Scroll Lock sincronizados, não como teclas).
pub const PROTOCOL_VERSION: u16 = 8;

/// Mensagem quando as versões não batem. Pura.
pub fn version_mismatch(theirs: u16, who: &str) -> Option<String> {
    (theirs != PROTOCOL_VERSION).then(|| {
        format!("versão do Reflex diferente: {who} usa o protocolo {theirs} e esta usa o {PROTOCOL_VERSION} — atualize as duas pontas")
    })
}

/// Porta padrão (UDP pro QUIC, TCP pra reserva).
pub const DEFAULT_PORT: u16 = 47800;

/// ALPN do QUIC/TLS.
pub const ALPN: &[u8] = b"reflex/1";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msg::Refine;

    #[test]
    fn cursor_datagram_roundtrips_including_negative_positions() {
        use crate::msg::Datagram;
        for d in [Datagram::cursor(-7, -3, true, 9), Datagram::cursor(3839, 2159, false, 1), Datagram::cursor(100_000, -100_000, true, 2)] {
            assert_eq!(Datagram::from_bytes(&d.to_bytes()), Some(d));
        }
        assert_eq!(Datagram::cursor(100_000, -100_000, true, 2), Datagram::CursorPos { x: i16::MAX, y: i16::MIN, visible: true, shape_id: 2 });
        // O formato de stream (com prefixo de tamanho) NÃO é um datagrama.
        let framed = crate::frame::encode(&Datagram::cursor(5, 5, true, 1)).unwrap();
        assert_ne!(Datagram::from_bytes(&framed), Some(Datagram::cursor(5, 5, true, 1)));
    }

    #[test]
    fn only_a_different_version_is_refused() {
        assert_eq!(version_mismatch(PROTOCOL_VERSION, "a máquina"), None);
        let msg = version_mismatch(1, "a máquina").unwrap();
        assert!(msg.contains("protocolo 1") && msg.contains(&format!("o {PROTOCOL_VERSION}")), "{msg}");
    }

    #[test]
    fn higher_refine_levels_do_more_and_sooner() {
        let plans: Vec<_> = Refine::ALL.iter().map(|r| r.plan()).collect();
        assert_eq!((plans[0].frames, plans[0].vbv_frames), (0, 1), "Desligado = como era");
        for w in plans.windows(2) {
            assert!(w[1].frames > w[0].frames && w[1].vbv_frames > w[0].vbv_frames);
        }
        assert!(plans[1].idle_ms > plans[2].idle_ms && plans[2].idle_ms > plans[3].idle_ms);
        assert_eq!(Refine::default(), Refine::Normal);
    }

    #[test]
    fn refine_levels_parse_from_the_command_line() {
        assert_eq!(Refine::parse("off"), Some(Refine::Off));
        assert_eq!(Refine::parse("LEVE"), Some(Refine::Light));
        assert_eq!(Refine::parse("padrão"), Some(Refine::Normal));
        assert_eq!(Refine::parse("max"), Some(Refine::Max));
        assert_eq!(Refine::parse("turbo"), None);
    }
}
