//! Captura de tela por DXGI Desktop Duplication (§4.1: funciona na área de
//! trabalho Winlogon, desde que a thread esteja nela — quem troca é o
//! `platform::desktop`). Também lista os monitores na MESMA ordem que a
//! captura usa (o índice do `SessionConfig.monitor`).

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_INVALID_CALL,
    DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_POINTER_SHAPE_INFO,
};
use reflex_proto::msg::Monitor;

use crate::cursor::RawCursorShape;
use crate::gpu::{BgraTexture, Device};
use crate::{MediaError, Result};

/// Um monitor ligado à área de trabalho, com a placa a que pertence.
pub struct Output {
    pub adapter: IDXGIAdapter1,
    pub output: IDXGIOutput1,
    pub info: Monitor,
}

fn utf16(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// Placa de vídeo virtual (driver IddCx — §4.2)?
pub fn is_virtual_adapter(description: &str) -> bool {
    let d = description.to_ascii_lowercase();
    ["virtual display", "indirect display", "iddsample", "parsec", "mttvdd", "virtual monitor"].iter().any(|k| d.contains(k))
}

/// Monitores ativos, na ordem de enumeração (placa, saída).
/// Há placa de vídeo de verdade (não WARP/Basic Render/Hyper-V Video)?
/// Sem ela, o próprio Windows desenha a tela na CPU.
pub fn has_hardware_gpu() -> bool {
    use windows::Win32::Graphics::Dxgi::DXGI_ADAPTER_FLAG_SOFTWARE;
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else { return false };
    (0..)
        .map_while(|i| unsafe { factory.EnumAdapters1(i) }.ok())
        .filter_map(|a| unsafe { a.GetDesc1() }.ok())
        .any(|d| d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0 && d.VendorId != crate::gpu::VENDOR_MICROSOFT)
}

pub fn outputs() -> Result<Vec<Output>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut list = Vec::new();
    let mut ai = 0;
    loop {
        let adapter = match unsafe { factory.EnumAdapters1(ai) } {
            Ok(a) => a,
            Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(e) => return Err(e.into()),
        };
        ai += 1;
        let adesc = unsafe { adapter.GetDesc1()? };
        let virtual_display = is_virtual_adapter(&utf16(&adesc.Description));
        let mut oi = 0;
        loop {
            let output = match unsafe { adapter.EnumOutputs(oi) } {
                Ok(o) => o,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(e) => return Err(e.into()),
            };
            oi += 1;
            let desc = unsafe { output.GetDesc()? };
            if !desc.AttachedToDesktop.as_bool() {
                continue;
            }
            let r = desc.DesktopCoordinates;
            let info = Monitor {
                id: list.len() as u32,
                name: utf16(&desc.DeviceName),
                x: r.left,
                y: r.top,
                width: (r.right - r.left).max(0) as u32,
                height: (r.bottom - r.top).max(0) as u32,
                primary: r.left == 0 && r.top == 0,
                virtual_display,
            };
            list.push(Output { adapter: adapter.clone(), output: output.cast()?, info });
        }
    }
    Ok(list)
}

pub fn monitors() -> Vec<Monitor> {
    outputs().map(|o| o.into_iter().map(|o| o.info).collect()).unwrap_or_default()
}

/// O que veio num `acquire`.
#[derive(Default)]
pub struct FrameInfo {
    /// A imagem mudou (já está em [`ScreenCapture::image`]).
    pub new_image: bool,
    /// Posição do cursor relativa ao monitor, se mudou.
    pub cursor_pos: Option<(i32, i32, bool)>,
    /// Forma nova do cursor, se mudou.
    pub cursor_shape: Option<RawCursorShape>,
    /// O que a imagem nova mudou (`None` = não se sabe: tudo).
    pub dirty: Option<Vec<crate::damage::Region>>,
}

/// Duplicação de um monitor.
pub struct ScreenCapture {
    dup: IDXGIOutputDuplication,
    pub image: BgraTexture,
    pub monitor: Monitor,
    holding: bool,
    /// Copiar pra [`ScreenCapture::image`] só o que mudou (caminho da CPU: a
    /// cópia de uma placa de software é memcpy). A 1ª imagem vai inteira.
    pub partial: bool,
    fresh: bool,
}

impl ScreenCapture {
    /// Abre o monitor `index`; o [`Device`] é criado na placa dele.
    pub fn open(index: u32) -> Result<(Device, ScreenCapture)> {
        let mut outs = outputs()?;
        if outs.is_empty() {
            return Err(MediaError::from("nenhum monitor ativo (sem monitor físico nem virtual)"));
        }
        let pos = (index as usize).min(outs.len() - 1);
        let out = outs.swap_remove(pos);
        let dev = Device::create_on(Some(&out.adapter))?;
        let dup = unsafe { out.output.DuplicateOutput(&dev.device) }.map_err(|e| {
            if e.code() == windows::Win32::Foundation::E_ACCESSDENIED {
                MediaError::AccessLost
            } else {
                MediaError::from(e)
            }
        })?;
        let image = BgraTexture::new(&dev, out.info.width, out.info.height)?;
        Ok((dev, ScreenCapture { dup, image, monitor: out.info, holding: false, partial: false, fresh: true }))
    }

