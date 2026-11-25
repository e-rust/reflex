//! Input do viewer injetado no Linux pela sessão do Mutter
//! ([`crate::platform::mutter`]): mouse em pixels do monitor gravado, botões
//! e teclas pelos códigos do evdev. O protocolo manda a tecla como o scancode
//! do PC (conjunto 1) + "estendida"; [`crate::keymap`] traduz.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::keymap::evdev_key;
use crate::platform::mutter::{self, Session, AXIS_HORIZONTAL, AXIS_VERTICAL};
use crate::proto::msg::{Input, MouseButtons};

/// Um retângulo da área de trabalho (o monitor, ou todos).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A área de trabalho: no Linux, o monitor gravado pela sessão.
pub fn virtual_screen() -> Rect {
    mutter::shared().map(|s| Rect { x: s.stream.position.0, y: s.stream.position.1, w: s.stream.size.0 as i32, h: s.stream.size.1 as i32 }).unwrap_or_default()
}

/// A posição do cursor vem no metadado do fluxo: não há o que ler à parte.
pub fn cursor_screen_pos() -> Option<(i32, i32, bool)> {
    None
}

/// O `WHEEL_DELTA` do Windows: um dente da roda.
const WHEEL_DELTA: i32 = 120;

/// Botões do protocolo → códigos do evdev (`BTN_LEFT`...).
const BUTTONS: [(u8, i32); 5] = [(MouseButtons::LEFT, 0x110), (MouseButtons::RIGHT, 0x111), (MouseButtons::MIDDLE, 0x112), (MouseButtons::X1, 0x113), (MouseButtons::X2, 0x114)];

/// Normalizado (0..=65535) no monitor → pixel do monitor. Pura.
pub fn to_pixels(monitor: Rect, nx: u16, ny: u16) -> (f64, f64) {
    let px = nx as f64 * (monitor.w.max(1) - 1) as f64 / 65535.0;
    let py = ny as f64 * (monitor.h.max(1) - 1) as f64 / 65535.0;
    (px.round(), py.round())
}

/// Um evento para o Mutter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Synth {
    Motion(f64, f64),
    Button(i32, bool),
    Axis(u32, i32),
    Key(u32, bool),
}

/// O que está apertado lá (pra soltar no fim) e o resto da roda. Pura.
#[derive(Debug, Default)]
pub struct InputState {
    buttons: u8,
    keys: BTreeSet<u32>,
    /// Pedaço de dente da roda ainda não mandado (touchpad do viewer), por eixo.
    wheel: (i32, i32),
}

impl InputState {
    /// O que injetar por `input` (monitor em pixels).
    pub fn translate(&mut self, input: Input, monitor: Rect) -> Vec<Synth> {
        match input {
            Input::Mouse { x, y, buttons } => {
                let (px, py) = to_pixels(monitor, x, y);
                let mut out = vec![Synth::Motion(px, py)];
                for (bit, code) in BUTTONS {
                    let (was, is) = (self.buttons & bit != 0, buttons.is_down(bit));
                    if was != is {
                        out.push(Synth::Button(code, is));
                    }
                }
                self.buttons = buttons.0;
                out
            }
            Input::Wheel { dx, dy } => {
                // No Windows, positivo é pra cima; no Mutter, pra baixo.
                self.wheel.0 += dx as i32;
                self.wheel.1 -= dy as i32;
                let (sx, sy) = (self.wheel.0 / WHEEL_DELTA, self.wheel.1 / WHEEL_DELTA);
                self.wheel.0 -= sx * WHEEL_DELTA;
                self.wheel.1 -= sy * WHEEL_DELTA;
                [(AXIS_HORIZONTAL, sx), (AXIS_VERTICAL, sy)].into_iter().filter(|(_, s)| *s != 0).map(|(a, s)| Synth::Axis(a, s)).collect()
            }
            Input::Key { scancode, extended, down } => match evdev_key(scancode, extended) {
                Some(code) => {
                    if down {
                        self.keys.insert(code);
                    } else {
                        self.keys.remove(&code);
                    }
                    vec![Synth::Key(code, down)]
                }
                None => {
                    log::debug!("tecla sem equivalente no Linux: {scancode:#x} (estendida: {extended})");
                    Vec::new()
                }
            },
            Input::ReleaseAll => self.release_all(),
            // As travas daqui não se leem pelo Mutter: ficam como estão lá.
            Input::Locks { .. } => Vec::new(),
        }
    }

