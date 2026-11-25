//! Log em arquivo, um por modo. O serviço e o agente (SYSTEM) gravam em
//! `%ProgramData%\Reflex\logs`; a interface e o processo do usuário em
//! `%LOCALAPPDATA%\Reflex\logs`. Pânicos também vão pro log.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Acima disto o arquivo é recomeçado (guarda o anterior como `.old`).
const MAX_BYTES: u64 = 4 * 1024 * 1024;

struct FileLogger {
    path: PathBuf,
    /// Arquivo (fica `None` só durante a rotação) e linhas escritas.
    file: Mutex<(Option<File>, u32)>,
}

/// A cada quantas linhas confere o tamanho do arquivo.
const CHECK_EVERY: u32 = 500;

/// Bibliotecas que falam demais no `info` (o zbus conta cada passo do
/// handshake do D-Bus): delas, só avisos e erros.
const QUIET: [&str; 1] = ["zbus"];

/// O nível que vai pro arquivo, por origem. Pura.
fn wanted(target: &str, level: log::Level) -> bool {
    let quiet = QUIET.iter().any(|q| target == *q || target.starts_with(&format!("{q}::")));
    level <= if quiet { log::Level::Warn } else { log::Level::Info }
}

impl log::Log for FileLogger {
    fn enabled(&self, meta: &log::Metadata) -> bool {
        wanted(meta.target(), meta.level())
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        if let Ok(mut guard) = self.file.lock() {
            let (slot, lines) = &mut *guard;
            let Some(file) = slot.as_mut() else { return };
            let _ = writeln!(file, "{} {:5} {}: {}", timestamp(), record.level(), record.target(), record.args());
            *lines += 1;
            // Um laço com erro não pode encher o disco: recomeça o arquivo.
            // Fecha antes (o Windows não renomeia arquivo aberto).
            if *lines % CHECK_EVERY == 0 && file.metadata().map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
                *slot = None;
                *slot = open(&self.path).ok();
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut guard) = self.file.lock()
            && let Some(f) = guard.0.as_mut()
        {
            let _ = f.flush();
        }
    }
}

/// `HH:MM:SS.mmm` em UTC (sem dependência de fuso).
fn timestamp() -> String {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let s = d.as_secs() % 86_400;
    format!("{:02}:{:02}:{:02}.{:03}Z", s / 3600, s / 60 % 60, s % 60, d.subsec_millis())
}

/// Pasta de logs do modo: da máquina (SYSTEM) ou do usuário.
pub fn log_dir(machine: bool) -> PathBuf {
    let base = if machine { crate::platform::paths::machine_dir() } else { crate::platform::paths::user_dir() };
    base.join("logs")
}

fn open(path: &Path) -> std::io::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(path, path.with_extension("old"));
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// Liga o log em `<pasta>\<modo>.log`. Falhar aqui não impede o programa.
pub fn init(mode: &str, machine: bool) {
    let path = log_dir(machine).join(format!("{mode}.log"));
    let Ok(file) = open(&path) else { return };
    if log::set_boxed_logger(Box::new(FileLogger { path, file: Mutex::new((Some(file), 0)) })).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    std::panic::set_hook(Box::new(|info| {
        log::error!("pânico: {info}");
    }));
    log::info!("--- reflex {} ({mode}, pid {}) ---", env!("CARGO_PKG_VERSION"), std::process::id());
}

#[cfg(test)]
mod tests {
    #[test]
    fn noisy_libraries_only_warn() {
        use log::Level;
        assert!(!super::wanted("zbus::connection::handshake", Level::Info));
        assert!(super::wanted("zbus", Level::Warn));
        assert!(super::wanted("reflex::agent", Level::Info));
        assert!(!super::wanted("reflex::agent", Level::Debug));
        // Outro nome que só começa igual não é o zbus.
        assert!(super::wanted("zbusdg", Level::Info));
    }

    #[test]
    fn timestamp_has_fixed_shape() {
        let t = super::timestamp();
        assert_eq!(t.len(), 13);
        assert!(t.ends_with('Z'));
    }
}
