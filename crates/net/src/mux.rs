//! Multiplexador de streams sobre UM fluxo de bytes (o TCP+TLS de reserva).
//!
//! Dá ao TCP o mesmo formato do QUIC: vários streams bi/unidirecionais e
//! datagramas. Cada quadro: `id: u32`, `tipo: u8`, `tamanho: u32`, dados.
//! Cada stream lógico é um `tokio::io::duplex`: o app lê/escreve numa ponta e
//! duas tarefas levam a outra ponta de/para os quadros.
//!
//! Sem controle de fluxo por stream (os canais são limitados; um stream lento
//! segura os outros — aceitável pra uma reserva em LAN).

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::conn::{BoxRead, BoxWrite};

const OPEN_BI: u8 = 1;
const OPEN_UNI: u8 = 2;
const DATA: u8 = 3;
const FIN: u8 = 4;
const DGRAM: u8 = 5;
const CLOSE: u8 = 6;

const MAX_PAYLOAD: usize = 64 * 1024;
const PIPE_BYTES: usize = 256 * 1024;

struct Frame {
    id: u32,
    kind: u8,
    payload: Vec<u8>,
}

struct Shared {
    out: mpsc::Sender<Frame>,
    streams: Mutex<HashMap<u32, mpsc::Sender<Vec<u8>>>>,
    next_id: AtomicU32,
}

impl Shared {
    /// Liga um stream lógico aos quadros e devolve as pontas do app.
    fn attach(self: &Arc<Self>, id: u32) -> (BoxWrite, BoxRead) {
        let (app, mux_end) = tokio::io::duplex(PIPE_BYTES);
        let (mut from_app, mut to_app) = tokio::io::split(mux_end);
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
        self.streams.lock().expect("mutex").insert(id, tx);

        // Quadros recebidos → app. Canal fechado (FIN) → EOF pro app.
        tokio::spawn(async move {
            while let Some(buf) = rx.recv().await {
                if to_app.write_all(&buf).await.is_err() {
                    break;
                }
            }
            let _ = to_app.shutdown().await;
        });

        // App → quadros. EOF do app → FIN.
        let out = self.out.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; MAX_PAYLOAD];
            loop {
                match from_app.read(&mut buf).await {
                    Ok(0) | Err(_) => {
                        let _ = out.send(Frame { id, kind: FIN, payload: Vec::new() }).await;
                        break;
                    }
                    Ok(n) => {
                        if out.send(Frame { id, kind: DATA, payload: buf[..n].to_vec() }).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let (r, w) = tokio::io::split(app);
        (Box::new(w), Box::new(r))
    }

    fn close_all(&self) {
        self.streams.lock().expect("mutex").clear();
    }
}

pub struct Mux {
    shared: Arc<Shared>,
    bi_rx: tokio::sync::Mutex<mpsc::Receiver<(BoxWrite, BoxRead)>>,
    uni_rx: tokio::sync::Mutex<mpsc::Receiver<BoxRead>>,
    dgram_rx: tokio::sync::Mutex<mpsc::Receiver<Vec<u8>>>,
}

fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "conexão fechada")
}

impl Mux {
    /// `initiator`: o lado que conectou (ids ímpares); o outro usa pares.
    pub fn new<R, W>(reader: R, writer: W, initiator: bool) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let (out_tx, out_rx) = mpsc::channel::<Frame>(256);
        let shared = Arc::new(Shared { out: out_tx, streams: Mutex::new(HashMap::new()), next_id: AtomicU32::new(if initiator { 1 } else { 2 }) });
        let (bi_tx, bi_rx) = mpsc::channel(16);
        let (uni_tx, uni_rx) = mpsc::channel(16);
        let (dg_tx, dg_rx) = mpsc::channel(64);

        tokio::spawn(write_loop(writer, out_rx));
        tokio::spawn(read_loop(reader, shared.clone(), bi_tx, uni_tx, dg_tx));

