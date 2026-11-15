//! Uma conexão com outra máquina (o viewer, §5.2), sem tela: conecta e
//! autentica, roda os laços do viewer, reconecta quando cai e entrega o que
//! chega (quadros, cursor, título) numa [`Saida`] que a janela de vídeo lê.
//!
//! Roda numa thread própria ([`rodar`]): a tela só é avisada pelo `post` do
//! Aylis. A janela em si é do form ([`Janela`]: como abrir, pintar e titular),
//! assim a conexão não depende de nenhum form.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use aylis::prelude::{ComponentId, DlgKind, ModalResult, Poster, Ui, message_dlg};
use crate::media::decode::BgraFrame;
use crate::net::TransportMode;
use crate::platform::clipboard::{ClipSnapshot, ClipboardWatcher};
use crate::platform::files::{FileOffer, Opener, RemoteFiles, SharedOffer};
use crate::proto::codec::CodecChoice;
use crate::proto::msg::{Chroma, Control, DesiredDisplay, Input, Refine, RemoteSize, SessionConfig};
use crate::viewer::client::{self, Credential, ViewerEvent};
use crate::viewer::hosts::{ClipboardMode, CursorMode, Hosts, ViewScale};

use crate::addr;
use crate::identity::viewer_cert;
use crate::video::geometria::RemoteCursor;

/// `%LOCALAPPDATA%\Reflex\hosts.json`: hosts salvos, o último endereço e as opções.
fn hosts_file() -> std::path::PathBuf {
    crate::platform::paths::user_dir().join("hosts.json")
}

/// Opções de imagem da aba Conectar (salvas; o `--connect` usa as mesmas).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewOptions {
    pub refine: Refine,
    pub scale: ViewScale,
    pub chroma: Chroma,
    pub remote_size: RemoteSize,
    pub cursor: CursorMode,
    pub clipboard: ClipboardMode,
}

impl ViewOptions {
    pub fn saved() -> Self {
        let h = Hosts::load(&hosts_file());
        Self { refine: h.refine, scale: h.scale, chroma: h.chroma, remote_size: h.remote_size, cursor: h.cursor, clipboard: h.clipboard }
    }

    /// Grava (falha só vai pro log).
    pub fn save(self) {
        let path = hosts_file();
        let mut h = Hosts::load(&path);
        (h.refine, h.scale, h.chroma, h.remote_size, h.cursor, h.clipboard) = (self.refine, self.scale, self.chroma, self.remote_size, self.cursor, self.clipboard);
        if let Err(e) = h.save(&path) {
            log::warn!("salvar as opções de imagem: {e}");
        }
    }

    /// Escala efetiva: com a tela remota no tamanho da janela, ela já vem na
    /// escala daqui — ampliar de novo dobraria. Pura.
    pub fn effective_scale(self) -> ViewScale {
        match self.remote_size {
            RemoteSize::MatchWindow => ViewScale::Native,
            RemoteSize::Keep => self.scale,
        }
    }
}

/// O último endereço que conectou (vazio na 1ª vez).
pub fn last_address() -> String {
    Hosts::load(&hosts_file()).last_address.unwrap_or_default()
}

/// Grava `address` como o último que conectou (falha só vai pro log).
fn remember_last(address: &str) {
    let path = hosts_file();
    let mut hosts = Hosts::load(&path);
    hosts.set_last(address);
    if let Err(e) = hosts.save(&path) {
        log::warn!("salvar o último endereço: {e}");
    }
}

/// Onde a conexão conta o que está acontecendo (a linha de status da aba Conectar).
pub type Report = Arc<dyn Fn(&str) + Send + Sync>;

/// Recebe o tamanho (pixels reais) e a escala que a tela remota deve ter.
pub type OnResize = Box<dyn Fn(DesiredDisplay) + Send>;

/// A janela de vídeo, vista pela conexão: o form diz como abrir, pintar e
/// titular (sempre na thread da tela).
#[derive(Clone, Copy)]
pub struct Janela {
    /// Cria e mostra a janela da conexão; chama [`Saida::ligar`] com o form dela.
    pub abrir: fn(&mut Ui, Abertura),
    /// Chegou quadro (ou o cursor andou): pinta.
    pub pintar: fn(&mut Ui, ComponentId),
    pub titular: fn(&mut Ui, ComponentId, &str),
}

