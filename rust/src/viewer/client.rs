//! Conectar, autenticar e rodar os laços do viewer (§5.2, §10).
//!
//! O handshake é o espelho do [`crate::host::handshake`]. Depois: um laço de
//! decode (stream de vídeo → [`crate::media::decode`] → quadros BGRA pra UI),
//! o cursor, os datagramas de posição, o envio de input e o controle (ping,
//! troca de codec). A UI recebe os quadros por um canal e manda input por outro.

use std::sync::Arc;

use crate::media::decode::{BgraFrame, VideoDecoder};
use crate::net::{connect, Connection, LocalCert, ServerCheck, TransportMode};
use crate::proto::codec::{Codec, CodecCap};
use crate::proto::frame::{read_known, write_msg};
use crate::proto::msg::{AdminRequest, AdminResponse, Caps, ClipboardOffer, Control, CursorShape, Datagram, Hello, Input, SessionConfig, UniKind, VideoFrame};
use crate::proto::perms::{Level, Perms};
use tokio::sync::mpsc;

use crate::viewer::ViewerError;

/// Como o viewer entra: pela chave (cadastrado) ou por um código de convite.
#[derive(Debug, Clone)]
pub enum Credential {
    Key,
    Invite { code: String },
}

/// O que a UI recebe do laço do viewer.
#[derive(Debug)]
pub enum ViewerEvent {
    Connected { level: Level, perms: Perms, width: u32, height: u32, codec: Codec },
    Frame(BgraFrame),
    Cursor(CursorShape),
    CursorPos { x: i16, y: i16, visible: bool, shape_id: u32 },
    AwaitingConfirmation,
    Disconnected(String),
    /// Tempos de um quadro, no relógio DESTE viewer (QPC em ns): capturado no
    /// host (já convertido pelo [`ViewerEvent::Clock`]), recebido, decodificado.
    Timing { capture_ns: u64, received_ns: u64, decoded_ns: u64 },
    /// Medida do ping/pong: ida e volta e diferença entre os relógios (host − viewer).
    Clock { rtt_us: u32, offset_ns: i64 },
    /// Alguém copiou algo na máquina remota.
    ClipboardOffer(ClipboardOffer),
    /// Medidas do motor do host (diagnóstico).
    HostReport(String),
    /// O vídeo passou a chegar neste codec (no 1º quadro e a cada troca).
    VideoCodec { codec: Codec, width: u32, height: u32 },
}

/// Resultado do handshake do viewer.
pub struct Connected {
    pub conn: Arc<dyn Connection>,
    pub control: (crate::net::BoxWrite, crate::net::BoxRead),
    pub level: Level,
    pub perms: Perms,
    /// O que o host sabe codificar (do `Hello` dele).
    pub host_encoders: Vec<CodecCap>,
}

/// Conecta e autentica. `on_await` é chamado se o host espera confirmação local.
#[allow(clippy::too_many_arguments)]
pub async fn connect_and_auth(
    addr: std::net::SocketAddr,
    local: &LocalCert,
    check: ServerCheck,
    mode: TransportMode,
    app_version: &str,
    decoders: Vec<CodecCap>,
    cred: Credential,
    mut on_await: impl FnMut(),
) -> Result<Connected, ViewerError> {
    let conn = connect(addr, local, check, mode).await?;
    let (mut w, mut r) = conn.open_bi().await?;

    let hello = Hello {
        proto: crate::proto::PROTOCOL_VERSION,
        app_version: app_version.to_string(),
        machine_name: String::new(),
        caps: Caps { encoders: Vec::new(), decoders },
    };
    write_msg(&mut w, &Control::Hello(hello)).await?;
    let host_hello: Hello = match read_known::<_, Control>(&mut r).await? {
        Control::Hello(h) => h,
        _ => return Err(ViewerError::Denied("host não mandou Hello".into())),
    };
    if let Some(reason) = crate::proto::version_mismatch(host_hello.proto, "a máquina") {
        return Err(ViewerError::Denied(reason));
    }

    if let Credential::Invite { code } = &cred {
        prove_invite(&mut w, &mut r, code, &conn.channel_binding()).await?;
    }

    // Espera o AuthResult (pode vir AwaitingConfirmation antes).
    let (level, perms) = loop {
        match read_known::<_, Control>(&mut r).await? {
            Control::AwaitingConfirmation => on_await(),
            Control::AuthResult(ar) if ar.ok => break (ar.level.unwrap_or(Level::View), ar.perms),
            Control::AuthResult(ar) => return Err(ViewerError::Denied(ar.reason)),
            _ => {}
        }
    };
    Ok(Connected { conn, control: (w, r), level, perms, host_encoders: host_hello.caps.encoders })
}

/// Um pedido de administração (só master) antes de abrir o vídeo — o
/// `reflex --admin` faz isso e fecha a conexão.
pub async fn admin(connected: &mut Connected, req: AdminRequest) -> Result<AdminResponse, ViewerError> {
    let (w, r) = &mut connected.control;
    write_msg(w, &Control::Admin { id: 1, req }).await?;
    loop {
        if let Control::AdminReply { id: 1, res } = read_known::<_, Control>(r).await? {
            return Ok(res);
        }
    }
}

