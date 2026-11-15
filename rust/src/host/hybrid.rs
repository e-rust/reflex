//! Etapa 2 do híbrido (`docs/hibrido.md`): sem perda enquanto a banda deixa,
//! vídeo quando não deixa. Pura: o motor entrega o que aconteceu (quanto cada
//! quadro levou pra sair, quanto o sem perda ocuparia) e pergunta se troca.
//!
//! - **Sem perda → vídeo:** o motor não enfileira (só codifica o próximo
//!   quando o anterior saiu), então banda curta aparece como quadro que
//!   demora a sair. Muitos atrasados na janela: troca, e guarda a banda que a
//!   rede deu (bytes ÷ tempo de envio).
//! - **Vídeo → sem perda:** o motor estima quanto a tela de agora ocuparia
//!   sem perda; cabendo com folga na banda guardada a janela inteira, volta.
//! - **Começo:** em vídeo. O keyframe sem perda (a tela inteira, centenas de KB
//!   a mais de 1 MB) levava segundos num link lento — medido: 12 s a 100 KB/s
//!   antes da 1ª imagem. O keyframe de vídeo é pequeno e mede o link: saindo
//!   rápido (rede local), sem perda em 1 s; a rede segurando, fica no vídeo.
//! - Tempo mínimo em cada modo, pra não oscilar.

use std::collections::VecDeque;

/// O que vai pela rede agora.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Lossless,
    Video,
}

/// Um quadro que saiu: quanto levou (de entregue ao envio até todas as
/// conexões terem escrito) e quantos bytes tinha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivery {
    pub took_ns: u64,
    pub bytes: usize,
}

/// Janela das decisões.
const WINDOW_NS: u64 = 2_000_000_000;
/// Um quadro que levou mais que isto pra sair está atrasado (3 quadros a 30/s).
const LATE_NS: u64 = 100_000_000;
/// Atrasados na janela que fazem trocar pro vídeo (fração e mínimo de
/// quadros: com a rede travada saem poucos).
const LATE_SHARE: f64 = 0.3;
const MIN_SAMPLES: usize = 4;
/// Um quadro que levou isto pra sair já basta (a rede está travada).
const STUCK_NS: u64 = 1_000_000_000;
/// Tempo mínimo em cada modo antes de trocar.
const DWELL_LOSSLESS_NS: u64 = 2_000_000_000;
const DWELL_VIDEO_NS: u64 = 6_000_000_000;
/// Volta pro sem perda se ele ocuparia até esta fração da banda medida.
const BACK_SHARE: f64 = 0.5;
/// Quadro que levou ao menos isto mede a velocidade do link (foi a rede que
/// segurou, não a fila vazia).
const RATE_SAMPLE_NS: u64 = 20_000_000;
/// Peso da medida nova na média da velocidade do link.
const RATE_WEIGHT: f64 = 0.3;
/// Um quadro sem perda que levaria mais que isto pra passar nem sai: vai vídeo.
const TOO_BIG_NS: f64 = 250_000_000.0;
/// No começo (vídeo): sem a rede segurar quadro nenhum até aqui, o link é
/// rápido e o sem perda começa.
const PROBE_NS: u64 = 1_000_000_000;

/// A decisão de modo.
#[derive(Debug)]
pub struct HybridPolicy {
    mode: Mode,
    since_ns: u64,
    /// Sem perda: os quadros que saíram (instante, entrega).
    deliveries: VecDeque<(u64, Delivery)>,
    /// Vídeo: quanto o sem perda ocuparia (instante, bytes).
    estimates: VecDeque<(u64, usize)>,
    /// A banda que a rede deu no sem perda, em bytes/s.
    capacity: Option<f64>,
    /// A velocidade do link, pelos quadros que a rede segurou (média, bytes/s).
    link_rate: Option<f64>,
}