/// O que a janela recebe ao abrir.
pub struct Abertura {
    pub titulo: String,
    pub saida: Arc<Saida>,
    /// Mouse e teclado pra máquina remota.
    pub input: tokio::sync::mpsc::UnboundedSender<Input>,
    pub opcoes: ViewOptions,
    /// Resolução "igual à janela": pra onde mandar o tamanho da janela.
    pub pedir_tamanho: Option<OnResize>,
}

/// O que a conexão entrega à janela de vídeo (escrito por outras threads).
pub struct Saida {
    /// O quadro mais novo ainda não pintado.
    pub quadro: Mutex<Option<BgraFrame>>,
    pub cursor: Mutex<RemoteCursor>,
    /// Já há uma pintura na fila da tela (não empilha uma por quadro).
    pendente: AtomicBool,
    aberta: AtomicBool,
    form: Mutex<Option<ComponentId>>,
    poster: Poster,
    janela: Janela,
    fechou: tokio::sync::Notify,
}

impl Saida {
    fn new(poster: Poster, janela: Janela) -> Self {
        Self {
            quadro: Mutex::default(),
            cursor: Mutex::default(),
            pendente: AtomicBool::new(false),
            aberta: AtomicBool::new(true),
            form: Mutex::new(None),
            poster,
            janela,
            fechou: tokio::sync::Notify::new(),
        }
    }

    /// A janela existe: as pinturas vão pra ela (e a que ficou esperando, já).
    pub fn ligar(&self, form: ComponentId) {
        if let Ok(mut f) = self.form.lock() {
            *f = Some(form);
        }
        self.avisar();
    }

    /// A janela ainda está aberta (fechar encerra a conexão).
    pub fn aberta(&self) -> bool {
        self.aberta.load(Ordering::Acquire)
    }

    /// A janela fechou: a conexão termina.
    pub fn fechar(&self) {
        self.aberta.store(false, Ordering::Release);
        self.fechou.notify_one();
    }

    /// A pintura pedida rodou (a próxima novidade pede outra).
    pub fn pintado(&self) {
        self.pendente.store(false, Ordering::Release);
    }

    /// Há novidade pra pintar: pede uma pintura, se ainda não há uma na fila.
    fn avisar(&self) {
        let Some(form) = self.form.lock().ok().and_then(|f| *f) else { return };
        if !self.pendente.swap(true, Ordering::AcqRel) {
            let pintar = self.janela.pintar;
            self.poster.post_to(form, move |ui| pintar(ui, form));
        }
    }

    fn titular(&self, titulo: String) {
        let Some(form) = self.form.lock().ok().and_then(|f| *f) else { return };
        let titular = self.janela.titular;
        self.poster.post_to(form, move |ui| titular(ui, form, &titulo));
    }

    async fn fechada(&self) {
        while self.aberta() {
            self.fechou.notified().await;
        }
    }
}

/// Mensagem de erro na tela (de qualquer thread).
fn mostrar_erro(poster: &Poster, texto: String) {
    poster.post(move |ui| {
        message_dlg(ui, &texto, DlgKind::Error, &[ModalResult::Ok], |_, _| {});
    });
}

/// Erro de conexão: na linha de status, no log e numa mensagem.
fn falhou(report: &Report, poster: &Poster, address: &str, error: &str) {
    log::error!("conexão com {address}: {error}");
    report(&format!("Não foi possível conectar a {address}: {error}"));
    mostrar_erro(poster, format!("Não foi possível conectar a {address}.\n\n{error}"));
}

