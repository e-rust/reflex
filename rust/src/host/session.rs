//! Uma conexão autenticada (§10.2). Liga o motor de captura aos streams:
//! vídeo (host→viewer), input (viewer→host, injetado se houver permissão),
//! cursor (host→viewer) e controle (troca de codec/monitor, keyframe, ping,
//! estatísticas).
//!
//! O motor roda numa thread (D3D11 não é `Send` entre threads async); a ponte
//! é feita por canais `std::sync::mpsc`, drenados por tarefas tokio.

use std::sync::Arc;

use crate::net::Connection;
use crate::platform::input::{virtual_screen, Injector, Rect};
use crate::proto::codec::{Codec, CodecCap};
use crate::proto::frame::{read_known, write_msg};
use crate::proto::msg::{AdminRequest, AdminResponse, Chroma, Control, Datagram, Input, Monitor, RemoteSize, SessionConfig, Stats, UniKind};
use crate::proto::perms::Perms;

use crate::host::engine::EngineConfig;
use crate::host::hub::{EngineHub, Feed};

/// O que a sessão precisa pra começar.
pub struct SessionParams {
    pub perms: Perms,
    pub monitors: Vec<Monitor>,
    /// Encoders do host (codec → tem hardware?).
    pub encoders: Vec<crate::proto::codec::CodecCap>,
    /// Decoders que o viewer informou.
    pub decoders: Vec<crate::proto::codec::CodecCap>,
    /// Núcleos lógicos do host (limita o fps do encode por CPU).
    pub cpu_cores: usize,
    /// O host faz cor completa (4:4:4)? Só com o x264 na build do FFmpeg.
    pub full_chroma: bool,
    /// Número da sessão (identifica o viewer na ponte com o usuário).
    pub id: u64,
    /// Clipboard e arquivos do usuário logado (sem ninguém logado: `None`).
    pub user: Option<Arc<crate::host::user_bridge::UserBridge>>,
    /// O motor da máquina, compartilhado com as outras conexões.
    pub hub: Arc<EngineHub>,
}

/// Teto de quadros/s do encode por CPU em máquina fraca (1–2 núcleos): o
/// mesmo núcleo codifica e desenha a tela; acima disso tudo fica lento.
pub const WEAK_CPU_MAX_FPS: u8 = 30;

/// fps efetivo: o pedido, limitado em CPU fraca sem encoder de hardware. Pura.
pub fn effective_fps(requested: u8, hw_encode: bool, cpu_cores: usize) -> u8 {
    let fps = requested.clamp(1, 240);
    if !hw_encode && cpu_cores <= 2 {
        fps.min(WEAK_CPU_MAX_FPS)
    } else {
        fps
    }
}

/// Resolve a configuração do motor a partir do pedido do viewer e do que os
/// dois lados suportam. Pura (testada).
pub fn resolve_engine_config(req: &SessionConfig, params: &SessionParams) -> Result<EngineConfig, String> {
    let mut sel = crate::proto::codec::select_codec(req.codec, &params.encoders, &params.decoders).ok_or("nenhum codec em comum")?;
    // Cor completa: o sem perda (é 4:4:4 por natureza), se os dois lados
    // fazem; senão H.264 pelo x264, decodificado em software no viewer
    // (encoders e decoders de placa de vídeo não fazem 4:4:4). Sem nenhum
    // dos dois, fica o 4:2:0 de sempre (com a placa, se houver).
    let lossless = |caps: &[CodecCap]| caps.iter().any(|c| c.codec == Codec::Lossless);
    if req.chroma == Chroma::Yuv444 && lossless(&params.encoders) && lossless(&params.decoders) {
        (sel.codec, sel.hw_encode) = (Codec::Lossless, false);
    } else if req.chroma == Chroma::Yuv444 && params.full_chroma {
        (sel.codec, sel.hw_encode) = (Codec::H264, false);
    }
    let monitor = if params.monitors.iter().any(|m| m.id == req.monitor) { req.monitor } else { params.monitors.first().map(|m| m.id).unwrap_or(0) };
    Ok(EngineConfig {
        monitor,
        codec: sel.codec,
        hw_encode: sel.hw_encode,
        max_fps: effective_fps(req.max_fps, sel.hw_encode, params.cpu_cores),
        bitrate_kbps: req.bitrate_kbps.max(200),
        max_width: req.max_width,
        max_height: req.max_height,
        refine: req.refine,
        chroma: req.chroma,
        desired: match req.remote_size {
            RemoteSize::MatchWindow => req.desired,
            RemoteSize::Keep => None,
        },
    })
}

