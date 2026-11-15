//! Ponte do agente com o processo do usuário (`--user`, §10.8.2).
//!
//! O agente roda como SYSTEM: não pode ser dono do clipboard do usuário nem
//! ler os arquivos dele com as permissões certas. Quem faz isso é o `--user`;
//! o agente só repassa, por três pipes da sessão:
//!
//! - controle ([`crate::platform::paths::user_pipe`], servidor aqui): cópias
//!   de lá ([`UserMsg::LocalCopy`]) vão pros viewers; as dos viewers
//!   ([`UserMsg::RemoteCopy`]) vão pro clipboard de lá;
//! - viewer cola arquivos daqui: cada stream que ele abre é ligado a uma
//!   conexão no pipe de arquivos do `--user` (servidor lá);
//! - `--user` cola arquivos de um viewer: conecta no pipe de arquivos do
//!   agente, diz de qual viewer ([`FilesFrom`]) e o pipe é ligado a um
//!   stream novo até ele.
//!
//! Cada cliente é conferido: o de controle tem de estar na sessão do agente,
//! e o pipe de arquivos (dos dois lados) tem de ser do mesmo processo que
//! conectou no controle.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::windows::named_pipe::NamedPipeServer;
use tokio::sync::mpsc;

use crate::net::Connection;
use crate::platform::{paths, pipe};
use crate::proto::frame::{read_known, write_msg};
use crate::proto::msg::{ClipboardOffer, Reply, StreamError};

/// Mensagens do pipe de controle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserMsg {
    /// Cópia nesta máquina (`--user` → agente).
    LocalCopy(ClipboardOffer),
    /// Cópia num viewer (agente → `--user`); os arquivos dela vêm de `viewer`.
    RemoteCopy { viewer: u64, offer: ClipboardOffer },
}

/// 1ª mensagem no pipe de arquivos do agente: de qual viewer buscar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesFrom {
    pub viewer: u64,
}

/// A cópia como um viewer pode recebê-la: sem a permissão de arquivos, os
/// arquivos ficam de fora; não sobrando nada, nada vai. Pura.
pub fn for_viewer(offer: &ClipboardOffer, files: bool) -> Option<ClipboardOffer> {
    let mut o = offer.clone();
    if !files {
        o.files.clear();
    }
    (!o.items.is_empty() || !o.files.is_empty()).then_some(o)
}

struct UserLink {
    pid: u32,
    tx: mpsc::UnboundedSender<UserMsg>,
    generation: u64,
}

struct ViewerLink {
    conn: Arc<dyn Connection>,
    files: bool,
    copies: mpsc::UnboundedSender<ClipboardOffer>,
}

#[derive(Default)]
struct State {
    user: Option<UserLink>,
    generation: u64,
    viewers: HashMap<u64, ViewerLink>,
    /// A última cópia daqui (quem conecta depois já recebe).
    last_local: Option<ClipboardOffer>,
}

pub struct UserBridge {
    session: u32,
    state: Mutex<State>,
}

impl UserBridge {
    /// Começa a ouvir os pipes da sessão `session` (dentro de um runtime tokio).
    pub fn start(session: u32) -> Arc<Self> {
        let me = Arc::new(Self { session, state: Mutex::default() });
        tokio::spawn(me.clone().accept_control());
        tokio::spawn(me.clone().accept_files());
        me
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Um viewer com permissão de clipboard entrou: recebe as cópias daqui
    /// (a atual já vem). `files`: pode colar e mandar arquivos.
    pub fn join(&self, viewer: u64, conn: Arc<dyn Connection>, files: bool) -> mpsc::UnboundedReceiver<ClipboardOffer> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut s = self.state();
        if let Some(o) = s.last_local.as_ref().and_then(|o| for_viewer(o, files)) {
            let _ = tx.send(o);
        }
        s.viewers.insert(viewer, ViewerLink { conn, files, copies: tx });
        rx
    }

