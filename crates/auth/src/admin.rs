//! API de administração (§11.5): a mesma pra interface local (pipe do
//! serviço) e pro master remoto (canal de controle). Quem chama diz quem é
//! ([`Actor`]); as regras ficam no [`StoreData`].

use reflex_proto::msg::{AdminRequest, AdminResponse, InviteInfo, SessionInfo};

use crate::identity::AccessId;
use crate::store::{Actor, SignedFile, Storage, StoreError, User};

/// Sessões abertas (quem implementa é o host).
pub trait Sessions {
    fn list(&self) -> Vec<SessionInfo>;
    fn disconnect(&self, session_id: u64) -> bool;
}

/// Sem sessões (interface local fora do agente).
pub struct NoSessions;

impl Sessions for NoSessions {
    fn list(&self) -> Vec<SessionInfo> {
        Vec::new()
    }
    fn disconnect(&self, _: u64) -> bool {
        false
    }
}

fn who(actor: &Actor) -> String {
    match actor {
        Actor::LocalUser => "local".into(),
        Actor::LocalAdmin => "admin-local".into(),
        Actor::RemoteMaster(id) => id.to_string(),
    }
}

fn is_admin(actor: &Actor) -> bool {
    !matches!(actor, Actor::LocalUser)
}

fn err(e: StoreError) -> AdminResponse {
    AdminResponse::Err(e.to_string())
}