/// Monitor escolhido como `Rect` pra traduzir o input. Sumiu (o monitor
/// virtual trocou de lugar com o do Hyper-V, p.ex.): o 1º; sem nenhum, a área
/// de trabalho virtual inteira.
fn monitor_rect(monitors: &[Monitor], id: u32, virt: Rect) -> Rect {
    monitors
        .iter()
        .find(|m| m.id == id)
        .or(monitors.first())
        .map(|m| Rect { x: m.x, y: m.y, w: m.width as i32, h: m.height as i32 })
        .unwrap_or(virt)
}

/// Atende um pedido de administração (§11.5). Só existe pra sessão de master.
pub type AdminFn = Arc<dyn Fn(AdminRequest) -> AdminResponse + Send + Sync>;

/// Resposta a `Control::Admin`: o pedido atendido, ou recusa se a sessão não
/// é de master. Pura (dado o atendente).
pub fn admin_reply(admin: Option<&AdminFn>, id: u32, req: AdminRequest) -> Control {
    let res = match admin {
        Some(handle) => handle(req),
        None => AdminResponse::Err("só um master administra esta máquina".into()),
    };
    Control::AdminReply { id, res }
}

/// Roda a sessão até o viewer desconectar ou o controle fechar.
pub async fn serve(conn: Arc<dyn Connection>, params: SessionParams, initial: SessionConfig, control: ControlStreams, admin: Option<AdminFn>) -> Result<(), crate::host::HostError> {
    let cfg = resolve_engine_config(&initial, &params).map_err(crate::host::HostError::Denied)?;
    // O motor é um só pra todas as conexões: entra na lista de quem assiste.
    let hub = params.hub.clone();
    let (feed, _) = hub.join(params.id, cfg);

    // Vídeo + cursor: o que o motor manda → streams.
    let (report_tx, mut reports) = tokio::sync::mpsc::unbounded_channel::<String>();
    let video_task = tokio::spawn(forward_feed(conn.clone(), feed, report_tx));
    // Input: stream → injeção (se permitido).
    let input_task = tokio::spawn(forward_input(conn.clone(), params.monitors.clone(), hub.clone(), params.perms));

    // Controle: a leitura numa tarefa (uma leitura interrompida no meio
    // perderia bytes do enquadramento); aqui chegam as mensagens do viewer e
    // as que o host quer mandar (cópias no clipboard daqui).
    let ControlStreams { mut write, mut read } = control;
    let (in_tx, mut in_rx) = tokio::sync::mpsc::unbounded_channel::<Control>();
    let reader = tokio::spawn(async move {
        while let Ok(msg) = read_known::<_, Control>(&mut read).await {
            if in_tx.send(msg).is_err() {
                break;
            }
        }
    });
    // Clipboard (§10.7): as cópias daqui chegam por `copies`; os streams que
    // o viewer abre buscam arquivos copiados aqui.
    let clipboard = params.user.clone().filter(|_| params.perms.contains(Perms::CLIPBOARD));
    let (_no_copies, mut copies) = tokio::sync::mpsc::unbounded_channel();
    if let Some(b) = &clipboard {
        copies = b.join(params.id, conn.clone(), params.perms.contains(Perms::FILES));
    }
    let files_task = clipboard.clone().map(|b| tokio::spawn(files_from_user(conn.clone(), b, params.id)));
    loop {
        let msg = tokio::select! {
            m = in_rx.recv() => match m {
                Some(m) => m,
                None => break,
            },
            Some(offer) = copies.recv() => {
                if write_msg(&mut write, &Control::ClipboardOffer(offer)).await.is_err() {
                    break;
                }
                continue;
            }
            Some(line) = reports.recv() => {
                if write_msg(&mut write, &Control::HostReport(line)).await.is_err() {
                    break;
                }
                continue;
            }
        };
        match msg {
            Control::Ping { t_ns } => {
                let _ = write_msg(&mut write, &Control::Pong { t_ns, host_ns: crate::media::clock::now_ns() }).await;
            }
            // Cópia no viewer: vai pro clipboard daqui (sem voltar como eco).
            Control::ClipboardOffer(offer) => {
                if let Some(b) = &clipboard {
                    b.remote_copy(params.id, offer);
                }
            }
            Control::RequestKeyframe => hub.request_keyframe(),
            Control::MeasureLossless(on) => hub.measure_lossless(on),
            Control::Stats(_s) => { /* registrado pela camada de cima quando quiser */ }
            Control::SessionConfig(req) => {
                match resolve_engine_config(&req, &params) {
                    Ok(new) => {
                        let eff = hub.request(params.id, new);
                        let _ = write_msg(&mut write, &Control::ConfigApplied { codec: eff.codec, width: eff.max_width, height: eff.max_height }).await;
                    }
                    Err(e) => {
                        let _ = write_msg(&mut write, &Control::ConfigRejected { reason: e }).await;
                    }
                }
            }
            Control::Admin { id, req } => {
                let _ = write_msg(&mut write, &admin_reply(admin.as_ref(), id, req)).await;
            }
            Control::Bye { .. } => break,
            _ => {}
        }
    }

    if let Some(b) = &clipboard {
        b.leave(params.id);
    }
    if let Some(t) = files_task {
        t.abort();
    }
    reader.abort();
    video_task.abort();
    input_task.abort();
    // A última conexão para o motor (espera ele soltar a captura).
    tokio::task::block_in_place(|| hub.leave(params.id));
    Ok(())
}

