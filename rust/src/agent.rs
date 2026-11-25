//! Modo `--agent` (§4.1): o servidor do host — escuta QUIC/TCP, autentica,
//! captura e injeta input.
//!
//! - Windows: roda na sessão do console como SYSTEM (o serviço o lança), e
//!   assim enxerga a tela de login (DXGI) e injeta input nela;
//! - Linux: roda como o usuário da sessão gráfica (o systemd do usuário o
//!   lança) e fala com o Mutter pelo D-Bus dela.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use crate::host::server::{self, HostDeps};
use crate::media::caps;
use crate::net::{Listener, LocalCert};
use crate::platform::secret::Scope;
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
    // Linux: a sessão do Mutter abre já (no login a tela está desbloqueada; com
    // ela bloqueada o GNOME não deixaria) e fica aberta.
    #[cfg(target_os = "linux")]
    if let Err(e) = crate::platform::mutter::shared() {
        log::warn!("sessão de acesso remoto: {e} (tenta de novo na 1ª conexão)");
    }
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
        monitors: Arc::new(crate::host::screen::monitors),
        // A confirmação do usuário local é pedida ao processo --user (pipe);
        // sem ele (tela de login), nega por segurança.
        confirm: Arc::new(|_grant| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(false);
            rx
        }),
        next_session_id: Arc::new(AtomicU64::new(0)),
        // Clipboard e arquivos: pelo processo do usuário desta sessão (Windows).
        #[cfg(windows)]
        user: Some(crate::host::user_bridge::UserBridge::start(crate::platform::session::current_session())),
        #[cfg(not(windows))]
        user: None,
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