    /// Solta tudo o que está apertado.
    pub fn release_all(&mut self) -> Vec<Synth> {
        let mut out: Vec<Synth> = std::mem::take(&mut self.keys).into_iter().map(|k| Synth::Key(k, false)).collect();
        out.extend(BUTTONS.iter().filter(|(bit, _)| self.buttons & bit != 0).map(|&(_, code)| Synth::Button(code, false)));
        self.buttons = 0;
        out
    }
}

/// Injeta pela sessão do Mutter; solta o que ficou apertado ao acabar.
pub struct Injector {
    session: Option<Arc<Session>>,
    state: InputState,
}

impl Injector {
    pub fn spawn() -> Self {
        let session = mutter::shared().map_err(|e| log::warn!("input: {e}")).ok();
        Self { session, state: InputState::default() }
    }

    /// `monitor` é o monitor gravado (o Mutter mede em pixels dele).
    pub fn apply(&mut self, input: Input, monitor: Rect, _virt: Rect) {
        let events = self.state.translate(input, monitor);
        self.send(&events);
    }

    fn send(&self, events: &[Synth]) {
        let Some(s) = self.session.as_ref() else { return };
        for e in events {
            match *e {
                Synth::Motion(x, y) => s.pointer_motion(x, y),
                Synth::Button(b, down) => s.pointer_button(b, down),
                Synth::Axis(a, steps) => s.pointer_axis(a, steps),
                Synth::Key(k, down) => s.key(k, down),
            }
        }
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        let events = self.state.release_all();
        self.send(&events);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_moves_in_monitor_pixels_and_buttons_only_on_change() {
        let mut s = InputState::default();
        let m = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        assert_eq!(s.translate(Input::Mouse { x: 65535, y: 0, buttons: MouseButtons(0) }, m), vec![Synth::Motion(1919.0, 0.0)]);
        let down = s.translate(Input::Mouse { x: 0, y: 65535, buttons: MouseButtons(MouseButtons::LEFT) }, m);
        assert_eq!(down, vec![Synth::Motion(0.0, 1079.0), Synth::Button(0x110, true)]);
        let up = s.translate(Input::Mouse { x: 0, y: 65535, buttons: MouseButtons(0) }, m);
        assert_eq!(up, vec![Synth::Motion(0.0, 1079.0), Synth::Button(0x110, false)]);
    }

    #[test]
    fn wheel_goes_by_notches_and_up_is_negative() {
        let mut s = InputState::default();
        let m = Rect::default();
        assert_eq!(s.translate(Input::Wheel { dx: 0, dy: 120 }, m), vec![Synth::Axis(AXIS_VERTICAL, -1)]);
        // Meio dente (touchpad): espera completar.
        assert_eq!(s.translate(Input::Wheel { dx: 60, dy: 0 }, m), vec![]);
        assert_eq!(s.translate(Input::Wheel { dx: 60, dy: 0 }, m), vec![Synth::Axis(AXIS_HORIZONTAL, 1)]);
    }

    #[test]
    fn release_all_lifts_keys_and_buttons() {
        let mut s = InputState::default();
        let m = Rect { x: 0, y: 0, w: 10, h: 10 };
        s.translate(Input::Key { scancode: 0x1D, extended: false, down: true }, m);
        s.translate(Input::Mouse { x: 0, y: 0, buttons: MouseButtons(MouseButtons::RIGHT) }, m);
        assert_eq!(s.translate(Input::ReleaseAll, m), vec![Synth::Key(29, false), Synth::Button(0x111, false)]);
        assert_eq!(s.release_all(), vec![]);
    }
}