/// Executa um pedido. Leituras são livres; alterações exigem admin local ou
/// master remoto — menos o compartilhamento, que o usuário logado pode ligar.
pub fn handle<S: Storage>(file: &SignedFile<S>, actor: &Actor, sessions: &dyn Sessions, req: AdminRequest, now: u64) -> AdminResponse {
    let by = who(actor);
    let result = match req {
        AdminRequest::ListUsers => file.load().map(|d| AdminResponse::Users(d.users.iter().map(User::to_info).collect())),
        AdminRequest::GetConfig => file.load().map(|d| AdminResponse::Config(d.config)),
        AdminRequest::GetLog { since } => file.load().map(|d| AdminResponse::Log(d.log.into_iter().filter(|e| e.time >= since).collect())),
        AdminRequest::GetInvite => file.load().map(|d| {
            AdminResponse::Invite(d.invite.current_code(now).map(|code| InviteInfo { code, level: d.invite.level, next_rotation: d.invite.next_rotation(now) }))
        }),
        AdminRequest::ListSessions => Ok(AdminResponse::Sessions(sessions.list())),

        AdminRequest::AddUser(info) => User::from_info(&info).and_then(|u| {
            file.update(|d| {
                let text = format!("adicionou {} ({:?})", u.access_id, u.level);
                d.add_user(actor, u)?;
                d.log(now, &by, text);
                Ok(AdminResponse::Ok)
            })
        }),
        AdminRequest::UpdateUser(info) => User::from_info(&info).and_then(|u| {
            file.update(|d| {
                let text = format!("alterou {} ({:?})", u.access_id, u.level);
                d.update_user(actor, u)?;
                d.log(now, &by, text);
                Ok(AdminResponse::Ok)
            })
        }),
        AdminRequest::RemoveUser { access_id } => match AccessId::parse(&access_id) {
            None => Err(StoreError::BadAccessId),
            Some(id) => file.update(|d| {
                d.remove_user(actor, &id)?;
                d.log(now, &by, format!("removeu {id}"));
                Ok(AdminResponse::Ok)
            }),
        },
        AdminRequest::SetConfig(cfg) => file.update(|d| {
            d.set_config(actor, cfg)?;
            d.log(now, &by, "alterou a configuração");
            Ok(AdminResponse::Ok)
        }),
        AdminRequest::SetSharing { enabled, level } => file.update(|d| {
            if enabled {
                if !d.config.allow_invites {
                    return Err(StoreError::NotAllowed("convites estão desligados nesta máquina"));
                }
                d.invite.enable(level, now);
                d.log(now, &by, format!("ligou o compartilhar acesso ({level:?})"));
            } else {
                d.invite.disable();
                d.log(now, &by, "desligou o compartilhar acesso");
            }
            Ok(AdminResponse::Ok)
        }),
        AdminRequest::Disconnect { session_id } => {
            if !is_admin(actor) {
                Err(StoreError::NotAllowed("só administrador derruba sessões"))
            } else if sessions.disconnect(session_id) {
                let _ = file.update(|d| {
                    d.log(now, &by, format!("derrubou a sessão {session_id}"));
                    Ok(())
                });
                Ok(AdminResponse::Ok)
            } else {
                Err(StoreError::NotFound)
            }
        }
    };
    result.unwrap_or_else(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemStorage;
    use reflex_proto::msg::{HostConfig, UserInfo};
    use reflex_proto::perms::{Level, Perms};

    fn info(n: u8, level: Level) -> UserInfo {
        UserInfo { access_id: AccessId::of_cert(&[n]).to_string(), name: format!("u{n}"), level, perms: level.default_perms(), confirm: false, last_seen: 0 }
    }

    fn file() -> SignedFile<MemStorage> {
        SignedFile::new(MemStorage::default(), b"k".to_vec())
    }

    #[test]
    fn local_user_reads_but_cannot_write() {
        let f = file();
        assert!(matches!(handle(&f, &Actor::LocalUser, &NoSessions, AdminRequest::ListUsers, 0), AdminResponse::Users(_)));
        assert!(matches!(handle(&f, &Actor::LocalUser, &NoSessions, AdminRequest::AddUser(info(1, Level::View)), 0), AdminResponse::Err(_)));
        assert!(matches!(handle(&f, &Actor::LocalUser, &NoSessions, AdminRequest::SetConfig(HostConfig::default()), 0), AdminResponse::Err(_)));
    }

    #[test]
    fn master_flow_through_the_api() {
        let f = file();
        assert_eq!(handle(&f, &Actor::LocalAdmin, &NoSessions, AdminRequest::AddUser(info(1, Level::Master)), 0), AdminResponse::Ok);
        let m = Actor::RemoteMaster(AccessId::of_cert(&[1]));
        assert_eq!(handle(&f, &m, &NoSessions, AdminRequest::AddUser(info(2, Level::Master)), 1), AdminResponse::Ok);
        // Admin local não remove master...
        let r = handle(&f, &Actor::LocalAdmin, &NoSessions, AdminRequest::RemoveUser { access_id: info(2, Level::Master).access_id }, 2);
        assert!(matches!(r, AdminResponse::Err(_)));
        // ...o master remoto remove.
        let r = handle(&f, &m, &NoSessions, AdminRequest::RemoveUser { access_id: info(2, Level::Master).access_id }, 3);
        assert_eq!(r, AdminResponse::Ok);
        let AdminResponse::Users(users) = handle(&f, &m, &NoSessions, AdminRequest::ListUsers, 4) else { panic!() };
        assert_eq!(users.len(), 1);
        let AdminResponse::Log(log) = handle(&f, &m, &NoSessions, AdminRequest::GetLog { since: 0 }, 5) else { panic!() };
        assert_eq!(log.len(), 3);
    }

    #[test]
    fn local_user_can_share_access_and_read_the_code() {
        let f = file();
        assert_eq!(handle(&f, &Actor::LocalUser, &NoSessions, AdminRequest::SetSharing { enabled: true, level: Level::View }, 0), AdminResponse::Ok);
        let AdminResponse::Invite(Some(inv)) = handle(&f, &Actor::LocalUser, &NoSessions, AdminRequest::GetInvite, 10) else { panic!() };
        assert_eq!(inv.level, Level::View);
        assert_eq!(inv.code.len(), 11);
        handle(&f, &Actor::LocalUser, &NoSessions, AdminRequest::SetSharing { enabled: false, level: Level::View }, 20);
        assert_eq!(handle(&f, &Actor::LocalUser, &NoSessions, AdminRequest::GetInvite, 30), AdminResponse::Invite(None));
    }

    #[test]
    fn sharing_refused_when_invites_disabled() {
        let f = file();
        let cfg = HostConfig { allow_invites: false, ..HostConfig::default() };
        handle(&f, &Actor::LocalAdmin, &NoSessions, AdminRequest::SetConfig(cfg), 0);
        let r = handle(&f, &Actor::LocalUser, &NoSessions, AdminRequest::SetSharing { enabled: true, level: Level::View }, 0);
        assert!(matches!(r, AdminResponse::Err(_)));
    }

    #[test]
    fn bad_access_id_is_reported() {
        let f = file();
        let mut bad = info(1, Level::View);
        bad.access_id = "nada".into();
        assert!(matches!(handle(&f, &Actor::LocalAdmin, &NoSessions, AdminRequest::AddUser(bad), 0), AdminResponse::Err(_)));
    }

    struct OneSession;
    impl Sessions for OneSession {
        fn list(&self) -> Vec<SessionInfo> {
            vec![SessionInfo { session_id: 7, who: "x".into(), access_id: "x".into(), level: Level::View, remote_addr: "1.2.3.4".into(), started: 0, via_invite: true }]
        }
        fn disconnect(&self, id: u64) -> bool {
            id == 7
        }
    }

    #[test]
    fn sessions_are_listed_and_disconnected_by_admins_only() {
        let f = file();
        let AdminResponse::Sessions(s) = handle(&f, &Actor::LocalUser, &OneSession, AdminRequest::ListSessions, 0) else { panic!() };
        assert_eq!(s.len(), 1);
        assert!(matches!(handle(&f, &Actor::LocalUser, &OneSession, AdminRequest::Disconnect { session_id: 7 }, 0), AdminResponse::Err(_)));
        assert_eq!(handle(&f, &Actor::LocalAdmin, &OneSession, AdminRequest::Disconnect { session_id: 7 }, 0), AdminResponse::Ok);
        assert!(matches!(handle(&f, &Actor::LocalAdmin, &OneSession, AdminRequest::Disconnect { session_id: 8 }, 0), AdminResponse::Err(_)));
    }

    #[test]
    fn perms_are_sanitized_on_list() {
        let f = file();
        let mut u = info(3, Level::View);
        u.perms = Perms::ALL;
        handle(&f, &Actor::LocalAdmin, &NoSessions, AdminRequest::AddUser(u), 0);
        let AdminResponse::Users(users) = handle(&f, &Actor::LocalAdmin, &NoSessions, AdminRequest::ListUsers, 0) else { panic!() };
        assert!(!users[0].perms.contains(Perms::ADMIN));
    }
}
