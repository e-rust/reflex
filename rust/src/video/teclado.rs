//! Teclado inteiro pra máquina remota enquanto a janela de vídeo está em
//! primeiro plano — inclusive Windows, Alt+Tab, Ctrl+Esc e Win+atalhos.
//!
//! Sem isto, o `WM_KEYDOWN` da tecla Windows chegava à janela e ia pro host,
//! mas o Windows DESTE PC ficava com a tecla (abria o menu Iniciar daqui) e
//! o "soltar" se perdia: no host a tecla Windows ficava presa e o próximo
//! "+" virava Win+"+", que abre a Lupa (aconteceu na VM). Um gancho de
//! teclado de baixo nível vê cada tecla antes do Windows; com a nossa janela
//! em foco ele manda aperto e soltura pro host e engole a tecla aqui.
//! Ctrl+Alt+Del não passa por gancho nenhum (é do Windows) — fica local.
//! As teclas de trava (Caps, Num e Scroll Lock) passam: mudam a luz daqui e a
//! janela manda o estado delas ([`Input::Locks`]), como o RDP.

use std::sync::Mutex;

use crate::proto::msg::Input;
use tokio::sync::mpsc::UnboundedSender;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_UP, WH_KEYBOARD_LL,
};

/// Pra onde vão as teclas: cada janela de vídeo aberta (só a que está em foco
/// recebe) e o canal de input da conexão dela.
static TARGETS: Mutex<Vec<(isize, UnboundedSender<Input>)>> = Mutex::new(Vec::new());

/// Enquanto existir, o gancho está ativo pra janela `hwnd` (instale e mantenha
/// na thread da interface — o gancho só roda enquanto ela processa mensagens).
/// Com várias janelas, cada uma tem o seu; o primeiro que acha a janela em foco
/// engole a tecla, e os outros nem a veem.
pub struct KeyboardHook {
    hook: HHOOK,
    hwnd: isize,
}

impl KeyboardHook {
    pub fn install(hwnd: isize, input: UnboundedSender<Input>) -> Option<Self> {
        TARGETS.lock().ok()?.push((hwnd, input));
        let hinst = unsafe { GetModuleHandleW(None) }.ok()?;
        match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), Some(hinst.into()), 0) } {
            Ok(hook) => Some(Self { hook, hwnd }),
            Err(_) => {
                forget(hwnd);
                None
            }
        }
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.hook);
        }
        forget(self.hwnd);
    }
}

/// Tira a janela da lista.
fn forget(hwnd: isize) {
    if let Ok(mut t) = TARGETS.lock() {
        t.retain(|(h, _)| *h != hwnd);
    }
}

/// Caps, Num e Scroll Lock (códigos virtuais).
pub fn is_lock_key(vk: u32) -> bool {
    matches!(vk, 0x14 | 0x90 | 0x91)
}

/// Evento do gancho → tecla do protocolo (scancode, estendida, apertada).
/// `None` pra eventos que não são nossos (injetados por outro programa) e
/// pras teclas de trava (seguem aqui; o estado vai depois). Pura.
pub fn key_from_hook(vk: u32, scan_code: u32, flags: u32) -> Option<Input> {
    if flags & LLKHF_INJECTED.0 != 0 || is_lock_key(vk) {
        return None;
    }
    Some(Input::Key { scancode: scan_code as u16, extended: flags & LLKHF_EXTENDED.0 != 0, down: flags & LLKHF_UP.0 == 0 })
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: com HC_ACTION, o lParam do gancho de teclado é um KBDLLHOOKSTRUCT.
        let k = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let foreground = unsafe { GetForegroundWindow() }.0 as isize;
        if let Ok(targets) = TARGETS.lock()
            && let Some((_, input)) = targets.iter().find(|(h, _)| *h == foreground)
            && let Some(key) = key_from_hook(k.vkCode, k.scanCode, k.flags.0)
        {
            let _ = input.send(key);
            // Engole: nem o Windows daqui nem a janela veem a tecla.
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_key_goes_down_and_up_as_extended() {
        // Win esquerda: scancode 0x5B, estendida.
        assert_eq!(key_from_hook(0x5B, 0x5B, LLKHF_EXTENDED.0), Some(Input::Key { scancode: 0x5B, extended: true, down: true }));
        assert_eq!(key_from_hook(0x5B, 0x5B, LLKHF_EXTENDED.0 | LLKHF_UP.0), Some(Input::Key { scancode: 0x5B, extended: true, down: false }));
    }

    #[test]
    fn plain_key_and_injected_events() {
        assert_eq!(key_from_hook(0x41, 0x1E, 0), Some(Input::Key { scancode: 0x1E, extended: false, down: true }));
        // Tecla que outro programa injetou (ou nós mesmos) não vai pro host.
        assert_eq!(key_from_hook(0x41, 0x1E, LLKHF_INJECTED.0), None);
        // Num Lock (que o gancho marca como estendida) e Caps Lock: só o estado vai.
        assert_eq!(key_from_hook(0x90, 0x45, LLKHF_EXTENDED.0), None);
        assert_eq!(key_from_hook(0x14, 0x3A, 0), None);
    }
}
