//! Relógio monotônico em nanossegundos (QPC), o mesmo no processo inteiro.

use std::sync::OnceLock;

use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

fn frequency() -> i64 {
    static F: OnceLock<i64> = OnceLock::new();
    *F.get_or_init(|| {
        let mut f = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut f);
        }
        f.max(1)
    })
}

/// Agora, em ns desde o boot.
pub fn now_ns() -> u64 {
    let mut c = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
    }
    let f = frequency() as i128;
    ((c as i128) * 1_000_000_000 / f) as u64
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