async fn prove_invite<W, R>(w: &mut W, r: &mut R, code: &str, binding: &[u8]) -> Result<(), ViewerError>
where
    W: tokio::io::AsyncWrite + Unpin,
    R: tokio::io::AsyncRead + Unpin,
{
    use crate::auth::pake::{Pake, Role};
    let (pake, msg) = Pake::start(code, Role::Viewer);
    write_msg(w, &Control::InviteStart { spake: msg }).await?;
    let reply = match read_known::<_, Control>(r).await? {
        Control::InviteReply { spake } => spake,
        Control::AuthResult(ar) => return Err(ViewerError::Denied(ar.reason)),
        _ => return Err(ViewerError::Denied("host não respondeu ao convite".into())),
    };
    let confirmed = pake.finish(&reply, binding).map_err(|_| ViewerError::Denied("código incorreto".into()))?;
    match read_known::<_, Control>(r).await? {
        Control::InviteConfirm { .. } => {}
        Control::AuthResult(ar) => return Err(ViewerError::Denied(ar.reason)),
        _ => return Err(ViewerError::Denied("host não confirmou o convite".into())),
    }
    write_msg(w, &Control::InviteConfirm { mac: confirmed.my_proof() }).await?;
    Ok(())
}

/// Roda os laços até a conexão cair. `events` vai pra UI; `input_rx` traz o
/// input capturado pela UI; `outgoing`, mensagens de controle da UI no meio
/// da sessão (configuração nova quando a janela muda de tamanho, cópias no
/// clipboard daqui).
pub async fn run(
    connected: Connected,
    initial: SessionConfig,
    events: mpsc::UnboundedSender<ViewerEvent>,
    mut input_rx: mpsc::UnboundedReceiver<Input>,
    outgoing: mpsc::UnboundedReceiver<Control>,
    files: crate::platform::files::SharedOffer,
) {
    let Connected { conn, control, level, perms, .. } = connected;
    let (mut cw, mut cr) = control;
    let _ = write_msg(&mut cw, &Control::SessionConfig(initial)).await;
    let _ = events.send(ViewerEvent::Connected { level, perms, width: 0, height: 0, codec: Codec::H264 });

    // Input: UI → host (um uni stream).
    let conn_in = conn.clone();
    let input_task = tokio::spawn(async move {
        let Ok(mut s) = conn_in.open_uni().await else { return };
        use tokio::io::AsyncWriteExt;
        let kind = crate::proto::frame::encode(&UniKind::Input).unwrap_or_default();
        if s.write_all(&kind).await.is_err() {
            return;
        }
        while let Some(input) = input_rx.recv().await {
            if write_msg(&mut s, &input).await.is_err() {
                break;
            }
        }
    });

    // Datagramas (posição do cursor).
    let conn_dg = conn.clone();
    let ev_dg = events.clone();
    let dgram_task = tokio::spawn(async move {
        while let Ok(buf) = conn_dg.read_datagram().await {
            if let Some(Datagram::CursorPos { x, y, visible, shape_id }) = Datagram::from_bytes(&buf) {
                let _ = ev_dg.send(ViewerEvent::CursorPos { x, y, visible, shape_id });
            }
        }
    });

    // Streams uni do host: vídeo e cursor (decididos pela 1ª mensagem).
    let conn_uni = conn.clone();
    let ev_uni = events.clone();
    let uni_task = tokio::spawn(async move {
        loop {
            let Ok(mut s) = conn_uni.accept_uni().await else { break };
            let ev = ev_uni.clone();
            tokio::spawn(async move {
                match read_known::<_, UniKind>(&mut s).await {
                    Ok(UniKind::Video) => decode_loop(&mut s, ev).await,
                    Ok(UniKind::Cursor) => cursor_loop(&mut s, ev).await,
                    _ => {}
                }
            });
        }
    });

    // Streams bi que o host abre: pedidos dos arquivos copiados aqui (o host
    // está colando no Explorer de lá).
    let conn_bi = conn.clone();
    let files_task = tokio::spawn(async move {
        while let Ok((mut w, mut r)) = conn_bi.accept_bi().await {
            let files = files.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::platform::files::serve(&files, &mut r, &mut w).await {
                    log::info!("arquivos pedidos pela máquina remota: {e}");
                }
            });
        }
    });

    // Controle: ping periódico e o que a UI mandar (um escritor só).
    let ping = tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
        let mut outgoing = Some(outgoing);
        loop {
            let next = async {
                match outgoing.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            };
            let msg = tokio::select! {
                _ = tick.tick() => Control::Ping { t_ns: crate::media::clock::now_ns() },
                m = next => match m {
                    Some(m) => m,
                    // A UI fechou o canal: só ping daqui pra frente.
                    None => {
                        outgoing = None;
                        continue;
                    }
                },
            };
            if write_msg(&mut cw, &msg).await.is_err() {
                break;
            }
        }
    });
    // Lê o controle até cair. O Pong dá o desvio de relógio (pra latência).
    while let Ok(msg) = read_known::<_, Control>(&mut cr).await {
        match msg {
            Control::Pong { t_ns, host_ns } => {
                let now = crate::media::clock::now_ns();
                let _ = events.send(ViewerEvent::Clock { rtt_us: rtt_us(t_ns, now), offset_ns: clock_offset(t_ns, now, host_ns) });
            }
            Control::ClipboardOffer(offer) => {
                let _ = events.send(ViewerEvent::ClipboardOffer(offer));
            }
            Control::HostReport(line) => {
                let _ = events.send(ViewerEvent::HostReport(line));
            }
            _ => {}
        }
    }
    let _ = events.send(ViewerEvent::Disconnected("conexão encerrada".into()));
    ping.abort();
    files_task.abort();
    input_task.abort();
    dgram_task.abort();
    uni_task.abort();
}

