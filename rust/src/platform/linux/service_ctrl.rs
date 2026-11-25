//! O serviço do Reflex no systemd do usuário (`reflex.service`, instalado por
//! `install_linux`).

pub use crate::platform::service_state::ServiceState;

/// Nome da unidade do systemd.
pub const SERVICE_NAME: &str = "reflex.service";

/// O estado pelo `systemctl --user is-active` (sem o systemctl, "não instalado").
pub fn state() -> ServiceState {
    match std::process::Command::new("systemctl").args(["--user", "is-active", SERVICE_NAME]).output() {
        Ok(out) => from_is_active(String::from_utf8_lossy(&out.stdout).trim()),
        Err(_) => ServiceState::NotInstalled,
    }
}

/// A resposta do `systemctl is-active` → estado. Pura.
fn from_is_active(answer: &str) -> ServiceState {
    match answer {
        "active" | "reloading" => ServiceState::Running,
        "inactive" | "failed" => ServiceState::Stopped,
        "activating" | "deactivating" => ServiceState::Other,
        // "unknown" (não existe a unidade) e o que vier de novo.
        _ => ServiceState::NotInstalled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemctl_answers_become_states() {
        assert_eq!(from_is_active("active"), ServiceState::Running);
        assert_eq!(from_is_active("failed"), ServiceState::Stopped);
        assert_eq!(from_is_active("activating"), ServiceState::Other);
        assert_eq!(from_is_active("unknown"), ServiceState::NotInstalled);
    }
}
