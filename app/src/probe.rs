//! `reflex --probe <ip:porta> [segundos] [arquivo.png] [--win | --tap=0x39]` — viewer sem janela,
//! pra testar uma máquina: conecta com o ID deste usuário, recebe vídeo por
//! alguns segundos, mostra quadros/s e resolução e salva o último quadro em
//! PNG (dá pra conferir o que o host está mostrando, inclusive a tela de login).
//! Com `--tap=<scancode>` (ou `--win`, a tecla Windows), aperta a tecla no host
//! depois de 1 s (testa o input, inclusive na tela de bloqueio).
//! `--motion` abre/fecha o menu Iniciar (animação curta); `--drag` arrasta um
//! retângulo de seleção em círculo na área de trabalho — a tela muda sem
//! parar, então os quadros/s medem o motor e não o conteúdo.
//! `--arquivos=CAMINHO` testa a cópia de arquivos nos dois sentidos: cola
//! CAMINHO numa pasta de lá (Win+R e Ctrl+V), pede os hashes do que chegou e
//! depois copia a pasta de lá e busca tudo pra cá.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reflex_auth::identity::AccessId;
use reflex_media::decode::BgraFrame;
use reflex_net::{LocalCert, ServerCheck, TransportMode};
use reflex_platform::dpapi::Scope;
use reflex_platform::paths;
use reflex_proto::codec::CodecChoice;
use reflex_proto::msg::SessionConfig;
use reflex_viewer::client::{self, Credential, ViewerEvent};

use crate::identity_store::load_or_create_identity;

pub fn run(all: &[String]) -> Result<(), String> {
    let tap = tap_key(all);
    let motion = all.iter().any(|a| a == "--motion");
    let typing = all.iter().any(|a| a == "--typing");
    let drag = all.iter().any(|a| a == "--drag");
    let refine = match all.iter().find_map(|a| a.strip_prefix("--refine=")) {
        Some(v) => reflex_proto::msg::Refine::parse(v).ok_or_else(|| format!("--refine={v}: use off, leve, padrao ou max"))?,
        None => reflex_proto::msg::Refine::default(),
    };
    let chroma = match all.iter().find_map(|a| a.strip_prefix("--cor=")) {
        Some("444") => reflex_proto::msg::Chroma::Yuv444,
        Some("420") | None => reflex_proto::msg::Chroma::Yuv420,
        Some(v) => return Err(format!("--cor={v}: use 420 ou 444")),
    };
    let desired = match all.iter().find_map(|a| a.strip_prefix("--tamanho=")) {
        Some(v) => Some(parse_size(v).ok_or_else(|| format!("--tamanho={v}: use LARGURAxALTURA@ESCALA, ex.: 1536x864@150"))?),
        None => None,
    };
    let args: Vec<String> = all.iter().filter(|a| !a.starts_with("--")).cloned().collect();
    let addr = args.first().ok_or("uso: reflex --probe <ip:porta | nome:porta> [segundos] [arquivo.png]")?;
    let addr = reflex_ui::addr::normalize_address(addr).ok_or("endereço vazio")?;
    let socket = reflex_ui::addr::resolve(&addr)?;
    if addr.parse::<std::net::SocketAddr>().is_err() {
        println!("{addr} → {socket}");
    }
    let seconds: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5);
    let out = args.get(2).cloned().unwrap_or_else(|| "reflex-probe.png".into());

    let id = load_or_create_identity(&paths::user_dir().join("identity.bin"), "Reflex Viewer", Scope::User)?;
    println!("Meu ID: {}", id.access_id());
    let cert = LocalCert { cert_der: id.cert_der, key_der: id.key_der };

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
    // --depois: o pedido de tamanho sai no meio da sessão (como a janela faz).
    let late = all.iter().any(|a| a == "--depois");
    // --copiar=TEXTO: depois de 2 s, "copia" TEXTO deste lado (vai pro clipboard de lá).
    let copy = all.iter().find_map(|a| a.strip_prefix("--copiar=")).map(str::to_string);
    // --ponto=0.5,0.3: leva o mouse até lá (fração da tela) depois de 2 s.
    let point = all.iter().find_map(|a| a.strip_prefix("--ponto=")).and_then(|v| {
        let (x, y) = v.split_once(',')?;
        Some(((x.parse::<f64>().ok()? * 65535.0) as u16, (y.parse::<f64>().ok()? * 65535.0) as u16))
    });
    let files = all.iter().find_map(|a| a.strip_prefix("--arquivos=")).map(std::path::PathBuf::from);
    let paste = all.iter().any(|a| a == "--colar");
    let run_cmd = all.iter().find_map(|a| a.strip_prefix("--executar=")).map(str::to_string);
    let ctrl_v = all.iter().any(|a| a == "--ctrl-v");
    let numpad = all.iter().any(|a| a == "--numpad");
    let scenario = Scenario { tap, motion, typing, drag, refine, chroma, desired, late, point, copy, files, paste, run_cmd, ctrl_v, numpad };
    rt.block_on(probe(socket, cert, Duration::from_secs(seconds), &out, scenario))
}

