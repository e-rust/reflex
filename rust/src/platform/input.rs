//! Injeção de teclado e mouse no host (§4).
//!
//! [`InputState`] transforma as mensagens do protocolo em eventos sintéticos
//! ([`Synth`]) — guardando o que está pressionado, pra soltar tudo quando o
//! viewer perde o foco — e [`send`] entrega ao Windows (`SendInput`). Mouse
//! em coordenada absoluta da área de trabalho virtual (vários monitores);
//! teclado por scancode (independe do layout ABNT/US).

use std::collections::BTreeSet;

use crate::proto::msg::{Input, MouseButtons};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN};

/// Retângulo em pixels da área de trabalho virtual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A área de trabalho virtual (todos os monitores) agora.
pub fn virtual_screen() -> Rect {
    unsafe { Rect { x: GetSystemMetrics(SM_XVIRTUALSCREEN), y: GetSystemMetrics(SM_YVIRTUALSCREEN), w: GetSystemMetrics(SM_CXVIRTUALSCREEN), h: GetSystemMetrics(SM_CYVIRTUALSCREEN) } }
}

/// Coordenada normalizada (0..=65535) no monitor → absoluta (0..=65535) na
/// área de trabalho virtual, como o `MOUSEEVENTF_VIRTUALDESK` espera.
pub fn to_virtual_absolute(monitor: Rect, virt: Rect, nx: u16, ny: u16) -> (i32, i32) {
    let px = monitor.x as i64 + nx as i64 * (monitor.w.max(1) - 1) as i64 / 65535;
    let py = monitor.y as i64 + ny as i64 * (monitor.h.max(1) - 1) as i64 / 65535;
    let ax = (px - virt.x as i64) * 65535 / (virt.w.max(2) - 1) as i64;
    let ay = (py - virt.y as i64) * 65535 / (virt.h.max(2) - 1) as i64;
    (ax.clamp(0, 65535) as i32, ay.clamp(0, 65535) as i32)
}

/// Evento sintético (independente da API, pra testar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Synth {
    MoveAbsolute { x: i32, y: i32 },
    Button { button: u8, down: bool },
    Wheel { delta: i32, horizontal: bool },
    Key { scancode: u16, extended: bool, down: bool },
    /// Deixar as travas assim (a thread de injeção confere o estado do host).
    Locks { caps: bool, num: bool, scroll: bool },
    /// Tecla por código virtual (as de trava: o Num Lock por scancode se
    /// confunde com o Pause).
    Vk { vk: u16, scancode: u16, extended: bool, down: bool },
}

/// Teclas de trava: (código virtual, scancode, estendida).
const CAPS: (u16, u16, bool) = (0x14, 0x3A, false);
const NUM: (u16, u16, bool) = (0x90, 0x45, true);
const SCROLL: (u16, u16, bool) = (0x91, 0x46, false);

/// Apertos (e solturas) pra levar as travas de `have` a `want`. Pura.
pub fn lock_taps(want: (bool, bool, bool), have: (bool, bool, bool)) -> Vec<Synth> {
    [(want.0 != have.0, CAPS), (want.1 != have.1, NUM), (want.2 != have.2, SCROLL)]
        .into_iter()
        .filter(|(differs, _)| *differs)
        .flat_map(|(_, (vk, scancode, extended))| [Synth::Vk { vk, scancode, extended, down: true }, Synth::Vk { vk, scancode, extended, down: false }])
        .collect()
}

/// Travas ligadas agora no host (Caps, Num, Scroll).
fn host_locks() -> (bool, bool, bool) {
    let on = |vk: u16| unsafe { GetKeyState(vk as i32) } & 1 != 0;
    (on(CAPS.0), on(NUM.0), on(SCROLL.0))
}

/// Troca cada `Locks` pelos apertos que faltam, olhando o host agora.
fn expand_locks(events: Vec<Synth>) -> Vec<Synth> {
    events
        .into_iter()
        .flat_map(|e| match e {
            Synth::Locks { caps, num, scroll } => lock_taps((caps, num, scroll), host_locks()),
            other => vec![other],
        })
        .collect()
}

/// O que está pressionado no host por causa do viewer.
#[derive(Debug, Default)]
pub struct InputState {
    buttons: MouseButtons,
    keys: BTreeSet<(u16, bool)>,
}

