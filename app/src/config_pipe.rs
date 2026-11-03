//! Servidor do pipe de configuração (§5.3), no modo `--service`. A aba "Esta
//! máquina" conecta aqui. Leituras (status, listar) são livres; alterações só
//! de um cliente administrador (conferido por `pipe::client_is_admin`).

use reflex_auth::admin::{self, NoSessions};
use reflex_auth::store::Actor;
use reflex_platform::paths::CONFIG_PIPE;
use reflex_platform::pipe::{self};
use reflex_proto::frame;
use reflex_proto::msg::{ConfigRequest, ConfigResponse};
use tokio::net::windows::named_pipe::NamedPipeServer;

/// Laço do servidor de pipe (bloqueante — chame numa thread).
pub fn serve() {
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            log::error!("runtime do pipe: {e}");
            return;
        }
    };
    rt.block_on(async {
        let mut first = true;
        loop {
            let server = match pipe::create_server(CONFIG_PIPE, first, pipe::CONFIG_PIPE_SDDL) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("pipe {CONFIG_PIPE}: {e}");
                    return;
                }
            };
            first = false;
            if server.connect().await.is_err() {
                continue;
            }
            tokio::spawn(handle_client(server));
        }
    });
}

async fn handle_client(mut server: NamedPipeServer) {
    // Admin é por conexão: a identidade do cliente não muda no meio.
    let is_admin = pipe::client_is_admin(&server);
    let actor = if is_admin { Actor::LocalAdmin } else { Actor::LocalUser };

    loop {
        let body = match pipe::read_frame(&mut server).await {
            Ok(b) => b,
            Err(_) => break,
        };
        let req: ConfigRequest = match frame::decode(&body) {
            Ok(r) => r,
            Err(_) => break,
        };
        let resp = respond(req, &actor);
        let Ok(out) = frame::encode_body(&resp) else { break };
        if pipe::write_frame(&mut server, &out).await.is_err() {
            break;
        }
    }
}

fn respond(req: ConfigRequest, actor: &Actor) -> ConfigResponse {
    let now = reflex_auth::unix_now();
    match req {
        ConfigRequest::Status => match crate::hoststore::status(now) {
            Ok(s) => ConfigResponse::Status(s),
            Err(e) => ConfigResponse::Admin(reflex_proto::msg::AdminResponse::Err(e)),
        },
        ConfigRequest::Admin(a) => {
            let store = match crate::hoststore::open_store() {
                Ok(s) => s,
                Err(e) => return ConfigResponse::Admin(reflex_proto::msg::AdminResponse::Err(e)),
            };
            ConfigResponse::Admin(admin::handle(&store, actor, &NoSessions, a, now))
        }
    }
}
