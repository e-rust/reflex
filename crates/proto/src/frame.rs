//! Enquadramento: `u32` (tamanho, little-endian) + mensagem `postcard`.
//!
//! Funciona sobre qualquer `AsyncRead`/`AsyncWrite` (stream QUIC, TCP ou
//! pipe). Uma mensagem que não decodifica (versão futura) vira
//! [`FrameError::Decode`] sem desalinhar o stream: o tamanho já foi lido,
//! quem chama pode ignorar e seguir.

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maior mensagem aceita (um keyframe 4K cabe com folga).
pub const MAX_FRAME: usize = 32 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("mensagem de {0} bytes passa do limite")]
    TooLarge(usize),
    #[error("mensagem não decodifica: {0}")]
    Decode(postcard::Error),
    #[error("mensagem não codifica: {0}")]
    Encode(postcard::Error),
}

impl FrameError {
    /// O outro lado fechou o stream de forma limpa.
    pub fn is_eof(&self) -> bool {
        matches!(self, FrameError::Io(e) if e.kind() == std::io::ErrorKind::UnexpectedEof)
    }
}

/// Serializa uma mensagem já com o prefixo de tamanho.
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = postcard::to_stdvec(msg).map_err(FrameError::Encode)?;
    if body.len() > MAX_FRAME {
        return Err(FrameError::TooLarge(body.len()));
    }
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Serializa só o corpo (sem o prefixo de tamanho) — pra quem já tem o próprio
/// enquadramento, como o pipe nomeado.
pub fn encode_body<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    postcard::to_stdvec(msg).map_err(FrameError::Encode)
}

/// Decodifica o corpo (sem o prefixo).
pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, FrameError> {
    postcard::from_bytes(body).map_err(FrameError::Decode)
}

pub async fn write_msg<W, T>(w: &mut W, msg: &T) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin + ?Sized,
    T: Serialize,
{
    let bytes = encode(msg)?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_msg<R, T>(r: &mut R) -> Result<T, FrameError>
where
    R: AsyncRead + Unpin + ?Sized,
    T: DeserializeOwned,
{
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(FrameError::TooLarge(len));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    decode(&body)
}

/// Lê a próxima mensagem que decodifica, pulando as desconhecidas.
pub async fn read_known<R, T>(r: &mut R) -> Result<T, FrameError>
where
    R: AsyncRead + Unpin + ?Sized,
    T: DeserializeOwned,
{
    loop {
        match read_msg(r).await {
            Err(FrameError::Decode(_)) => continue,
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::Codec;
    use crate::msg::{Control, Input, MouseButtons, VideoFrame};

    #[tokio::test]
    async fn roundtrip_over_a_pipe() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let sent = Control::Ping { t_ns: 42 };
        write_msg(&mut a, &sent).await.unwrap();
        let got: Control = read_msg(&mut b).await.unwrap();
        assert_eq!(got, sent);
    }

    #[tokio::test]
    async fn large_frame_survives_small_buffer() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let frame = VideoFrame { seq: 7, capture_ns: 1, key: true, codec: Codec::H264, width: 1920, height: 1080, data: vec![0xAB; 100_000] };
        let f2 = frame.clone();
        let w = tokio::spawn(async move { write_msg(&mut a, &f2).await.unwrap() });
        let got: VideoFrame = read_msg(&mut b).await.unwrap();
        w.await.unwrap();
        assert_eq!(got, frame);
    }

    #[tokio::test]
    async fn unknown_message_is_skipped_without_desync() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        // Variante inexistente: índice 200 no enum.
        let mut junk = vec![3u8, 0, 0, 0];
        junk.extend_from_slice(&[200, 1, 1]);
        tokio::io::AsyncWriteExt::write_all(&mut a, &junk).await.unwrap();
        write_msg(&mut a, &Input::Mouse { x: 1, y: 2, buttons: MouseButtons(MouseButtons::LEFT) }).await.unwrap();
        let got: Input = read_known(&mut b).await.unwrap();
        assert_eq!(got, Input::Mouse { x: 1, y: 2, buttons: MouseButtons(1) });
    }

    #[tokio::test]
    async fn rejects_oversized_length() {
        let (mut a, mut b) = tokio::io::duplex(64);
        tokio::io::AsyncWriteExt::write_all(&mut a, &(u32::MAX).to_le_bytes()).await.unwrap();
        let r: Result<Control, _> = read_msg(&mut b).await;
        assert!(matches!(r, Err(FrameError::TooLarge(_))));
    }

    #[tokio::test]
    async fn eof_is_detected() {
        let (a, mut b) = tokio::io::duplex(64);
        drop(a);
        let r: Result<Control, _> = read_msg(&mut b).await;
        assert!(r.unwrap_err().is_eof());
    }
}
