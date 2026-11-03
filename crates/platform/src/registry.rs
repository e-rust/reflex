//! Valores em HKLM (exige admin): políticas e configuração de drivers.

use windows::core::PCWSTR;
use windows::Win32::System::Registry::{RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE};

use crate::wide;

fn set(path: &str, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> Result<(), String> {
    let (path, name) = (wide(path), wide(name));
    unsafe {
        let mut key = HKEY::default();
        RegCreateKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(path.as_ptr()), None, None, REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None).ok().map_err(|e| format!("RegCreateKeyExW: {e}"))?;
        let r = RegSetValueExW(key, PCWSTR(name.as_ptr()), None, kind, Some(data));
        let _ = RegCloseKey(key);
        r.ok().map_err(|e| format!("RegSetValueExW: {e}"))
    }
}

/// `HKLM\<path>\<name>` = DWORD (cria a chave se faltar).
pub fn set_dword(path: &str, name: &str, value: u32) -> Result<(), String> {
    set(path, name, REG_DWORD, &value.to_le_bytes())
}

/// `HKLM\<path>\<name>` = texto (REG_SZ, cria a chave se faltar).
pub fn set_string(path: &str, name: &str, value: &str) -> Result<(), String> {
    let bytes: Vec<u8> = wide(value).into_iter().flat_map(u16::to_le_bytes).collect();
    set(path, name, REG_SZ, &bytes)
}

/// Apaga `HKLM\<path>` com tudo dentro (ausente = ok).
pub fn delete_tree(path: &str) {
    let path = wide(path);
    unsafe {
        let _ = RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(path.as_ptr()));
    }
}
