//! Clipboard no Linux. Ainda não sincroniza: o viewer conecta sem ele (fase 2
//! de docs/linux.md: texto e imagem pelo Wayland). A cópia como viaja é a
//! comum ([`crate::platform::clip`]).

pub use crate::platform::clip::{ClipSnapshot, ClipboardContent, MAX_CLIP_BYTES, utf16_text};

/// Vigia o clipboard daqui e escreve o que vem do outro lado.
pub struct ClipboardWatcher {
    _private: (),
}

impl ClipboardWatcher {
    /// Sem clipboard por enquanto: `None` (quem chama segue sem sincronizar).
    pub fn spawn(_on_change: impl Fn(ClipSnapshot) + Send + 'static) -> Option<Self> {
        log::info!("clipboard: ainda não sincroniza no Linux");
        None
    }

    /// Escreve o que veio do outro lado.
    pub fn set(&self, _snap: ClipSnapshot) {}

    /// Arquivos copiados lá: colar arquivos ainda não existe no Linux (só o
    /// resto da cópia vai).
    pub fn set_with_files<F: ?Sized>(&self, snap: ClipSnapshot, _files: std::sync::Arc<F>) {
        self.set(snap);
    }
}
