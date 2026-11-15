//! Motor de captura numa thread própria (§4.1, §10.4). Captura DXGI → NV12 na
//! GPU → encode, obedecendo ao [`Pacer`] (nunca enfileira quadro). Segue a
//! área de trabalho de entrada (Winlogon na tela de login) e recria a captura
//! quando ela troca ou quando o acesso é perdido.
//!
//! O resto do host fala com o motor por comandos ([`EngineCmd`]) e recebe
//! eventos ([`EngineEvent`]); não sabe de D3D11 nem FFmpeg.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;

use crate::media::capture::ScreenCapture;
use crate::media::cursor::{to_image, RawCursorShape};
use crate::media::encode::{EncoderConfig, VideoEncoder, FULL_CHROMA_BACKEND};
use crate::media::gpu::{fit_even, BgraReadback, Device, Nv12Converter};
use crate::media::lossless::{LosslessMeter, LosslessStats};
use crate::media::pacing::{Decision, Pacer};
use crate::media::{caps, clock};
use crate::platform::desktop::DesktopFollower;
use crate::platform::display::DisplayOverride;
use crate::platform::power::DisplayAwake;
use crate::platform::timer::HighResTimer;
use crate::proto::codec::Codec;
use crate::proto::msg::{Chroma, DesiredDisplay, Refine, VideoFrame};

/// Como o motor deve capturar e codificar agora.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineConfig {
    pub monitor: u32,
    pub codec: Codec,
    pub hw_encode: bool,
    pub max_fps: u8,
    pub bitrate_kbps: u32,
    pub max_width: u32,
    pub max_height: u32,
    /// Nitidez com a tela parada.
    pub refine: Refine,
    /// Cor completa (4:4:4): só com o x264 (se houver) e caminho BGRA na CPU.
    pub chroma: Chroma,
    /// Tela remota no tamanho que o viewer pediu (como o RDP).
    pub desired: Option<DesiredDisplay>,
}

#[derive(Debug)]
pub enum EngineCmd {
    Reconfigure(EngineConfig),
    RequestKeyframe,
    /// Medir também a imagem sem perda (o híbrido; só no caminho de CPU).
    MeasureLossless(bool),
    Stop,
}

#[derive(Debug)]
pub enum EngineEvent {
    /// Vídeo pronto pra enviar.
    Frame(VideoFrame),
    /// Forma do cursor mudou (BGRA).
    CursorShape(crate::proto::msg::CursorShape),
    /// Posição do cursor relativa ao monitor.
    CursorPos { x: i32, y: i32, visible: bool, shape_id: u32 },
    /// Tamanho/codec efetivos após (re)configurar.
    Configured { width: u32, height: u32, codec: Codec },
    Error(String),
    /// Resumo periódico das medidas (vai pro viewer também).
    Report(String),
}

/// Espera entre tentativas de (re)abrir a captura: a 1ª é imediata (troca de
/// área de trabalho), depois dobra até 1 s. Pura, com o tempo em ms.
#[derive(Debug, Default)]
pub struct Backoff {
    failures: u32,
    next_at_ms: u64,
}

impl Backoff {
    const MAX_MS: u64 = 1000;

    pub fn ready(&self, now_ms: u64) -> bool {
        now_ms >= self.next_at_ms
    }

    pub fn failed(&mut self, now_ms: u64) {
        let delay = if self.failures == 0 { 0 } else { (25u64 << self.failures.min(6)).min(Self::MAX_MS) };
        self.failures += 1;
        self.next_at_ms = now_ms + delay;
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Avisa a 1ª falha e depois uma a cada 20 (sem inundar o log).
    pub fn should_report(&self) -> bool {
        self.failures == 1 || self.failures.is_multiple_of(20)
    }
}

/// Quadros que o motor entregou e quem envia ainda não escreveu na rede. O
/// motor soma, o envio subtrai; com quadro pendente o [`Pacer`] não codifica
/// (a tela fica marcada e sai atualizada no próximo) — nada se acumula na fila.
#[derive(Debug, Clone, Default)]
pub struct InFlight(Arc<AtomicUsize>);

impl InFlight {
    pub fn sent(&self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }

