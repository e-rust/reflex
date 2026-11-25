//! Instalar o lado controlado no Linux (docs/linux.md): um serviço do usuário
//! no systemd que roda `reflex --agent` junto com a sessão gráfica (o agente
//! precisa do D-Bus dela pra falar com o Mutter). Não precisa de root.
//!
//! | `--install`   | cria e liga o serviço do usuário; mostra o ID da máquina |
//! | `--uninstall` | desliga e apaga o serviço (os dados ficam) |

use std::path::{Path, PathBuf};

use crate::hoststore;
use crate::platform::paths;
use crate::platform::service_ctrl::SERVICE_NAME;

/// A unidade do systemd do usuário. Pura.
pub fn unit_text(exe: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=Reflex (acesso remoto a esta sessão)\n\
         PartOf=graphical-session.target\n\
         After=graphical-session.target\n\
         \n\
         [Service]\n\
         ExecStart=\"{}\" --agent\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n",
        exe.display()
    )
}

/// `~/.config/systemd/user/reflex.service`.
fn unit_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("sem HOME")?;
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(home).join(".config"));
    Ok(config.join("systemd").join("user").join(SERVICE_NAME))
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("systemctl").arg("--user").args(args).output().map_err(|e| format!("systemctl: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("systemctl --user {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

pub fn install() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Identidade e store já agora, pra o --status funcionar antes da 1ª conexão.
    hoststore::open_store()?;
    let fp = hoststore::machine_fingerprint()?;
    let unit = unit_path()?;
    if let Some(dir) = unit.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&unit, unit_text(&exe)).map_err(|e| format!("{}: {e}", unit.display()))?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", SERVICE_NAME])?;
    println!("Serviço do usuário instalado e ligado ({}).", unit.display());
    println!("ID desta máquina: {fp}");
    println!("Cadastre quem vai acessar com: reflex --add-user <ID> <nome> <master|full|view>");
    println!("Com o firewall (ufw) ligado, libere a porta: sudo ufw allow {}/udp && sudo ufw allow {}/tcp", crate::proto::DEFAULT_PORT, crate::proto::DEFAULT_PORT);
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    let unit = unit_path()?;
    let _ = systemctl(&["disable", "--now", SERVICE_NAME]);
    if unit.exists() {
        std::fs::remove_file(&unit).map_err(|e| format!("{}: {e}", unit.display()))?;
    }
    let _ = systemctl(&["daemon-reload"]);
    println!("Serviço removido. Os dados ficam em {}", paths::machine_dir().display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_runs_the_agent_with_the_graphical_session() {
        let t = unit_text(Path::new("/opt/reflex/reflex"));
        assert!(t.contains("ExecStart=\"/opt/reflex/reflex\" --agent\n"), "{t}");
        assert!(t.contains("WantedBy=graphical-session.target"));
        assert!(t.contains("PartOf=graphical-session.target"));
    }
}
