//! Handshake do host (§11): sobre o canal de controle, conferido TLS.
//!
//! 1. recebe `Hello`, manda o `Hello` do host (com os encoders);
//! 2. identifica o viewer pela chave (certificado TLS → `AccessId`);
//! 3. se cadastrado, resolve pela política; se não, tenta o convite
//!    (SPAKE2 amarrado ao channel binding do TLS);
//! 4. se a política exigir confirmação, pede ao usuário local (callback);
//! 5. manda `AuthResult`.
//!
//! Tudo o que toca o sistema (relógio, confirmação, binding) entra por
//! parâmetro, então o fluxo roda inteiro sobre um `tokio::io::duplex`.

use crate::auth::identity::AccessId;
use crate::auth::invite::InviteState;
use crate::auth::pake::{Pake, Role};
use crate::auth::policy::{decide, Grant};
use crate::auth::store::StoreData;
use crate::proto::frame::{read_known, write_msg, FrameError};
use crate::proto::msg::{AuthResult, Control, Hello};
#[cfg(test)]
use crate::proto::msg::Caps;
use tokio::io::{AsyncRead, AsyncWrite};

/// O que o host precisa saber do ambiente (injetado pra testar).
pub struct Env<'a> {
    pub store: &'a StoreData,
    pub host_hello: Hello,
    /// Channel binding do TLS (igual nos dois lados só no mesmo canal).
    pub binding: [u8; 32],
    pub now: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Granted { peer: AccessId, grant: Grant, session_id: u64, decoders: Vec<crate::proto::codec::CodecCap> },
    Denied(String),
}

#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error("viewer mandou {0} em vez de {1}")]
    Unexpected(&'static str, &'static str),
}

fn control_name(c: &Control) -> &'static str {
    match c {
        Control::Hello(_) => "Hello",
        Control::InviteStart { .. } => "InviteStart",
        Control::InviteConfirm { .. } => "InviteConfirm",
        _ => "outra",
    }
}

/// Roda o handshake. `confirm` é chamado quando a política pede confirmação do
/// usuário local; devolve `true` se alguém permitiu.
pub async fn run<S, R, F, Fut>(write: &mut S, read: &mut R, env: &Env<'_>, peer_cert: &[u8], next_session_id: u64, confirm: F) -> Result<Outcome, HandshakeError>
where
    S: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
    F: FnOnce(Grant) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    // 1. Hello ↔ Hello.
    let hello: Hello = match read_known::<_, Control>(read).await? {
        Control::Hello(h) => h,
        other => return Err(HandshakeError::Unexpected(control_name(&other), "Hello")),
    };
    let decoders = hello.caps.decoders.clone();
    write_msg(write, &Control::Hello(env.host_hello.clone())).await?;
    if let Some(reason) = crate::proto::version_mismatch(hello.proto, "o viewer") {
        write_msg(write, &Control::AuthResult(AuthResult::denied(&reason))).await?;
        return Ok(Outcome::Denied(reason));
    }

    let peer = AccessId::of_cert(peer_cert);
    let registered = env.store.find(&peer).is_some();

    // 2/3. Convite (SPAKE2) só pra quem não é cadastrado.
    let invite_proven = if registered {
        false
    } else {
        match prove_invite(write, read, &env.store.invite, &env.binding).await? {
            Some(true) => true,
            Some(false) => {
                write_msg(write, &Control::AuthResult(AuthResult::denied("código de acesso incorreto"))).await?;
                return Ok(Outcome::Denied("código incorreto".into()));
            }
            None => false,
        }
    };

    // 4. Política.
    let grant = match decide(env.store, &peer, invite_proven, env.now) {
        Ok(g) => g,
        Err(e) => {
            write_msg(write, &Control::AuthResult(AuthResult::denied(e.to_string()))).await?;
            return Ok(Outcome::Denied(e.to_string()));
        }
    };

    // 5. Confirmação do usuário local, se preciso.
    if grant.needs_confirmation {
        write_msg(write, &Control::AwaitingConfirmation).await?;
        if !confirm(grant.clone()).await {
            write_msg(write, &Control::AuthResult(AuthResult::denied("o usuário na máquina recusou"))).await?;
            return Ok(Outcome::Denied("recusado na máquina".into()));
        }
    }

    write_msg(
        write,
        &Control::AuthResult(AuthResult { ok: true, reason: String::new(), session_id: next_session_id, level: Some(grant.level), perms: grant.perms }),
    )
    .await?;
    Ok(Outcome::Granted { peer, grant, session_id: next_session_id, decoders })
}

