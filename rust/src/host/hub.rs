//! Um motor de captura por máquina, compartilhado por todas as conexões.
//!
//! A tela é capturada e codificada UMA vez; cada viewer recebe os mesmos
//! pacotes ([`Feed`]). Isso evita duplicar o trabalho (a parte cara numa
//! máquina fraca) e contorna a DXGI, que não duplica o mesmo monitor duas
//! vezes no mesmo processo (a 2ª conexão ficava sem imagem).
//!
//! - Ritmo: um quadro só conta como entregue quando TODAS as conexões
//!   terminaram de enviá-lo ([`Delivered`]); a mais lenta dita o ritmo e nada
//!   se acumula (o [`crate::host::engine::InFlight`] de sempre).
//! - Quem entra recebe um keyframe na hora e a forma/posição atuais do cursor.
//! - Configuração: vale o último pedido (de qualquer conexão); com codecs
//!   diferentes entre as conexões, H.264 (todo viewer decodifica).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::proto::codec::Codec;
use crate::proto::msg::{CursorShape, VideoFrame};
use tokio::sync::mpsc;

use crate::host::engine::{EngineCmd, EngineConfig, EngineEvent, InFlight};

/// Um quadro codificado, o mesmo pra todas as conexões. Quando a última o
/// solta (enviou ou desistiu), o motor sabe que ele saiu.
pub struct Delivered {
    pub frame: VideoFrame,
    in_flight: InFlight,
}

impl Drop for Delivered {
    fn drop(&mut self) {
        self.in_flight.done();
    }
}

/// O que cada conexão recebe do motor.
pub enum Feed {
    Frame(Arc<Delivered>),
    CursorShape(Arc<CursorShape>),
    CursorPos { x: i32, y: i32, visible: bool, shape_id: u32 },
    /// Medidas do motor (vão pro viewer como `HostReport`).
    Report(Arc<str>),
}

/// Quem recebe, e o que um recém-chegado precisa saber do cursor.
#[derive(Default)]
struct Fanout {
    subs: HashMap<u64, mpsc::UnboundedSender<Feed>>,
    shape: Option<Arc<CursorShape>>,
    pos: Option<(i32, i32, bool, u32)>,
}

impl Fanout {
    fn send(&self, make: impl Fn() -> Feed) {
        for tx in self.subs.values() {
            let _ = tx.send(make());
        }
    }
}

struct Running {
    cmd_tx: std::sync::mpsc::Sender<EngineCmd>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

#[derive(Default)]
struct Inner {
    engine: Option<Running>,
    /// O pedido de cada conexão, com a ordem em que chegou.
    configs: HashMap<u64, (u64, EngineConfig)>,
    stamp: u64,
    current: Option<EngineConfig>,
}

/// O motor da máquina e quem está assistindo.
#[derive(Default)]
pub struct EngineHub {
    inner: Mutex<Inner>,
    fanout: Arc<Mutex<Fanout>>,
}

/// A configuração do motor pra todas as conexões: o pedido mais recente; se
/// os codecs não batem, H.264 (com placa se alguma conexão já o tinha com
/// placa). Pura.
pub fn merge(all: &[&EngineConfig], latest: &EngineConfig) -> EngineConfig {
    let mut eff = latest.clone();
    if all.iter().any(|c| c.codec != latest.codec) {
        eff.codec = Codec::H264;
        eff.hw_encode = all.iter().find(|c| c.codec == Codec::H264).is_some_and(|c| c.hw_encode);
    }
    eff
}

impl EngineHub {
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn fanout(&self) -> std::sync::MutexGuard<'_, Fanout> {
        self.fanout.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Uma conexão começa a assistir com o pedido `cfg`. Devolve o que ela vai
    /// receber e a configuração em vigor.
    pub fn join(&self, id: u64, cfg: EngineConfig) -> (mpsc::UnboundedReceiver<Feed>, EngineConfig) {
        let (tx, rx) = mpsc::unbounded_channel();
        {
            let mut f = self.fanout();
            if let Some(s) = &f.shape {
                let _ = tx.send(Feed::CursorShape(s.clone()));
            }
            if let Some((x, y, visible, shape_id)) = f.pos {
                let _ = tx.send(Feed::CursorPos { x, y, visible, shape_id });
            }
            f.subs.insert(id, tx);
        }
        let eff = self.request(id, cfg);
        // Quem chega no meio precisa de um quadro completo.
        self.request_keyframe();
        (rx, eff)
    }

    /// Novo pedido de uma conexão; devolve a configuração em vigor.
    pub fn request(&self, id: u64, cfg: EngineConfig) -> EngineConfig {
        let mut inner = self.inner();
        inner.stamp += 1;
        let stamp = inner.stamp;
        inner.configs.insert(id, (stamp, cfg.clone()));
        let all: Vec<&EngineConfig> = inner.configs.values().map(|(_, c)| c).collect();
        let eff = merge(&all, &cfg);
        match &inner.engine {
            None => inner.engine = Some(self.start(eff.clone())),
            Some(r) if inner.current.as_ref() != Some(&eff) => {
                let _ = r.cmd_tx.send(EngineCmd::Reconfigure(eff.clone()));
            }
            Some(_) => {}
        }
        inner.current = Some(eff.clone());
        eff
    }

    pub fn request_keyframe(&self) {
        if let Some(r) = &self.inner().engine {
            let _ = r.cmd_tx.send(EngineCmd::RequestKeyframe);
        }
    }