impl HybridPolicy {
    /// Começa no vídeo, medindo o link (ver o começo do módulo).
    pub fn new(now_ns: u64) -> Self {
        Self::starting(Mode::Video, now_ns)
    }

    /// Começa em `mode`.
    pub fn starting(mode: Mode, now_ns: u64) -> Self {
        Self { mode, since_ns: now_ns, deliveries: VecDeque::new(), estimates: VecDeque::new(), capacity: None, link_rate: None }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// A banda medida no sem perda (bytes/s), se já se mediu.
    pub fn capacity(&self) -> Option<f64> {
        self.capacity
    }

    /// Um quadro saiu (a janela só conta no sem perda).
    pub fn delivered(&mut self, now_ns: u64, d: Delivery) {
        if d.took_ns >= RATE_SAMPLE_NS && d.bytes > 0 {
            let rate = d.bytes as f64 / (d.took_ns as f64 / 1e9);
            self.link_rate = Some(self.link_rate.map_or(rate, |r| r + RATE_WEIGHT * (rate - r)));
        }
        if self.mode == Mode::Lossless {
            self.deliveries.push_back((now_ns, d));
        }
    }

    /// No sem perda: um quadro de `bytes` levaria tanto pra passar no link
    /// medido que é melhor nem mandar. Devolve o motivo.
    pub fn too_big(&self, bytes: usize) -> Option<String> {
        let rate = self.link_rate.filter(|_| self.mode == Mode::Lossless)?;
        let ns = bytes as f64 / rate * 1e9;
        (ns > TOO_BIG_NS).then(|| format!("quadro de {} KB levaria {:.1} s a ~{} KB/s", bytes / 1024, ns / 1e9, (rate / 1024.0) as u64))
    }

    /// Troca pro vídeo já (o quadro grande demais), guardando a banda do link.
    pub fn force_video(&mut self, now_ns: u64) {
        self.capacity = self.link_rate.or(self.capacity);
        self.switch(Mode::Video, now_ns);
    }

    /// No vídeo: a tela de agora ocuparia `bytes` sem perda.
    pub fn estimated(&mut self, now_ns: u64, bytes: usize) {
        if self.mode == Mode::Video {
            self.estimates.push_back((now_ns, bytes));
        }
    }

    /// Troca de modo? Devolve o novo, com o motivo pro relatório.
    pub fn decide(&mut self, now_ns: u64) -> Option<(Mode, String)> {
        let start = now_ns.saturating_sub(WINDOW_NS);
        self.deliveries.retain(|(t, _)| *t >= start);
        self.estimates.retain(|(t, _)| *t >= start);
        let dwell = now_ns.saturating_sub(self.since_ns);
        match self.mode {
            Mode::Lossless if dwell >= DWELL_LOSSLESS_NS && !self.deliveries.is_empty() => {
                let late = self.deliveries.iter().filter(|(_, d)| d.took_ns > LATE_NS).count();
                let stuck = self.deliveries.iter().any(|(_, d)| d.took_ns > STUCK_NS);
                let many_late = self.deliveries.len() >= MIN_SAMPLES && (late as f64) >= LATE_SHARE * self.deliveries.len() as f64;
                if !stuck && !many_late {
                    return None;
                }
                let bytes: usize = self.deliveries.iter().map(|(_, d)| d.bytes).sum();
                let took: u64 = self.deliveries.iter().map(|(_, d)| d.took_ns).sum();
                let capacity = bytes as f64 / (took.max(1) as f64 / 1e9);
                self.capacity = Some(capacity);
                let why = format!("{late} de {} quadros atrasados, banda ~{} KB/s", self.deliveries.len(), (capacity / 1024.0) as u64);
                self.switch(Mode::Video, now_ns);
                Some((Mode::Video, why))
            }
            // A rede não segurou quadro nenhum (o link é rápido): sem perda.
            Mode::Video if self.link_rate.is_none() && self.capacity.is_none() && dwell >= PROBE_NS => {
                self.switch(Mode::Lossless, now_ns);
                Some((Mode::Lossless, "a rede não segurou nenhum quadro".into()))
            }
            Mode::Video if dwell >= DWELL_VIDEO_NS => {
                let capacity = self.capacity.or(self.link_rate)?;
                let rate = self.estimates.iter().map(|(_, b)| *b).sum::<usize>() as f64 / (WINDOW_NS as f64 / 1e9);
                if rate > BACK_SHARE * capacity {
                    return None;
                }
                let why = format!("sem perda ocuparia ~{} KB/s de ~{} KB/s", (rate / 1024.0) as u64, (capacity / 1024.0) as u64);
                self.switch(Mode::Lossless, now_ns);
                Some((Mode::Lossless, why))
            }
            _ => None,
        }
    }

    fn switch(&mut self, mode: Mode, now_ns: u64) {
        self.mode = mode;
        self.since_ns = now_ns;
        self.deliveries.clear();
        self.estimates.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;
    const S: u64 = 1_000 * MS;

    /// `n` quadros de `bytes` que levaram `took_ms` cada, a 30/s a partir de `from`.
    fn deliver(p: &mut HybridPolicy, from: u64, n: u64, took_ms: u64, bytes: usize) -> u64 {
        let mut t = from;
        for _ in 0..n {
            t += 33 * MS;
            p.delivered(t, Delivery { took_ns: took_ms * MS, bytes });
        }
        t
    }

    #[test]
    fn a_fast_link_starts_lossless_after_a_second_of_video() {
        let mut p = HybridPolicy::new(0);
        assert_eq!(p.mode(), Mode::Video);
        // O keyframe de vídeo saiu em 2 ms: a rede não segurou.
        p.delivered(50 * MS, Delivery { took_ns: 2 * MS, bytes: 60 * 1024 });
        assert_eq!(p.decide(500 * MS), None, "ainda medindo");
        assert_eq!(p.decide(S).map(|(m, _)| m), Some(Mode::Lossless));
    }

    #[test]
    fn a_slow_link_stays_in_video_from_the_start() {
        let mut p = HybridPolicy::new(0);
        // 60 KB levando 600 ms: ~100 KB/s.
        p.delivered(600 * MS, Delivery { took_ns: 600 * MS, bytes: 60 * 1024 });
        assert_eq!(p.decide(S), None, "a rede segurou: nada de sem perda");
        // A tela continua mexendo: o sem perda ocuparia ~150 KB/s.
        let mut t = S;
        while t < 8 * S {
            t += 33 * MS;
            p.estimated(t, 5 * 1024);
        }
        assert_eq!(p.decide(t), None);
        assert_eq!(p.mode(), Mode::Video);
    }

    #[test]
    fn a_fast_network_stays_lossless() {
        let mut p = HybridPolicy::starting(Mode::Lossless, 0);
        let t = deliver(&mut p, 0, 120, 5, 20_000);
        assert_eq!(p.decide(t), None);
        assert_eq!(p.mode(), Mode::Lossless);
    }

    #[test]
    fn many_late_frames_switch_to_video_and_remember_the_bandwidth() {
        let mut p = HybridPolicy::starting(Mode::Lossless, 0);
        // 100 KB por quadro levando 200 ms: a rede dá ~500 KB/s.
        let t = deliver(&mut p, 0, 90, 200, 100 * 1024);
        let (mode, why) = p.decide(t).expect("troca");
        assert_eq!(mode, Mode::Video);
        assert!(why.contains("atrasados"), "{why}");
        let cap = p.capacity().unwrap();
        assert!((cap - 500.0 * 1024.0).abs() < 1024.0, "{cap}");
    }

    #[test]
    fn it_waits_the_dwell_time_before_leaving_lossless() {
        let mut p = HybridPolicy::starting(Mode::Lossless, 0);
        let t = deliver(&mut p, 0, 30, 200, 1000);
        assert!(t < DWELL_LOSSLESS_NS);
        assert_eq!(p.decide(t), None, "ainda no tempo mínimo");
    }

    #[test]
    fn one_frame_stuck_for_over_a_second_is_enough() {
        let mut p = HybridPolicy::starting(Mode::Lossless, 0);
        let t = 3 * S;
        p.delivered(t, Delivery { took_ns: 1_500 * MS, bytes: 800 * 1024 });
        assert_eq!(p.decide(t).map(|(m, _)| m), Some(Mode::Video));
    }

    #[test]
    fn a_frame_too_big_for_the_measured_link_goes_as_video() {
        let mut p = HybridPolicy::starting(Mode::Lossless, 0);
        assert_eq!(p.too_big(10_000_000), None, "sem medida do link, não adivinha");
        // 30 KB que a rede segurou por 100 ms: ~300 KB/s.
        p.delivered(S, Delivery { took_ns: 100 * MS, bytes: 30 * 1024 });
        assert_eq!(p.too_big(50 * 1024), None, "50 KB passam em ~170 ms");
        let why = p.too_big(500 * 1024).expect("500 KB levariam ~1,7 s");
        assert!(why.contains("levaria"), "{why}");
        p.force_video(S);
        assert_eq!(p.mode(), Mode::Video);
        assert!(p.capacity().is_some_and(|c| (c - 300.0 * 1024.0).abs() < 1024.0));
        assert_eq!(p.too_big(10_000_000), None, "no vídeo não se aplica");
    }

    #[test]
    fn a_fast_lan_never_measures_a_slow_link() {
        let mut p = HybridPolicy::starting(Mode::Lossless, 0);
        // Quadros grandes, mas que saem em 2 ms: a fila estava vazia.
        p.delivered(S, Delivery { took_ns: 2 * MS, bytes: 800 * 1024 });
        assert_eq!(p.too_big(5_000_000), None);
    }

    #[test]
    fn a_few_late_frames_are_not_enough() {
        let mut p = HybridPolicy::starting(Mode::Lossless, 0);
        let t = deliver(&mut p, 0, 80, 5, 1000);
        let t = deliver(&mut p, t, 5, 200, 1000);
        assert_eq!(p.decide(t), None);
    }

    /// Força o vídeo com banda de ~500 KB/s; devolve o instante.
    fn in_video() -> (HybridPolicy, u64) {
        let mut p = HybridPolicy::starting(Mode::Lossless, 0);
        let t = deliver(&mut p, 0, 90, 200, 100 * 1024);
        assert_eq!(p.decide(t).map(|(m, _)| m), Some(Mode::Video));
        (p, t)
    }

    #[test]
    fn video_goes_back_when_lossless_fits_with_room() {
        let (mut p, mut t) = in_video();
        // A tela acalmou: 30 quadros/s de 2 KB = ~60 KB/s, bem menos que a metade.
        while t < 10 * S {
            t += 33 * MS;
            p.estimated(t, 2 * 1024);
        }
        let (mode, why) = p.decide(t).expect("volta");
        assert_eq!(mode, Mode::Lossless);
        assert!(why.contains("ocuparia"), "{why}");
    }

    #[test]
    fn video_stays_while_lossless_would_not_fit() {
        let (mut p, mut t) = in_video();
        // ~300 KB/s: mais que a metade dos 500.
        while t < 10 * S {
            t += 33 * MS;
            p.estimated(t, 10 * 1024);
        }
        assert_eq!(p.decide(t), None);
        assert_eq!(p.mode(), Mode::Video);
    }

    #[test]
    fn video_waits_its_dwell_time_too() {
        let (mut p, t) = in_video();
        assert_eq!(p.decide(t + S), None, "tela parada, mas ainda no tempo mínimo do vídeo");
        assert_eq!(p.decide(t + DWELL_VIDEO_NS).map(|(m, _)| m), Some(Mode::Lossless), "parada e no tempo: volta");
    }
}
