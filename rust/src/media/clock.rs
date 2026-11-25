//! Relógio monotônico em nanossegundos, o mesmo no processo inteiro (o
//! `Instant` do Rust: QPC no Windows, `CLOCK_MONOTONIC` no Linux). Entre
//! máquinas, o viewer acha a diferença dos relógios pelo ping.

use std::sync::OnceLock;
use std::time::Instant;

/// Agora, em ns desde a 1ª leitura do processo.
pub fn now_ns() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    // +1 s: o zero fica reservado pra "nunca" (quem guarda um instante usa 0
    // como "ainda não aconteceu").
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1_000_000_000
}

#[cfg(test)]
mod tests {
    #[test]
    fn clock_is_monotonic() {
        let a = super::now_ns();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = super::now_ns();
        assert!(b > a + 1_000_000);
    }
}