    /// Monitor que o motor está mostrando (pra traduzir o mouse).
    pub fn monitor(&self) -> Option<u32> {
        self.inner().current.as_ref().map(|c| c.monitor)
    }

    /// A conexão saiu. A última para o motor (a resolução volta, a captura
    /// solta o monitor).
    pub fn leave(&self, id: u64) {
        self.fanout().subs.remove(&id);
        let stopped = {
            let mut inner = self.inner();
            inner.configs.remove(&id);
            if inner.configs.is_empty() {
                inner.current = None;
                inner.engine.take()
            } else {
                // Volta a valer o pedido mais recente de quem ficou.
                let all: Vec<&EngineConfig> = inner.configs.values().map(|(_, c)| c).collect();
                let latest = inner.configs.values().max_by_key(|(s, _)| *s).map(|(_, c)| c.clone());
                let eff = latest.map(|l| merge(&all, &l));
                if let (Some(eff), Some(r)) = (eff.as_ref(), inner.engine.as_ref())
                    && inner.current.as_ref() != Some(eff)
                {
                    let _ = r.cmd_tx.send(EngineCmd::Reconfigure(eff.clone()));
                }
                inner.current = eff;
                None
            }
        };
        if let Some(r) = stopped {
            let _ = r.cmd_tx.send(EngineCmd::Stop);
            // Espera soltar a captura: quem conectar logo depois abre de novo.
            for t in r.threads {
                let _ = t.join();
            }
            let mut f = self.fanout();
            (f.shape, f.pos) = (None, None);
        }
    }

    /// Motor + distribuidor (thread que repassa cada evento a todos).
    fn start(&self, cfg: EngineConfig) -> Running {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (ev_tx, ev_rx) = std::sync::mpsc::channel();
        let in_flight = InFlight::default();
        let engine_in_flight = in_flight.clone();
        let engine = std::thread::Builder::new().name("reflex-engine".into()).spawn(move || crate::host::engine::run(cfg, cmd_rx, ev_tx, engine_in_flight));
        let fanout = self.fanout.clone();
        let distributor = std::thread::Builder::new().name("reflex-fanout".into()).spawn(move || distribute(ev_rx, &fanout, &in_flight));
        Running { cmd_tx, threads: [engine, distributor].into_iter().filter_map(Result::ok).collect() }
    }
}

/// Repassa os eventos do motor a todas as conexões até ele parar.
fn distribute(ev_rx: std::sync::mpsc::Receiver<EngineEvent>, fanout: &Mutex<Fanout>, in_flight: &InFlight) {
    let lock = || fanout.lock().unwrap_or_else(|e| e.into_inner());
    while let Ok(ev) = ev_rx.recv() {
        match ev {
            EngineEvent::Frame(frame) => {
                // Sem ninguém (ou quando o último soltar), conta como entregue.
                let d = Arc::new(Delivered { frame, in_flight: in_flight.clone() });
                lock().send(|| Feed::Frame(d.clone()));
            }
            EngineEvent::CursorShape(cs) => {
                let cs = Arc::new(cs);
                let mut f = lock();
                f.shape = Some(cs.clone());
                f.send(|| Feed::CursorShape(cs.clone()));
            }
            EngineEvent::CursorPos { x, y, visible, shape_id } => {
                let mut f = lock();
                f.pos = Some((x, y, visible, shape_id));
                f.send(|| Feed::CursorPos { x, y, visible, shape_id });
            }
            EngineEvent::Report(line) => {
                let line: Arc<str> = line.into();
                lock().send(|| Feed::Report(line.clone()));
            }
            EngineEvent::Error(e) => log::warn!("motor: {e}"),
            EngineEvent::Configured { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::msg::{Chroma, Refine};

    fn cfg(codec: Codec, hw: bool) -> EngineConfig {
        EngineConfig { monitor: 0, codec, hw_encode: hw, max_fps: 30, bitrate_kbps: 8000, max_width: 0, max_height: 0, refine: Refine::default(), chroma: Chroma::default(), desired: None }
    }

    #[test]
    fn same_codec_keeps_the_latest_request() {
        let a = cfg(Codec::Av1, true);
        let mut b = cfg(Codec::Av1, true);
        b.max_fps = 60;
        assert_eq!(merge(&[&a, &b], &b), b);
    }

    #[test]
    fn different_codecs_fall_back_to_h264() {
        let av1 = cfg(Codec::Av1, true);
        let h264 = cfg(Codec::H264, true);
        let m = merge(&[&av1, &h264], &av1);
        assert_eq!((m.codec, m.hw_encode), (Codec::H264, true));
        // Ninguém tinha H.264: software (o openh264).
        let hevc = cfg(Codec::Hevc, true);
        assert!(!merge(&[&av1, &hevc], &hevc).hw_encode);
    }

    #[test]
    fn a_frame_counts_as_sent_when_the_last_viewer_lets_go() {
        let in_flight = InFlight::default();
        in_flight.sent();
        let frame = VideoFrame { seq: 1, capture_ns: 0, key: true, codec: Codec::H264, width: 2, height: 2, data: Vec::new() };
        let d = Arc::new(Delivered { frame, in_flight: in_flight.clone() });
        let (a, b) = (d.clone(), d.clone());
        drop(d);
        drop(a);
        assert_eq!(in_flight.get(), 1, "uma conexão ainda está enviando");
        drop(b);
        assert_eq!(in_flight.get(), 0);
    }
}
