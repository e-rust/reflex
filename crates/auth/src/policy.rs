//! Quem pode entrar e com quais permissões (§11.2–§11.4).

use reflex_proto::perms::{Level, Perms};

use crate::identity::AccessId;
use crate::store::StoreData;

/// Acesso concedido.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub name: String,
    pub level: Level,
    pub perms: Perms,
    /// Alguém na máquina precisa clicar Permitir antes.
    pub needs_confirmation: bool,
    pub via_invite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Denied {
    #[error("este viewer não está cadastrado nesta máquina")]
    Unknown,
    #[error("o acesso a esta máquina está pausado")]
    Paused,
    #[error("convites estão desligados nesta máquina")]
    InvitesDisabled,
}

/// Decide o acesso de `peer`. `invite_proven` = provou o código de convite (SPAKE2).
///
/// - master: entra sempre, com tudo e sem confirmação (ignora a pausa);
/// - usuário cadastrado: as permissões dele, menos o que a máquina desligou;
/// - convidado: o nível do convite, com confirmação se a máquina exigir.
pub fn decide(store: &StoreData, peer: &AccessId, invite_proven: bool, now: u64) -> Result<Grant, Denied> {
    let cfg = &store.config;
    let paused = cfg.paused_until > now;
    let mask = |p: Perms| {
        let mut p = p;
        if !cfg.allow_clipboard {
            p = p.without(Perms::CLIPBOARD);
        }
        if !cfg.allow_files {
            p = p.without(Perms::FILES);
        }
        p
    };

    if let Some(user) = store.find(peer) {
        if user.level.is_master() {
            return Ok(Grant { name: user.name.clone(), level: Level::Master, perms: Perms::ALL, needs_confirmation: false, via_invite: false });
        }
        if paused {
            return Err(Denied::Paused);
        }
        return Ok(Grant {
            name: user.name.clone(),
            level: user.level,
            perms: mask(user.effective_perms()),
            needs_confirmation: user.confirm,
            via_invite: false,
        });
    }

    if !invite_proven {
        return Err(Denied::Unknown);
    }
    if !cfg.allow_invites || !store.invite.enabled {
        return Err(Denied::InvitesDisabled);
    }
    if paused {
        return Err(Denied::Paused);
    }
    let level = store.invite.level;
    Ok(Grant {
        name: format!("Convidado {}", &peer.to_string()[..4]),
        level,
        perms: mask(level.default_perms()),
        needs_confirmation: cfg.confirm_invites,
        via_invite: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Actor, User};

    fn id(n: u8) -> AccessId {
        AccessId::of_cert(&[n])
    }

    fn store() -> StoreData {
        let mut s = StoreData::default();
        s.add_user(&Actor::LocalAdmin, User::new(id(1), "Dono", Level::Master)).unwrap();
        let mut u = User::new(id(2), "Suporte", Level::FullControl);
        u.confirm = true;
        s.add_user(&Actor::LocalAdmin, u).unwrap();
        s.add_user(&Actor::LocalAdmin, User::new(id(3), "Olho", Level::View)).unwrap();
        s
    }

    #[test]
    fn unknown_without_invite_is_denied() {
        assert_eq!(decide(&store(), &id(9), false, 0), Err(Denied::Unknown));
    }

    #[test]
    fn master_ignores_pause_and_confirmation() {
        let mut s = store();
        s.config.paused_until = 100;
        s.config.allow_files = false;
        let g = decide(&s, &id(1), false, 50).unwrap();
        assert_eq!(g.level, Level::Master);
        assert_eq!(g.perms, Perms::ALL);
        assert!(!g.needs_confirmation);
    }

    #[test]
    fn pause_blocks_regular_users() {
        let mut s = store();
        s.config.paused_until = 100;
        assert_eq!(decide(&s, &id(2), false, 50), Err(Denied::Paused));
        assert!(decide(&s, &id(2), false, 100).is_ok());
    }

    #[test]
    fn user_confirmation_flag_is_honoured() {
        assert!(decide(&store(), &id(2), false, 0).unwrap().needs_confirmation);
        assert!(!decide(&store(), &id(3), false, 0).unwrap().needs_confirmation);
    }

    #[test]
    fn host_switches_mask_user_permissions() {
        let mut s = store();
        s.config.allow_clipboard = false;
        let g = decide(&s, &id(2), false, 0).unwrap();
        assert!(!g.perms.contains(Perms::CLIPBOARD));
        assert!(g.perms.contains(Perms::INPUT));
    }

    #[test]
    fn invite_grants_invite_level_with_confirmation() {
        let mut s = store();
        s.invite.enable(Level::View, 0);
        let g = decide(&s, &id(9), true, 0).unwrap();
        assert!(g.via_invite);
        assert_eq!(g.level, Level::View);
        assert!(!g.perms.contains(Perms::INPUT));
        assert!(g.needs_confirmation);
    }

    #[test]
    fn invite_refused_when_sharing_off_or_invites_disabled() {
        let mut s = store();
        assert_eq!(decide(&s, &id(9), true, 0), Err(Denied::InvitesDisabled));
        s.invite.enable(Level::View, 0);
        s.config.allow_invites = false;
        assert_eq!(decide(&s, &id(9), true, 0), Err(Denied::InvitesDisabled));
    }

    #[test]
    fn registered_user_entering_with_invite_keeps_own_level() {
        let mut s = store();
        s.invite.enable(Level::View, 0);
        assert_eq!(decide(&s, &id(2), true, 0).unwrap().level, Level::FullControl);
    }
}
