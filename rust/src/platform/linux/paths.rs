//! Pastas do Reflex no Linux.

use std::path::PathBuf;

/// `$XDG_DATA_HOME/reflex` (`~/.local/share/reflex`): dados do host
/// (usuários, configuração, identidade da máquina). No Linux o host é o da
/// sessão gráfica do usuário (o agente roda como ele, pelo systemd do
/// usuário), então os dados são dele.
pub fn machine_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(".local").join("share")))
        .unwrap_or_else(std::env::temp_dir)
        .join("reflex")
}

/// `$XDG_CONFIG_HOME/reflex` (`~/.config/reflex`): dados do viewer
/// (identidade, hosts salvos).
pub fn user_dir() -> PathBuf {
    config_home(std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from), std::env::var_os("HOME").map(PathBuf::from)).join("reflex")
}

/// A pasta de configuração do XDG: a variável, senão `~/.config` (sem casa,
/// a pasta temporária). Pura.
fn config_home(xdg: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    xdg.filter(|p| p.is_absolute()).or_else(|| home.map(|h| h.join(".config"))).unwrap_or_else(std::env::temp_dir)
}

/// O socket de configuração (interface ↔ serviço).
pub const CONFIG_PIPE: &str = "/run/reflex/config.sock";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_wins_over_home_when_absolute() {
        assert_eq!(config_home(Some("/x/cfg".into()), Some("/home/ana".into())), PathBuf::from("/x/cfg"));
        // O XDG manda ignorar caminho relativo.
        assert_eq!(config_home(Some("cfg".into()), Some("/home/ana".into())), PathBuf::from("/home/ana/.config"));
        assert!(user_dir().ends_with("reflex"));
    }
}
