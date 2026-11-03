//! Pastas do Reflex.

use std::path::PathBuf;

/// `%ProgramData%\Reflex`: dados do host (usuários, configuração, identidade
/// da máquina). ACL só pra SYSTEM e administradores (o serviço cria).
pub fn machine_dir() -> PathBuf {
    std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData")).join("Reflex")
}

/// `%LOCALAPPDATA%\Reflex`: dados do viewer (identidade, hosts salvos).
pub fn user_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Reflex")
}

/// Nome do pipe de configuração (interface ↔ serviço).
pub const CONFIG_PIPE: &str = r"\\.\pipe\reflex-config";

/// Nome do pipe agente ↔ processo do usuário, por sessão.
pub fn user_pipe(session: u32) -> String {
    format!(r"\\.\pipe\reflex-user-{session}")
}

/// Pipe em que o processo do usuário serve os arquivos copiados lá (o agente
/// liga cada pedido de um viewer a uma conexão nele).
pub fn user_files_pipe(session: u32) -> String {
    format!(r"\\.\pipe\reflex-userfiles-{session}")
}

/// Pipe do agente pelo qual o processo do usuário busca arquivos copiados
/// num viewer (cada conexão vira um stream até aquele viewer).
pub fn agent_files_pipe(session: u32) -> String {
    format!(r"\\.\pipe\reflex-agentfiles-{session}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn dirs_end_with_reflex() {
        assert!(super::machine_dir().ends_with("Reflex"));
        assert!(super::user_dir().ends_with("Reflex"));
        assert_eq!(super::user_pipe(2), r"\\.\pipe\reflex-user-2");
    }
}