/// SPAKE2 do lado do host. `Ok(None)` = o viewer nem tentou convite (não é
/// cadastrado e não mandou `InviteStart`); `Ok(Some(ok))` = tentou.
async fn prove_invite<S, R>(write: &mut S, read: &mut R, invite: &InviteState, binding: &[u8]) -> Result<Option<bool>, HandshakeError>
where
    S: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    let start = read_known::<_, Control>(read).await?;
    let viewer_msg = match start {
        Control::InviteStart { spake } => spake,
        other => return Err(HandshakeError::Unexpected(control_name(&other), "InviteStart")),
    };
    // O código atual do host; se o compartilhar está desligado, usa um código
    // impossível pra manter o protocolo simétrico (vai falhar de qualquer jeito).
    let code = invite.current_code(u64::MAX / 2).unwrap_or_default();
    let (pake, host_msg) = Pake::start(&code, Role::Host);
    write_msg(write, &Control::InviteReply { spake: host_msg }).await?;
    let confirmed = match pake.finish(&viewer_msg, binding) {
        Ok(c) => c,
        Err(_) => return Ok(Some(false)),
    };
    write_msg(write, &Control::InviteConfirm { mac: confirmed.my_proof() }).await?;
    let peer_confirm = match read_known::<_, Control>(read).await? {
        Control::InviteConfirm { mac } => mac,
        other => return Err(HandshakeError::Unexpected(control_name(&other), "InviteConfirm")),
    };
    Ok(Some(confirmed.verify_peer(&peer_confirm)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::pake::{Pake as VPake, Role as VRole};
    use crate::auth::store::{Actor, User};
    use crate::proto::perms::{Level, Perms};

    fn hello() -> Hello {
        Hello { proto: crate::proto::PROTOCOL_VERSION, app_version: "t".into(), machine_name: "host".into(), caps: Caps::default() }
    }

    fn store_with(users: &[(u8, Level)]) -> StoreData {
        let mut s = StoreData::default();
        for &(n, lvl) in users {
            s.add_user(&Actor::LocalAdmin, User::new(AccessId::of_cert(&[n]), "u", lvl)).unwrap();
        }
        s
    }

    /// Roda o handshake do host contra um "viewer" de teste e devolve os dois lados.
    async fn exchange(store: StoreData, peer_cert: Vec<u8>, viewer: impl FnOnce(tokio::io::DuplexStream) -> tokio::task::JoinHandle<AuthResult> + Send, confirm: bool) -> (Outcome, AuthResult) {
        let (host_io, viewer_io) = tokio::io::duplex(8192);
        let (mut r, mut w) = tokio::io::split(host_io);
        let env = Env { store: &store, host_hello: hello(), binding: [7u8; 32], now: 100 };
        let v = viewer(viewer_io);
        let outcome = run(&mut w, &mut r, &env, &peer_cert, 42, |_g| async move { confirm }).await.unwrap();
        (outcome, v.await.unwrap())
    }

    /// Viewer mínimo: manda Hello, lê Hello, (opcional convite), lê o AuthResult.
    fn plain_viewer(binding: [u8; 32], invite_code: Option<String>, registered: bool) -> impl FnOnce(tokio::io::DuplexStream) -> tokio::task::JoinHandle<AuthResult> {
        move |io| {
            tokio::spawn(async move {
                let (mut r, mut w) = tokio::io::split(io);
                write_msg(&mut w, &Control::Hello(hello())).await.unwrap();
                let _: Control = read_known(&mut r).await.unwrap(); // Hello do host
                if !registered {
                    let code = invite_code.clone().unwrap_or_default();
                    let (pake, msg) = VPake::start(&code, VRole::Viewer);
                    write_msg(&mut w, &Control::InviteStart { spake: msg }).await.unwrap();
                    let reply = match read_known::<_, Control>(&mut r).await.unwrap() {
                        Control::InviteReply { spake } => spake,
                        other => return deny_or(other),
                    };
                    let confirmed = pake.finish(&reply, &binding).unwrap();
                    // host manda InviteConfirm (ou AuthResult de negação)
                    match read_known::<_, Control>(&mut r).await.unwrap() {
                        Control::InviteConfirm { .. } => {}
                        Control::AuthResult(ar) => return ar,
                        _ => return AuthResult::denied("x"),
                    }
                    write_msg(&mut w, &Control::InviteConfirm { mac: confirmed.my_proof() }).await.unwrap();
                }
                loop {
                    match read_known::<_, Control>(&mut r).await.unwrap() {
                        Control::AwaitingConfirmation => continue,
                        Control::AuthResult(ar) => return ar,
                        _ => return AuthResult::denied("inesperado"),
                    }
                }
            })
        }
    }

    fn deny_or(_c: Control) -> AuthResult {
        AuthResult::denied("fluxo inesperado")
    }

    #[tokio::test]
    async fn registered_user_is_granted_by_key() {
        let store = store_with(&[(1, Level::FullControl)]);
        let (outcome, ar) = exchange(store, vec![1], plain_viewer([7u8; 32], None, true), false).await;
        assert!(matches!(outcome, Outcome::Granted { .. }));
        assert!(ar.ok && ar.session_id == 42 && ar.level == Some(Level::FullControl));
        assert!(ar.perms.contains(Perms::INPUT));
    }

    #[tokio::test]
    async fn unknown_key_without_invite_is_denied() {
        let store = store_with(&[(1, Level::Master)]);
        let (outcome, ar) = exchange(store, vec![9], plain_viewer([7u8; 32], Some(String::new()), false), false).await;
        assert!(matches!(outcome, Outcome::Denied(_)));
        assert!(!ar.ok);
    }

    #[tokio::test]
    async fn correct_invite_grants_access() {
        let mut store = store_with(&[]);
        store.invite.enable(Level::View, 100);
        let code = store.invite.current_code(u64::MAX / 2).unwrap();
        let (outcome, ar) = exchange(store, vec![50], plain_viewer([7u8; 32], Some(code), false), true).await;
        assert!(matches!(outcome, Outcome::Granted { .. }), "{outcome:?}");
        assert!(ar.ok && ar.level == Some(Level::View));
    }

    #[tokio::test]
    async fn wrong_invite_code_is_denied() {
        let mut store = store_with(&[]);
        store.invite.enable(Level::View, 100);
        let (outcome, ar) = exchange(store, vec![50], plain_viewer([7u8; 32], Some("000-000-000".into()), false), false).await;
        assert!(matches!(outcome, Outcome::Denied(_)));
        assert!(!ar.ok);
    }

    #[tokio::test]
    async fn confirmation_required_user_refuses() {
        let mut store = store_with(&[]);
        let mut u = User::new(AccessId::of_cert(&[2]), "s", Level::FullControl);
        u.confirm = true;
        store.add_user(&Actor::LocalAdmin, u).unwrap();
        let (outcome, ar) = exchange(store, vec![2], plain_viewer([7u8; 32], None, true), false).await;
        assert!(matches!(outcome, Outcome::Denied(_)));
        assert!(!ar.ok && ar.reason.contains("recusou"));
    }

    #[tokio::test]
    async fn confirmation_required_user_accepts() {
        let mut store = store_with(&[]);
        let mut u = User::new(AccessId::of_cert(&[2]), "s", Level::FullControl);
        u.confirm = true;
        store.add_user(&Actor::LocalAdmin, u).unwrap();
        let (outcome, ar) = exchange(store, vec![2], plain_viewer([7u8; 32], None, true), true).await;
        assert!(matches!(outcome, Outcome::Granted { .. }));
        assert!(ar.ok);
    }
}
