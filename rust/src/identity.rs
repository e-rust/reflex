//! Identidade do viewer (par de chaves + certificado), guardada como segredo
//! do usuário em `identity.bin` na pasta dele.

use crate::identity_store::load_or_create_identity;
use crate::net::LocalCert;
use crate::platform::paths;
use crate::platform::secret::Scope;

/// Carrega a identidade do viewer, criando na 1ª vez.
pub fn viewer_cert() -> Result<LocalCert, String> {
    let id = load_or_create_identity(&paths::user_dir().join("identity.bin"), "Reflex Viewer", Scope::User)?;
    Ok(LocalCert { cert_der: id.cert_der, key_der: id.key_der })
}
