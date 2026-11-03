//! Timer do Windows em 1 ms enquanto o motor roda. No padrão (15,6 ms) uma
//! espera de 9 ms vira 15,6: o pacer perde a vez e, a 30 quadros/s, saíam só
//! ~24 (medido na VM com a tela mudando sem parar).

use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};

const PERIOD_MS: u32 = 1;

/// Enquanto existir, os timers e esperas do processo têm resolução de 1 ms.
pub struct HighResTimer {
    active: bool,
}

impl HighResTimer {
    pub fn new() -> Self {
        // TIMERR_NOERROR = 0.
        let active = unsafe { timeBeginPeriod(PERIOD_MS) } == 0;
        Self { active }
    }
}

impl Default for HighResTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for HighResTimer {
    fn drop(&mut self) {
        if self.active {
            unsafe {
                timeEndPeriod(PERIOD_MS);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_sleeps_are_short_while_active() {
        let _t = HighResTimer::new();
        let t0 = std::time::Instant::now();
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        // Com 15,6 ms de resolução seriam ~156 ms.
        assert!(t0.elapsed() < std::time::Duration::from_millis(80), "{:?}", t0.elapsed());
    }
}
