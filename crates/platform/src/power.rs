//! Tela acesa durante a sessão. Com o monitor em economia de energia a DXGI
//! não entrega quadro nenhum (medido no PC: só timeouts) ou entrega preto
//! (medido na VM, no bloqueio: quadro todo preto até alguém mexer).
//!
//! - [`DisplayAwake`] impede que a tela apague durante a sessão;
//! - [`wake_display`] religa uma tela que JÁ apagou — o pedido acima não faz
//!   isso; só uma entrada de usuário faz.

use std::time::{Duration, Instant};

use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT};

/// De quanto em quanto tempo o pedido de "tela acesa" é renovado.
const REFRESH: Duration = Duration::from_secs(30);

/// Enquanto existir, a tela e o sistema não dormem. O estado é da THREAD:
/// crie e mantenha na thread de captura e chame [`DisplayAwake::refresh`] no laço.
pub struct DisplayAwake {
    last: Instant,
}

fn request() {
    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED);
    }
}

impl DisplayAwake {
    pub fn new() -> Self {
        request();
        Self { last: Instant::now() }
    }

    /// Renova o pedido (zera o contador de inatividade da tela) a cada [`REFRESH`].
    pub fn refresh(&mut self) {
        if self.last.elapsed() >= REFRESH {
            request();
            self.last = Instant::now();
        }
    }
}

impl Default for DisplayAwake {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for DisplayAwake {
    fn drop(&mut self) {
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}

/// Religa a tela se estiver apagada: um movimento de mouse NULO (0,0) conta
/// como entrada de usuário pro Windows, sem mexer o cursor nem clicar nada.
/// Precisa rodar na área de trabalho de entrada (a thread de captura já está nela).
pub fn wake_display() {
    let nudge = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: 0 } } };
    unsafe {
        SendInput(&[nudge], std::mem::size_of::<INPUT>() as i32);
    }
}
