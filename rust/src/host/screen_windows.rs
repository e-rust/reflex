//! A tela do motor no Windows: DXGI Desktop Duplication num dispositivo
//! D3D11. Dois caminhos: o NV12 feito na placa (shader) vai pro encoder da
//! placa; ou o BGRA lido pela CPU (sem perda, 4:4:4, placa de software) vai
//! pro encoder de memória — e só as áreas alteradas são lidas.

use crate::media::capture::ScreenCapture;
use crate::media::damage::Region;
use crate::media::encode::{EncodedPacket, EncoderConfig, VideoEncoder};
use crate::media::frame_info::FrameInfo;
use crate::media::gpu::{fit_even, BgraReadback, Device, Nv12Converter};
use crate::proto::msg::Monitor;

pub struct Screen {
    dev: Device,
    capture: ScreenCapture,
    conv: Option<Nv12Converter>,
    /// Caminho de CPU: lê o BGRA e o encoder converte. Sem ele, NV12 no shader.
    readback: Option<BgraReadback>,
}

/// Os monitores, na ordem que a captura usa.
pub fn monitors() -> Vec<Monitor> {
    crate::media::capture::monitors()
}

impl Screen {
    /// Abre o monitor `index` (o dispositivo nasce na placa dele).
    pub fn open(index: u32) -> Result<Self, String> {
        let (dev, capture) = ScreenCapture::open(index).map_err(|e| e.to_string())?;
        Ok(Self { dev, capture, conv: None, readback: None })
    }

    pub fn image_size(&self) -> (u32, u32) {
        (self.capture.image.width, self.capture.image.height)
    }

    /// O canto do monitor na área de trabalho virtual.
    pub fn origin(&self) -> (i32, i32) {
        (self.capture.monitor.x, self.capture.monitor.y)
    }

    /// O tamanho que cabe em `max` (par; a placa escala).
    pub fn fit(&self, max_w: u32, max_h: u32) -> (u32, u32) {
        let (w, h) = self.image_size();
        fit_even(w, h, max_w, max_h)
    }

    /// Prepara o caminho pro encoder já aberto: o da CPU quando `want_cpu` (ou
    /// a placa é de software), o encoder aceita BGRA e não há escala. Devolve
    /// se ficou no caminho da CPU.
    pub fn prepare(&mut self, width: u32, height: u32, want_cpu: bool, encoder_bgra: bool) -> Result<bool, String> {
        self.conv = Some(Nv12Converter::new(&self.dev, width, height).map_err(|e| e.to_string())?);
        let unscaled = (width, height) == self.image_size();
        self.readback = if (self.dev.is_software() || want_cpu) && encoder_bgra && unscaled {
            log::info!("leitura na CPU ({})", if want_cpu { "sem perda ou cor 4:4:4" } else { "placa de software" });
            Some(BgraReadback::new(&self.dev, &self.capture.image).map_err(|e| e.to_string())?)
        } else {
            None
        };
        self.capture.partial = self.readback.is_some();
        Ok(self.readback.is_some())
    }

    /// No caminho da CPU (BGRA lido, só o que mudou)?
    pub fn is_cpu(&self) -> bool {
        self.readback.is_some()
    }

    pub fn open_encoder(&self, cfg: &EncoderConfig) -> crate::media::Result<Box<dyn VideoEncoder>> {
        crate::media::encode::open_encoder(&self.dev, cfg)
    }

    pub fn acquire(&mut self, wait_ms: u32) -> crate::media::Result<Option<FrameInfo>> {
        self.capture.acquire(&self.dev, wait_ms)
    }

    /// O BGRA das `regions` (`None` = tudo) enquanto `f` roda.
    pub fn read_bgra<R>(&self, regions: Option<&[Region]>, f: impl FnOnce(&[u8], usize) -> R) -> crate::media::Result<R> {
        let rb = self.readback.as_ref().ok_or("sem o caminho da CPU")?;
        rb.read(&self.dev, &self.capture.image.tex, regions, f)
    }

    /// O quadro pela placa: NV12 no shader e o encoder dela.
    pub fn encode_gpu(&mut self, encoder: &mut dyn VideoEncoder, force_key: bool) -> crate::media::Result<Vec<EncodedPacket>> {
        let conv = self.conv.as_ref().ok_or("conversor não preparado")?;
        conv.convert(&self.dev, &self.capture.image.srv);
        encoder.encode(&self.dev, &conv.nv12, force_key)
    }
}