/// Streams que o viewer abre: pedidos dos arquivos copiados aqui.
async fn files_from_user(conn: Arc<dyn Connection>, bridge: Arc<crate::host::user_bridge::UserBridge>, id: u64) {
    while let Ok((write, read)) = conn.accept_bi().await {
        let bridge = bridge.clone();
        tokio::spawn(async move {
            if let Err(e) = bridge.files_from_user(id, write, read).await {
                log::info!("arquivos pedidos pelo viewer: {e}");
            }
        });
    }
}

/// Streams de controle (bidirecional) já abertos.
pub struct ControlStreams {
    pub write: crate::net::BoxWrite,
    pub read: crate::net::BoxRead,
}

/// Manda o que o motor produz pros streams de vídeo e cursor desta conexão.
async fn forward_feed(conn: Arc<dyn Connection>, mut feed: tokio::sync::mpsc::UnboundedReceiver<Feed>, reports: tokio::sync::mpsc::UnboundedSender<String>) {
    let mut video = match conn.open_uni().await {
        Ok(w) => w,
        Err(_) => return,
    };
    let kind = crate::proto::frame::encode(&UniKind::Video).unwrap_or_default();
    use tokio::io::AsyncWriteExt;
    if video.write_all(&kind).await.is_err() {
        return;
    }
    let mut cursor = conn.open_uni().await.ok();
    if let Some(c) = cursor.as_mut() {
        let k = crate::proto::frame::encode(&UniKind::Cursor).unwrap_or_default();
        let _ = c.write_all(&k).await;
    }

    while let Some(ev) = feed.recv().await {
        match ev {
            // O quadro é de todos; quando esta conexão o solta, conta como enviado por ela.
            Feed::Frame(d) => {
                if write_msg(&mut video, &d.frame).await.is_err() {
                    break;
                }
            }
            Feed::CursorShape(cs) => {
                if let Some(c) = cursor.as_mut() {
                    let _ = write_msg(c, &*cs).await;
                }
            }
            Feed::CursorPos { x, y, visible, shape_id } => {
                let _ = conn.send_datagram(Datagram::cursor(x, y, visible, shape_id).to_bytes());
            }
            Feed::Report(line) => {
                let _ = reports.send(line.to_string());
            }
        }
    }
}