/// Resolve `address` (`ip:porta` ou `nome:porta`), conecta, autentica, abre a
/// janela de vídeo e fica até ela fechar (bloqueante: chame numa thread).
/// Devolve se chegou a conectar.
pub fn rodar(address: &str, opts: ViewOptions, report: Report, poster: Poster, janela: Janela) -> bool {
    let socket = match addr::resolve(address) {
        Ok(s) => s,
        Err(e) => {
            falhou(&report, &poster, address, &e);
            return false;
        }
    };
    log::info!("{address} → {socket}");
    let cert = match viewer_cert() {
        Ok(c) => c,
        Err(e) => {
            falhou(&report, &poster, address, &format!("identidade deste viewer indisponível ({e})"));
            return false;
        }
    };
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            falhou(&report, &poster, address, &format!("runtime: {e}"));
            return false;
        }
    };
    // 1. Conecta e autentica antes de abrir a janela: erro vira mensagem.
    // TOFU no MVP: aceita o certificado (a checagem de mudança entra com a
    // lista salva de hosts).
    let check: crate::net::ServerCheck = Arc::new(|_| true);
    let decoders = crate::media::caps::decoders();
    let awaiting = || report(&format!("{address}: aguardando alguém na máquina permitir o acesso…"));
    let connected = rt.block_on(client::connect_and_auth(socket, &cert, check, TransportMode::Auto, env!("CARGO_PKG_VERSION"), decoders, Credential::Key, awaiting));
    let connected = match connected {
        Ok(c) => c,
        Err(e) => {
            falhou(&report, &poster, address, &e.to_string());
            return false;
        }
    };
    log::info!("conectado a {address} como {:?}", connected.level);
    let level = connected.level;
    report(&format!("Conectado a {address}; recebendo a imagem…"));
    remember_last(address);

    // 2. Laços do viewer no runtime; a janela pinta a saída. A janela, o
    // clipboard e o tamanho pedido falam com a conexão ATUAL (`link`): se ela
    // cair (sair da conta no Windows recria a sessão de lá), outra entra no
    // lugar sem fechar a janela.
    let saida = Arc::new(Saida::new(poster.clone(), janela));
    let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
    let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel();
    let cfg = SessionConfig { codec: CodecChoice::Auto, max_fps: 60, bitrate_kbps: 8000, max_width: 0, max_height: 0, monitor: 0, refine: opts.refine, chroma: opts.chroma, remote_size: opts.remote_size, desired: None };
    let link = Arc::new(Link::new(connected.conn.clone(), cfg.clone()));
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel::<Control>();
    // Resolução "igual à janela": a janela pede o tamanho e isso vira uma
    // SessionConfig nova (o host troca o modo de vídeo e recria a captura).
    let pedir_tamanho: Option<OnResize> = (opts.remote_size == RemoteSize::MatchWindow).then(|| {
        let tx = out_tx.clone();
        let link = link.clone();
        Box::new(move |want| {
            let _ = tx.send(Control::SessionConfig(link.want_size(want)));
        }) as OnResize
    });
    // Arquivos: os copiados aqui ficam em `files_offer` (o host pede quando
    // cola lá); os copiados lá são buscados por streams novos (`opener`).
    let files_offer: SharedOffer = Arc::default();
    let opener = link.opener();
    let handle = rt.handle().clone();
    let clipboard = viewer_clipboard(opts.clipboard, out_tx, files_offer.clone());
    rt.spawn(relay(in_rx, link.clone(), |l| &l.input));
    rt.spawn(relay(out_rx, link.clone(), |l| &l.control));
    link.start(&handle, connected, ev_tx.clone(), files_offer.clone());

    let titulo = format!("Reflex — {address}");
    let abertura = Abertura { titulo: titulo.clone(), saida: saida.clone(), input: in_tx, opcoes: opts, pedir_tamanho };
    poster.post(move |ui| (janela.abrir)(ui, abertura));

    // Sem imagem em 15 s (a janela aparece mesmo assim): avisa no status.
    let (frame_seen_tx, frame_seen_rx) = tokio::sync::oneshot::channel::<()>();
    let report_late = report.clone();
    let addr_late = address.to_string();
    rt.spawn(async move {
        if tokio::time::timeout(std::time::Duration::from_secs(15), frame_seen_rx).await.is_err() {
            report_late(&format!("{addr_late}: conectado, mas a máquina ainda não mandou imagem (tela desligada ou sem monitor?)"));
        }
    });
    let mut frame_seen_tx = Some(frame_seen_tx);
    let (saida_loop, addr_loop, report_loop, poster_loop) = (saida.clone(), address.to_string(), report.clone(), poster.clone());
    rt.spawn(async move {
        let saida = saida_loop;
        while let Some(ev) = ev_rx.recv().await {
            match ev {
                ViewerEvent::Frame(f) => {
                    // 1º quadro: o status confirma.
                    if let Some(tx) = frame_seen_tx.take() {
                        let _ = tx.send(());
                        report_loop(&format!("Conectado a {addr_loop} ({level:?})."));
                    }
                    if let Ok(mut g) = saida.quadro.lock() {
                        *g = Some(f);
                    }
                    saida.avisar();
                }
                // Cursor remoto: forma e posição chegam fora do vídeo.
                ViewerEvent::Cursor(shape) => {
                    if let Ok(mut c) = saida.cursor.lock() {
                        c.shape = Some(shape);
                    }
                }
                ViewerEvent::CursorPos { x, y, visible, .. } => {
                    if let Ok(mut c) = saida.cursor.lock() {
                        (c.x, c.y, c.visible, c.known) = (x as i32, y as i32, visible, true);
                    }
                    // Desenhado na imagem: pinta pra ele andar.
                    if opts.cursor.draws_remote() {
                        saida.avisar();
                    }
                }
                // Cópia lá: vai pro clipboard daqui (sem voltar como eco).
                ViewerEvent::ClipboardOffer(offer) => {
                    if let Some(w) = clipboard.as_ref().filter(|_| opts.clipboard.receives()) {
                        let snap = ClipSnapshot::from_offer(&offer);
                        if offer.files.is_empty() {
                            w.set(snap);
                        } else {
                            // Arquivos: colar no Explorer copia daqui de lá, com o progresso do Windows.
                            w.set_with_files(snap, Arc::new(RemoteFiles::new(offer.seq, opener.clone(), handle.clone())));
                        }
                    }
                }
                // Medidas do host: no log daqui (diagnóstico de desempenho).
                ViewerEvent::HostReport(line) => log::info!("host: {line}"),
                ViewerEvent::Disconnected(e) => {
                    log::info!("desconectado de {addr_loop}: {e}");
                    if !saida.aberta() {
                        break;
                    }
                    // Caiu com a janela aberta: tenta voltar (a máquina pode
                    // estar trocando de sessão — sair da conta, reiniciar o serviço).
                    report_loop(&format!("A conexão com {addr_loop} caiu; reconectando…"));
                    saida.titular(format!("{titulo} (reconectando…)"));
                    match reconnect(&addr_loop, &saida).await {
                        Ok(connected) => {
                            log::info!("reconectado a {addr_loop}");
                            report_loop(&format!("Conectado a {addr_loop} ({:?}).", connected.level));
                            saida.titular(titulo.clone());
                            link.start(&handle, connected, ev_tx.clone(), files_offer.clone());
                        }
                        Err(why) => {
                            if !saida.aberta() {
                                break;
                            }
                            report_loop(&format!("A conexão com {addr_loop} terminou: {e}"));
                            saida.titular(format!("{titulo} (desconectado)"));
                            mostrar_erro(&poster_loop, format!("A conexão com {addr_loop} terminou.\n\n{e}\n\nNão deu pra reconectar: {why}"));
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
    });

    // Até a janela fechar; o runtime (e a conexão) termina junto.
    rt.block_on(saida.fechada());
    true
}

/// Quanto tempo tentar voltar depois que a conexão cai (a troca de sessão
/// no Windows, ao sair da conta, leva uns segundos).
const RECONNECT_FOR: std::time::Duration = std::time::Duration::from_secs(60);

/// Tenta conectar de novo a cada segundo, enquanto a janela estiver aberta.
async fn reconnect(address: &str, saida: &Saida) -> Result<client::Connected, String> {
    let cert = viewer_cert()?;
    let deadline = tokio::time::Instant::now() + RECONNECT_FOR;
    let mut last = String::from("tempo esgotado");
    while tokio::time::Instant::now() < deadline && saida.aberta() {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        // O nome pode apontar pra outro endereço agora (a VM mudou de IP).
        let socket = match addr::resolve(address) {
            Ok(s) => s,
            Err(e) => {
                last = e;
                continue;
            }
        };
        let check: crate::net::ServerCheck = Arc::new(|_| true);
        match client::connect_and_auth(socket, &cert, check, TransportMode::Auto, env!("CARGO_PKG_VERSION"), crate::media::caps::decoders(), Credential::Key, || {}).await {
            Ok(c) => return Ok(c),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

/// A conexão atual, por onde a janela e o clipboard falam (troca ao reconectar).
struct Link {
    conn: Mutex<Arc<dyn crate::net::Connection>>,
    input: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Input>>>,
    control: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Control>>>,
    /// O pedido atual (com o tamanho da janela): a conexão nova começa com ele.
    cfg: Mutex<SessionConfig>,
}

impl Link {
    fn new(conn: Arc<dyn crate::net::Connection>, cfg: SessionConfig) -> Self {
        Self { conn: Mutex::new(conn), input: Mutex::new(None), control: Mutex::new(None), cfg: Mutex::new(cfg) }
    }

    /// Liga uma conexão (a 1ª ou a de uma reconexão) e põe o viewer pra rodar nela.
    fn start(&self, rt: &tokio::runtime::Handle, connected: client::Connected, events: tokio::sync::mpsc::UnboundedSender<ViewerEvent>, files: SharedOffer) {
        let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
        let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
        if let Ok(mut c) = self.conn.lock() {
            *c = connected.conn.clone();
        }
        if let Ok(mut i) = self.input.lock() {
            *i = Some(in_tx);
        }
        if let Ok(mut o) = self.control.lock() {
            *o = Some(out_tx);
        }
        let cfg = self.cfg.lock().map(|c| c.clone()).unwrap_or_default();
        rt.spawn(client::run(connected, cfg, events, in_rx, out_rx, files));
    }

    /// O pedido com o tamanho novo da janela (guardado pra próxima conexão).
    fn want_size(&self, want: DesiredDisplay) -> SessionConfig {
        let mut c = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        c.desired = Some(want);
        c.clone()
    }

    /// Cada pedido de arquivo abre um stream novo até o host (pela conexão atual).
    fn opener(self: &Arc<Self>) -> Opener {
        let me = self.clone();
        Arc::new(move || {
            let conn = me.conn.lock().ok().map(|c| c.clone());
            Box::pin(async move { conn.ok_or_else(|| std::io::Error::other("conexão indisponível"))?.open_bi().await })
        })
    }
}

/// Repassa o que a janela/clipboard mandam pra conexão atual.
async fn relay<T: Send + 'static>(mut rx: tokio::sync::mpsc::UnboundedReceiver<T>, link: Arc<Link>, slot: fn(&Link) -> &Mutex<Option<tokio::sync::mpsc::UnboundedSender<T>>>) {
    while let Some(msg) = rx.recv().await {
        if let Ok(tx) = slot(&link).lock()
            && let Some(tx) = tx.as_ref()
        {
            let _ = tx.send(msg);
        }
    }
}

/// Clipboard compartilhado (§10.7): observa o daqui e, se a opção manda,
/// oferece cada cópia à máquina remota. Desligado = nada.
fn viewer_clipboard(mode: ClipboardMode, out: tokio::sync::mpsc::UnboundedSender<Control>, files: SharedOffer) -> Option<ClipboardWatcher> {
    if mode == ClipboardMode::Off {
        return None;
    }
    let seq = std::sync::atomic::AtomicU64::new(0);
    ClipboardWatcher::spawn(move |snap| {
        if mode.sends() {
            let n = seq.fetch_add(1, Ordering::Relaxed) + 1;
            // Arquivos copiados: só os nomes vão; o conteúdo, quando o host colar.
            if let Ok(mut f) = files.lock() {
                *f = (!snap.files.is_empty()).then(|| Arc::new(FileOffer::new(n, snap.files.clone())));
            }
            let _ = out.send(Control::ClipboardOffer(snap.to_offer(n)));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_window_shows_remote_pixels_one_to_one() {
        let o = ViewOptions { remote_size: RemoteSize::MatchWindow, scale: ViewScale::Windows, ..Default::default() };
        assert_eq!(o.effective_scale(), ViewScale::Native);
        let o = ViewOptions { remote_size: RemoteSize::Keep, scale: ViewScale::Windows, ..Default::default() };
        assert_eq!(o.effective_scale(), ViewScale::Windows);
    }
}
