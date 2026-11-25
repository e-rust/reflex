//! Sem ponte com o processo do usuário (Linux): o agente já roda como o
//! usuário da sessão gráfica. O tipo não tem valor nenhum — quem guarda um
//! `Option<Arc<UserBridge>>` tem sempre `None` — e o clipboard do host no
//! Linux virá direto do agente (docs/linux.md).

use std::io;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use crate::net::Connection;
use crate::proto::msg::ClipboardOffer;

pub enum UserBridge {}

impl UserBridge {
    pub fn join(&self, _viewer: u64, _conn: Arc<dyn Connection>, _files: bool) -> mpsc::UnboundedReceiver<ClipboardOffer> {
        match *self {}
    }

    pub fn leave(&self, _viewer: u64) {
        match *self {}
    }

    pub fn remote_copy(&self, _viewer: u64, _offer: ClipboardOffer) {
        match *self {}
    }

    pub async fn files_from_user<W, R>(&self, _viewer: u64, _write: W, _read: R) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
        R: AsyncRead + Unpin,
    {
        match *self {}
    }
}
