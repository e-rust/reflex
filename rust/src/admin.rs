//! `reflex --admin <endereço> <comando…>` (§11.5): conecta com o ID deste
//! usuário — que precisa ser master na máquina —, executa um comando de
//! administração e sai, sem abrir vídeo.
//!
//! | comando | o quê |
//! |---|---|
//! | `users list` | usuários cadastrados |
//! | `users add <ID> <nome> <master\|full\|view>` | cadastra |
//! | `users remove <ID>` | remove |
//! | `sessions list` | sessões abertas |
//! | `config get` | configuração |

use std::sync::Arc;
use std::time::Duration;

use crate::net::{LocalCert, ServerCheck, TransportMode};
use crate::platform::secret::Scope;
use crate::platform::paths;
use crate::proto::msg::{AdminRequest, AdminResponse, UserInfo};
use crate::proto::perms::Perms;
use crate::viewer::client::{self, Credential};

use crate::cli::parse_level;
use crate::identity_store::load_or_create_identity;

const USAGE: &str = "uso: reflex --admin <endereço> users list | users add <ID> <nome> <master|full|view> | users remove <ID> | sessions list | config get";

pub fn run(args: &[String]) -> Result<(), String> {
    let [address, command @ ..] = args else { return Err(USAGE.into()) };
    let req = parse_command(command)?;
    let address = crate::addr::normalize_address(address).ok_or(USAGE)?;
    let socket = crate::addr::resolve(&address)?;

    let id = load_or_create_identity(&paths::user_dir().join("identity.bin"), "Reflex Viewer", Scope::User)?;
    let cert = LocalCert { cert_der: id.cert_der, key_der: id.key_der };

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
    let res = rt.block_on(async {
        // TOFU, como o probe: a impressão digital do host aparece no --status dele.
        let check: ServerCheck = Arc::new(|_| true);
        let mut connected = client::connect_and_auth(socket, &cert, check, TransportMode::Auto, env!("CARGO_PKG_VERSION"), Vec::new(), Credential::Key, || {})
            .await
            .map_err(|e| format!("{address}: {e}"))?;
        // Host de versão antiga ignora o pedido: não espera pra sempre.
        let res = match tokio::time::timeout(Duration::from_secs(10), client::admin(&mut connected, req)).await {
            Ok(r) => r.map_err(|e| format!("{address}: {e}")),
            Err(_) => Err(format!("{address} não respondeu ao pedido (versão do Reflex lá sem administração remota?)")),
        };
        connected.conn.close("fim da administração");
        tokio::time::sleep(Duration::from_millis(150)).await;
        res
    })?;
    print_response(res)
}

/// Palavras do comando → pedido. Pura.
pub fn parse_command(words: &[String]) -> Result<AdminRequest, String> {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["users", "list"] => Ok(AdminRequest::ListUsers),
        ["users", "add", id, name, level] => {
            let level = parse_level(level).ok_or("nível inválido: use master, full ou view")?;
            Ok(AdminRequest::AddUser(UserInfo { access_id: id.to_string(), name: name.to_string(), level, perms: level.default_perms(), confirm: false, last_seen: 0 }))
        }
        ["users", "remove", id] => Ok(AdminRequest::RemoveUser { access_id: id.to_string() }),
        ["sessions", "list"] => Ok(AdminRequest::ListSessions),
        ["config", "get"] => Ok(AdminRequest::GetConfig),
        _ => Err(format!("comando não reconhecido: {}\n{USAGE}", words.join(" "))),
    }
}

fn print_response(res: AdminResponse) -> Result<(), String> {
    match res {
        AdminResponse::Ok => println!("Feito."),
        AdminResponse::Users(users) => {
            println!("Usuários ({}):", users.len());
            for u in users {
                println!("  {:<20} {:<12} {}{}", u.name, format!("{:?}", u.level), u.access_id, if u.perms == Perms::NONE { "  (sem permissões)" } else { "" });
            }
        }
        AdminResponse::Sessions(sessions) => {
            println!("Sessões ({}):", sessions.len());
            for s in sessions {
                println!("  {:<6} {:<20} {:<12} {}", s.session_id, s.who, format!("{:?}", s.level), s.remote_addr);
            }
        }
        AdminResponse::Config(c) => println!("{c:#?}"),
        AdminResponse::Log(entries) => entries.iter().for_each(|e| println!("{e:?}")),
        AdminResponse::Invite(i) => println!("{i:?}"),
        AdminResponse::Err(e) => return Err(e),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::perms::Level;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_known_commands() {
        assert!(matches!(parse_command(&words("users list")), Ok(AdminRequest::ListUsers)));
        assert!(matches!(parse_command(&words("config get")), Ok(AdminRequest::GetConfig)));
        assert!(matches!(parse_command(&words("users remove AB")), Ok(AdminRequest::RemoveUser { .. })));
        assert!(parse_command(&words("foo")).is_err());
    }

    #[test]
    fn add_user_carries_level_and_its_default_permissions() {
        let Ok(AdminRequest::AddUser(u)) = parse_command(&words("users add VUNS-P7N6-SISF-FDFJ Eduardo master")) else { panic!() };
        assert_eq!((u.access_id.as_str(), u.name.as_str(), u.level), ("VUNS-P7N6-SISF-FDFJ", "Eduardo", Level::Master));
        assert_eq!(u.perms, Level::Master.default_perms());
        assert!(parse_command(&words("users add X Y dono")).is_err());
    }
}