    pub fn done(&self) {
        let _ = self.0.try_update(Ordering::AcqRel, Ordering::Acquire, |n| Some(n.saturating_sub(1)));
    }

    pub fn get(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}

/// O que sobrevive à recriação da captura.
#[derive(Debug, Default)]
struct Shared {
    seq: u64,
    in_flight: InFlight,
    stats: Stats,
    cursor: CursorTracker,
    /// Medida da imagem sem perda, ligada pelo viewer (`MeasureLossless`).
    lossless: Option<LosslessMeter>,
}

/// Posição do cursor pra o viewer. A captura (DXGI) informa quando o cursor
/// mexe — mas não com o monitor virtual, que recebe o cursor como "de
/// hardware" (medido na VM: a forma veio, posição nenhuma, nem arrastando o
/// mouse). Sem a da captura, lê a do Windows e manda quando muda. A posição
/// é a do canto da forma (como a da captura): a do Windows menos o ponto de
/// clique da forma atual.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CursorTracker {
    hot: (i32, i32),
    last: Option<(i32, i32, bool)>,
}

impl CursorTracker {
    /// Forma nova: o ponto de clique dela.
    pub fn set_hot(&mut self, hot_x: u32, hot_y: u32) {
        self.hot = (hot_x as i32, hot_y as i32);
    }

    /// O que mandar agora: a posição da captura, se veio; senão a do Windows
    /// (`screen`, na área de trabalho virtual) relativa a `origin` (canto do
    /// monitor) — só se mudou. Pura.
    pub fn next(&mut self, captured: Option<(i32, i32, bool)>, screen: Option<(i32, i32, bool)>, origin: (i32, i32)) -> Option<(i32, i32, bool)> {
        let pos = captured.or_else(|| screen.map(|(x, y, v)| (x - origin.0 - self.hot.0, y - origin.1 - self.hot.1, v)))?;
        if self.last == Some(pos) && captured.is_none() {
            return None;
        }
        self.last = Some(pos);
        Some(pos)
    }
}

/// Um tempo acumulado: soma e máximo, em µs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Timer {
    pub sum_us: u64,
    pub max_us: u32,
}

impl Timer {
    pub fn add(&mut self, us: u32) {
        self.sum_us += us as u64;
        self.max_us = self.max_us.max(us);
    }

    /// Média em ms sobre `n` amostras.
    pub fn avg_ms(&self, n: u32) -> f64 {
        if n == 0 {
            0.0
        } else {
            self.sum_us as f64 / n as f64 / 1000.0
        }
    }
}

/// Medidas do motor num intervalo (vão pro log a cada [`Stats::PERIOD_NS`]):
/// quantas imagens a captura entregou, quantos quadros saíram e quanto cada
/// etapa levou. É o que separa "a tela gera poucos quadros" de "o motor não
/// dá conta".
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    pub since_ns: u64,
    pub images: u32,
    pub frames: u32,
    /// Desses quadros, quantos foram refino (tela parada).
    pub refines: u32,
    pub bytes: u64,
    /// Tempo dentro do AcquireNextFrame quando veio imagem nova.
    pub acquire: Timer,
    /// O resto do quadro: shader NV12 ou leitura do BGRA pela CPU.
    pub prepare: Timer,
    pub upload: Timer,
    pub codec: Timer,
    /// A medida da imagem sem perda, quando ligada.
    pub lossless: LosslessStats,
}

impl Stats {
    pub const PERIOD_NS: u64 = 5_000_000_000;

