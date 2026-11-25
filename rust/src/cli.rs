//! Comandos de linha de comando (todos os sistemas):
//!
//! | `--my-id`  | mostra o ID de acesso deste viewer (pra cadastrar no host) |
//! | `--add-user <ID> <nome> <master\|full\|view>` | cadastra um usuário nesta máquina |
//! | `--status` | estado do serviço, ID da máquina, endereços e usuários |
//!
//! No Windows os dois últimos pedem administrador (os dados são da máquina);
//! no Linux, não (são do usuário da sessão gráfica). Instalar fica em
//! `install` (Windows) e `install_linux`.

use crate::auth::identity::AccessId;
use crate::auth::store::{Actor, User};
use crate::hoststore;
use crate::identity_store::load_or_create_identity;
use crate::platform::paths;
use crate::platform::secret::Scope;
use crate::proto::perms::Level;

/// ID do viewer deste usuário (a mesma identidade que a interface usa).
pub fn my_id() -> Result<(), String> {
    let id = load_or_create_identity(&paths::user_dir().join("identity.bin"), "Reflex Viewer", Scope::User)?;
    println!("{}", id.access_id());
    Ok(())
}

/// Mexer nos dados do host: no Windows, só como administrador.
fn may_change_machine() -> Result<(), String> {
    #[cfg(windows)]
    crate::install::require_admin()?;
    Ok(())
}

pub fn add_user(args: &[String]) -> Result<(), String> {
    let [id, name, level] = args else { return Err("uso: reflex --add-user <ID> <nome> <master|full|view>".into()) };
    may_change_machine()?;
    let access_id = AccessId::parse(id).ok_or("ID de acesso inválido (formato XXXX-XXXX-XXXX-XXXX)")?;
    let level = parse_level(level).ok_or("nível inválido: use master, full ou view")?;
    let store = hoststore::open_store()?;
    store
        .update(|d| {
            d.add_user(&Actor::LocalAdmin, User::new(access_id.clone(), name, level))?;
            d.log(crate::auth::unix_now(), "admin-local", format!("adicionou {access_id} ({level:?}) pela linha de comando"));
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    println!("Cadastrado: {name} ({level:?}) {access_id}");
    Ok(())
}

pub fn status() -> Result<(), String> {
    may_change_machine()?;
    println!("Serviço: {}", crate::platform::service_ctrl::state().label());
    #[cfg(windows)]
    println!("Monitor virtual: {}", crate::install::virtual_monitor_label());
    let s = hoststore::status(crate::auth::unix_now())?;
    println!("Máquina: {}", s.machine_name);
    println!("ID desta máquina: {}", s.fingerprint);
    println!("Endereços: {}", if s.addresses.is_empty() { format!("{}:{}", s.machine_name, s.port) } else { s.addresses.join("  ") });
    let data = hoststore::open_store()?.load().map_err(|e| e.to_string())?;
    println!("Usuários ({}):", data.users.len());
    for u in &data.users {
        println!("  {:<20} {:<12} {}", u.name, format!("{:?}", u.level), u.access_id);
    }
    if let Some(inv) = s.invite {
        println!("Convite ativo: {}", inv.code);
    }
    println!("Logs: {}", crate::logger::log_dir(true).display());
    Ok(())
}

/// `master`, `full`/`controle`, `view`/`ver`.
pub fn parse_level(text: &str) -> Option<Level> {
    match text.to_ascii_lowercase().as_str() {
        "master" => Some(Level::Master),
        "full" | "controle" | "fullcontrol" => Some(Level::FullControl),
        "view" | "ver" | "visualizacao" => Some(Level::View),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_parsed_in_both_languages() {
        assert_eq!(parse_level("MASTER"), Some(Level::Master));
        assert_eq!(parse_level("controle"), Some(Level::FullControl));
        assert_eq!(parse_level("full"), Some(Level::FullControl));
        assert_eq!(parse_level("ver"), Some(Level::View));
        assert_eq!(parse_level("dono"), None);
    }

    #[test]
    fn add_user_rejects_wrong_arity() {
        assert!(add_user(&["só-um".into()]).is_err());
    }
}