/// Recebe o stream de input e injeta (se `INPUT` estiver nas permissões).
async fn forward_input(conn: Arc<dyn Connection>, monitors: Vec<Monitor>, hub: Arc<EngineHub>, perms: Perms) {
    let mut stream = match conn.accept_uni().await {
        Ok(r) => r,
        Err(_) => return,
    };
    let _kind: Result<UniKind, _> = read_known(&mut stream).await;
    let may_inject = perms.contains(Perms::INPUT);
    // Solta o que ficou pressionado quando a sessão acaba (drop).
    let mut injector = Injector::spawn();
    // Os monitores mudam no meio da sessão (monitor virtual liga/desliga,
    // resolução): relê quando a área de trabalho virtual muda de tamanho.
    let mut monitors = monitors;
    let mut virt = virtual_screen();
    loop {
        let input: Input = match read_known(&mut stream).await {
            Ok(i) => i,
            Err(_) => break,
        };
        if !may_inject {
            continue;
        }
        // O monitor que o motor mostra (outra conexão pode ter trocado).
        let monitor = hub.monitor().unwrap_or(0);
        let now = virtual_screen();
        if now != virt {
            virt = now;
            let fresh = crate::media::capture::monitors();
            if !fresh.is_empty() {
                monitors = fresh;
            }
        }
        let rect = monitor_rect(&monitors, monitor, virt);
        injector.apply(input, rect, virt);
    }
}

/// `Stats` do viewer → medida simples (pra UI do host, se quiser).
pub fn summarize_stats(s: &Stats) -> String {
    format!("{} kbps, rtt {} ms, {} perdidos", s.kbps, s.rtt_us / 1000, s.frames_dropped)
}

