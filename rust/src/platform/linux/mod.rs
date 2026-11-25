//! Integração com o Linux (docs/linux.md):
//! - [`mutter`]: a sessão de acesso remoto do GNOME (tela, teclado, mouse);
//! - [`input`]: o input do viewer injetado pelo Mutter;
//! - [`paths`]: pastas do Reflex (XDG);
//! - [`secret`]: segredos em disco, num arquivo que só o dono lê;
//! - [`service_ctrl`]: o serviço do usuário no systemd;
//! - [`cpu`]: uso de CPU (`/proc/stat`);
//! - [`clipboard`]: o clipboard (ainda sem sincronizar);
//! - `desktop`, `display`, `power`, `timer`: o que o motor do Windows ajusta
//!   na tela e que no Linux não se aplica ou ainda não existe.

pub mod clipboard;
pub mod cpu;
pub mod input;
pub mod mutter;
pub mod paths;
pub mod secret;
pub mod service_ctrl;
mod session_screen;

pub mod desktop {
    pub use super::session_screen::DesktopFollower;
}

pub mod display {
    pub use super::session_screen::DisplayOverride;
}

pub mod power {
    pub use super::session_screen::{wake_display, DisplayAwake};
}

pub mod timer {
    pub use super::session_screen::HighResTimer;
}
