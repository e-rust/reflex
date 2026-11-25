//! O estado do serviço do Reflex nesta máquina, igual em todos os sistemas
//! (quem consulta é o `service_ctrl` de cada um).

/// Estado do serviço desta máquina.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// Não existe (acesso a esta máquina nunca foi ativado).
    NotInstalled,
    Running,
    Stopped,
    /// Instalado, mas num estado de transição (iniciando/parando).
    Other,
}

impl ServiceState {
    pub fn label(self) -> &'static str {
        match self {
            ServiceState::NotInstalled => "acesso a esta máquina desativado",
            ServiceState::Running => "em execução",
            ServiceState::Stopped => "instalado, parado",
            ServiceState::Other => "mudando de estado…",
        }
    }
}