/// Codec legível (log/UI).
pub fn codec_label(c: Codec) -> &'static str {
    c.label()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::codec::{Codec, CodecCap, CodecChoice};

    fn params() -> SessionParams {
        SessionParams {
            perms: Perms::ALL,
            monitors: vec![
                Monitor { id: 0, name: "p".into(), x: 0, y: 0, width: 1920, height: 1080, primary: true, virtual_display: false },
                Monitor { id: 1, name: "s".into(), x: 1920, y: 0, width: 1280, height: 1024, primary: false, virtual_display: false },
            ],
            encoders: vec![CodecCap { codec: Codec::H264, hw: true }, CodecCap { codec: Codec::Av1, hw: true }],
            decoders: vec![CodecCap { codec: Codec::H264, hw: true }, CodecCap { codec: Codec::Av1, hw: true }],
            cpu_cores: 8,
            full_chroma: true,
            id: 1,
            user: None,
            hub: Arc::default(),
        }
    }

    #[test]
    fn only_master_sessions_get_admin_answers() {
        let admin: AdminFn = Arc::new(|_| AdminResponse::Ok);
        assert_eq!(admin_reply(Some(&admin), 7, AdminRequest::ListUsers), Control::AdminReply { id: 7, res: AdminResponse::Ok });
        assert!(matches!(admin_reply(None, 8, AdminRequest::ListUsers), Control::AdminReply { id: 8, res: AdminResponse::Err(_) }));
    }

    #[test]
    fn weak_cpu_without_hardware_encoder_is_capped() {
        assert_eq!(effective_fps(60, false, 1), 30);
        assert_eq!(effective_fps(20, false, 2), 20);
        assert_eq!(effective_fps(60, true, 1), 60);
        assert_eq!(effective_fps(60, false, 4), 60);
        assert_eq!(effective_fps(0, false, 1), 1);
    }

    #[test]
    fn auto_picks_best_shared_codec() {
        let req = SessionConfig { codec: CodecChoice::Auto, ..Default::default() };
        let cfg = resolve_engine_config(&req, &params()).unwrap();
        assert_eq!(cfg.codec, Codec::Av1);
        assert!(cfg.hw_encode);
    }

    #[test]
    fn full_chroma_forces_software_h264() {
        let req = SessionConfig { codec: CodecChoice::Auto, chroma: Chroma::Yuv444, ..Default::default() };
        let cfg = resolve_engine_config(&req, &params()).unwrap();
        assert_eq!((cfg.codec, cfg.hw_encode, cfg.chroma), (Codec::H264, false, Chroma::Yuv444));
    }

    #[test]
    fn full_chroma_prefers_lossless_when_both_sides_have_it() {
        let mut p = params();
        p.encoders.push(CodecCap { codec: Codec::Lossless, hw: false });
        p.decoders.push(CodecCap { codec: Codec::Lossless, hw: false });
        let req = SessionConfig { codec: CodecChoice::Auto, chroma: Chroma::Yuv444, ..Default::default() };
        assert_eq!(resolve_engine_config(&req, &p).unwrap().codec, Codec::Lossless);
        // 4:2:0: a placa, como sempre.
        let plain = SessionConfig { codec: CodecChoice::Auto, ..Default::default() };
        assert_eq!(resolve_engine_config(&plain, &p).unwrap().codec, Codec::Av1);
    }

    #[test]
    fn desired_size_only_when_matching_the_window() {
        let want = crate::proto::msg::DesiredDisplay { width: 1500, height: 900, scale_pct: 150 };
        let keep = SessionConfig { desired: Some(want), ..Default::default() };
        assert_eq!(resolve_engine_config(&keep, &params()).unwrap().desired, None);
        let mat = SessionConfig { desired: Some(want), remote_size: RemoteSize::MatchWindow, ..Default::default() };
        assert_eq!(resolve_engine_config(&mat, &params()).unwrap().desired, Some(want));
    }

    #[test]
    fn invalid_monitor_falls_back_to_first() {
        let req = SessionConfig { monitor: 99, ..Default::default() };
        assert_eq!(resolve_engine_config(&req, &params()).unwrap().monitor, 0);
    }

    #[test]
    fn fps_and_bitrate_are_clamped() {
        let req = SessionConfig { max_fps: 0, bitrate_kbps: 1, ..Default::default() };
        let cfg = resolve_engine_config(&req, &params()).unwrap();
        assert_eq!(cfg.max_fps, 1);
        assert_eq!(cfg.bitrate_kbps, 200);
    }

    #[test]
    fn no_shared_codec_is_error() {
        let mut p = params();
        p.decoders = vec![CodecCap { codec: Codec::Hevc, hw: true }];
        let req = SessionConfig::default();
        assert!(resolve_engine_config(&req, &p).is_err());
    }

    #[test]
    fn monitor_rect_uses_offsets() {
        let virt = Rect { x: 0, y: 0, w: 3200, h: 1080 };
        assert_eq!(monitor_rect(&params().monitors, 1, virt), Rect { x: 1920, y: 0, w: 1280, h: 1024 });
        // Monitor que sumiu: o 1º da lista; lista vazia: a área virtual.
        assert_eq!(monitor_rect(&params().monitors, 7, virt), Rect { x: 0, y: 0, w: 1920, h: 1080 });
        assert_eq!(monitor_rect(&[], 0, virt), virt);
    }
}
