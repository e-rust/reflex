//! O que o motor do Windows ajusta na tela e que no Linux não se aplica (ou
//! ainda não existe), com a mesma interface — o motor é um só:
//! - `DesktopFollower`: no Windows segue a área de trabalho de entrada
//!   (Default ↔ Winlogon); no Linux a sessão gráfica não troca;
//! - `DisplayAwake`/`wake_display`: manter a tela acesa (no Linux, depois:
//!   o *inhibit* do GNOME);
//! - `DisplayOverride`: a resolução "igual à janela" (no Linux, depois: a
//!   configuração de monitores do Mutter);
//! - `HighResTimer`: o Linux já dorme com precisão de 1 ms.

/// A área de trabalho de entrada: no Linux não troca.
#[derive(Debug, Default)]
pub struct DesktopFollower;

impl DesktopFollower {
    pub fn new() -> Self {
        Self
    }

    /// Trocou de área de trabalho? Nunca no Linux.
    pub fn sync(&mut self) -> bool {
        false
    }
}

/// Manter a tela acesa durante a sessão (ainda não no Linux).
#[derive(Debug, Default)]
pub struct DisplayAwake;

impl DisplayAwake {
    pub fn new() -> Self {
        Self
    }

    pub fn refresh(&mut self) {}
}

/// Acender a tela apagada (ainda não no Linux).
pub fn wake_display() {}

/// A resolução pedida pelo viewer (ainda não no Linux: fica a da máquina).
#[derive(Debug, Default)]
pub struct DisplayOverride {
    _nada_mudado: (),
}

impl DisplayOverride {
    pub fn apply(&mut self, _device: &str, _size: (u32, u32), _scale_pct: u16) -> Result<(u32, u32), String> {
        Err("mudar a resolução ainda não existe no Linux".into())
    }

    pub fn restore(&mut self) {}
}

/// Resolução de timer: o Linux já tem 1 ms.
#[derive(Debug, Default)]
pub struct HighResTimer;

impl HighResTimer {
    pub fn new() -> Self {
        Self
    }
}