    pub fn leave(&self, viewer: u64) {
        self.state().viewers.remove(&viewer);
    }

    /// Cópia no viewer: vai pro clipboard daqui (pelo `--user`).
    pub fn remote_copy(&self, viewer: u64, offer: ClipboardOffer) {
        let s = self.state();
        let (Some(v), Some(user)) = (s.viewers.get(&viewer), s.user.as_ref()) else {
            return;
        };
        if let Some(offer) = for_viewer(&offer, v.files) {
            let _ = user.tx.send(UserMsg::RemoteCopy { viewer, offer });
        }
    }

    /// Stream aberto pelo viewer pra buscar arquivos copiados aqui: liga a
    /// uma conexão no pipe de arquivos do `--user`.
    pub async fn files_from_user<W, R>(&self, viewer: u64, mut write: W, read: R) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
        R: AsyncRead + Unpin,
    {
        let (allowed, pid) = {
            let s = self.state();
            (s.viewers.get(&viewer).is_some_and(|v| v.files), s.user.as_ref().map(|u| u.pid))
        };
        let refuse = match (allowed, pid) {
            (false, _) => Some(StreamError::NotAllowed),
            (true, None) => Some(StreamError::Io("ninguém logado nesta máquina".into())),
            _ => None,
        };
        if let Some(e) = refuse {
            return write_msg(&mut write, &Reply::Error(e)).await.map_err(io::Error::other);
        }
        let client = pipe::connect(&paths::user_files_pipe(self.session)).await?;
        if pipe::server_process(&client) != pid {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "pipe de arquivos não é do processo do usuário"));
        }
        let (pr, pw) = tokio::io::split(client);
        splice(pr, pw, read, write).await
    }

    async fn accept_control(self: Arc<Self>) {
        let name = paths::user_pipe(self.session);
        let mut first = true;
        loop {
            let Some(server) = next_instance(&name, &mut first).await else { continue };
            match pipe::client_session_process(&server) {
                Some((session, pid)) if session == self.session => {
                    tokio::spawn(self.clone().serve_user(server, pid));
                }
                other => log::warn!("pipe do usuário: cliente de outra sessão ({other:?}), recusado"),
            }
        }
    }

    /// Fala com o `--user` até o pipe fechar. Um novo substitui o anterior.
    async fn serve_user(self: Arc<Self>, server: NamedPipeServer, pid: u32) {
        let (mut read, mut write) = tokio::io::split(server);
        let (tx, mut rx) = mpsc::unbounded_channel::<UserMsg>();
        let generation = {
            let mut s = self.state();
            s.generation += 1;
            s.user = Some(UserLink { pid, tx, generation: s.generation });
            s.generation
        };
        log::info!("processo do usuário conectado (pid {pid})");
        let writer = tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                if write_msg(&mut write, &m).await.is_err() {
                    break;
                }
            }
        });
        while let Ok(msg) = read_known::<_, UserMsg>(&mut read).await {
            if let UserMsg::LocalCopy(offer) = msg {
                self.local_copy(offer);
            }
        }
        writer.abort();
        let mut s = self.state();
        if s.user.as_ref().is_some_and(|u| u.generation == generation) {
            s.user = None;
            log::info!("processo do usuário desconectou");
        }
    }

    /// Cópia daqui: pra todos os viewers com clipboard.
    fn local_copy(&self, offer: ClipboardOffer) {
        let mut s = self.state();
        for v in s.viewers.values() {
            if let Some(o) = for_viewer(&offer, v.files) {
                let _ = v.copies.send(o);
            }
        }
        s.last_local = Some(offer);
    }

    async fn accept_files(self: Arc<Self>) {
        let name = paths::agent_files_pipe(self.session);
        let mut first = true;
        loop {
            let Some(server) = next_instance(&name, &mut first).await else { continue };
            let me = self.clone();
            tokio::spawn(async move {
                if let Err(e) = me.files_to_viewer(server).await {
                    log::info!("arquivos do viewer: {e}");
                }
            });
        }
    }

    /// O `--user` está colando arquivos de um viewer: liga o pipe a um
    /// stream novo até ele.
    async fn files_to_viewer(&self, mut server: NamedPipeServer) -> io::Result<()> {
        let pid = pipe::client_session_process(&server).map(|(_, pid)| pid);
        if pid.is_none() || self.state().user.as_ref().map(|u| u.pid) != pid {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "cliente não é o processo do usuário"));
        }
        let FilesFrom { viewer } = read_known(&mut server).await.map_err(io::Error::other)?;
        let conn = self.state().viewers.get(&viewer).filter(|v| v.files).map(|v| v.conn.clone());
        let conn = conn.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "o viewer saiu ou não pode mandar arquivos"))?;
        let (write, read) = conn.open_bi().await?;
        let (pr, pw) = tokio::io::split(server);
        splice(pr, pw, read, write).await
    }
}

