//! Seguir a área de trabalho de entrada (§4.1).
//!
//! Na tela de login/bloqueio/UAC a entrada vai pra área de trabalho
//! `Winlogon`. A captura (DXGI) e o `SendInput` só funcionam na área de
//! trabalho da thread, então quem captura chama [`DesktopFollower::sync`] a
//! cada volta: se mudou, a thread troca e a captura é recriada. Só um
//! processo SYSTEM consegue abrir a `Winlogon`.

use windows::Win32::Foundation::GENERIC_ALL;
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, SetThreadDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, HDESK, UOI_NAME};

fn desktop_name(h: HDESK) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut needed = 0u32;
    unsafe { GetUserObjectInformationW(windows::Win32::Foundation::HANDLE(h.0), UOI_NAME, Some(buf.as_mut_ptr() as *mut _), (buf.len() * 2) as u32, Some(&mut needed)) }.ok()?;
    Some(crate::platform::from_wide(&buf))
}

/// Nome da área de trabalho que recebe a entrada agora (`Default`, `Winlogon`…).
pub fn input_desktop_name() -> Option<String> {
    let h = unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(GENERIC_ALL.0)) }.ok()?;
    let name = desktop_name(h);
    unsafe {
        let _ = CloseDesktop(h);
    }
    name
}

/// A área de trabalho é a segura (login, bloqueio, UAC)?
pub fn is_secure_desktop(name: &str) -> bool {
    !name.eq_ignore_ascii_case("Default")
}

/// Mantém a thread na área de trabalho de entrada.
pub struct DesktopFollower {
    current: Option<(HDESK, String)>,
}

impl Default for DesktopFollower {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopFollower {
    pub fn new() -> Self {
        Self { current: None }
    }

    pub fn current_name(&self) -> Option<&str> {
        self.current.as_ref().map(|(_, n)| n.as_str())
    }

    /// Troca a thread pra área de trabalho de entrada se mudou.
    /// `true` = trocou (quem captura tem de recriar a captura).
    pub fn sync(&mut self) -> bool {
        let Ok(h) = (unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(GENERIC_ALL.0)) }) else { return false };
        let name = desktop_name(h).unwrap_or_default();
        if self.current.as_ref().is_some_and(|(_, n)| *n == name) {
            unsafe {
                let _ = CloseDesktop(h);
            }
            return false;
        }
        if unsafe { SetThreadDesktop(h) }.is_err() {
            log::warn!("SetThreadDesktop({name}) falhou");
            unsafe {
                let _ = CloseDesktop(h);
            }
            return false;
        }
        if let Some((old, _)) = self.current.replace((h, name.clone())) {
            unsafe {
                let _ = CloseDesktop(old);
            }
        }
        log::info!("área de trabalho de entrada: {name}");
        true
    }
}

impl Drop for DesktopFollower {
    fn drop(&mut self) {
        if let Some((h, _)) = self.current.take() {
            unsafe {
                let _ = CloseDesktop(h);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_not_secure() {
        assert!(!is_secure_desktop("Default"));
        assert!(!is_secure_desktop("default"));
        assert!(is_secure_desktop("Winlogon"));
        assert!(is_secure_desktop("Screen-saver"));
    }
}
