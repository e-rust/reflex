//! A tecla física entre o protocolo e o Linux. No fio vai o scancode do PC
//! (conjunto 1) + "estendida" (o prefixo `E0`); o Linux fala o código do
//! evdev (`KEY_A` = 30...). As não estendidas têm o mesmo número até o F12
//! (0x58); as estendidas e as do ABNT2 vão pela tabela.

/// (scancode, estendida, evdev) das que não são o mesmo número.
const TABLE: [(u16, bool, u32); 20] = [
    (0x73, false, 89),  // KEY_RO (ABNT2 /?)
    (0x7E, false, 121), // KEY_KPCOMMA (ABNT2 . do numérico)
    (0x1C, true, 96),   // KEY_KPENTER
    (0x1D, true, 97),   // KEY_RIGHTCTRL
    (0x35, true, 98),   // KEY_KPSLASH
    (0x37, true, 99),   // KEY_SYSRQ (Print Screen)
    (0x38, true, 100),  // KEY_RIGHTALT (AltGr)
    (0x47, true, 102),  // KEY_HOME
    (0x48, true, 103),  // KEY_UP
    (0x49, true, 104),  // KEY_PAGEUP
    (0x4B, true, 105),  // KEY_LEFT
    (0x4D, true, 106),  // KEY_RIGHT
    (0x4F, true, 107),  // KEY_END
    (0x50, true, 108),  // KEY_DOWN
    (0x51, true, 109),  // KEY_PAGEDOWN
    (0x52, true, 110),  // KEY_INSERT
    (0x53, true, 111),  // KEY_DELETE
    (0x5B, true, 125),  // KEY_LEFTMETA (Windows)
    (0x5C, true, 126),  // KEY_RIGHTMETA
    (0x5D, true, 127),  // KEY_COMPOSE (menu)
];

/// O evdev das travas (o viewer manda o estado delas à parte, não a tecla).
pub const EVDEV_LOCKS: [u32; 3] = [58, 69, 70]; // Caps, Num, Scroll

/// Scancode do PC → evdev. Pura.
pub fn evdev_key(scancode: u16, extended: bool) -> Option<u32> {
    if let Some(&(_, _, code)) = TABLE.iter().find(|&&(s, e, _)| s == scancode && e == extended) {
        return Some(code);
    }
    (!extended && (0x01..=0x58).contains(&scancode)).then_some(scancode as u32)
}

/// evdev → scancode do PC + estendida. Pura.
pub fn pc_scancode(evdev: u32) -> Option<(u16, bool)> {
    if let Some(&(s, e, _)) = TABLE.iter().find(|&&(_, _, c)| c == evdev) {
        return Some((s, e));
    }
    (1..=0x58).contains(&evdev).then_some((evdev as u16, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_scancodes_are_the_evdev_codes_and_extended_ones_go_by_the_table() {
        assert_eq!(evdev_key(0x1E, false), Some(30)); // A
        assert_eq!(evdev_key(0x58, false), Some(88)); // F12
        assert_eq!(evdev_key(0x48, true), Some(103)); // seta pra cima
        assert_eq!(evdev_key(0x48, false), Some(72)); // 8 do numérico
        assert_eq!(evdev_key(0x38, true), Some(100)); // AltGr
        assert_eq!(evdev_key(0x5B, true), Some(125)); // Windows
        assert_eq!(evdev_key(0x73, false), Some(89)); // ABNT2 /?
        assert_eq!(evdev_key(0x00, false), None);
        assert_eq!(evdev_key(0x1E, true), None);
    }

    #[test]
    fn every_key_goes_and_comes_back() {
        for code in (1..=0x58).chain(TABLE.iter().map(|t| t.2)) {
            let (s, e) = pc_scancode(code).unwrap();
            assert_eq!(evdev_key(s, e), Some(code), "evdev {code}");
        }
        assert_eq!(pc_scancode(103), Some((0x48, true)));
        assert_eq!(pc_scancode(500), None);
    }
}