const BUTTON_ORDER: [u8; 5] = [MouseButtons::LEFT, MouseButtons::RIGHT, MouseButtons::MIDDLE, MouseButtons::X1, MouseButtons::X2];

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Eventos pra uma mensagem do viewer.
    pub fn translate(&mut self, input: Input, monitor: Rect, virt: Rect) -> Vec<Synth> {
        let mut out = Vec::new();
        match input {
            Input::Mouse { x, y, buttons } => {
                let (ax, ay) = to_virtual_absolute(monitor, virt, x, y);
                out.push(Synth::MoveAbsolute { x: ax, y: ay });
                for b in BUTTON_ORDER {
                    let was = self.buttons.is_down(b);
                    let now = buttons.is_down(b);
                    if was != now {
                        out.push(Synth::Button { button: b, down: now });
                    }
                }
                self.buttons = buttons;
            }
            Input::Wheel { dx, dy } => {
                if dy != 0 {
                    out.push(Synth::Wheel { delta: dy as i32, horizontal: false });
                }
                if dx != 0 {
                    out.push(Synth::Wheel { delta: dx as i32, horizontal: true });
                }
            }
            Input::Key { scancode, extended, down } => {
                if down {
                    self.keys.insert((scancode, extended));
                } else {
                    self.keys.remove(&(scancode, extended));
                }
                out.push(Synth::Key { scancode, extended, down });
            }
            Input::ReleaseAll => out = self.release_all(),
            Input::Locks { caps, num, scroll } => out.push(Synth::Locks { caps, num, scroll }),
        }
        out
    }

    /// Solta tudo o que o viewer deixou pressionado.
    pub fn release_all(&mut self) -> Vec<Synth> {
        let mut out: Vec<Synth> = self.keys.iter().map(|&(scancode, extended)| Synth::Key { scancode, extended, down: false }).collect();
        for b in BUTTON_ORDER {
            if self.buttons.is_down(b) {
                out.push(Synth::Button { button: b, down: false });
            }
        }
        self.keys.clear();
        self.buttons = MouseButtons::default();
        out
    }
}

fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) -> INPUT {
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

fn to_win(s: Synth) -> INPUT {
    match s {
        Synth::MoveAbsolute { x, y } => mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, x, y, 0),
        Synth::Button { button, down } => {
            let (flag, data) = match (button, down) {
                (MouseButtons::LEFT, true) => (MOUSEEVENTF_LEFTDOWN, 0),
                (MouseButtons::LEFT, false) => (MOUSEEVENTF_LEFTUP, 0),
                (MouseButtons::RIGHT, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
                (MouseButtons::RIGHT, false) => (MOUSEEVENTF_RIGHTUP, 0),
                (MouseButtons::MIDDLE, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
                (MouseButtons::MIDDLE, false) => (MOUSEEVENTF_MIDDLEUP, 0),
                (MouseButtons::X1, d) => (if d { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, 1),
                (_, d) => (if d { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, 2),
            };
            mouse(flag, 0, 0, data)
        }
        Synth::Wheel { delta, horizontal } => mouse(if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL }, 0, 0, delta),
        Synth::Vk { vk, scancode, extended, down } => {
            let mut flags = KEYBD_EVENT_FLAGS(0);
            if extended {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            if !down {
                flags |= KEYEVENTF_KEYUP;
            }
            INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scancode, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
        }
        // Resolvido antes de enviar (`expand_locks`); sozinho não faz nada.
        Synth::Locks { .. } => INPUT { r#type: INPUT_KEYBOARD, ..Default::default() },
        Synth::Key { scancode, extended, down } => {
            let mut flags = KEYEVENTF_SCANCODE;
            if extended {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            if !down {
                flags |= KEYEVENTF_KEYUP;
            }
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: scancode, dwFlags: KEYBD_EVENT_FLAGS(flags.0), time: 0, dwExtraInfo: 0 } },
            }
        }
    }
}

/// Entrega os eventos ao Windows. Devolve quantos entraram.
pub fn send(events: &[Synth]) -> u32 {
    if events.is_empty() {
        return 0;
    }
    let inputs: Vec<INPUT> = events.iter().copied().map(to_win).collect();
    unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) }
}

/// Posição do cursor na área de trabalho virtual e se ele está aparecendo.
/// A thread tem de estar na área de trabalho de entrada.
pub fn cursor_screen_pos() -> Option<(i32, i32, bool)> {
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorInfo, CURSORINFO, CURSOR_SHOWING};
    let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
    unsafe { GetCursorInfo(&mut ci) }.ok()?;
    Some((ci.ptScreenPos.x, ci.ptScreenPos.y, ci.flags.0 & CURSOR_SHOWING.0 != 0))
}