    /// Resumo legível do intervalo terminado em `now_ns`.
    pub fn summary(&self, now_ns: u64) -> String {
        let secs = (now_ns.saturating_sub(self.since_ns) as f64 / 1e9).max(0.001);
        let n = self.frames;
        let lossless = if self.lossless.frames > 0 { format!(" | {}", self.lossless.summary(secs)) } else { String::new() };
        format!(
            "motor ({secs:.1} s): {} imagens novas, {} quadros{} ({:.1}/s, {} KB/s) | captura {:.1} ms | preparo {:.1} ms | subida {:.1} ms (máx {:.1}) | codec {:.1} ms (máx {:.1}){lossless}",
            self.images,
            n,
            if self.refines > 0 { format!(" ({} de refino)", self.refines) } else { String::new() },
            n as f64 / secs,
            (self.bytes as f64 / secs / 1024.0) as u64,
            self.acquire.avg_ms(self.images),
            self.prepare.avg_ms(n),
            self.upload.avg_ms(n),
            self.upload.max_us as f64 / 1000.0,
            self.codec.avg_ms(n),
            self.codec.max_us as f64 / 1000.0,
        )
    }

    /// Hora de registrar? Se sim, devolve o resumo e recomeça a contagem.
    pub fn take_if_due(&mut self, now_ns: u64) -> Option<String> {
        if self.since_ns == 0 {
            self.since_ns = now_ns;
            return None;
        }
        if now_ns.saturating_sub(self.since_ns) < Self::PERIOD_NS {
            return None;
        }
        let line = (self.images > 0 || self.frames > 0).then(|| self.summary(now_ns));
        *self = Stats { since_ns: now_ns, ..Default::default() };
        line
    }
}

fn now_ms() -> u64 {
    clock::now_ns() / 1_000_000
}

