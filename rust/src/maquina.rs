//! A aba "Esta máquina" (§5.2, §5.3) sem a tela: o que mostrar ([`Resumo`],
//! montado por funções puras) e as idas ao serviço pelo pipe de configuração
//! ([`ler`], [`alternar_compartilhar`]), que rodam fora da thread da tela.

use crate::platform::service_ctrl::{self, ServiceState};
use crate::proto::msg::{AdminRequest, AdminResponse, InviteInfo, MachineStatus, UserInfo};
use crate::proto::perms::Level;
use crate::configclient;

/// Tudo o que a aba mostra, já em texto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resumo {
    pub servico: String,
    pub id: String,
    pub enderecos: String,
    /// Os usuários no formato do `ListView` (`Nome|detalhe;…`).
    pub usuarios: String,
    pub convite: String,
    pub botao_compartilhar: &'static str,
    /// Dá pra ligar/desligar o compartilhamento (o serviço respondeu).
    pub pode_compartilhar: bool,
}

/// O que o serviço respondeu (ou não).
pub struct Leitura {
    pub estado: ServiceState,
    pub status: Result<MachineStatus, String>,
    pub usuarios: Vec<UserInfo>,
}

/// Lê o serviço: o estado, o status e os usuários. Bloqueia (pipe): chame fora da tela.
pub fn ler() -> Leitura {
    let estado = service_ctrl::state();
    if estado == ServiceState::NotInstalled {
        return Leitura { estado, status: Err(String::new()), usuarios: Vec::new() };
    }
    let status = configclient::status().map_err(|e| e.to_string());
    let usuarios = match configclient::admin(AdminRequest::ListUsers) {
        Ok(AdminResponse::Users(u)) => u,
        _ => Vec::new(),
    };
    Leitura { estado, status, usuarios }
}

/// Liga ou desliga o compartilhamento de acesso (convite de uso único, controle
/// total). `Err` com o motivo. Bloqueia (pipe).
pub fn alternar_compartilhar() -> Result<(), String> {
    let ligado = configclient::status().ok().and_then(|s| s.invite).is_some();
    let pedido = AdminRequest::SetSharing { enabled: !ligado, level: Level::FullControl };
    match configclient::admin(pedido) {
        Ok(AdminResponse::Err(e)) => Err(e),
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// O texto do nível de acesso.
pub fn nivel(level: Level) -> &'static str {
    match level {
        Level::Master => "Master",
        Level::FullControl => "Controle total",
        Level::View => "Visualização",
    }
}

/// O que a aba mostra para uma leitura. Pura.
pub fn resumo(leitura: &Leitura) -> Resumo {
    let servico = format!("Serviço: {}", leitura.estado.label());
    if leitura.estado == ServiceState::NotInstalled {
        return Resumo {
            servico,
            id: "ID desta máquina: — (ative o acesso para gerar)".into(),
            enderecos: "Endereços: —".into(),
            usuarios: String::new(),
            convite: String::new(),
            botao_compartilhar: "Compartilhar acesso",
            pode_compartilhar: false,
        };
    }
    match &leitura.status {
        Ok(s) => {
            let enderecos = if s.addresses.is_empty() { format!("{}:{}", s.machine_name, s.port) } else { s.addresses.join("   ") };
            let (convite, botao) = convite(s.invite.as_ref());
            Resumo {
                servico: if s.has_master { servico } else { format!("{servico} — sem master (cadastre um administrador)") },
                id: format!("ID desta máquina: {}", s.fingerprint),
                enderecos: format!("Endereços: {enderecos}"),
                usuarios: lista_usuarios(&leitura.usuarios),
                convite,
                botao_compartilhar: botao,
                pode_compartilhar: true,
            }
        }
        Err(e) => Resumo {
            servico,
            id: format!("Não foi possível ler o serviço: {e}"),
            enderecos: "Endereços: —".into(),
            usuarios: String::new(),
            convite: String::new(),
            botao_compartilhar: "Compartilhar acesso",
            pode_compartilhar: false,
        },
    }
}

/// O texto do convite e o do botão (ligado: "Parar de compartilhar").
fn convite(invite: Option<&InviteInfo>) -> (String, &'static str) {
    match invite {
        Some(inv) => (format!("Código: {}\nnível {} — troca a cada 10 min", inv.code, nivel(inv.level)), "Parar de compartilhar"),
        None => (String::new(), "Compartilhar acesso"),
    }
}

/// Os usuários no formato do `ListView`: `Nome|Nível · ID`, separados por `;`
/// (os separadores que aparecerem nos nomes viram vírgula).
fn lista_usuarios(usuarios: &[UserInfo]) -> String {
    let limpo = |s: &str| s.replace([';', '|'], ",");
    usuarios.iter().map(|u| format!("{}|{} · {}", limpo(&u.name), nivel(u.level), limpo(&u.access_id))).collect::<Vec<_>>().join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(invite: Option<InviteInfo>, has_master: bool) -> MachineStatus {
        MachineStatus {
            fingerprint: "ABCD-1234".into(),
            machine_name: "PC-SALA".into(),
            port: 47800,
            has_master,
            user_count: 1,
            invite,
            addresses: vec!["192.168.0.10:47800".into()],
        }
    }

    fn usuario(nome: &str, level: Level) -> UserInfo {
        UserInfo { access_id: "VUNS-P7N6".into(), name: nome.into(), level, perms: Default::default(), confirm: false, last_seen: 0 }
    }

    #[test]
    fn service_not_installed_has_nothing_to_share() {
        let r = resumo(&Leitura { estado: ServiceState::NotInstalled, status: Err(String::new()), usuarios: vec![] });
        assert!(r.id.contains("ative o acesso"));
        assert!(!r.pode_compartilhar);
        assert!(r.usuarios.is_empty());
    }

    #[test]
    fn running_service_lists_users_and_addresses() {
        let r = resumo(&Leitura { estado: ServiceState::Running, status: Ok(status(None, true)), usuarios: vec![usuario("Ana; Paula", Level::Master)] });
        assert_eq!(r.id, "ID desta máquina: ABCD-1234");
        assert_eq!(r.enderecos, "Endereços: 192.168.0.10:47800");
        assert_eq!(r.usuarios, "Ana, Paula|Master · VUNS-P7N6");
        assert_eq!(r.botao_compartilhar, "Compartilhar acesso");
        assert!(r.pode_compartilhar);
    }

    #[test]
    fn active_invite_shows_the_code_and_the_stop_button() {
        let inv = InviteInfo { code: "123-456".into(), level: Level::FullControl, next_rotation: 0 };
        let r = resumo(&Leitura { estado: ServiceState::Running, status: Ok(status(Some(inv), true)), usuarios: vec![] });
        assert!(r.convite.starts_with("Código: 123-456"));
        assert_eq!(r.botao_compartilhar, "Parar de compartilhar");
    }

    #[test]
    fn missing_master_is_pointed_out() {
        let r = resumo(&Leitura { estado: ServiceState::Running, status: Ok(status(None, false)), usuarios: vec![] });
        assert!(r.servico.contains("sem master"));
    }

    #[test]
    fn unreadable_service_shows_the_reason() {
        let r = resumo(&Leitura { estado: ServiceState::Running, status: Err("o serviço não está em execução".into()), usuarios: vec![] });
        assert!(r.id.contains("o serviço não está em execução"));
        assert!(!r.pode_compartilhar);
    }
}