/// Próxima instância do servidor, já com um cliente conectado. Erro: espera
/// um pouco (outro processo com o nome? o próximo laço tenta de novo).
async fn next_instance(name: &str, first: &mut bool) -> Option<NamedPipeServer> {
    let server = match pipe::create_server(name, *first, pipe::USER_PIPE_SDDL) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("pipe {name}: {e}");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            return None;
        }
    };
    *first = false;
    server.connect().await.ok()?;
    Some(server)
}

/// Copia nos dois sentidos até os dois lados terminarem (ou um falhar).
async fn splice<AR, AW, BR, BW>(a_read: AR, a_write: AW, b_read: BR, b_write: BW) -> io::Result<()>
where
    AR: AsyncRead + Unpin,
    AW: AsyncWrite + Unpin,
    BR: AsyncRead + Unpin,
    BW: AsyncWrite + Unpin,
{
    async fn pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(mut r: R, mut w: W) -> io::Result<()> {
        tokio::io::copy(&mut r, &mut w).await?;
        w.shutdown().await
    }
    tokio::try_join!(pump(a_read, b_write), pump(b_read, a_write)).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::msg::{ClipFormatId, ClipItem};

    fn offer(items: bool, files: bool) -> ClipboardOffer {
        ClipboardOffer {
            seq: 3,
            items: if items { vec![ClipItem { format: ClipFormatId::Standard(13), data: vec![1, 0] }] } else { Vec::new() },
            files: if files { vec!["a.txt".into()] } else { Vec::new() },
        }
    }

    #[test]
    fn files_only_with_permission() {
        assert_eq!(for_viewer(&offer(true, true), true), Some(offer(true, true)));
        assert_eq!(for_viewer(&offer(true, true), false), Some(offer(true, false)));
        // Só arquivos e sem permissão: nada a mandar.
        assert_eq!(for_viewer(&offer(false, true), false), None);
        assert_eq!(for_viewer(&offer(false, false), true), None);
    }

    #[tokio::test]
    async fn splice_carries_both_ways_and_ends() {
        use tokio::io::AsyncReadExt;
        let (a, a_far) = tokio::io::duplex(64);
        let (b, b_far) = tokio::io::duplex(64);
        let (ar, aw) = tokio::io::split(a);
        let (br, bw) = tokio::io::split(b);
        let mid = tokio::spawn(splice(ar, aw, br, bw));
        let (mut a_far_r, mut a_far_w) = tokio::io::split(a_far);
        let (mut b_far_r, mut b_far_w) = tokio::io::split(b_far);
        a_far_w.write_all(b"pedido").await.unwrap();
        a_far_w.shutdown().await.unwrap();
        let mut got = Vec::new();
        b_far_r.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"pedido");
        b_far_w.write_all(b"resposta").await.unwrap();
        b_far_w.shutdown().await.unwrap();
        let mut got = Vec::new();
        a_far_r.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"resposta");
        mid.await.unwrap().unwrap();
    }
}
