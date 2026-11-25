//! Segredos em disco protegidos pelo Windows (DPAPI): a chave do viewer (por
//! usuário) e o segredo da máquina que assina os dados do host.

use std::path::Path;

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Só o usuário atual consegue abrir.
    User,
    /// Qualquer processo da máquina (o arquivo fica protegido pela ACL).
    Machine,
}

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    // SAFETY: `out` veio do DPAPI: `cbData` bytes em `pbData`, liberados com LocalFree.
    unsafe {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
        v
    }
}

pub fn protect(data: &[u8], scope: Scope) -> Result<Vec<u8>, String> {
    let mut flags = CRYPTPROTECT_UI_FORBIDDEN;
    if scope == Scope::Machine {
        flags |= CRYPTPROTECT_LOCAL_MACHINE;
    }
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(&blob(data), windows::core::w!("reflex"), None, None, None, flags, &mut out).map_err(|e| e.to_string())?;
        Ok(take(out))
    }
}

pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&blob(data), None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).map_err(|e| e.to_string())?;
        Ok(take(out))
    }
}

/// O segredo gravado em `path` (nada se não existe ou não abre).
pub fn read(path: &Path) -> Option<Vec<u8>> {
    unprotect(&std::fs::read(path).ok()?).ok()
}

/// Grava `data` em `path`, selado pelo DPAPI (troca atômica pelo `.tmp`).
pub fn write(path: &Path, data: &[u8], scope: Scope) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let sealed = protect(data, scope).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &sealed)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_user_scope() {
        let sealed = protect(b"segredo", Scope::User).unwrap();
        assert_ne!(sealed, b"segredo");
        assert_eq!(unprotect(&sealed).unwrap(), b"segredo");
    }

    #[test]
    fn tampered_blob_fails() {
        let mut sealed = protect(b"x", Scope::Machine).unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0xFF;
        assert!(unprotect(&sealed).is_err());
    }
}