        Self { shared, bi_rx: tokio::sync::Mutex::new(bi_rx), uni_rx: tokio::sync::Mutex::new(uni_rx), dgram_rx: tokio::sync::Mutex::new(dg_rx) }
    }

    fn next_id(&self) -> u32 {
        self.shared.next_id.fetch_add(2, Ordering::Relaxed)
    }

    pub async fn open_bi(&self) -> io::Result<(BoxWrite, BoxRead)> {
        let id = self.next_id();
        let ends = self.shared.attach(id);
        self.shared.out.send(Frame { id, kind: OPEN_BI, payload: Vec::new() }).await.map_err(|_| closed())?;
        Ok(ends)
    }

    pub async fn open_uni(&self) -> io::Result<BoxWrite> {
        let id = self.next_id();
        let (w, _r) = self.shared.attach(id);
        self.shared.out.send(Frame { id, kind: OPEN_UNI, payload: Vec::new() }).await.map_err(|_| closed())?;
        Ok(w)
    }

    pub async fn accept_bi(&self) -> io::Result<(BoxWrite, BoxRead)> {
        self.bi_rx.lock().await.recv().await.ok_or_else(closed)
    }

    pub async fn accept_uni(&self) -> io::Result<BoxRead> {
        self.uni_rx.lock().await.recv().await.ok_or_else(closed)
    }

    /// Não bloqueia: fila cheia = datagrama descartado (como no QUIC).
    pub fn send_datagram(&self, data: Vec<u8>) -> io::Result<()> {
        match self.shared.out.try_send(Frame { id: 0, kind: DGRAM, payload: data }) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(closed()),
        }
    }

    pub async fn read_datagram(&self) -> io::Result<Vec<u8>> {
        self.dgram_rx.lock().await.recv().await.ok_or_else(closed)
    }

    pub fn close(&self) {
        let _ = self.shared.out.try_send(Frame { id: 0, kind: CLOSE, payload: Vec::new() });
        self.shared.close_all();
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(mut w: W, mut rx: mpsc::Receiver<Frame>) {
    while let Some(f) = rx.recv().await {
        let mut head = [0u8; 9];
        head[..4].copy_from_slice(&f.id.to_le_bytes());
        head[4] = f.kind;
        head[5..].copy_from_slice(&(f.payload.len() as u32).to_le_bytes());
        let ok = w.write_all(&head).await.is_ok() && w.write_all(&f.payload).await.is_ok() && w.flush().await.is_ok();
        if !ok || f.kind == CLOSE {
            break;
        }
    }
    let _ = w.shutdown().await;
}

async fn read_loop<R: AsyncRead + Unpin>(
    mut r: R,
    shared: Arc<Shared>,
    bi_tx: mpsc::Sender<(BoxWrite, BoxRead)>,
    uni_tx: mpsc::Sender<BoxRead>,
    dg_tx: mpsc::Sender<Vec<u8>>,
) {
    loop {
        let mut head = [0u8; 9];
        if r.read_exact(&mut head).await.is_err() {
            break;
        }
        let id = u32::from_le_bytes(head[..4].try_into().expect("4 bytes"));
        let kind = head[4];
        let len = u32::from_le_bytes(head[5..].try_into().expect("4 bytes")) as usize;
        if len > MAX_PAYLOAD.max(reflex_proto::frame::MAX_FRAME) {
            break;
        }
        let mut payload = vec![0u8; len];
        if r.read_exact(&mut payload).await.is_err() {
            break;
        }
        match kind {
            OPEN_BI => {
                let ends = shared.attach(id);
                if bi_tx.send(ends).await.is_err() {
                    break;
                }
            }
            OPEN_UNI => {
                let (_w, r) = shared.attach(id);
                if uni_tx.send(r).await.is_err() {
                    break;
                }
            }
            DATA => {
                let tx = shared.streams.lock().expect("mutex").get(&id).cloned();
                if let Some(tx) = tx {
                    let _ = tx.send(payload).await;
                }
            }
            FIN => {
                shared.streams.lock().expect("mutex").remove(&id);
            }
            DGRAM => {
                let _ = dg_tx.try_send(payload);
            }
            CLOSE => break,
            _ => {}
        }
    }
    shared.close_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Mux, Mux) {
        let (a, b) = tokio::io::duplex(1 << 20);
        let (ar, aw) = tokio::io::split(a);
        let (br, bw) = tokio::io::split(b);
        (Mux::new(ar, aw, true), Mux::new(br, bw, false))
    }

    #[tokio::test]
    async fn bidirectional_stream_echo() {
        let (a, b) = pair();
        let server = tokio::spawn(async move {
            let (mut w, mut r) = b.accept_bi().await.unwrap();
            let mut buf = [0u8; 5];
            r.read_exact(&mut buf).await.unwrap();
            w.write_all(&buf).await.unwrap();
            w.shutdown().await.unwrap();
            b
        });
        let (mut w, mut r) = a.open_bi().await.unwrap();
        w.write_all(b"hello").await.unwrap();
        let mut back = Vec::new();
        r.read_to_end(&mut back).await.unwrap();
        assert_eq!(back, b"hello");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn uni_stream_carries_large_payload_and_eof() {
        let (a, b) = pair();
        let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
        let expected = data.clone();
        tokio::spawn(async move {
            let mut w = a.open_uni().await.unwrap();
            w.write_all(&data).await.unwrap();
            w.shutdown().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            drop(a);
        });
        let mut r = b.accept_uni().await.unwrap();
        let mut got = Vec::new();
        r.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, expected);
    }

    #[tokio::test]
    async fn many_streams_do_not_mix() {
        let (a, b) = pair();
        let server = tokio::spawn(async move {
            for _ in 0..10 {
                let (mut w, mut r) = b.accept_bi().await.unwrap();
                tokio::spawn(async move {
                    let mut v = Vec::new();
                    r.read_to_end(&mut v).await.unwrap();
                    w.write_all(&v).await.unwrap();
                    w.shutdown().await.unwrap();
                });
            }
            b
        });
        let mut tasks = Vec::new();
        let a = Arc::new(a);
        for i in 0..10u8 {
            let a = a.clone();
            tasks.push(tokio::spawn(async move {
                let (mut w, mut r) = a.open_bi().await.unwrap();
                let msg = vec![i; 10_000];
                w.write_all(&msg).await.unwrap();
                w.shutdown().await.unwrap();
                let mut back = Vec::new();
                r.read_to_end(&mut back).await.unwrap();
                assert_eq!(back, msg);
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn datagrams_arrive() {
        let (a, b) = pair();
        a.send_datagram(vec![1, 2, 3]).unwrap();
        assert_eq!(b.read_datagram().await.unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn close_ends_accept() {
        let (a, b) = pair();
        a.close();
        assert!(b.accept_bi().await.is_err());
    }
}