/// Modificadores (virtual-key, scancode, estendida): Windows, Ctrl, Alt, Shift.
const MODIFIERS: [(i32, u16, bool); 8] = [
    (0x5B, 0x5B, true),  // VK_LWIN
    (0x5C, 0x5C, true),  // VK_RWIN
    (0xA2, 0x1D, false), // VK_LCONTROL
    (0xA3, 0x1D, true),  // VK_RCONTROL
    (0xA4, 0x38, false), // VK_LMENU (Alt)
    (0xA5, 0x38, true),  // VK_RMENU (AltGr)
    (0xA0, 0x2A, false), // VK_LSHIFT
    (0xA1, 0x36, false), // VK_RSHIFT
];

/// Soltura dos modificadores que `is_down` diz estarem apertados. Pura.
pub fn stuck_modifiers(is_down: impl Fn(i32) -> bool) -> Vec<Synth> {
    MODIFIERS.iter().filter(|(vk, ..)| is_down(*vk)).map(|&(_, scancode, extended)| Synth::Key { scancode, extended, down: false }).collect()
}

/// Thread de injeção que acompanha a área de trabalho de entrada.
///
/// O `SendInput` só chega se a thread que chama está na área de trabalho
/// ativa. Na tela do UAC, de login ou de bloqueio essa é a `Winlogon`, e as
/// threads do tokio ficam na `Default`: o vídeo mostrava o UAC, mas mouse e
/// teclado sumiam (medido na VM). Aqui uma thread só, dona do seu
/// [`DesktopFollower`], troca de área de trabalho antes de cada envio.
///
/// Guarda o que está pressionado ([`InputState`]) e, ao sair de escopo —
/// fim da sessão, inclusive interrompida —, solta tudo: uma tecla Windows
/// presa no host transforma cada "+" digitado depois em Win+"+" (a Lupa).
pub struct Injector {
    state: InputState,
    tx: Option<std::sync::mpsc::Sender<Vec<Synth>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Injector {
    pub fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<Synth>>();
        let thread = std::thread::Builder::new()
            .name("reflex-input".into())
            .spawn(move || {
                let mut follower = crate::platform::desktop::DesktopFollower::new();
                follower.sync();
                // Sessão anterior caiu com tecla apertada? Começa limpo.
                let stuck = stuck_modifiers(|vk| unsafe { GetAsyncKeyState(vk) } < 0);
                if !stuck.is_empty() {
                    log::warn!("teclas presas de antes, soltando: {stuck:?}");
                    send(&stuck);
                }
                while let Ok(events) = rx.recv() {
                    follower.sync();
                    let events = expand_locks(events);
                    // Trocou bem entre o sync e o envio: tenta de novo na nova.
                    if (send(&events) as usize) < events.len() && follower.sync() {
                        send(&events);
                    }
                }
            })
            .ok();
        Self { state: InputState::new(), tx: thread.as_ref().map(|_| tx), thread }
    }

    /// Traduz uma mensagem do viewer e injeta (não bloqueia).
    pub fn apply(&mut self, input: Input, monitor: Rect, virt: Rect) {
        let events = self.state.translate(input, monitor, virt);
        self.send(events);
    }

    /// Enfileira os eventos pra thread de injeção.
    fn send(&self, events: Vec<Synth>) {
        if events.is_empty() {
            return;
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(events);
        }
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        let held = self.state.release_all();
        self.send(held);
        // Fecha o canal: a thread solta o que faltava, sai do `recv` e termina.
        self.tx.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };
    const VIRT: Rect = Rect { x: 0, y: 0, w: 3840, h: 1080 };

    #[test]
    fn corners_map_into_virtual_desktop() {
        assert_eq!(to_virtual_absolute(MON, VIRT, 0, 0), (0, 0));
        let (x, y) = to_virtual_absolute(MON, VIRT, 65535, 65535);
        // Canto direito do 1º monitor ≈ metade da área virtual.
        assert!((32700..=32780).contains(&x), "{x}");
        assert_eq!(y, 65535);
    }

    #[test]
    fn second_monitor_offset() {
        let second = Rect { x: 1920, y: 0, w: 1920, h: 1080 };
        let (x, _) = to_virtual_absolute(second, VIRT, 0, 0);
        assert!((32700..=32790).contains(&x), "{x}");
    }

    #[test]
    fn monitor_left_of_primary_has_negative_origin() {
        let virt = Rect { x: -1920, y: 0, w: 3840, h: 1080 };
        let left = Rect { x: -1920, y: 0, w: 1920, h: 1080 };
        assert_eq!(to_virtual_absolute(left, virt, 0, 0).0, 0);
    }

