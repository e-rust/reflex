//! Identidade do viewer (par de chaves + certificado), guardada com DPAPI em
//! `%LOCALAPPDATA%\Reflex\identity.bin`.

use reflex_auth::identity::Identity;
use reflex_net::LocalCert;
use reflex_platform::dpapi::{self, Scope};
use reflex_platform::paths;

/// Carrega a identidade do viewer, criando na 1ª vez.
pub fn viewer_cert() -> Result<LocalCert, String> {
    let path = paths::user_dir().join("identity.bin");
    if let Ok(sealed) = std::fs::read(&path) {
        if let Ok(bytes) = dpapi::unprotect(&sealed) {
            if let Ok(id) = serde_json::from_slice::<Identity>(&bytes) {
                return Ok(LocalCert { cert_der: id.cert_der, key_der: id.key_der });
            }
        }
    }
    let id = Identity::generate("Reflex Viewer").map_err(|e| e.to_string())?;
    if let Ok(bytes) = serde_json::to_vec(&id) {
        if let Ok(sealed) = dpapi::protect(&bytes, Scope::User) {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&path, sealed);
        }
    }
    Ok(LocalCert { cert_der: id.cert_der, key_der: id.key_der })
}
