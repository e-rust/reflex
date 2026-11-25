//! O sistema operacional (docs/levantamento.md §4, §10.7, §10.8;
//! docs/linux.md).
//!
//! Cada sistema tem a sua pasta, com os mesmos nomes de módulo e a mesma
//! interface: `crate::platform::clipboard`, `crate::platform::paths`... são os
//! do sistema em que o Reflex foi compilado.
//! - `windows/`: a API Win32 (DXGI à parte, em `media`);
//! - `linux/`: systemd, D-Bus e o Mutter (o GNOME no Wayland).
//!
//! Comuns aos dois:
//! - [`clip`]: uma cópia do clipboard como viaja entre as máquinas;
//! - [`files`]: árvore de arquivos copiados e leitura em blocos;
//! - [`service_state`]: o estado do serviço do Reflex.

pub mod clip;
pub mod files;
pub mod service_state;

#[cfg(windows)]
#[path = "windows/mod.rs"]
mod sys;

#[cfg(target_os = "linux")]
#[path = "linux/mod.rs"]
mod sys;

pub use sys::*;
