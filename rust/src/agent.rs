//! Modo `--agent` (§4.1): roda na sessão do console como SYSTEM. É o servidor
//! do host — escuta QUIC/TCP, autentica, captura e injeta input. Sendo SYSTEM
//! na sessão do console, enxerga a tela de login (DXGI) e injeta input nela.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use crate::host::server::{self, HostDeps};
use crate::media::caps;
use crate::net::{Listener, LocalCert};
use crate::platform::dpapi::Scope;
use crate::platform::paths;
use crate::proto::DEFAULT_PORT;

use crate::hoststore::{hostname, open_store};
use crate::identity_store::load_or_create_identity;

fn machine_cert() -> Result<LocalCert, String> {
    let dir = paths::machine_dir();
    let name = hostname();
    let id = load_or_create_identity(&dir.join("identity.bin"), &name, Scope::Machine)?;
    Ok(LocalCert { cert_der: id.cert_der, key_der: id.key_der })
}

pub fn run() -> i32 {
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            log::error!("runtime: {e}");
            return 1;
        }
    };
    rt.block_on(async {
        match serve().await {
            Ok(()) => 0,
            Err(e) => {
                log::error!("agente: {e}");
                1
            }
        }
    })
}

async fn serve() -> Result<(), String> {
    let cert = machine_cert()?;
    let store = open_store()?;
    let port = store.load().map_err(|e| e.to_string())?.config.port;
    let addr = std::net::SocketAddr::new([0, 0, 0, 0].into(), if port == 0 { DEFAULT_PORT } else { port });
    let listener = Listener::bind(addr, &cert).await.map_err(|e| e.to_string())?;
    log::info!("agente escutando em {addr}");

    let deps = Arc::new(HostDeps {
        store,
        machine_name: hostname(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        encoders: caps::encoders().iter().map(|c| c.cap).collect(),
        monitors: Arc::new(crate::media::capture::monitors),
        // A confirmação do usuário local é pedida ao processo --user (pipe);
        // sem ele (tela de login), nega por segurança.
        confirm: Arc::new(|_grant| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(false);
            rx
        }),
        next_session_id: Arc::new(AtomicU64::new(0)),
        // Clipboard e arquivos: pelo processo do usuário desta sessão.
        user: Some(crate::host::user_bridge::UserBridge::start(crate::platform::session::current_session())),
        hub: Arc::default(),
    });
    server::serve(listener, deps).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_is_not_empty() {
        assert!(!hostname().is_empty());
    }
}
