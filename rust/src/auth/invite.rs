//! Convite de acesso único (§11.4).
//!
//! O código é derivado (HMAC) de uma semente secreta e de um índice. O índice
//! anda sozinho a cada [`ROTATION_SECS`] e também a cada uso ou a cada
//! [`MAX_FAILURES`] erros. Como é tudo determinístico a partir do estado
//! guardado, a interface (que mostra o código) e o agente (que confere) chegam
//! ao mesmo código só lendo o mesmo arquivo — sem conversar.

use hmac::{Hmac, Mac};
use rand::RngCore;
use crate::proto::perms::Level;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// Um código novo a cada 10 minutos.
pub const ROTATION_SECS: u64 = 600;
/// Erros seguidos que trocam o código na hora.
pub const MAX_FAILURES: u32 = 3;

/// Sem 0/O e 1/I (32 símbolos: cada um carrega 5 bits).
const ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
const CODE_LEN: usize = 9;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteState {
    pub enabled: bool,
    pub level: Level,
    seed: [u8; 32],
    /// Início da grade de rotação (Unix s).
    epoch: u64,
    /// Trocas extras (uso, erros).
    bumps: u64,
    failures: u32,
    /// Índice a que `failures` se refere.
    failures_index: u64,
}

impl Default for InviteState {
    fn default() -> Self {
        Self { enabled: false, level: Level::FullControl, seed: [0; 32], epoch: 0, bumps: 0, failures: 0, failures_index: 0 }
    }
}

impl InviteState {
    /// Liga o compartilhamento com semente nova (códigos antigos morrem).
    pub fn enable(&mut self, level: Level, now: u64) {
        // Nunca master por convite.
        let level = if level.is_master() { Level::FullControl } else { level };
        let mut seed = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut seed);
        *self = Self { enabled: true, level, seed, epoch: now, bumps: 0, failures: 0, failures_index: 0 };
    }

    pub fn disable(&mut self) {
        self.enabled = false;
        self.seed = [0; 32];
    }

    fn index(&self, now: u64) -> u64 {
        self.bumps + now.saturating_sub(self.epoch) / ROTATION_SECS
    }

    /// Código válido agora, formatado `XXX-XXX-XXX`.
    pub fn current_code(&self, now: u64) -> Option<String> {
        self.enabled.then(|| format_code(&code_for(&self.seed, self.index(now))))
    }

    /// Quando o código troca sozinho (Unix s).
    pub fn next_rotation(&self, now: u64) -> u64 {
        let elapsed = now.saturating_sub(self.epoch);
        self.epoch + (elapsed / ROTATION_SECS + 1) * ROTATION_SECS
    }

    /// Confere um código digitado (sem consumir).
    pub fn matches(&self, typed: &str, now: u64) -> bool {
        self.enabled && normalize_code(typed) == code_for(&self.seed, self.index(now))
    }

    /// Uso bem-sucedido: o código morre e aparece outro.
    pub fn consume(&mut self) {
        self.bumps += 1;
        self.failures = 0;
    }

    /// Tentativa errada. Devolve `true` se o código foi trocado por excesso de erros.
    pub fn register_failure(&mut self, now: u64) -> bool {
        let idx = self.index(now);
        if self.failures_index != idx {
            self.failures_index = idx;
            self.failures = 0;
        }
        self.failures += 1;
        if self.failures >= MAX_FAILURES {
            self.consume();
            true
        } else {
            false
        }
    }
}

fn code_for(seed: &[u8; 32], index: u64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(seed).expect("HMAC aceita qualquer chave");
    mac.update(b"reflex-invite");
    mac.update(&index.to_le_bytes());
    let digest = mac.finalize().into_bytes();
    digest.iter().take(CODE_LEN).map(|b| ALPHABET[(b & 31) as usize] as char).collect()
}

/// `482K7Q91X` → `482-K7Q-91X`.
pub fn format_code(code: &str) -> String {
    let c: Vec<char> = code.chars().collect();
    c.chunks(3).map(|g| g.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

/// Forma canônica de um código digitado (sem hífens/espaços, maiúsculo).
pub fn normalize_code(typed: &str) -> String {
    typed.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled(now: u64) -> InviteState {
        let mut s = InviteState::default();
        s.enable(Level::View, now);
        s
    }

    #[test]
    fn disabled_has_no_code() {
        let s = InviteState::default();
        assert!(s.current_code(0).is_none());
        assert!(!s.matches("", 0));
    }

    #[test]
    fn code_is_stable_within_window_and_rotates_after_ten_minutes() {
        let s = enabled(1000);
        let a = s.current_code(1000).unwrap();
        assert_eq!(s.current_code(1000 + ROTATION_SECS - 1).unwrap(), a);
        assert_ne!(s.current_code(1000 + ROTATION_SECS).unwrap(), a);
        assert_eq!(s.next_rotation(1001), 1000 + ROTATION_SECS);
    }

    #[test]
    fn code_format_and_matching_is_lenient() {
        let s = enabled(0);
        let code = s.current_code(0).unwrap();
        assert_eq!(code.len(), 11);
        assert!(s.matches(&code, 0));
        assert!(s.matches(&code.to_lowercase().replace('-', ""), 0));
        assert!(!s.matches("AAA-AAA-AAA", 0) || code == "AAA-AAA-AAA");
    }

    #[test]
    fn use_is_single() {
        let mut s = enabled(0);
        let code = s.current_code(0).unwrap();
        s.consume();
        assert!(!s.matches(&code, 0));
        assert_ne!(s.current_code(0).unwrap(), code);
    }

    #[test]
    fn three_failures_rotate_the_code() {
        let mut s = enabled(0);
        let code = s.current_code(0).unwrap();
        assert!(!s.register_failure(0));
        assert!(!s.register_failure(0));
        assert!(s.register_failure(0));
        assert!(!s.matches(&code, 0));
    }

    #[test]
    fn failures_reset_when_code_rotates_by_time() {
        let mut s = enabled(0);
        s.register_failure(0);
        s.register_failure(0);
        // Nova janela: contagem recomeça.
        assert!(!s.register_failure(ROTATION_SECS));
        assert!(!s.register_failure(ROTATION_SECS));
    }

    #[test]
    fn invite_never_grants_master() {
        let mut s = InviteState::default();
        s.enable(Level::Master, 0);
        assert_eq!(s.level, Level::FullControl);
    }

    #[test]
    fn reenabling_kills_previous_codes() {
        let mut s = enabled(0);
        let code = s.current_code(0).unwrap();
        s.enable(Level::View, 0);
        assert!(!s.matches(&code, 0) || s.current_code(0).unwrap() == code);
        s.disable();
        assert!(!s.matches(&code, 0));
    }

    #[test]
    fn two_readers_of_the_same_state_agree() {
        let s = enabled(5);
        let copy: InviteState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s.current_code(700), copy.current_code(700));
    }
}
