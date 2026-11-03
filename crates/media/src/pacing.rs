//! Quando codificar (§10.4): **nunca enfileirar quadro**.
//!
//! - só codifica se a tela mudou (ou se pediram keyframe);
//! - respeita o fps máximo;
//! - se ainda há quadro esperando a rede, pula: a imagem fica marcada como
//!   suja e o próximo quadro já sai com a tela atual. Pular ANTES do encoder
//!   não quebra a cadeia de referência do H.264;
//! - com refino ([`RefinePlan`]), depois que a tela para, manda mais alguns
//!   quadros da mesma imagem: o encoder usa a cota deles pra apagar o borrão
//!   que ficou do último quadro com mudança.

use reflex_proto::msg::RefinePlan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Wait,
    Encode { force_key: bool },
    /// Mesma imagem de novo, pra melhorar a qualidade com a tela parada.
    Refine,
}

#[derive(Debug, Clone)]
pub struct Pacer {
    min_interval_ns: u64,
    last_encode_ns: Option<u64>,
    dirty: bool,
    key_pending: bool,
    max_in_flight: usize,
    /// Espera depois do último quadro com mudança antes do 1º refino.
    refine_idle_ns: u64,
    refine_frames: u32,
    /// Refinos que faltam nesta parada da tela.
    refine_left: u32,
    /// Já começou a refinar (os seguintes saem no ritmo do fps).
    refining: bool,
}

impl Pacer {
    /// Sem refino (o comportamento original).
    pub fn new(max_fps: u32) -> Self {
        let mut p = Self {
            min_interval_ns: 0,
            last_encode_ns: None,
            dirty: false,
            key_pending: true,
            max_in_flight: 1,
            refine_idle_ns: 0,
            refine_frames: 0,
            refine_left: 0,
            refining: false,
        };
        p.set_fps(max_fps);
        p
    }

    /// Com refino depois que a tela para (inclusive depois do 1º keyframe).
    pub fn with_refine(mut self, plan: RefinePlan) -> Self {
        self.refine_idle_ns = plan.idle_ms as u64 * 1_000_000;
        self.refine_frames = plan.frames;
        self.refine_left = plan.frames;
        self
    }

    pub fn set_fps(&mut self, max_fps: u32) {
        self.min_interval_ns = 1_000_000_000 / max_fps.clamp(1, 240) as u64;
    }

    fn restart_refine(&mut self) {
        self.refine_left = self.refine_frames;
        self.refining = false;
    }

    /// Chegou imagem nova da captura.
    pub fn on_new_image(&mut self) {
        self.dirty = true;
        self.restart_refine();
    }

    /// Viewer pediu keyframe (entrada, perda, troca de codec).
    pub fn request_key(&mut self) {
        self.key_pending = true;
        self.restart_refine();
    }

    /// Intervalo até o próximo refino: o 1º espera a tela parar de vez; os
    /// seguintes vão no ritmo do fps.
    fn refine_gap_ns(&self) -> u64 {
        if self.refining {
            self.min_interval_ns
        } else {
            self.refine_idle_ns.max(self.min_interval_ns)
        }
    }

    /// `in_flight`: quadros codificados que ainda não saíram pra rede.
    pub fn decide(&mut self, now_ns: u64, in_flight: usize) -> Decision {
        if in_flight >= self.max_in_flight {
            return Decision::Wait;
        }
        let since_last = self.last_encode_ns.map(|last| now_ns.saturating_sub(last));
        if self.dirty || self.key_pending {
            if since_last.is_some_and(|d| d < self.min_interval_ns) {
                return Decision::Wait;
            }
            let force_key = self.key_pending;
            self.key_pending = false;
            self.dirty = false;
            self.last_encode_ns = Some(now_ns);
            return Decision::Encode { force_key };
        }
        if self.refine_left > 0 && since_last.is_some_and(|d| d >= self.refine_gap_ns()) {
            self.refine_left -= 1;
            self.refining = true;
            self.last_encode_ns = Some(now_ns);
            return Decision::Refine;
        }
        Decision::Wait
    }

