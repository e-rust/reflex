//! A tela do motor no Linux: o fluxo do PipeWire da sessão do Mutter
//! ([`crate::platform::mutter`]). Sempre o caminho da CPU: a imagem já chega
//! na memória, e o encoder (sem perda ou openh264) recebe o BGRA.

use std::sync::Arc;

use crate::media::damage::Region;
use crate::media::encode::{EncodedPacket, EncoderConfig, VideoEncoder};
use crate::media::frame_info::FrameInfo;
use crate::media::pipewire::PwCapture;
use crate::platform::mutter::{self, Session};
use crate::proto::msg::Monitor;

pub struct Screen {
    session: Arc<Session>,
    capture: PwCapture,
}

/// O monitor gravado pela sessão do Mutter (por enquanto, o principal).
pub fn monitors() -> Vec<Monitor> {
    match mutter::shared() {
        Ok(s) => {
            let st = &s.stream;
            vec![Monitor { id: 0, name: st.name.clone(), x: st.position.0, y: st.position.1, width: st.size.0, height: st.size.1, primary: true, virtual_display: false }]
        }
        Err(e) => {
            log::warn!("monitores: {e}");
            Vec::new()
        }
    }
}

impl Screen {
    /// Abre a tela (só o monitor principal por enquanto: `index` é ignorado).
    pub fn open(_index: u32) -> Result<Self, String> {
        let session = mutter::shared()?;
        let capture = PwCapture::open(session.stream.node).map_err(|e| e.to_string())?;
        Ok(Self { session, capture })
    }

    pub fn image_size(&self) -> (u32, u32) {
        self.capture.size()
    }

    pub fn origin(&self) -> (i32, i32) {
        self.session.stream.position
    }

    /// Sem placa não há escala: a tela vai no tamanho real.
    pub fn fit(&self, _max_w: u32, _max_h: u32) -> (u32, u32) {
        let (w, h) = self.image_size();
        (w & !1, h & !1)
    }

    /// Só o caminho da CPU: o encoder tem que aceitar BGRA.
    pub fn prepare(&mut self, _width: u32, _height: u32, _want_cpu: bool, encoder_bgra: bool) -> Result<bool, String> {
        if encoder_bgra { Ok(true) } else { Err("no Linux o encoder tem que aceitar BGRA (sem placa)".into()) }
    }

    pub fn is_cpu(&self) -> bool {
        true
    }

    pub fn open_encoder(&self, cfg: &EncoderConfig) -> crate::media::Result<Box<dyn VideoEncoder>> {
        crate::media::encode::open_encoder(cfg)
    }

    pub fn acquire(&mut self, wait_ms: u32) -> crate::media::Result<Option<FrameInfo>> {
        self.capture.acquire(wait_ms)
    }

    /// O BGRA (a imagem inteira já está na memória; `regions` não importa).
    pub fn read_bgra<R>(&self, _regions: Option<&[Region]>, f: impl FnOnce(&[u8], usize) -> R) -> crate::media::Result<R> {
        self.capture.read(f)
    }

    /// Não há caminho da placa no Linux.
    pub fn encode_gpu(&mut self, _encoder: &mut dyn VideoEncoder, _force_key: bool) -> crate::media::Result<Vec<EncodedPacket>> {
        Err("sem caminho da placa no Linux".into())
    }
}