/// Diagnóstico (`reflex --probe --banda=`): lê o vídeo a no máximo tantos
/// KB/s, como uma rede lenta (o controle de fluxo segura o host). 0: sem limite.
pub static READ_LIMIT_KBPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

async fn decode_loop<R: tokio::io::AsyncRead + Unpin>(s: &mut R, events: mpsc::UnboundedSender<ViewerEvent>) {
    let mut decoder: Option<Box<dyn VideoDecoder>> = None;
    let mut current = None;
    loop {
        let frame: VideoFrame = match read_known(s).await {
            Ok(f) => f,
            Err(_) => break,
        };
        let limit = READ_LIMIT_KBPS.load(std::sync::atomic::Ordering::Relaxed);
        if limit > 0 {
            tokio::time::sleep(std::time::Duration::from_secs_f64(frame.data.len() as f64 / (limit as f64 * 1024.0))).await;
        }
        if current != Some(frame.codec) {
            // Decoder novo ao ver codec diferente (troca em sessão). Vídeo: tenta hardware.
            decoder = crate::media::decode::open(frame.codec).map_err(|e| log::warn!("decoder: {e}")).ok();
            current = Some(frame.codec);
            log::info!("vídeo em {} ({}x{})", frame.codec.label(), frame.width, frame.height);
            let _ = events.send(ViewerEvent::VideoCodec { codec: frame.codec, width: frame.width, height: frame.height });
        }
        let Some(dec) = decoder.as_mut() else { continue };
        let received_ns = crate::media::clock::now_ns();
        match dec.decode(&frame.data) {
            Ok(Some(img)) => {
                let decoded_ns = crate::media::clock::now_ns();
                let _ = events.send(ViewerEvent::Timing { capture_ns: frame.capture_ns, received_ns, decoded_ns });
                if events.send(ViewerEvent::Frame(img)).is_err() {
                    break;
                }
            }
            Ok(None) => {}
            Err(e) => {
                log::warn!("decode: {e}");
            }
        }
    }
}

async fn cursor_loop<R: tokio::io::AsyncRead + Unpin>(s: &mut R, events: mpsc::UnboundedSender<ViewerEvent>) {
    while let Ok(cs) = read_known::<_, CursorShape>(s).await {
        if events.send(ViewerEvent::Cursor(cs)).is_err() {
            break;
        }
    }
}

/// Ida e volta de um ping (ns do viewer no envio e agora). Pura.
pub fn rtt_us(ping_sent_ns: u64, now_ns: u64) -> u32 {
    (now_ns.saturating_sub(ping_sent_ns) / 1000) as u32
}

/// Diferença entre os relógios (host − viewer), supondo ida e volta
/// simétricas: o host respondeu no meio do caminho. Pura.
pub fn clock_offset(ping_sent_ns: u64, now_ns: u64, host_ns: u64) -> i64 {
    let midpoint = ping_sent_ns as i128 + (now_ns as i128 - ping_sent_ns as i128) / 2;
    (host_ns as i128 - midpoint) as i64
}

/// Instante de captura de um quadro convertido pro relógio do viewer. Pura.
pub fn to_viewer_clock(host_ns: u64, offset_ns: i64) -> u64 {
    (host_ns as i128 - offset_ns as i128).max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_offset_assumes_symmetric_path() {
        // Ping saiu em 1000, voltou em 3000; o host disse 10_000 no meio (2000).
        assert_eq!(clock_offset(1000, 3000, 10_000), 8000);
        // Host atrás do viewer.
        assert_eq!(clock_offset(5000, 7000, 1000), -5000);
        assert_eq!(to_viewer_clock(10_000, 8000), 2000);
        assert_eq!(to_viewer_clock(1000, -5000), 6000);
    }

    #[test]
    fn rtt_is_half_microseconds() {
        assert_eq!(rtt_us(1_000_000, 3_000_000), 2000);
        assert_eq!(rtt_us(5, 1), 0);
    }
}
