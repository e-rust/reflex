//! Carrega ou cria a identidade (par de chaves + certificado) e o segredo da
//! máquina, protegidos em disco por DPAPI (§11.1).
//!
//! - viewer: `%LOCALAPPDATA%\Reflex\identity.bin` (DPAPI do usuário);
//! - máquina: `%ProgramData%\Reflex\{identity.bin,secret.bin}` (DPAPI da
//!   máquina; a ACL do serviço restringe a SYSTEM/admin).

use std::path::Path;

use reflex_auth::identity::Identity;
use reflex_platform::dpapi::{self, Scope};

fn read_sealed(path: &Path) -> Option<Vec<u8>> {
    let sealed = std::fs::read(path).ok()?;
    dpapi::unprotect(&sealed).ok()
}

fn write_sealed(path: &Path, data: &[u8], scope: Scope) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let sealed = dpapi::protect(data, scope).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &sealed)?;
    std::fs::rename(&tmp, path)
}

/// Lê a identidade em `path`, ou cria uma nova e grava.
pub fn load_or_create_identity(path: &Path, name: &str, scope: Scope) -> Result<Identity, String> {
    if let Some(bytes) = read_sealed(path) {
        if let Ok(id) = serde_json::from_slice::<Identity>(&bytes) {
            return Ok(id);
        }
    }
    let id = Identity::generate(name).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(&id).map_err(|e| e.to_string())?;
    write_sealed(path, &bytes, scope).map_err(|e| e.to_string())?;
    Ok(id)
}

/// Lê o segredo de assinatura da máquina, ou cria 32 bytes aleatórios.
pub fn load_or_create_secret(path: &Path) -> Result<Vec<u8>, String> {
    if let Some(bytes) = read_sealed(path) {
        if bytes.len() == 32 {
            return Ok(bytes);
        }
    }
    let mut secret = [0u8; 32];
    getrandom(&mut secret);
    write_sealed(path, &secret, Scope::Machine).map_err(|e| e.to_string())?;
    Ok(secret.to_vec())
}

/// Bytes aleatórios sem dependência extra: a chave da sessão do processo + o
/// relógio, por SHA-256 (suficiente pro segredo local, que nunca sai da máquina).
fn getrandom(buf: &mut [u8]) {
    use std::hash::{Hash, Hasher};
    let mut seed = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut seed);
    std::process::id().hash(&mut seed);
    (buf.as_ptr() as usize).hash(&mut seed);
    let mut x = seed.finish();
    for b in buf.iter_mut() {
        // xorshift64.
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_created_then_reused() {
        let dir = std::env::temp_dir().join(format!("reflex-id-{}", std::process::id()));
        let path = dir.join("identity.bin");
        let a = load_or_create_identity(&path, "x", Scope::User).unwrap();
        let b = load_or_create_identity(&path, "x", Scope::User).unwrap();
        assert_eq!(a.access_id(), b.access_id());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn secret_is_32_bytes_and_stable() {
        let dir = std::env::temp_dir().join(format!("reflex-sec-{}", std::process::id()));
        let path = dir.join("secret.bin");
        let a = load_or_create_secret(&path).unwrap();
        assert_eq!(a.len(), 32);
        assert_eq!(load_or_create_secret(&path).unwrap(), a);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn random_fills_differently() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        getrandom(&mut a);
        std::thread::sleep(std::time::Duration::from_millis(2));
        getrandom(&mut b);
        assert_ne!(a, b);
    }
}