/// O que o probe faz no host enquanto mede.
struct Scenario {
    tap: Option<(u16, bool)>,
    motion: bool,
    typing: bool,
    drag: bool,
    refine: reflex_proto::msg::Refine,
    chroma: reflex_proto::msg::Chroma,
    /// Resolução "igual à janela" com este tamanho.
    desired: Option<reflex_proto::msg::DesiredDisplay>,
    late: bool,
    point: Option<(u16, u16)>,
    copy: Option<String>,
    /// Arquivo ou pasta pra colar lá (e buscar de volta).
    files: Option<std::path::PathBuf>,
    paste: bool,
    run_cmd: Option<String>,
    ctrl_v: bool,
    numpad: bool,
}

/// Passo do roteiro de `--arquivos`.
enum Step {
    /// Copia este texto aqui (vai pro clipboard de lá).
    Text(String),
    /// Copia estes arquivos aqui.
    Files(std::path::PathBuf),
    /// Teclas apertadas juntas (scancode, estendida) e soltas na ordem inversa.
    Chord(&'static [(u16, bool)]),
    /// Estado das travas como o viewer manda (Caps, Num, Scroll).
    Locks(bool, bool, bool),
}

const WIN_R: &[(u16, bool)] = &[(0x5B, true), (0x13, false)];
const CTRL_V: &[(u16, bool)] = &[(0x1D, false), (0x2F, false)];
const ENTER: &[(u16, bool)] = &[(0x1C, false)];
const CTRL_A: &[(u16, bool)] = &[(0x1D, false), (0x1E, false)];
const CTRL_C: &[(u16, bool)] = &[(0x1D, false), (0x2E, false)];
const ESC: &[(u16, bool)] = &[(0x01, false)];

/// Pasta de lá onde os arquivos são colados.
const REMOTE_DIR: &str = r"C:\Users\Public\reflex-teste";

/// Roteiro de `--arquivos`: (quando, passo). Comandos rodam pelo Executar
/// (Win+R, Ctrl+V do texto, Enter) — só teclas iguais em qualquer layout.
fn files_script(local: &std::path::Path) -> Vec<(Duration, Step)> {
    let s = Duration::from_secs;
    let run = |at: u64, cmd: String| vec![(s(at), Step::Text(cmd)), (s(at + 1), Step::Chord(WIN_R)), (s(at + 3), Step::Chord(CTRL_V)), (s(at + 4), Step::Chord(ENTER))];
    let mut steps = run(2, format!("cmd /c rmdir /s /q {REMOTE_DIR} & mkdir {REMOTE_DIR} & start explorer {REMOTE_DIR}"));
    steps.push((s(10), Step::Files(local.to_path_buf())));
    steps.push((s(12), Step::Chord(CTRL_V)));
    let hashes = format!("powershell -w hidden -c \"Get-ChildItem {REMOTE_DIR} -Recurse -File | Get-FileHash -Algorithm SHA256 | % {{ $_.Hash.Substring(0,16) + ' ' + $_.Path }} | Out-String | Set-Clipboard\"");
    steps.extend(run(40, hashes));
    steps.extend(run(55, format!("powershell -w hidden -c \"Set-Clipboard -Path {REMOTE_DIR}\"")));
    steps
}

/// Busca todos os arquivos que o outro lado copiou pra `dest`: (arquivos, bytes).
fn fetch_all(files: &reflex_platform::files::RemoteFiles, dest: &std::path::Path) -> std::io::Result<(usize, u64)> {
    let tree = files.tree()?;
    let (mut count, mut bytes) = (0, 0);
    for (i, e) in tree.iter().enumerate() {
        let path = dest.join(&e.rel_path);
        if e.is_dir {
            std::fs::create_dir_all(&path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let (_, mut remote) = files.open(i as u32, 0)?;
        bytes += std::io::copy(&mut remote, &mut std::fs::File::create(&path)?)?;
        count += 1;
    }
    Ok((count, bytes))
}

/// `1536x864@150` → tamanho e escala. Pura.
fn parse_size(text: &str) -> Option<reflex_proto::msg::DesiredDisplay> {
    let (size, scale) = text.split_once('@').unwrap_or((text, "100"));
    let (w, h) = size.split_once('x')?;
    Some(reflex_proto::msg::DesiredDisplay { width: w.parse().ok()?, height: h.parse().ok()?, scale_pct: scale.parse().ok()? })
}

async fn probe(socket: std::net::SocketAddr, cert: LocalCert, duration: Duration, out: &str, scenario: Scenario) -> Result<(), String> {
    let Scenario { tap, motion, typing, drag, refine, chroma, desired, late, point, copy, files, paste, run_cmd, ctrl_v, numpad } = scenario;
    let verbose = files.is_some() || paste || run_cmd.is_some();
    let mut steps = files.as_deref().map(files_script).unwrap_or_default();
    // --executar=CMD: roda CMD lá pelo Executar (Win+R, cola, Enter).
    if let Some(cmd) = run_cmd {
        steps = vec![(Duration::from_secs(2), Step::Text(cmd)), (Duration::from_secs(3), Step::Chord(WIN_R)), (Duration::from_secs(5), Step::Chord(CTRL_V)), (Duration::from_secs(6), Step::Chord(ENTER))];
    }
    // --numpad: tira a cortina da tela de bloqueio, liga o Num Lock de lá
    // (como a janela faz ao ganhar o foco) e digita 1 2 3 pelo teclado
    // numérico; a foto sai antes de apagar.
    if numpad {
        let ms = Duration::from_millis;
        steps = vec![(ms(1000), Step::Chord(&[(0x39, false)])), (ms(3000), Step::Locks(false, true, false)), (ms(3500), Step::Chord(&[(0x4F, false)])), (ms(3700), Step::Chord(&[(0x50, false)])), (ms(3900), Step::Chord(&[(0x51, false)]))];
    }
    // --ctrl-v: só aperta Ctrl+V lá (cola onde estiver o foco).
    if ctrl_v {
        steps = vec![(Duration::from_secs(2), Step::Chord(CTRL_V))];
    }
    // --colar: abre o Executar lá e cola (Ctrl+V) o que estiver no clipboard de lá.
    if paste {
        // Depois seleciona e copia o que colou: volta pra cá como cópia de lá.
        let ms = Duration::from_millis;
        steps = vec![(ms(2000), Step::Chord(WIN_R)), (ms(3500), Step::Chord(CTRL_V)), (ms(4500), Step::Chord(CTRL_A)), (ms(5000), Step::Chord(CTRL_C)), (ms(6000), Step::Chord(ESC))];
    }
    steps.reverse();
    let mut seq = 0u64;
    let offer: reflex_platform::files::SharedOffer = Arc::default();
    let mut fetches = Vec::new();
    let mut copy = copy;
    let mut point = point;
    // TOFU: aceita e mostra a impressão digital do host pra conferir.
    let seen = Arc::new(Mutex::new(None::<String>));
    let seen2 = seen.clone();
    let check: ServerCheck = Arc::new(move |der| {
        *seen2.lock().expect("mutex") = Some(AccessId::of_cert(der).to_string());
        true
    });

    let t0 = Instant::now();
    let decoders = reflex_media::caps::decoders();
    let connected = client::connect_and_auth(socket, &cert, check, TransportMode::Auto, env!("CARGO_PKG_VERSION"), decoders, Credential::Key, || println!("aguardando confirmação na máquina…"))
        .await
        .map_err(|e| format!("conexão: {e}"))?;
    println!("ID do host: {}", seen.lock().expect("mutex").clone().unwrap_or_default());
    println!("Conectado em {:?} via {:?}, nível {:?}", t0.elapsed(), connected.conn.transport(), connected.level);

    let conn = connected.conn.clone();
    let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel();
    let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
    let remote_size = if desired.is_some() { reflex_proto::msg::RemoteSize::MatchWindow } else { reflex_proto::msg::RemoteSize::Keep };
    let cfg = SessionConfig { codec: CodecChoice::Auto, max_fps: 30, bitrate_kbps: 6000, max_width: 0, max_height: 0, monitor: 0, refine, chroma, desired: desired.filter(|_| !late), remote_size };
    let (cfg_tx, cfg_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut late_cfg = desired.filter(|_| late).map(|d| SessionConfig { desired: Some(d), ..cfg.clone() });
    let session = tokio::spawn(client::run(connected, cfg, ev_tx, in_rx, cfg_rx, offer.clone()));
    let mut sizes: Vec<(u32, u32)> = Vec::new();
    let (mut cursor_shapes, mut cursor_moves, mut last_cursor) = (0u32, 0u32, None);

    let start = Instant::now();
    let mut frames = 0u32;
    let mut first_at = None;
    let mut last: Option<BgraFrame> = None;
    let mut pending = tap;
    let mut offset: Option<i64> = None;
    let mut timings: Vec<(u64, u64, u64)> = Vec::new();
    let mut next_motion = Duration::from_secs(1);
    // --typing: abre o menu Iniciar e digita "a"/Backspace na busca a cada
    // 100 ms; mede de cada tecla até o 1º quadro que chega depois dela.
    let mut script = if typing { typing_script(duration) } else { Vec::new() };
    script.reverse();
    let mut key_sent: Vec<u64> = Vec::new();
    let mut arrivals: Vec<u64> = Vec::new();
    while start.elapsed() < duration {
        if start.elapsed() >= Duration::from_secs(2) {
            if let Some(text) = copy.take() {
                println!("copiado deste lado: {text:?}");
                let snap = reflex_platform::clipboard::ClipSnapshot::text(&text);
                let _ = cfg_tx.send(reflex_proto::msg::Control::ClipboardOffer(snap.to_offer(1)));
            }
            if let Some((x, y)) = point.take() {
                println!("mouse levado a {x},{y} aos {:?}", start.elapsed());
                let _ = in_tx.send(reflex_proto::msg::Input::Mouse { x, y, buttons: reflex_proto::msg::MouseButtons(0) });
            }
            if let Some(c) = late_cfg.take() {
                println!("pedido de tamanho enviado aos {:?}", start.elapsed());
                let _ = cfg_tx.send(reflex_proto::msg::Control::SessionConfig(c));
            }
        }
        while steps.last().is_some_and(|(at, _)| start.elapsed() >= *at) {
            let (at, step) = steps.pop().expect("conferido acima");
            seq += 1;
            match step {
                Step::Text(t) => {
                    println!("{at:?}: copiado deste lado: {t}");
                    let _ = cfg_tx.send(reflex_proto::msg::Control::ClipboardOffer(reflex_platform::clipboard::ClipSnapshot::text(&t).to_offer(seq)));
                }
                Step::Files(path) => {
                    println!("{at:?}: arquivos copiados deste lado: {}", path.display());
                    *offer.lock().expect("mutex") = Some(Arc::new(reflex_platform::files::FileOffer::new(seq, vec![path.clone()])));
                    let snap = reflex_platform::clipboard::ClipSnapshot { items: Vec::new(), files: vec![path] };
                    let _ = cfg_tx.send(reflex_proto::msg::Control::ClipboardOffer(snap.to_offer(seq)));
                }
                Step::Locks(caps, num, scroll) => {
                    let _ = in_tx.send(reflex_proto::msg::Input::Locks { caps, num, scroll });
                }
                Step::Chord(keys) => {
                    for &(scancode, extended) in keys {
                        let _ = in_tx.send(reflex_proto::msg::Input::Key { scancode, extended, down: true });
                    }
                    for &(scancode, extended) in keys.iter().rev() {
                        let _ = in_tx.send(reflex_proto::msg::Input::Key { scancode, extended, down: false });
                    }
                }
            }
        }
        while script.last().is_some_and(|(at, _)| start.elapsed() >= *at) {
            let (_, (scancode, extended, measure)) = script.pop().expect("conferido acima");
            let _ = in_tx.send(reflex_proto::msg::Input::Key { scancode, extended, down: true });
            let _ = in_tx.send(reflex_proto::msg::Input::Key { scancode, extended, down: false });
            if measure {
                key_sent.push(reflex_media::clock::now_ns());
            }
        }
        if motion && start.elapsed() >= next_motion {
            // Abre/fecha o menu Iniciar: animação pra medir quadros em movimento.
            let _ = in_tx.send(reflex_proto::msg::Input::Key { scancode: 0x5B, extended: true, down: true });
            let _ = in_tx.send(reflex_proto::msg::Input::Key { scancode: 0x5B, extended: true, down: false });
            next_motion += Duration::from_millis(700);
        }
        if drag && start.elapsed() >= Duration::from_secs(1) {
            let (x, y) = drag_point(start.elapsed() - Duration::from_secs(1));
            let _ = in_tx.send(reflex_proto::msg::Input::Mouse { x, y, buttons: reflex_proto::msg::MouseButtons(reflex_proto::msg::MouseButtons::LEFT) });
        }
        if let Some((scancode, extended)) = pending.filter(|_| start.elapsed() > Duration::from_secs(1)) {
            let _ = in_tx.send(reflex_proto::msg::Input::Key { scancode, extended, down: true });
            let _ = in_tx.send(reflex_proto::msg::Input::Key { scancode, extended, down: false });
            println!("tecla 0x{scancode:02X} enviada");
            pending = None;
        }
        match tokio::time::timeout(Duration::from_millis(10), ev_rx.recv()).await {
            Ok(Some(ViewerEvent::Frame(f))) => {
                frames += 1;
                if sizes.last() != Some(&(f.width, f.height)) {
                    println!("{:?}: imagem {}x{}", start.elapsed(), f.width, f.height);
                    sizes.push((f.width, f.height));
                }
                first_at.get_or_insert(start.elapsed());
                last = Some(f);
            }
            Ok(Some(ViewerEvent::Disconnected(e))) => {
                println!("desconectado: {e}");
                break;
            }
            Ok(Some(ViewerEvent::Clock { rtt_us, offset_ns })) => {
                if offset.is_none() {
                    println!("ping: {:.1} ms ida e volta", rtt_us as f64 / 1000.0);
                }
                offset = Some(offset_ns);
            }
            Ok(Some(ViewerEvent::Timing { capture_ns, received_ns, decoded_ns })) => {
                timings.push((capture_ns, received_ns, decoded_ns));
                arrivals.push(decoded_ns);
            }
            Ok(Some(ViewerEvent::Cursor(c))) => {
                cursor_shapes += 1;
                println!("{:?}: forma de cursor {}x{} (ponto de clique {},{})", start.elapsed(), c.width, c.height, c.hot_x, c.hot_y);
            }
            Ok(Some(ViewerEvent::ClipboardOffer(o))) => {
                let snap = reflex_platform::clipboard::ClipSnapshot::from_offer(&o);
                let limit = if verbose { 4000 } else { 60 };
                let text: Option<String> = snap.as_text().map(|t| t.chars().take(limit).collect());
                let formats: Vec<String> = o.items.iter().map(|i| format!("{:?}={}B", i.format, i.data.len())).collect();
                println!("{:?}: clipboard de lá: texto {:?} | formatos {} | arquivos {:?}", start.elapsed(), text, formats.join(", "), o.files);
                if !o.files.is_empty() && files.is_some() {
                    // Arquivos copiados lá: busca tudo (como o Explorer faria ao colar aqui).
                    let c = conn.clone();
                    let opener: reflex_platform::files::Opener = Arc::new(move || {
                        let c = c.clone();
                        Box::pin(async move { c.open_bi().await })
                    });
                    let remote = reflex_platform::files::RemoteFiles::new(o.seq, opener, tokio::runtime::Handle::current());
                    let dest = std::env::temp_dir().join(format!("reflex-recebido-{}", o.seq));
                    let _ = std::fs::remove_dir_all(&dest);
                    fetches.push(tokio::task::spawn_blocking(move || {
                        let t = Instant::now();
                        let got = fetch_all(&remote, &dest);
                        (got, t.elapsed(), dest)
                    }));
                }
            }
            Ok(Some(ViewerEvent::HostReport(line))) => println!("{:?}: host: {line}", start.elapsed()),
            Ok(Some(ViewerEvent::CursorPos { x, y, visible, .. })) => {
                cursor_moves += 1;
                last_cursor = Some((x, y, visible));
            }
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => break,
        }
    }
    if drag {
        let (x, y) = drag_point(start.elapsed());
        let _ = in_tx.send(reflex_proto::msg::Input::Mouse { x, y, buttons: reflex_proto::msg::MouseButtons(0) });
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    for f in fetches {
        match f.await {
            Ok((Ok((count, bytes)), took, dest)) => println!(
                "recebido de lá: {count} arquivo(s), {:.1} MB em {took:?} ({:.1} MB/s) → {}",
                bytes as f64 / 1e6,
                bytes as f64 / 1e6 / took.as_secs_f64().max(0.001),
                dest.display()
            ),
            Ok((Err(e), _, _)) => println!("busca dos arquivos de lá falhou: {e}"),
            Err(e) => println!("busca dos arquivos de lá: {e}"),
        }
    }
    session.abort();
    // Avisa o host (senão ele só solta a sessão quando o QUIC expira).
    conn.close("fim do probe");
    tokio::time::sleep(Duration::from_millis(150)).await;

    let Some(f) = last else { return Err("nenhum quadro recebido".into()) };
    println!("1º quadro após {:?}; {frames} quadros em {:?} ({:.1}/s); resolução {}x{}", first_at.unwrap_or_default(), start.elapsed(), frames as f64 / start.elapsed().as_secs_f64(), f.width, f.height);
    if let Some(off) = offset {
        print_latency(&timings, off);
    }
    println!("cursor: {cursor_shapes} forma(s), {cursor_moves} posição(ões), última {last_cursor:?}");
    if typing {
        let mut lat: Vec<f64> = key_to_frame(&key_sent, &arrivals).into_iter().map(|ns| ns as f64 / 1e6).collect();
        lat.sort_by(f64::total_cmp);
        println!(
            "tecla→tela: p50 {:.1} ms, p90 {:.1} ms, máx {:.1} ms ({} de {} teclas viraram quadro)",
            percentile(&lat, 50),
            percentile(&lat, 90),
            percentile(&lat, 100),
            lat.len(),
            key_sent.len()
        );
    }
    save_png(&f, out)?;
    println!("Último quadro salvo em {out}");
    Ok(())
}

/// Ponto do --drag no instante `t`: uma volta por segundo num círculo no meio
/// da tela (onde a área de trabalho costuma estar vazia). Coordenadas 0..=65535. Pura.
fn drag_point(t: Duration) -> (u16, u16) {
    let a = t.as_secs_f64() * std::f64::consts::TAU;
    let x = 0.5 + 0.15 * a.cos();
    let y = 0.45 + 0.15 * a.sin();
    ((x * 65535.0) as u16, (y * 65535.0) as u16)
}

/// Roteiro do --typing: (quando, (scancode, estendida, medir)). Pura.
fn typing_script(duration: Duration) -> Vec<(Duration, (u16, bool, bool))> {
    const WIN: (u16, bool, bool) = (0x5B, true, false);
    const ESC: (u16, bool, bool) = (0x01, false, false);
    let mut steps = vec![(Duration::from_millis(500), WIN)];
    let mut t = Duration::from_millis(1500);
    let mut letter = true;
    while t + Duration::from_millis(600) < duration {
        // 'a' (0x1E) e Backspace (0x0E) alternados: a busca muda a cada tecla.
        steps.push((t, if letter { (0x1E, false, true) } else { (0x0E, false, true) }));
        letter = !letter;
        t += Duration::from_millis(100);
    }
    steps.push((t, ESC));
    steps
}

/// Pra cada tecla, quanto tempo até o 1º quadro decodificado depois dela.
/// Teclas sem quadro antes da próxima tecla ficam de fora. Pura.
fn key_to_frame(keys: &[u64], frames: &[u64]) -> Vec<u64> {
    let mut out = Vec::new();
    for (i, &k) in keys.iter().enumerate() {
        let limit = keys.get(i + 1).copied().unwrap_or(u64::MAX);
        if let Some(&f) = frames.iter().find(|&&f| f > k && f < limit) {
            out.push(f - k);
        }
    }
    out
}

/// Percentil `p` (0..=100) de valores já ordenados. Pura.
fn percentile(sorted: &[f64], p: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[(sorted.len() - 1) * p.min(100) / 100]
}

/// Latência por quadro, em ms: da captura no host até chegar aqui (rede +
/// captura/encode no host) e a decodificação.
fn print_latency(timings: &[(u64, u64, u64)], offset_ns: i64) {
    let ms = |ns: u64| ns as f64 / 1e6;
    let mut transit: Vec<f64> = timings.iter().map(|&(c, r, _)| ms(r.saturating_sub(reflex_viewer::client::to_viewer_clock(c, offset_ns)))).collect();
    let mut decode: Vec<f64> = timings.iter().map(|&(_, r, d)| ms(d.saturating_sub(r))).collect();
    transit.sort_by(f64::total_cmp);
    decode.sort_by(f64::total_cmp);
    println!(
        "captura→chegada: p50 {:.1} ms, p90 {:.1} ms, máx {:.1} ms | decode: p50 {:.1} ms, p90 {:.1} ms ({} quadros)",
        percentile(&transit, 50),
        percentile(&transit, 90),
        percentile(&transit, 100),
        percentile(&decode, 50),
        percentile(&decode, 90),
        timings.len()
    );
}

/// `--win` (Windows, 0x5B estendido) ou `--tap=0x39` (scancode, hex ou decimal).
fn tap_key(args: &[String]) -> Option<(u16, bool)> {
    if args.iter().any(|a| a == "--win") {
        return Some((0x5B, true));
    }
    let v = args.iter().find_map(|a| a.strip_prefix("--tap="))?;
    let code = match v.strip_prefix("0x") {
        Some(hex) => u16::from_str_radix(hex, 16).ok()?,
        None => v.parse().ok()?,
    };
    Some((code, false))
}

/// BGRA → PNG RGBA.
fn save_png(f: &BgraFrame, path: &str) -> Result<(), String> {
    let mut rgba = f.data.clone();
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
        px[3] = 255;
    }
    let file = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), f.width, f.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&rgba).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{drag_point, tap_key};

    #[test]
    fn size_with_and_without_scale() {
        let d = super::parse_size("1536x864@150").unwrap();
        assert_eq!((d.width, d.height, d.scale_pct), (1536, 864, 150));
        assert_eq!(super::parse_size("1280x720").unwrap().scale_pct, 100);
        assert!(super::parse_size("grande").is_none());
    }
    use std::time::Duration;

    #[test]
    fn drag_circles_inside_the_screen_once_per_second() {
        let p0 = drag_point(Duration::ZERO);
        assert_eq!(p0, drag_point(Duration::from_secs(1)));
        assert_ne!(p0, drag_point(Duration::from_millis(250)));
        for ms in (0..1000).step_by(50) {
            let (x, y) = drag_point(Duration::from_millis(ms));
            assert!((19000..=46000).contains(&x) && (19000..=40000).contains(&y), "{x},{y}");
        }
    }

    #[test]
    fn key_to_frame_pairs_each_key_with_next_frame() {
        assert_eq!(super::key_to_frame(&[100, 200, 300], &[150, 160, 290, 450]), vec![50, 90, 150]);
        // Tecla sem quadro antes da próxima: fica de fora.
        assert_eq!(super::key_to_frame(&[100, 200], &[250]), vec![50]);
    }

    #[test]
    fn typing_script_opens_types_and_closes() {
        let s = super::typing_script(std::time::Duration::from_secs(3));
        assert_eq!(s.first().unwrap().1 .0, 0x5B);
        assert_eq!(s.last().unwrap().1 .0, 0x01);
        assert!(s.iter().filter(|(_, k)| k.2).count() >= 8);
    }

    #[test]
    fn percentiles() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(super::percentile(&v, 50), 3.0);
        assert_eq!(super::percentile(&v, 100), 5.0);
        assert_eq!(super::percentile(&[], 90), 0.0);
    }

    #[test]
    fn tap_options() {
        assert_eq!(tap_key(&["--win".into()]), Some((0x5B, true)));
        assert_eq!(tap_key(&["--tap=0x39".into()]), Some((0x39, false)));
        assert_eq!(tap_key(&["--tap=57".into()]), Some((57, false)));
        assert_eq!(tap_key(&["x".into()]), None);
    }
}
