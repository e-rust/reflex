//! Modo `--user` (§10.8.2): roda como o usuário logado na sessão do console.
//! Cuida do clipboard e dos arquivos — o agente, SYSTEM, não pode ser dono do
//! clipboard do usuário nem ler os arquivos dele com as permissões certas — e
//! fala com o agente pelos pipes da sessão ([`reflex_host::user_bridge`]).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::windows::named_pipe::{NamedPipeClient, ServerOptions};
use tokio::sync::mpsc;

use reflex_host::user_bridge::{FilesFrom, UserMsg};
use reflex_platform::clipboard::{ClipSnapshot, ClipboardWatcher};
use reflex_platform::files::{self, BoxRead, BoxWrite, FileOffer, Opener, RemoteFiles, SharedOffer};
use reflex_platform::{desktop, paths, pipe, session};
use reflex_proto::frame::{read_known, write_msg};

pub fn run() -> i32 {
    let sess = session::current_session();
    log::info!("processo do usuário na sessão {sess}");
    let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            log::error!("runtime: {e}");
            return 1;
        }
    };
    // Os arquivos copiados aqui (o agente pede quando um viewer cola).
    let offer: SharedOffer = Arc::default();
    rt.spawn(serve_files(sess, offer.clone()));
    let seq = Arc::new(AtomicU64::new(0));
    let handle = rt.handle().clone();
    rt.block_on(async {
        // O agente reinicia (troca de sessão, atualização): reconecta.
        loop {
            match pipe::connect(&paths::user_pipe(sess)).await {
                Ok(client) => link(client, sess, offer.clone(), seq.clone(), handle.clone()).await,
                Err(e) => log::debug!("pipe do agente: {e}"),
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
}

/// Liga o clipboard daqui ao agente até o pipe fechar.
async fn link(client: NamedPipeClient, sess: u32, offer: SharedOffer, seq: Arc<AtomicU64>, handle: tokio::runtime::Handle) {
    let (mut read, mut write) = tokio::io::split(client);
    let (tx, mut rx) = mpsc::unbounded_channel::<UserMsg>();
    let Some(watcher) = local_clipboard(tx, offer, seq) else {
        log::warn!("clipboard: não deu pra observar o desta máquina");
        return;
    };
    log::info!("ligado ao agente");
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if write_msg(&mut write, &m).await.is_err() {
                break;
            }
        }
    });
    while let Ok(msg) = read_known::<_, UserMsg>(&mut read).await {
        if let UserMsg::RemoteCopy { viewer, offer } = msg {
            let snap = ClipSnapshot::from_offer(&offer);
            if offer.files.is_empty() {
                watcher.set(snap);
            } else {
                // Colar no Explorer copia do viewer, com o progresso do Windows.
                watcher.set_with_files(snap, Arc::new(RemoteFiles::new(offer.seq, from_viewer(sess, viewer), handle.clone())));
            }
        }
    }
    writer.abort();
    // `watcher` sai aqui e tira do clipboard os arquivos de um viewer que não
    // dá mais pra buscar.
}

/// Cada cópia aqui vira uma oferta pro agente — menos na tela de bloqueio ou
/// do UAC (área de trabalho segura, ou uma que este usuário nem abre).
fn local_clipboard(tx: mpsc::UnboundedSender<UserMsg>, offer: SharedOffer, seq: Arc<AtomicU64>) -> Option<ClipboardWatcher> {
    ClipboardWatcher::spawn(move |snap| {
        if desktop::input_desktop_name().is_none_or(|n| desktop::is_secure_desktop(&n)) {
            return;
        }
        let n = seq.fetch_add(1, Ordering::Relaxed) + 1;
        // Arquivos: só os nomes vão; o conteúdo, quando o viewer colar.
        if let Ok(mut o) = offer.lock() {
            *o = (!snap.files.is_empty()).then(|| Arc::new(FileOffer::new(n, snap.files.clone())));
        }
        let _ = tx.send(UserMsg::LocalCopy(snap.to_offer(n)));
    })
}

/// Cada pedido de arquivo do viewer `viewer`: uma conexão no pipe do agente.
fn from_viewer(sess: u32, viewer: u64) -> Opener {
    Arc::new(move || {
        Box::pin(async move {
            let mut client = pipe::connect(&paths::agent_files_pipe(sess)).await?;
            write_msg(&mut client, &FilesFrom { viewer }).await.map_err(std::io::Error::other)?;
            let (read, write) = tokio::io::split(client);
            Ok((Box::new(write) as BoxWrite, Box::new(read) as BoxRead))
        })
    })
}

/// Serve os arquivos copiados aqui pro agente. ACL padrão: só este usuário e
/// o SYSTEM conectam.
async fn serve_files(sess: u32, offer: SharedOffer) {
    let name = paths::user_files_pipe(sess);
    let mut first = true;
    loop {
        let server = match ServerOptions::new().first_pipe_instance(first).create(&name) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("pipe {name}: {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        first = false;
        if server.connect().await.is_err() {
            continue;
        }
        let offer = offer.clone();
        tokio::spawn(async move {
            let (mut read, mut write) = tokio::io::split(server);
            if let Err(e) = files::serve(&offer, &mut read, &mut write).await {
                log::info!("arquivos pedidos pelo viewer: {e}");
            }
        });
    }
}
