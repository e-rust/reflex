//! Uso de CPU da máquina inteira (pra saber se o motor está disputando CPU),
//! pela linha `cpu` do `/proc/stat`.

/// Mede o uso de CPU entre duas chamadas de [`CpuMeter::sample`].
#[derive(Debug, Default)]
pub struct CpuMeter {
    last: Option<(u64, u64)>,
}

/// Fração ocupada (0..=1) entre duas leituras (ocioso, total). Pura.
pub fn busy(before: (u64, u64), after: (u64, u64)) -> Option<f32> {
    let idle = after.0.checked_sub(before.0)?;
    let total = after.1.checked_sub(before.1)?;
    (total > 0).then(|| 1.0 - (idle.min(total) as f32 / total as f32))
}

/// (ocioso, total) da linha `cpu user nice system idle iowait ...`. Pura.
fn parse_stat(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().find(|l| l.starts_with("cpu "))?;
    let n: Vec<u64> = line.split_whitespace().skip(1).filter_map(|v| v.parse().ok()).collect();
    // idle + iowait é ocioso.
    let idle = n.get(3)? + n.get(4).copied().unwrap_or(0);
    Some((idle, n.iter().sum()))
}

impl CpuMeter {
    /// Uso desde a leitura anterior (a 1ª só marca o início).
    pub fn sample(&mut self) -> Option<f32> {
        let now = parse_stat(&std::fs::read_to_string("/proc/stat").ok()?)?;
        let out = self.last.and_then(|before| busy(before, now));
        self.last = Some(now);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_and_total_come_from_the_cpu_line() {
        let text = "cpu  10 0 5 80 5 0 0 0 0 0\ncpu0 1 0 1 1 1 0 0 0 0 0\n";
        assert_eq!(parse_stat(text), Some((85, 100)));
        assert_eq!(busy((85, 100), (125, 200)), Some(0.6));
    }
}