    #[test]
    fn mouse_state_produces_only_transitions() {
        let mut s = InputState::new();
        let down = s.translate(Input::Mouse { x: 0, y: 0, buttons: MouseButtons(MouseButtons::LEFT) }, MON, VIRT);
        assert_eq!(down[1], Synth::Button { button: MouseButtons::LEFT, down: true });
        let held = s.translate(Input::Mouse { x: 10, y: 10, buttons: MouseButtons(MouseButtons::LEFT) }, MON, VIRT);
        assert_eq!(held.len(), 1); // só o movimento
        let up = s.translate(Input::Mouse { x: 10, y: 10, buttons: MouseButtons(0) }, MON, VIRT);
        assert_eq!(up[1], Synth::Button { button: MouseButtons::LEFT, down: false });
    }

    #[test]
    fn injector_thread_ends_when_dropped() {
        let inj = Injector::spawn();
        inj.send(Vec::new());
        drop(inj); // não pode travar
    }

    #[test]
    fn only_held_modifiers_are_released() {
        assert!(stuck_modifiers(|_| false).is_empty());
        // Win esquerda presa (o caso da Lupa).
        assert_eq!(stuck_modifiers(|vk| vk == 0x5B), vec![Synth::Key { scancode: 0x5B, extended: true, down: false }]);
        assert_eq!(stuck_modifiers(|_| true).len(), 8);
    }

    #[test]
    fn injector_tracks_what_is_held() {
        // Sem injetar nada de verdade: o estado é o que o drop vai soltar.
        let mut s = InputState::new();
        s.translate(Input::Key { scancode: 0x5B, extended: true, down: true }, MON, VIRT);
        assert_eq!(s.release_all(), vec![Synth::Key { scancode: 0x5B, extended: true, down: false }]);
        assert!(s.release_all().is_empty());
    }

    #[test]
    fn release_all_lifts_keys_and_buttons() {
        let mut s = InputState::new();
        s.translate(Input::Key { scancode: 0x1D, extended: false, down: true }, MON, VIRT);
        s.translate(Input::Key { scancode: 0x1E, extended: false, down: true }, MON, VIRT);
        s.translate(Input::Key { scancode: 0x1E, extended: false, down: false }, MON, VIRT);
        s.translate(Input::Mouse { x: 0, y: 0, buttons: MouseButtons(MouseButtons::RIGHT) }, MON, VIRT);
        let out = s.translate(Input::ReleaseAll, MON, VIRT);
        assert_eq!(out, vec![Synth::Key { scancode: 0x1D, extended: false, down: false }, Synth::Button { button: MouseButtons::RIGHT, down: false }]);
        assert!(s.release_all().is_empty());
    }

    #[test]
    fn locks_press_only_what_differs() {
        assert!(lock_taps((true, true, false), (true, true, false)).is_empty());
        // Num Lock desligado lá (tela de login): um aperto, como tecla estendida.
        assert_eq!(
            lock_taps((false, true, false), (false, false, false)),
            vec![Synth::Vk { vk: 0x90, scancode: 0x45, extended: true, down: true }, Synth::Vk { vk: 0x90, scancode: 0x45, extended: true, down: false }]
        );
        assert_eq!(lock_taps((true, false, true), (false, false, false)).len(), 4);
    }

    #[test]
    #[ignore = "mexe no Scroll Lock desta máquina (e desfaz)"]
    fn scroll_lock_follows_the_wanted_state() {
        let before = host_locks();
        let mut inj = Injector::spawn();
        let virt = virtual_screen();
        inj.apply(Input::Locks { caps: before.0, num: before.1, scroll: !before.2 }, virt, virt);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let toggled = host_locks().2;
        inj.apply(Input::Locks { caps: before.0, num: before.1, scroll: before.2 }, virt, virt);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(toggled, !before.2, "o Scroll Lock não mudou");
        assert_eq!(host_locks(), before, "não voltou ao que era");
    }

    #[test]
    fn wheel_both_axes() {
        let mut s = InputState::new();
        assert_eq!(s.translate(Input::Wheel { dx: 120, dy: -120 }, MON, VIRT), vec![Synth::Wheel { delta: -120, horizontal: false }, Synth::Wheel { delta: 120, horizontal: true }]);
    }

    #[test]
    fn key_flags_are_built() {
        let i = to_win(Synth::Key { scancode: 0x4B, extended: true, down: false });
        let ki = unsafe { i.Anonymous.ki };
        assert_eq!(ki.wScan, 0x4B);
        assert!(ki.dwFlags.0 & KEYEVENTF_EXTENDEDKEY.0 != 0);
        assert!(ki.dwFlags.0 & KEYEVENTF_KEYUP.0 != 0);
    }
}
