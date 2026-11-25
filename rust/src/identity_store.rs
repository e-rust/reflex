//! Carrega ou cria a identidade (par de chaves + certificado) e o segredo da
//! máquina, protegidos em disco pelo [`secret`] da plataforma (§11.1): DPAPI
//! no Windows, arquivo `0600` no Linux.
//!
//! - viewer: `identity.bin` na pasta do usuário;
//! - máquina: `identity.bin` e `secret.bin` na pasta da máquina (só o
//!   serviço e os administradores abrem).

use std::path::Path;

use rand::RngCore;

use crate::auth::identity::Identity;
use crate::platform::secret::{self, Scope};

/// Lê a identidade em `path`, ou cria uma nova e grava.
pub fn load_or_create_identity(path: &Path, name: &str, scope: Scope) -> Result<Identity, String> {
    if let Some(bytes) = secret::read(path)
        && let Ok(id) = serde_json::from_slice::<Identity>(&bytes)
    {
        return Ok(id);
    }
    let id = Identity::generate(name).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(&id).map_err(|e| e.to_string())?;
    secret::write(path, &bytes, scope).map_err(|e| e.to_string())?;
    Ok(id)
}

/// Lê o segredo de assinatura da máquina, ou cria 32 bytes aleatórios. (O
/// lado controlado; no Linux, a partir da fase 3.)
#[cfg_attr(not(windows), allow(dead_code))]
pub fn load_or_create_secret(path: &Path) -> Result<Vec<u8>, String> {
    if let Some(bytes) = secret::read(path)
        && bytes.len() == 32
    {
        return Ok(bytes);
    }
    let mut key = [0u8; 32];
    getrandom(&mut key);
    secret::write(path, &key, Scope::Machine).map_err(|e| e.to_string())?;
    Ok(key.to_vec())
}

/// Bytes aleatórios do sistema (o segredo assina os dados do host: não pode
/// ser adivinhado).
#[cfg_attr(not(windows), allow(dead_code))]
fn getrandom(buf: &mut [u8]) {
    rand::rngs::OsRng.fill_bytes(buf);
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
