//! Uso de CPU da máquina inteira (pra saber se o motor está disputando CPU).

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::GetSystemTimes;

/// Mede o uso de CPU entre duas chamadas de [`CpuMeter::sample`].
#[derive(Debug, Default)]
pub struct CpuMeter {
    last: Option<(u64, u64)>,
}

fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// Fração ocupada (0..=1) entre duas leituras (ocioso, total). Pura.
pub fn busy(before: (u64, u64), after: (u64, u64)) -> Option<f32> {
    let idle = after.0.checked_sub(before.0)?;
    let total = after.1.checked_sub(before.1)?;
    (total > 0).then(|| 1.0 - (idle.min(total) as f32 / total as f32))
}

impl CpuMeter {
    /// Uso desde a leitura anterior (a 1ª só marca o início).
    pub fn sample(&mut self) -> Option<f32> {
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        // SAFETY: ponteiros pra variáveis locais.
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
        // O tempo de kernel já inclui o ocioso.
        let now = (ticks(idle), ticks(kernel) + ticks(user));
        let out = self.last.and_then(|before| busy(before, now));
        self.last = Some(now);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_fraction() {
        assert_eq!(busy((0, 0), (25, 100)), Some(0.75));
        assert_eq!(busy((10, 100), (10, 100)), None);
        assert_eq!(busy((10, 100), (5, 200)), None);
    }

    #[test]
    fn meter_reads_the_system() {
        let mut m = CpuMeter::default();
        assert_eq!(m.sample(), None);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(m.sample().is_some_and(|f| (0.0..=1.0).contains(&f)));
    }
}
