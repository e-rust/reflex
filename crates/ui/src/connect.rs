//! Página "Conectar" (§5.2): endereço + Conectar + status; sobe o viewer e a
//! janela de vídeo.

use std::sync::{Arc, Mutex};

use winsafe::co;
use winsafe::gui;
use winsafe::prelude::*;

use crate::addr;
use crate::theme;
use crate::identity::viewer_cert;
use crate::present::{self, FrameBuffer};
use reflex_net::TransportMode;
use reflex_proto::codec::CodecChoice;
use reflex_proto::msg::{Chroma, Control, Input, Refine, RemoteSize, SessionConfig};
use reflex_platform::clipboard::{ClipSnapshot, ClipboardWatcher};
use reflex_platform::files::{FileOffer, Opener, RemoteFiles, SharedOffer};
use reflex_viewer::client::{self, Credential, ViewerEvent};
use reflex_viewer::hosts::{ClipboardMode, CursorMode, Hosts, ViewScale};

/// `%LOCALAPPDATA%\Reflex\hosts.json`: hosts salvos e o último endereço.
fn hosts_file() -> std::path::PathBuf {
    reflex_platform::paths::user_dir().join("hosts.json")
}

/// Opções de imagem da aba Conectar (salvas; o `--connect` usa as mesmas).
/// Cada uma começa no comportamento original, exceto a nitidez (Padrão).
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
    fn save(self) {
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

/// Uma opção da aba: rótulo à esquerda, lista à direita, na linha `y`.
fn option_row<T: Copy + PartialEq>(wnd: &gui::WindowControl, text: &str, y: i32, all: &[T], label: fn(T) -> &'static str, current: T) -> (gui::Label, gui::ComboBox) {
    let hint = gui::Label::new(wnd, gui::LabelOpts { text, position: gui::dpi(24, y + 6), size: gui::dpi(180, 20), ..Default::default() });
    let labels: Vec<&str> = all.iter().map(|v| label(*v)).collect();
    let selected = all.iter().position(|v| *v == current).unwrap_or(0) as u32;
    let combo = gui::ComboBox::new(wnd, gui::ComboBoxOpts { position: gui::dpi(204, y), width: gui::dpi_x(240), items: &labels, selected_item: Some(selected), ..Default::default() });
    (hint, combo)
}

/// O valor escolhido na lista (a ordem é a de `all`).
fn chosen<T: Copy + Default>(combo: &gui::ComboBox, all: &[T]) -> T {
    combo.items().selected_index().and_then(|i| all.get(i as usize).copied()).unwrap_or_default()
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

#[derive(Clone)]
pub struct ConnectPage {
    wnd: gui::WindowControl,
    title: gui::Label,
    address: gui::Edit,
    connect: gui::Button,
    status: gui::Label,
    my_id: gui::Edit,
    refine: gui::ComboBox,
    scale: gui::ComboBox,
    remote_size: gui::ComboBox,
    chroma: gui::ComboBox,
    cursor: gui::ComboBox,
    clipboard: gui::ComboBox,
}

impl ConnectPage {
    pub fn new(parent: &(impl GuiParent + 'static), position: (i32, i32), size: (i32, i32)) -> Self {
        let wnd = gui::WindowControl::new(parent, theme::page_opts(position, size));
        let title = gui::Label::new(&wnd, gui::LabelOpts { text: "Conectar a uma máquina", position: gui::dpi(24, 18), size: gui::dpi(400, 34), ..Default::default() });
        let hint = gui::Label::new(&wnd, gui::LabelOpts { text: "Endereço (IP ou nome; porta opcional)", position: gui::dpi(24, 66), size: gui::dpi(300, 20), ..Default::default() });
        // ES_MULTILINE só pra poder centralizar o texto (ver theme::center_edit_text);
        // sem ES_WANTRETURN o Enter continua indo pro botão padrão.
        let last = Hosts::load(&hosts_file()).last_address.unwrap_or_default();
        let address = gui::Edit::new(
            &wnd,
            gui::EditOpts { text: &last, position: gui::dpi(24, 88), width: gui::dpi_x(300), height: gui::dpi_y(32), control_style: co::ES::AUTOHSCROLL | co::ES::MULTILINE, ..Default::default() },
        );
        let connect = gui::Button::new(
            &wnd,
            gui::ButtonOpts { text: "&Conectar", position: gui::dpi(334, 88), width: gui::dpi_x(110), height: gui::dpi_y(32), control_style: co::BS::DEFPUSHBUTTON, ..Default::default() },
        );
        let status = gui::Label::new(&wnd, gui::LabelOpts { text: "", position: gui::dpi(24, 128), size: gui::dpi(420, 44), ..Default::default() });

        // Meu ID (§11.1): o que se cadastra na outra máquina. Campo só leitura
        // pra dar pra selecionar e copiar.
        let my_id = viewer_cert().map(|c| reflex_auth::identity::AccessId::of_cert(&c.cert_der).to_string()).unwrap_or_else(|e| format!("indisponível: {e}"));
        let id_hint = gui::Label::new(&wnd, gui::LabelOpts { text: "Meu ID — cadastre na máquina que vai controlar", position: gui::dpi(24, 190), size: gui::dpi(420, 20), ..Default::default() });
        let my_id = gui::Edit::new(
            &wnd,
            gui::EditOpts {
                text: &my_id,
                position: gui::dpi(24, 212),
                width: gui::dpi_x(300),
                height: gui::dpi_y(32),
                control_style: co::ES::AUTOHSCROLL | co::ES::MULTILINE | co::ES::READONLY,
                ..Default::default()
            },
        );

        // Opções de imagem; o primeiro valor de cada uma é o comportamento original.
        let opts = ViewOptions::saved();
        let (refine_hint, refine) = option_row(&wnd, "Nitidez com a tela parada", 262, &Refine::ALL, Refine::label, opts.refine);
        let (scale_hint, scale) = option_row(&wnd, "Escala na janela", 298, &ViewScale::ALL, ViewScale::label, opts.scale);
        let (size_hint, remote_size) = option_row(&wnd, "Resolução remota", 334, &RemoteSize::ALL, RemoteSize::label, opts.remote_size);
        let (chroma_hint, chroma) = option_row(&wnd, "Cor", 370, &Chroma::ALL, Chroma::label, opts.chroma);
        let (cursor_hint, cursor) = option_row(&wnd, "Cursor", 406, &CursorMode::ALL, CursorMode::label, opts.cursor);
        let (clip_hint, clipboard) = option_row(&wnd, "Clipboard", 442, &ClipboardMode::ALL, ClipboardMode::label, opts.clipboard);

        theme::white_page(&wnd, vec![hint, id_hint, refine_hint, scale_hint, size_hint, chroma_hint, cursor_hint, clip_hint, status.clone()]);
        let me = Self { wnd, title, address, connect, status, my_id, refine, scale, remote_size, chroma, cursor, clipboard };
        me.events();
        me
    }

    pub fn set_visible(&self, visible: bool) {
        theme::set_visible(&self.wnd, visible);
    }

    /// Fonte de título (chamada depois da fonte de texto, que vale pra tudo).
    pub fn apply_titles(&self) {
        theme::apply_title_font(&self.title);
        // Texto centralizado nos campos (depois da fonte, que desfaz o retângulo).
        theme::center_edit_text(&self.address);
        theme::center_edit_text(&self.my_id);
    }

    fn events(&self) {
        let me = self.clone();
        self.connect.on().bn_clicked(move || {
            me.on_connect();
            Ok(())
        });
        // Enter no campo conecta. A janela não é um diálogo, então não há "botão
        // padrão" automático: o campo (subclasse) pede todas as teclas ao
        // gerenciador de diálogo e trata o Enter ele mesmo.
        self.address.on_subclass().wm_get_dlg_code(|_| Ok(unsafe { co::DLGC::from_raw(co::DLGC::WANTALLKEYS.raw() | co::DLGC::HASSETSEL.raw() | co::DLGC::WANTCHARS.raw()) }));
        let me = self.clone();
        self.address.on_subclass().wm_key_down(move |p| {
            if p.vkey_code == co::VK::RETURN {
                me.on_connect();
            }
            Ok(())
        });
        // O campo é multilinha só pra centralizar o texto: quebra de linha que
        // chegue (o Enter acima, ou uma colagem) é tirada.
        let me = self.clone();
        self.address.on().en_change(move || {
            let text = me.address.text().unwrap_or_default();
            if text.contains(['\r', '\n']) {
                me.address.set_text(&single_line(&text))?;
            }
            Ok(())
        });
    }

    fn set_status(&self, text: &str) {
        let _ = self.status.hwnd().SetWindowText(text);
    }

    fn on_connect(&self) {
        let raw = self.address.text().unwrap_or_default();
        let Some(address) = addr::normalize_address(&raw) else {
            self.set_status("Informe o endereço da máquina.");
            return;
        };
        self.set_status(&format!("Conectando a {address}…"));
        // A linha de status mostra o resultado (escrita pela thread do viewer:
        // o SetWindowText chega à janela pela fila de mensagens dela).
        let opts = ViewOptions {
            refine: chosen(&self.refine, &Refine::ALL),
            scale: chosen(&self.scale, &ViewScale::ALL),
            chroma: chosen(&self.chroma, &Chroma::ALL),
            remote_size: chosen(&self.remote_size, &RemoteSize::ALL),
            cursor: chosen(&self.cursor, &CursorMode::ALL),
            clipboard: chosen(&self.clipboard, &ClipboardMode::ALL),
        };
        opts.save();
        let status = self.status.hwnd().ptr() as isize;
        let report: Report = Arc::new(move |text: &str| {
            let label = unsafe { winsafe::HWND::from_ptr(status as *mut _) };
            let _ = label.SetWindowText(text);
        });
        spawn_viewer(address, opts, report);
    }
}

/// Onde o viewer conta o que está acontecendo: a linha de status da aba
/// Conectar (no `reflex --connect`, em lugar nenhum).
pub type Report = Arc<dyn Fn(&str) + Send + Sync>;

/// Sobe o viewer numa thread (a janela principal continua respondendo).
fn spawn_viewer(address: String, opts: ViewOptions, report: Report) {
    std::thread::spawn(move || run_viewer(&address, opts, report));
}

/// Erro de conexão: na linha de status, no log e numa caixa de mensagem.
fn fail(report: &Report, address: &str, error: &str) -> i32 {
    log::error!("conexão com {address}: {error}");
    report(&format!("Não foi possível conectar a {address}: {error}"));
    show_error(&format!("Não foi possível conectar a {address}.\n\n{error}"));
    1
}

/// Resolve `address` (`ip:porta` ou `nome:porta`), conecta, autentica e
/// mostra a janela de vídeo até ela fechar (bloqueante). Devolve o código de
/// saída (0 = ok).
pub fn run_viewer(address: &str, opts: ViewOptions, report: Report) -> i32 {
    let socket = match addr::resolve(address) {
        Ok(s) => s,
        Err(e) => return fail(&report, address, &e),
    };
    log::info!("{address} → {socket}");
    let cert = match viewer_cert() {
        Ok(c) => c,
        Err(e) => return fail(&report, address, &format!("identidade deste viewer indisponível ({e})")),
    };
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            log::error!("runtime: {e}");
            return 1;
        }
    };
    // 1. Conecta e autentica antes de abrir a janela: erro vira mensagem.
    // TOFU no MVP: aceita o certificado (a checagem de mudança entra com a
    // lista salva de hosts).
    let check: reflex_net::ServerCheck = Arc::new(|_| true);
    let decoders = reflex_media::caps::decoders();
    let awaiting = || report(&format!("{address}: aguardando alguém na máquina permitir o acesso…"));
    let connected = rt.block_on(client::connect_and_auth(socket, &cert, check, TransportMode::Auto, env!("CARGO_PKG_VERSION"), decoders, Credential::Key, awaiting));
    let connected = match connected {
        Ok(c) => c,
        Err(e) => return fail(&report, address, &e.to_string()),
    };
    log::info!("conectado a {address} como {:?}", connected.level);
    let level = connected.level;
    report(&format!("Conectado a {address}; recebendo a imagem…"));
    remember_last(address);

    // 2. Laços do viewer no runtime; a janela de vídeo pinta o buffer. A
    // janela, o clipboard e o tamanho pedido falam com a conexão ATUAL (`link`):
    // se ela cair (sair da conta no Windows recria a sessão de lá), outra entra
    // no lugar sem fechar a janela.
    let buffer = Arc::new(FrameBuffer::default());
    let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
    let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel();
    let cfg = SessionConfig { codec: CodecChoice::Auto, max_fps: 60, bitrate_kbps: 8000, max_width: 0, max_height: 0, monitor: 0, refine: opts.refine, chroma: opts.chroma, remote_size: opts.remote_size, desired: None };
    let link = Arc::new(Link::new(connected.conn.clone(), cfg.clone()));
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel::<Control>();
    // Resolução "igual à janela": a janela de vídeo pede o tamanho e isso vira
    // uma SessionConfig nova (o host troca o modo de vídeo e recria a captura).
    let on_resize: Option<present::OnResize> = (opts.remote_size == RemoteSize::MatchWindow).then(|| {
        let tx = out_tx.clone();
        let link = link.clone();
        Box::new(move |want| {
            let _ = tx.send(Control::SessionConfig(link.want_size(want)));
        }) as present::OnResize
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
    let buf_for_loop = buffer.clone();
    let addr_for_loop = address.to_string();
    let report_loop = report.clone();
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
    let title = format!("Reflex — {address}");
    let title_loop = title.clone();
    rt.spawn(async move {
        while let Some(ev) = ev_rx.recv().await {
            match ev {
                ViewerEvent::Frame(f) => {
                    // 1º quadro: a janela aparece agora; o status confirma.
                    if let Some(tx) = frame_seen_tx.take() {
                        let _ = tx.send(());
                        report_loop(&format!("Conectado a {addr_for_loop} ({level:?})."));
                    }
                    if let Ok(mut g) = buf_for_loop.frame.lock() {
                        *g = Some(f);
                    }
                    buf_for_loop.notify();
                }
                // Cursor remoto: forma e posição chegam fora do vídeo.
                ViewerEvent::Cursor(shape) => {
                    if let Ok(mut c) = buf_for_loop.cursor.lock() {
                        c.shape = Some(shape);
                    }
                }
                ViewerEvent::CursorPos { x, y, visible, .. } => {
                    if let Ok(mut c) = buf_for_loop.cursor.lock() {
                        (c.x, c.y, c.visible, c.known) = (x as i32, y as i32, visible, true);
                    }
                    // "Os dois": o remoto é desenhado na imagem; repinta pra ele andar.
                    if opts.cursor == reflex_viewer::hosts::CursorMode::Both {
                        buf_for_loop.notify();
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
                    log::info!("desconectado de {addr_for_loop}: {e}");
                    if !buf_for_loop.is_open() {
                        break;
                    }
                    // Caiu com a janela aberta: tenta voltar (a máquina pode
                    // estar trocando de sessão — sair da conta, reiniciar o serviço).
                    report_loop(&format!("A conexão com {addr_for_loop} caiu; reconectando…"));
                    buf_for_loop.set_title(&format!("{title_loop} (reconectando…)"));
                    match reconnect(&addr_for_loop, &buf_for_loop).await {
                        Ok(connected) => {
                            log::info!("reconectado a {addr_for_loop}");
                            report_loop(&format!("Conectado a {addr_for_loop} ({:?}).", connected.level));
                            buf_for_loop.set_title(&title_loop);
                            link.start(&handle, connected, ev_tx.clone(), files_offer.clone());
                        }
                        Err(why) => {
                            if !buf_for_loop.is_open() {
                                break;
                            }
                            report_loop(&format!("A conexão com {addr_for_loop} terminou: {e}"));
                            buf_for_loop.set_title(&format!("{title_loop} (desconectado)"));
                            show_error(&format!("A conexão com {addr_for_loop} terminou.\n\n{e}\n\nNão deu pra reconectar: {why}"));
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
    });

    present::run(&title, buffer, in_tx, opts.effective_scale(), opts.cursor, on_resize);
    0
}

/// Quanto tempo tentar voltar depois que a conexão cai (a troca de sessão
/// no Windows, ao sair da conta, leva uns segundos).
const RECONNECT_FOR: std::time::Duration = std::time::Duration::from_secs(60);

/// Tenta conectar de novo a cada segundo, enquanto a janela estiver aberta.
async fn reconnect(address: &str, buffer: &FrameBuffer) -> Result<client::Connected, String> {
    let cert = viewer_cert()?;
    let deadline = tokio::time::Instant::now() + RECONNECT_FOR;
    let mut last = String::from("tempo esgotado");
    while tokio::time::Instant::now() < deadline && buffer.is_open() {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        // O nome pode apontar pra outro endereço agora (a VM mudou de IP).
        let socket = match addr::resolve(address) {
            Ok(s) => s,
            Err(e) => {
                last = e;
                continue;
            }
        };
        let check: reflex_net::ServerCheck = Arc::new(|_| true);
        match client::connect_and_auth(socket, &cert, check, TransportMode::Auto, env!("CARGO_PKG_VERSION"), reflex_media::caps::decoders(), Credential::Key, || {}).await {
            Ok(c) => return Ok(c),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

/// A conexão atual, por onde a janela e o clipboard falam (troca ao reconectar).
struct Link {
    conn: Mutex<Arc<dyn reflex_net::Connection>>,
    input: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Input>>>,
    control: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Control>>>,
    /// O pedido atual (com o tamanho da janela): a conexão nova começa com ele.
    cfg: Mutex<SessionConfig>,
}

impl Link {
    fn new(conn: Arc<dyn reflex_net::Connection>, cfg: SessionConfig) -> Self {
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
    fn want_size(&self, want: reflex_proto::msg::DesiredDisplay) -> SessionConfig {
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
        if let Ok(tx) = slot(&link).lock() {
            if let Some(tx) = tx.as_ref() {
                let _ = tx.send(msg);
            }
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
            let n = seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            // Arquivos copiados: só os nomes vão; o conteúdo, quando o host colar.
            if let Ok(mut f) = files.lock() {
                *f = (!snap.files.is_empty()).then(|| Arc::new(FileOffer::new(n, snap.files.clone())));
            }
            let _ = out.send(Control::ClipboardOffer(snap.to_offer(n)));
        }
    })
}


/// Texto sem quebras de linha (o que se cola num campo de uma linha). Pura.
fn single_line(text: &str) -> String {
    text.split(['\r', '\n']).collect()
}

/// Caixa de erro (pode ser chamada de qualquer thread). Sem janela dona, ela
/// abria atrás da principal e parecia que nada acontecia: vem pra frente.
fn show_error(text: &str) {
    let _ = winsafe::HWND::NULL.MessageBox(text, "Reflex", co::MB::ICONERROR | co::MB::SETFOREGROUND | co::MB::TOPMOST);
}

#[cfg(test)]
mod tests {
    #[test]
    fn line_breaks_are_removed() {
        assert_eq!(super::single_line("192.168.0.5:47800\r\n"), "192.168.0.5:47800");
        assert_eq!(super::single_line("10.0.\n0.1"), "10.0.0.1");
    }
}