    fn release(&mut self) {
        if self.holding {
            unsafe {
                let _ = self.dup.ReleaseFrame();
            }
            self.holding = false;
        }
    }

    /// Espera até `timeout_ms` por novidade. `Ok(None)` = nada mudou.
    pub fn acquire(&mut self, dev: &Device, timeout_ms: u32) -> Result<Option<FrameInfo>> {
        self.release();
        let mut fi = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        match unsafe { self.dup.AcquireNextFrame(timeout_ms, &mut fi, &mut res) } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST || e.code() == DXGI_ERROR_INVALID_CALL => return Err(MediaError::AccessLost),
            Err(e) => return Err(e.into()),
        }
        self.holding = true;
        let mut info = FrameInfo::default();

        if fi.LastPresentTime != 0 {
            if let Some(res) = res {
                let tex: ID3D11Texture2D = res.cast()?;
                let dirty = if self.fresh { None } else { self.changed(&fi) };
                self.fresh = false;
                crate::gpu::copy_regions(dev, &self.image.tex, &tex, dirty.as_deref().filter(|_| self.partial));
                info.dirty = dirty;
                info.new_image = true;
            }
        }
        if fi.LastMouseUpdateTime != 0 {
            let p = fi.PointerPosition;
            info.cursor_pos = Some((p.Position.x, p.Position.y, p.Visible.as_bool()));
        }
        if fi.PointerShapeBufferSize > 0 {
            let mut buf = vec![0u8; fi.PointerShapeBufferSize as usize];
            let mut needed = 0u32;
            let mut shape = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
            let ok = unsafe { self.dup.GetFramePointerShape(buf.len() as u32, buf.as_mut_ptr() as *mut _, &mut needed, &mut shape) };
            if ok.is_ok() {
                info.cursor_shape = Some(RawCursorShape {
                    kind: shape.Type,
                    width: shape.Width,
                    height: shape.Height,
                    pitch: shape.Pitch,
                    hot_x: shape.HotSpot.x.max(0) as u32,
                    hot_y: shape.HotSpot.y.max(0) as u32,
                    data: buf,
                });
            }
        }
        Ok(Some(info))
    }
}

impl ScreenCapture {
    /// Retângulos que a imagem atual mudou: os destinos dos movidos (janela
    /// arrastada) e os sujos. `None` se a DXGI não disser.
    fn changed(&self, fi: &DXGI_OUTDUPL_FRAME_INFO) -> Option<Vec<crate::damage::Region>> {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_MOVE_RECT;
        let size = fi.TotalMetadataBufferSize as usize;
        if size == 0 {
            return None;
        }
        let region = |r: &RECT| crate::damage::Region { left: r.left.max(0) as u32, top: r.top.max(0) as u32, right: r.right.max(0) as u32, bottom: r.bottom.max(0) as u32 };
        let mut out = Vec::new();
        let mut moves = vec![DXGI_OUTDUPL_MOVE_RECT::default(); size / std::mem::size_of::<DXGI_OUTDUPL_MOVE_RECT>() + 1];
        let mut bytes = 0u32;
        let cap = (moves.len() * std::mem::size_of::<DXGI_OUTDUPL_MOVE_RECT>()) as u32;
        unsafe { self.dup.GetFrameMoveRects(cap, moves.as_mut_ptr(), &mut bytes) }.ok()?;
        out.extend(moves.iter().take(bytes as usize / std::mem::size_of::<DXGI_OUTDUPL_MOVE_RECT>()).map(|m| region(&m.DestinationRect)));
        let mut dirty = vec![RECT::default(); size / std::mem::size_of::<RECT>() + 1];
        let cap = (dirty.len() * std::mem::size_of::<RECT>()) as u32;
        unsafe { self.dup.GetFrameDirtyRects(cap, dirty.as_mut_ptr(), &mut bytes) }.ok()?;
        out.extend(dirty.iter().take(bytes as usize / std::mem::size_of::<RECT>()).map(region));
        Some(out)
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_adapters_are_recognised() {
        assert!(is_virtual_adapter("Virtual Display Driver"));
        assert!(is_virtual_adapter("Parsec Virtual Display Adapter"));
        assert!(!is_virtual_adapter("AMD Radeon RX 9070 XT"));
    }

    #[test]
    #[ignore = "precisa de monitor e GPU"]
    fn captures_primary_monitor() {
        // Com o monitor em economia de energia a DXGI não entrega quadro nenhum.
        unsafe {
            windows::Win32::System::Power::SetThreadExecutionState(windows::Win32::System::Power::ES_DISPLAY_REQUIRED);
        }
        let (dev, mut cap) = ScreenCapture::open(0).unwrap();
        // A 1ª aquisição sempre traz imagem.
        let mut got = false;
        for _ in 0..40 {
            if let Some(f) = cap.acquire(&dev, 100).unwrap() {
                got |= f.new_image;
            }
            if got {
                break;
            }
        }
        assert!(got);
    }
}