/// Roda o laço do motor até `Stop`. Pensado pra rodar numa thread dedicada.
///
/// A captura é opcional: perdeu o acesso (troca de área de trabalho,
/// resolução, UAC) → é SOLTA primeiro e recriada depois — duas duplicações do
/// mesmo monitor no mesmo processo invalidam uma à outra (medido na VM: laço
/// de 2.300 recriações em 40 s quando a nova era criada antes de soltar a antiga).
pub fn run(initial: EngineConfig, cmds: Receiver<EngineCmd>, events: Sender<EngineEvent>, in_flight: InFlight) {
    let mut awake = DisplayAwake::new();
    let _timer = HighResTimer::new();
    // Antes de abrir a captura: a thread já nasce na área de trabalho certa.
    let mut follower = DesktopFollower::new();
    follower.sync();
    // Tela apagada (bloqueio ocioso) só entrega preto: religa antes do 1º quadro.
    crate::platform::power::wake_display();
    let mut cfg = initial;
    let mut state: Option<State> = None;
    let mut retry = Backoff::default();
    let mut shared = Shared { in_flight, ..Default::default() };
    let mut shape_id = 0u32;
    // Resolução/escala que a sessão mudou; desfeitas quando o motor para (drop).
    let mut display = DisplayOverride::default();
    let mut cpu = crate::platform::cpu::CpuMeter::default();
    cpu.sample();

    loop {
        awake.refresh();
        if follower.sync() {
            // Trocou (Default ↔ Winlogon): recria a captura na nova.
            state = None;
            retry.reset();
        }
        match cmds.try_recv() {
            Ok(EngineCmd::Stop) | Err(TryRecvError::Disconnected) => break,
            Ok(EngineCmd::Reconfigure(c)) => {
                cfg = c;
                state = None;
                retry.reset();
            }
            // Sem captura aberta, a próxima já começa por keyframe.
            Ok(EngineCmd::RequestKeyframe) => state.iter_mut().for_each(|s| s.pacer.request_key()),
            Ok(EngineCmd::MeasureLossless(on)) => {
                if on && state.as_ref().is_some_and(|s| s.readback.is_none()) {
                    log::warn!("medida sem perda: só no caminho de CPU (placa de software); aqui não mede");
                }
                shared.lossless = on.then(LosslessMeter::default);
            }
            Err(TryRecvError::Empty) => {}
        }

        let st = match state.as_mut() {
            Some(s) => s,
            None => {
                if !retry.ready(now_ms()) {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                apply_display(&cfg, &mut display);
                match State::start(&cfg) {
                    Ok(s) => {
                        retry.reset();
                        let _ = events.send(s.configured());
                        state.insert(s)
                    }
                    Err(e) => {
                        retry.failed(now_ms());
                        if retry.should_report() {
                            log::warn!("abrir a captura: {e}");
                            let _ = events.send(EngineEvent::Error(e));
                        }
                        continue;
                    }
                }
            }
        };

        if let Some(line) = shared.stats.take_if_due(clock::now_ns()) {
            // Que encoder (e de que tamanho): o pedido pode ter caído noutro codec.
            let (w, h) = st.encoder.size();
            let line = format!("{} {w}x{h} | {line}", st.encoder.name());
            let line = match cpu.sample() {
                Some(f) => format!("{line} | CPU da máquina {:.0}%", f * 100.0),
                None => line,
            };
            log::info!("{line}");
            let _ = events.send(EngineEvent::Report(line));
        }
        match st.tick(&events, &mut shape_id, &mut shared) {
            Ok(()) => {}
            Err(StepError::AccessLost) => {
                // Solta ANTES de recriar.
                state = None;
                retry.failed(now_ms());
                if retry.should_report() {
                    log::info!("captura perdeu o acesso; recriando");
                }
            }
            Err(StepError::Fatal(e)) => {
                let _ = events.send(EngineEvent::Error(e));
                break;
            }
        }
    }
}

enum StepError {
    AccessLost,
    Fatal(String),
}

/// Tela remota no tamanho e na escala que o viewer pediu (resolução "igual à
/// janela"), ou de volta ao original. Falhar não impede a sessão: segue na
/// resolução que estiver.
fn apply_display(cfg: &EngineConfig, display: &mut DisplayOverride) {
    let Some(want) = cfg.desired else {
        display.restore();
        return;
    };
    let monitors = crate::media::capture::monitors();
    let Some(m) = monitors.iter().find(|m| m.id == cfg.monitor).or(monitors.first()) else { return };
    match display.apply(&m.name, (want.width, want.height), want.scale_pct) {
        Ok(mode) if mode != (m.width, m.height) => log::info!("{}: {}x{} a {}% (pedido {}x{})", m.name, mode.0, mode.1, want.scale_pct, want.width, want.height),
        Ok(_) => {}
        Err(e) => log::warn!("resolução remota: {e}"),
    }
}

struct State {
    cfg: EngineConfig,
    dev: Device,
    capture: ScreenCapture,
    conv: Nv12Converter,
    /// Caminho de CPU (placa de software + encoder de memória, sem escala):
    /// lê o BGRA e o encoder converte. Sem ele, NV12 no shader.
    readback: Option<BgraReadback>,
    encoder: Box<dyn VideoEncoder>,
    pacer: Pacer,
    width: u32,
    height: u32,
    /// O que mudou na tela desde o último quadro (caminho de CPU: só isso é
    /// lido e convertido).
    damage: crate::media::damage::Damage,
}

impl State {
    /// Nova captura + conversor + encoder. O `Pacer` novo começa por keyframe.
    fn start(cfg: &EngineConfig) -> Result<Self, String> {
        let (dev, mut capture) = ScreenCapture::open(cfg.monitor).map_err(|e| e.to_string())?;
        // Sem perda: a tela no tamanho real (não escala; já é 4:4:4) e sempre
        // pelo caminho de CPU (o BGRA lido só nas regiões alteradas).
        let lossless = cfg.codec == Codec::Lossless;
        let (width, height) = if lossless { (capture.image.width, capture.image.height) } else { fit_even(capture.image.width, capture.image.height, cfg.max_width, cfg.max_height) };
        let conv = Nv12Converter::new(&dev, width, height).map_err(|e| e.to_string())?;
        let unscaled = (width, height) == (capture.image.width, capture.image.height);
        // 4:4:4 em vídeo só sai pelo caminho BGRA, que não escala, e só com o x264.
        let full = caps::has_full_chroma(caps::encoders());
        let chroma444 = !lossless && cfg.chroma == Chroma::Yuv444 && unscaled && full;
        if !lossless && cfg.chroma == Chroma::Yuv444 && !chroma444 {
            log::warn!("cor 4:4:4 pedida, mas {}: fica 4:2:0", if full { format!("com escala {width}x{height}") } else { "não há encoder que faça (o x264 não está na build do FFmpeg)".into() });
        }
        let encoder = open_encoder(&dev, cfg, width, height, chroma444)?;
        let readback = if (dev.is_software() || chroma444 || lossless) && encoder.accepts_bgra() && unscaled {
            log::info!("leitura na CPU ({})", if lossless { "sem perda" } else if chroma444 { "cor 4:4:4" } else { "placa de software" });
            Some(BgraReadback::new(&dev, &capture.image).map_err(|e| e.to_string())?)
        } else {
            None
        };
        // Sem perda não refina: a tela parada já chegou exata.
        let refine = if lossless { Refine::Off } else { cfg.refine };
        let pacer = Pacer::new(cfg.max_fps as u32).with_refine(refine.plan());
        capture.partial = readback.is_some();
        Ok(Self { cfg: cfg.clone(), dev, capture, conv, readback, encoder, pacer, width, height, damage: crate::media::damage::Damage::Full })
    }

    fn configured(&self) -> EngineEvent {
        EngineEvent::Configured { width: self.width, height: self.height, codec: self.cfg.codec }
    }

    fn tick(&mut self, events: &Sender<EngineEvent>, shape_id: &mut u32, shared: &mut Shared) -> Result<(), StepError> {
        let wait = self.pacer.wait_hint_ms(clock::now_ns());
        let t_acquire = clock::now_ns();
        let info = match self.capture.acquire(&self.dev, wait) {
            Ok(Some(info)) => info,
            Ok(None) => Default::default(),
            Err(crate::media::MediaError::AccessLost) => return Err(StepError::AccessLost),
            Err(e) => return Err(StepError::Fatal(e.to_string())),
        };
        if info.new_image {
            if shared.lossless.is_some() {
                let (w, h) = (self.capture.image.width as u64, self.capture.image.height as u64);
                shared.stats.lossless.changed_px += info.dirty.as_ref().map_or(w * h, |d| d.iter().map(|r| r.area()).sum());
                shared.stats.lossless.moved_px += info.moved_px;
            }
            self.damage.add(info.dirty.as_deref());
            self.pacer.on_new_image();
            shared.stats.images += 1;
            shared.stats.acquire.add(((clock::now_ns() - t_acquire) / 1000) as u32);
        }
        if let Some(raw) = info.cursor_shape.as_ref() {
            shared.cursor.set_hot(raw.hot_x, raw.hot_y);
        }
        if let Some(cs) = info.cursor_shape.and_then(|raw| cursor_shape(&raw, shape_id)) {
            let _ = events.send(EngineEvent::CursorShape(cs));
        }
        let screen = if info.cursor_pos.is_none() { crate::platform::input::cursor_screen_pos() } else { None };
        let origin = (self.capture.monitor.x, self.capture.monitor.y);
        if let Some((x, y, visible)) = shared.cursor.next(info.cursor_pos, screen, origin) {
            let _ = events.send(EngineEvent::CursorPos { x, y, visible, shape_id: *shape_id });
        }

        let (force_key, refine) = match self.pacer.decide(clock::now_ns(), shared.in_flight.get()) {
            Decision::Wait => return Ok(()),
            Decision::Encode { force_key } => (force_key, false),
            // Mesma imagem (a captura guarda a última): o encoder refina.
            Decision::Refine => (false, true),
        };
        {
            let capture_ns = clock::now_ns();
            let mut measured_us = 0;
            let packets = match &self.readback {
                Some(rb) => {
                    let encoder = &mut self.encoder;
                    let (w, h) = (self.capture.image.width, self.capture.image.height);
                    let regions = self.damage.take().regions(w, h);
                    let regions = regions.as_deref();
                    let (meter, lossless) = (shared.lossless.as_mut(), &mut shared.stats.lossless);
                    rb.read(&self.dev, &self.capture.image.tex, regions, |bgra, stride| {
                        // O híbrido: o que estes mesmos pixels ocupariam sem perda.
                        if let Some(m) = meter.filter(|_| !refine) {
                            let t = clock::now_ns();
                            *lossless += m.measure(bgra, stride, w, h, regions);
                            measured_us = ((clock::now_ns() - t) / 1000) as u32;
                        }
                        encoder.encode_bgra(bgra, stride, regions, force_key)
                    })
                    .map_err(|e| StepError::Fatal(e.to_string()))?
                }
                None => {
                    self.conv.convert(&self.dev, &self.capture.image.srv);
                    self.encoder.encode(&self.dev, &self.conv.nv12, force_key)
                }
            }
            .map_err(|e| StepError::Fatal(e.to_string()))?;
            let timing = self.encoder.last_timing();
            // A medida sem perda não entra no tempo do quadro.
            let total_us = (((clock::now_ns() - capture_ns) / 1000) as u32).saturating_sub(measured_us);
            shared.stats.frames += 1;
            shared.stats.refines += refine as u32;
            shared.stats.prepare.add(total_us.saturating_sub(timing.upload_us + timing.codec_us));
            shared.stats.upload.add(timing.upload_us);
            shared.stats.codec.add(timing.codec_us);
            for p in packets {
                shared.seq += 1;
                shared.stats.bytes += p.data.len() as u64;
                shared.in_flight.sent();
                let _ = events.send(EngineEvent::Frame(VideoFrame {
                    seq: shared.seq,
                    capture_ns,
                    key: p.key,
                    codec: self.cfg.codec,
                    width: self.width,
                    height: self.height,
                    data: p.data,
                }));
            }
        }
        Ok(())
    }
}

fn cursor_shape(raw: &RawCursorShape, shape_id: &mut u32) -> Option<crate::proto::msg::CursorShape> {
    let img = to_image(raw)?;
    *shape_id += 1;
    Some(crate::proto::msg::CursorShape { id: *shape_id, width: img.width, height: img.height, hot_x: img.hot_x, hot_y: img.hot_y, bgra: img.bgra })
}

fn open_encoder(dev: &Device, cfg: &EngineConfig, width: u32, height: u32, chroma444: bool) -> Result<Box<dyn VideoEncoder>, String> {
    // 4:4:4: só o x264 (os de GPU e o openh264 não fazem).
    let hw = cfg.hw_encode && !chroma444;
    let backend = if chroma444 { Some(FULL_CHROMA_BACKEND) } else { caps::backend_for(caps::encoders(), cfg.codec, hw) };
    let backend = backend.ok_or_else(|| format!("sem encoder pra {:?}", cfg.codec))?;
    let ecfg = EncoderConfig {
        codec: cfg.codec,
        backend: backend.to_string(),
        width,
        height,
        fps: cfg.max_fps as u32,
        bitrate_kbps: cfg.bitrate_kbps,
        vbv_frames: cfg.refine.plan().vbv_frames,
        chroma444,
    };
    crate::media::encode::open_encoder(dev, &ecfg).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_report_every_period_and_reset() {
        let mut s = Stats::default();
        assert_eq!(s.take_if_due(1_000), None); // marca o início
        s.images = 10;
        s.frames = 10;
        s.codec.add(20_000);
        s.codec.add(40_000);
        assert_eq!(s.take_if_due(1_000 + Stats::PERIOD_NS - 1), None);
        let line = s.take_if_due(1_000 + Stats::PERIOD_NS).unwrap();
        assert!(line.contains("10 quadros (2.0/s"), "{line}");
        assert!(line.contains("codec 6.0 ms (máx 40.0)"), "{line}");
        assert_eq!(s.frames, 0);
    }

    #[test]
    fn idle_period_logs_nothing() {
        let mut s = Stats::default();
        s.take_if_due(1);
        assert_eq!(s.take_if_due(1 + Stats::PERIOD_NS), None);
    }

    #[test]
    fn cursor_comes_from_capture_or_from_windows_when_capture_is_silent() {
        let mut t = CursorTracker::default();
        // Da captura: sempre manda.
        assert_eq!(t.next(Some((5, 6, true)), None, (0, 0)), Some((5, 6, true)));
        // Sem a da captura: a do Windows, menos o canto do monitor e o ponto de clique.
        t.set_hot(4, 8);
        assert_eq!(t.next(None, Some((2000, 100, true)), (1920, 0)), Some((76, 92, true)));
        // Igual à última: nada.
        assert_eq!(t.next(None, Some((2000, 100, true)), (1920, 0)), None);
        // Escondeu: manda.
        assert_eq!(t.next(None, Some((2000, 100, false)), (1920, 0)), Some((76, 92, false)));
        // Nenhuma fonte: nada.
        assert_eq!(t.next(None, None, (0, 0)), None);
    }

    #[test]
    fn in_flight_counts_and_never_underflows() {
        let a = InFlight::default();
        let b = a.clone();
        a.sent();
        a.sent();
        assert_eq!(b.get(), 2);
        b.done();
        b.done();
        b.done();
        assert_eq!(a.get(), 0);
    }

    #[test]
    fn timer_average() {
        let mut t = Timer::default();
        t.add(1000);
        t.add(3000);
        assert_eq!(t.avg_ms(2), 2.0);
        assert_eq!(t.avg_ms(0), 0.0);
        assert_eq!(t.max_us, 3000);
    }

    #[test]
    fn first_retry_is_immediate_then_doubles_up_to_a_second() {
        let mut b = Backoff::default();
        b.failed(1000);
        assert!(b.ready(1000));
        b.failed(1000);
        assert!(!b.ready(1049) && b.ready(1050));
        for _ in 0..10 {
            b.failed(0);
        }
        assert!(!b.ready(999) && b.ready(1000));
        b.reset();
        assert!(b.ready(0));
    }

    #[test]
    fn reports_first_failure_then_sparsely() {
        let mut b = Backoff::default();
        let mut reported = 0;
        for _ in 0..40 {
            b.failed(0);
            reported += b.should_report() as u32;
        }
        assert_eq!(reported, 3); // 1ª, 20ª e 40ª
    }

    #[test]
    #[ignore = "precisa de GPU, monitor e FFmpeg"]
    fn engine_emits_frames_then_stops() {
        let (ctx, crx) = std::sync::mpsc::channel();
        let (etx, erx) = std::sync::mpsc::channel();
        let cfg = EngineConfig { monitor: 0, codec: Codec::H264, hw_encode: true, max_fps: 30, bitrate_kbps: 6000, max_width: 1280, max_height: 720, refine: Refine::Off, chroma: Chroma::Yuv420, desired: None };
        let h = std::thread::spawn(move || run(cfg, crx, etx, InFlight::default()));
        let mut frames = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if let Ok(EngineEvent::Frame(f)) = erx.recv_timeout(std::time::Duration::from_millis(500)) {
                assert!(!f.data.is_empty());
                frames += 1;
                if frames >= 3 {
                    break;
                }
            }
        }
        ctx.send(EngineCmd::Stop).unwrap();
        h.join().unwrap();
        assert!(frames >= 1, "nenhum quadro em 3 s");
    }
}