    /// Quanto esperar pela captura antes de reavaliar (ms). Com imagem suja
    /// esperando o intervalo, só até o próximo envio permitido — esperar o
    /// intervalo inteiro de novo atrasaria a imagem (medido na VM: 22 em vez
    /// de 30 quadros/s com a tela mudando sem parar). Idem pro próximo refino.
    pub fn wait_hint_ms(&self, now_ns: u64) -> u32 {
        let full = ((self.min_interval_ns / 1_000_000) as u32).clamp(1, 100);
        let until = |gap: u64, last: u64| (gap.saturating_sub(now_ns.saturating_sub(last)).div_ceil(1_000_000) as u32).clamp(1, full);
        match self.last_encode_ns {
            Some(last) if self.dirty || self.key_pending => until(self.min_interval_ns, last),
            Some(last) if self.refine_left > 0 => until(self.refine_gap_ns(), last),
            _ => full,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn first_frame_is_a_keyframe_even_without_change() {
        let mut p = Pacer::new(60);
        assert_eq!(p.decide(0, 0), Decision::Encode { force_key: true });
        assert_eq!(p.decide(100 * MS, 0), Decision::Wait);
    }

    #[test]
    fn static_screen_sends_nothing() {
        let mut p = Pacer::new(60);
        p.decide(0, 0);
        for t in 1..100 {
            assert_eq!(p.decide(t * 20 * MS, 0), Decision::Wait);
        }
    }

    #[test]
    fn respects_max_fps_and_keeps_dirty_image() {
        let mut p = Pacer::new(30);
        p.decide(0, 0);
        p.on_new_image();
        assert_eq!(p.decide(10 * MS, 0), Decision::Wait);
        // A imagem continua suja: sai no próximo tique permitido.
        assert_eq!(p.decide(34 * MS, 0), Decision::Encode { force_key: false });
    }

    #[test]
    fn never_queues_when_network_is_busy() {
        let mut p = Pacer::new(60);
        p.decide(0, 0);
        p.on_new_image();
        assert_eq!(p.decide(100 * MS, 1), Decision::Wait);
        assert_eq!(p.decide(101 * MS, 0), Decision::Encode { force_key: false });
    }

    #[test]
    fn keyframe_request_is_honoured_once() {
        let mut p = Pacer::new(60);
        p.decide(0, 0);
        p.request_key();
        assert_eq!(p.decide(50 * MS, 0), Decision::Encode { force_key: true });
        p.on_new_image();
        assert_eq!(p.decide(100 * MS, 0), Decision::Encode { force_key: false });
    }

    #[test]
    fn wait_hint_is_bounded() {
        assert_eq!(Pacer::new(60).wait_hint_ms(0), 16);
        assert_eq!(Pacer::new(1).wait_hint_ms(0), 100);
    }

    #[test]
    fn dirty_image_waits_only_until_the_next_slot() {
        let mut p = Pacer::new(30);
        p.decide(0, 0);
        // Tela parada: espera o intervalo inteiro por novidade.
        assert_eq!(p.wait_hint_ms(5 * MS), 33);
        // Imagem chegou cedo (10 ms): acorda no ~33 ms pra mandá-la.
        p.on_new_image();
        assert_eq!(p.wait_hint_ms(10 * MS), 24);
        // Intervalo vencido mas rede ocupada: reavalia logo.
        assert_eq!(p.wait_hint_ms(50 * MS), 1);
    }

    fn refine(idle_ms: u32, frames: u32) -> RefinePlan {
        RefinePlan { idle_ms, frames, vbv_frames: 1 }
    }

    #[test]
    fn stopped_screen_is_refined_after_the_idle_wait_then_at_frame_rate() {
        let mut p = Pacer::new(30).with_refine(refine(150, 3));
        assert_eq!(p.decide(0, 0), Decision::Encode { force_key: true });
        // Antes de a tela "parar de vez": nada.
        assert_eq!(p.decide(100 * MS, 0), Decision::Wait);
        assert_eq!(p.decide(150 * MS, 0), Decision::Refine);
        // Os seguintes no ritmo de 30 fps.
        assert_eq!(p.decide(170 * MS, 0), Decision::Wait);
        assert_eq!(p.decide(184 * MS, 0), Decision::Refine);
        assert_eq!(p.decide(218 * MS, 0), Decision::Refine);
        // Três e acabou.
        assert_eq!(p.decide(400 * MS, 0), Decision::Wait);
    }

    #[test]
    fn a_new_change_restarts_the_refinement() {
        let mut p = Pacer::new(30).with_refine(refine(150, 2));
        p.decide(0, 0);
        assert_eq!(p.decide(150 * MS, 0), Decision::Refine);
        // A tela mudou no meio do refino: manda a mudança e recomeça a espera.
        p.on_new_image();
        assert_eq!(p.decide(190 * MS, 0), Decision::Encode { force_key: false });
        assert_eq!(p.decide(250 * MS, 0), Decision::Wait);
        assert_eq!(p.decide(340 * MS, 0), Decision::Refine);
        assert_eq!(p.decide(380 * MS, 0), Decision::Refine);
        assert_eq!(p.decide(600 * MS, 0), Decision::Wait);
    }

    #[test]
    fn refinement_never_queues_and_hints_its_own_wait() {
        let mut p = Pacer::new(30).with_refine(refine(150, 1));
        p.decide(0, 0);
        assert_eq!(p.wait_hint_ms(100 * MS), 33);
        assert_eq!(p.wait_hint_ms(140 * MS), 10);
        assert_eq!(p.decide(200 * MS, 1), Decision::Wait);
        assert_eq!(p.decide(201 * MS, 0), Decision::Refine);
    }

    #[test]
    fn refine_off_is_the_original_behaviour() {
        let mut p = Pacer::new(30).with_refine(RefinePlan { idle_ms: 0, frames: 0, vbv_frames: 1 });
        p.decide(0, 0);
        for t in 1..50 {
            assert_eq!(p.decide(t * 40 * MS, 0), Decision::Wait);
        }
    }
}
