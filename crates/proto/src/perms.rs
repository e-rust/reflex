//! Níveis de acesso e permissões (§11.2).

use serde::{Deserialize, Serialize};

/// Nível de um usuário: um atalho pra um conjunto de permissões.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Level {
    /// Só vê a tela.
    View,
    /// Teclado, mouse, clipboard, arquivos e tela de login.
    FullControl,
    /// Tudo, mais administrar a máquina remotamente.
    Master,
}

impl Level {
    /// Permissões padrão do nível.
    pub fn default_perms(self) -> Perms {
        match self {
            Level::View => Perms::VIEW,
            Level::FullControl => Perms::VIEW | Perms::INPUT | Perms::CLIPBOARD | Perms::FILES | Perms::SECURE_DESKTOP,
            Level::Master => Perms::ALL,
        }
    }

    pub fn is_master(self) -> bool {
        self == Level::Master
    }
}

/// Conjunto de permissões (bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Perms(u32);

impl Perms {
    pub const NONE: Perms = Perms(0);
    pub const VIEW: Perms = Perms(1 << 0);
    pub const INPUT: Perms = Perms(1 << 1);
    pub const CLIPBOARD: Perms = Perms(1 << 2);
    pub const FILES: Perms = Perms(1 << 3);
    /// Tela de login, UAC e Ctrl+Alt+Del.
    pub const SECURE_DESKTOP: Perms = Perms(1 << 4);
    /// Configurar a máquina e gerenciar usuários (só master).
    pub const ADMIN: Perms = Perms(1 << 5);
    pub const ALL: Perms = Perms((1 << 6) - 1);

    pub fn contains(self, other: Perms) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn bits(self) -> u32 {
        self.0
    }

    pub fn from_bits(bits: u32) -> Perms {
        Perms(bits & Perms::ALL.0)
    }

    /// Remove `other`.
    pub fn without(self, other: Perms) -> Perms {
        Perms(self.0 & !other.0)
    }
}

impl std::ops::BitOr for Perms {
    type Output = Perms;
    fn bitor(self, rhs: Perms) -> Perms {
        Perms(self.0 | rhs.0)
    }
}

impl std::ops::BitAnd for Perms {
    type Output = Perms;
    fn bitand(self, rhs: Perms) -> Perms {
        Perms(self.0 & rhs.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_only_sees_the_screen() {
        let p = Level::View.default_perms();
        assert!(p.contains(Perms::VIEW));
        assert!(!p.contains(Perms::INPUT));
        assert!(!p.contains(Perms::ADMIN));
    }

    #[test]
    fn full_control_has_everything_but_admin() {
        let p = Level::FullControl.default_perms();
        assert!(p.contains(Perms::VIEW | Perms::INPUT | Perms::CLIPBOARD | Perms::FILES | Perms::SECURE_DESKTOP));
        assert!(!p.contains(Perms::ADMIN));
    }

    #[test]
    fn master_has_all() {
        assert_eq!(Level::Master.default_perms(), Perms::ALL);
        assert!(Perms::ALL.contains(Perms::ADMIN));
    }

    #[test]
    fn from_bits_drops_unknown_bits() {
        assert_eq!(Perms::from_bits(u32::MAX), Perms::ALL);
        assert!(!Perms::ALL.without(Perms::ADMIN).contains(Perms::ADMIN));
    }
}
