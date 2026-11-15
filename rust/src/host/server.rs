//! Aceitador do host. Para cada conexão: abre o controle, roda o handshake e,
//! se concedido, serve a sessão. O estado (usuários, convite, config) é
//! compartilhado e lido a cada conexão.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::auth::identity::AccessId;
use crate::auth::store::{Actor, MemStorage, SignedFile, Storage};
use crate::net::{Connection, Listener};
use crate::proto::frame::{read_known, write_msg};
use crate::proto::perms::Level;
use crate::proto::msg::{Caps, Control, Hello, Monitor, SessionConfig};

use crate::host::handshake::{self, Env, Outcome};
use crate::host::session::{self, AdminFn, ControlStreams, SessionParams};

/// Pede a confirmação do usuário local (a UI implementa; aqui um alias).
pub type ConfirmFn = Arc<dyn Fn(crate::auth::policy::Grant) -> tokio::sync::oneshot::Receiver<bool> + Send + Sync>;

/// Dependências do host que a UI/serviço fornece.
pub struct HostDeps<S: Storage + Send + Sync + 'static> {
    pub store: SignedFile<S>,
    pub machine_name: String,
    pub app_version: String,
    /// Encoders desta máquina (testados uma vez).
    pub encoders: Vec<crate::proto::codec::CodecCap>,
    /// Lista de monitores (recalculada por conexão por uma função).
    pub monitors: Arc<dyn Fn() -> Vec<Monitor> + Send + Sync>,
    pub confirm: ConfirmFn,
    pub next_session_id: Arc<AtomicU64>,
    /// Ponte com o processo do usuário (clipboard e arquivos).
    pub user: Option<Arc<crate::host::user_bridge::UserBridge>>,
    /// O motor de captura, um só pra todas as conexões.
    pub hub: Arc<crate::host::hub::EngineHub>,
}

impl<S: Storage + Send + Sync + 'static> HostDeps<S> {
    fn hello(&self) -> Hello {
        Hello {
            proto: crate::proto::PROTOCOL_VERSION,
            app_version: self.app_version.clone(),
            machine_name: self.machine_name.clone(),
            caps: Caps { encoders: self.encoders.clone(), decoders: Vec::new() },
        }
    }
}

/// Aceita conexões até o listener fechar.
pub async fn serve(mut listener: Listener, deps: Arc<HostDeps<impl Storage + Send + Sync + 'static>>) {
    while let Some(incoming) = listener.accept().await {
        match incoming {
            Ok(conn) => {
                let deps = deps.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle(conn, deps).await {
                        log::info!("sessão encerrada: {e}");
                    }
                });
            }
            Err(e) => log::warn!("handshake de conexão falhou: {e}"),
        }
    }
}

async fn handle(conn: Arc<dyn Connection>, deps: Arc<HostDeps<impl Storage + Send + Sync + 'static>>) -> Result<(), crate::host::HostError> {
    // O viewer abre o controle (bi) primeiro.
    let (mut write, mut read) = conn.accept_bi().await?;
    let store = deps.store.load().map_err(|e| crate::host::HostError::Denied(e.to_string()))?;
    let session_id = deps.next_session_id.fetch_add(1, Ordering::Relaxed) + 1;

    let env = Env { store: &store, host_hello: deps.hello(), binding: conn.channel_binding(), now: crate::auth::unix_now() };
    let peer_cert = conn.peer_cert();
    let confirm = deps.confirm.clone();
    let outcome = handshake::run(&mut write, &mut read, &env, &peer_cert, session_id, |grant| async move { confirm(grant).await.unwrap_or(false) })
        .await
        .map_err(|e| crate::host::HostError::Denied(e.to_string()))?;

    let Outcome::Granted { peer, grant, mut decoders, .. } = outcome else {
        return Ok(());
    };
    log::info!("sessão {session_id}: {} ({:?}) de {}", grant.name, grant.level, conn.remote_addr());
    let admin = admin_handler(&deps, &peer, grant.level);

    // Antes do vídeo: pedidos de administração (o `reflex --admin` só faz
    // isso e sai) até chegar a 1ª SessionConfig.
    let initial: SessionConfig = loop {
        match read_known::<_, Control>(&mut read).await {
            Ok(Control::SessionConfig(c)) => break c,
            Ok(Control::Admin { id, req }) => {
                log::info!("sessão {session_id}: administração {req:?}");
                write_msg(&mut write, &session::admin_reply(admin.as_ref(), id, req)).await?;
            }
            Ok(Control::Bye { .. }) | Err(_) => return Ok(()),
            Ok(_) => {}
        }
    };

    // Sem decoders no Hello do viewer, assume H.264 (todo decoder tem).
    if decoders.is_empty() {
        decoders.push(crate::proto::codec::CodecCap { codec: crate::proto::codec::Codec::H264, hw: false });
    }
    let params = SessionParams {
        perms: grant.perms,
        monitors: (deps.monitors)(),
        encoders: deps.encoders.clone(),
        decoders,
        cpu_cores: crate::media::encode::cpu_cores(),
        full_chroma: crate::media::caps::has_full_chroma(crate::media::caps::encoders()),
        id: session_id,
        user: deps.user.clone(),
        hub: deps.hub.clone(),
    };
    session::serve(conn, params, initial, ControlStreams { write, read }, admin).await
}

/// Administração remota pra sessão de master (§11.5); os outros níveis não têm.
fn admin_handler<S: Storage + Send + Sync + 'static>(deps: &Arc<HostDeps<S>>, peer: &AccessId, level: Level) -> Option<AdminFn> {
    if !level.is_master() {
        return None;
    }
    let deps = deps.clone();
    let actor = Actor::RemoteMaster(peer.clone());
    Some(Arc::new(move |req| crate::auth::admin::handle(&deps.store, &actor, &crate::auth::admin::NoSessions, req, crate::auth::unix_now())))
}

/// Host em memória pra teste (sem disco).
pub fn mem_store(secret: &[u8]) -> SignedFile<MemStorage> {
    SignedFile::new(MemStorage::default(), secret.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_carries_encoders() {
        let deps = HostDeps {
            store: mem_store(b"k"),
            machine_name: "pc".into(),
            app_version: "1".into(),
            encoders: vec![crate::proto::codec::CodecCap { codec: crate::proto::codec::Codec::Av1, hw: true }],
            monitors: Arc::new(Vec::new),
            confirm: Arc::new(|_| {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let _ = tx.send(true);
                rx
            }),
            next_session_id: Arc::new(AtomicU64::new(0)),
            user: None,
            hub: Arc::default(),
        };
        assert_eq!(deps.hello().caps.encoders.len(), 1);
        assert_eq!(deps.hello().proto, crate::proto::PROTOCOL_VERSION);
    }
}
