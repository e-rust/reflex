//! Cliente síncrono do pipe de configuração (§5.3). A aba "Esta máquina" usa
//! isto pra falar com o serviço: abre o pipe, manda um [`ConfigRequest`] e lê
//! o [`ConfigResponse`], com o mesmo enquadramento `u32`+corpo do serviço.
//!
//! Síncrono (std) de propósito: roda nas respostas de evento da interface, sem
//! runtime async. Se o serviço não está instalado/rodando, abrir falha e a aba
//! mostra "acesso desativado".

use std::fs::OpenOptions;
use std::io::{Read, Write};

use crate::platform::paths::CONFIG_PIPE;
use crate::proto::frame;
use crate::proto::msg::{AdminRequest, AdminResponse, ConfigRequest, ConfigResponse, MachineStatus};

#[derive(Debug)]
pub enum ConfigError {
    NotRunning,
    Io(String),
    Protocol(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::NotRunning => write!(f, "o serviço não está em execução"),
            ConfigError::Io(e) => write!(f, "comunicação com o serviço: {e}"),
            ConfigError::Protocol(e) => write!(f, "resposta inesperada: {e}"),
        }
    }
}

/// Uma ida e volta no pipe.
pub fn request(req: &ConfigRequest) -> Result<ConfigResponse, ConfigError> {
    let mut pipe = OpenOptions::new().read(true).write(true).open(CONFIG_PIPE).map_err(|_| ConfigError::NotRunning)?;
    let body = frame::encode_body(req).map_err(|e| ConfigError::Protocol(e.to_string()))?;
    pipe.write_all(&(body.len() as u32).to_le_bytes()).map_err(|e| ConfigError::Io(e.to_string()))?;
    pipe.write_all(&body).map_err(|e| ConfigError::Io(e.to_string()))?;
    pipe.flush().map_err(|e| ConfigError::Io(e.to_string()))?;

    let mut len = [0u8; 4];
    pipe.read_exact(&mut len).map_err(|e| ConfigError::Io(e.to_string()))?;
    let len = u32::from_le_bytes(len) as usize;
    if len > frame::MAX_FRAME {
        return Err(ConfigError::Protocol("resposta grande demais".into()));
    }
    let mut buf = vec![0u8; len];
    pipe.read_exact(&mut buf).map_err(|e| ConfigError::Io(e.to_string()))?;
    frame::decode(&buf).map_err(|e| ConfigError::Protocol(e.to_string()))
}

/// Atalhos tipados.
pub fn status() -> Result<MachineStatus, ConfigError> {
    match request(&ConfigRequest::Status)? {
        ConfigResponse::Status(s) => Ok(s),
        ConfigResponse::NeedsAdmin => Err(ConfigError::Protocol("precisa de administrador".into())),
        ConfigResponse::Admin(AdminResponse::Err(e)) => Err(ConfigError::Protocol(e)),
        other => Err(ConfigError::Protocol(format!("{other:?}"))),
    }
}

pub fn admin(req: AdminRequest) -> Result<AdminResponse, ConfigError> {
    match request(&ConfigRequest::Admin(req))? {
        ConfigResponse::Admin(a) => Ok(a),
        ConfigResponse::NeedsAdmin => Ok(AdminResponse::Err("é preciso executar como administrador".into())),
        other => Err(ConfigError::Protocol(format!("{other:?}"))),
    }
}
