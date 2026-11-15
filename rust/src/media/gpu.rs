//! GPU: dispositivo D3D11 e o conversor BGRA → NV12 BT.709 de faixa limitada
//! (o que todo encoder quer). Porte de `NoOBS/rust/src/capture/gpu.rs`; o
//! conversor também escala (a viewport tem o tamanho do NV12, a amostragem
//! bilinear cobre a origem inteira).
//!
//! O contexto imediato é usado só pela thread de captura; o dispositivo é
//! multithread-protegido porque o encoder (AMF/NVENC) também fala com ele.

use windows::core::{s, Interface, PCSTR};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_OPTIMIZATION_LEVEL3};
use windows::Win32::Graphics::Direct3D::{ID3DBlob, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D11_SRV_DIMENSION_TEXTURE2D};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_FORMAT_R8G8_UNORM, DXGI_FORMAT_R8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIAdapter1, IDXGIDevice};

use crate::media::{MediaError, Result};

/// PCI vendor id dos adaptadores de software da Microsoft.
pub const VENDOR_MICROSOFT: u32 = 0x1414;

/// Dispositivo + contexto imediato.
pub struct Device {
    pub device: ID3D11Device,
    pub ctx: ID3D11DeviceContext,
}

// SAFETY: o dispositivo é multithread-protegido; o contexto só é usado pela
// thread dona do `Device` (o tipo é movido pra ela e fica lá).
unsafe impl Send for Device {}

impl Device {
    /// Na placa padrão.
    pub fn create() -> Result<Self> {
        Self::create_on(None)
    }

    /// Na placa `adapter` (a DXGI Duplication exige a placa ligada ao monitor).
    ///
    /// Tenta, em ordem: com suporte de vídeo (encode/decode na GPU), sem ele
    /// (placas de VM/servidor que só desenham) e, sem placa indicada, o WARP
    /// (render por software do Windows) — a captura e o encoder de CPU funcionam nos três.
    pub fn create_on(adapter: Option<&IDXGIAdapter1>) -> Result<Self> {
        let adapter: Option<IDXGIAdapter> = adapter.map(|a| a.cast()).transpose()?;
        let base = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
        let mut attempts = vec![
            (if adapter.is_some() { D3D_DRIVER_TYPE_UNKNOWN } else { D3D_DRIVER_TYPE_HARDWARE }, base | D3D11_CREATE_DEVICE_VIDEO_SUPPORT),
            (if adapter.is_some() { D3D_DRIVER_TYPE_UNKNOWN } else { D3D_DRIVER_TYPE_HARDWARE }, base),
        ];
        if adapter.is_none() {
            attempts.push((D3D_DRIVER_TYPE_WARP, base));
        }
        let mut last_err = None;
        for (driver, flags) in attempts {
            let mut device = None;
            let mut ctx = None;
            let r = unsafe { D3D11CreateDevice(adapter.as_ref(), driver, HMODULE::default(), flags, Some(&[D3D_FEATURE_LEVEL_11_0]), D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut ctx)) };
            match (r, device, ctx) {
                (Ok(()), Some(device), Some(ctx)) => return Ok(Self::finish(device, ctx)),
                (Err(e), _, _) => {
                    log::debug!("D3D11CreateDevice({driver:?}, {flags:?}) falhou: {e}");
                    last_err = Some(e);
                }
                _ => {}
            }
        }
        Err(last_err.map(MediaError::from).unwrap_or_else(|| "D3D11CreateDevice sem dispositivo".into()))
    }

    fn finish(device: ID3D11Device, ctx: ID3D11DeviceContext) -> Self {
        // O encoder (AMF/NVENC) também usa o dispositivo, de outra thread.
        if let Ok(mt) = ctx.cast::<ID3D11Multithread>() {
            unsafe {
                let _ = mt.SetMultithreadProtected(true);
            }
        }
        Self { device, ctx }
    }

    /// Placa "da Microsoft" (0x1414): WARP, Basic Render, Hyper-V Video. Nelas
    /// o shader roda na CPU, então converter direto na CPU sai mais barato.
    pub fn is_software(&self) -> bool {
        self.vendor_id() == VENDOR_MICROSOFT
    }

    /// Fabricante da placa (PCI vendor id: 0x1002 AMD, 0x10DE NVIDIA, 0x8086 Intel).
    pub fn vendor_id(&self) -> u32 {
        let desc = self.device.cast::<IDXGIDevice>().and_then(|d| unsafe { d.GetAdapter() }).and_then(|a| unsafe { a.GetDesc() });
        desc.map(|d| d.VendorId).unwrap_or(0)
    }

    pub fn texture(&self, w: u32, h: u32, format: DXGI_FORMAT, bind: D3D11_BIND_FLAG) -> Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut tex = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex))? };
        tex.ok_or_else(|| MediaError::from("CreateTexture2D sem textura"))
    }

    /// Textura de leitura pela CPU, no formato/tamanho de `like`.
    pub fn staging_like(&self, like: &ID3D11Texture2D) -> Result<ID3D11Texture2D> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { like.GetDesc(&mut desc) };
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut tex = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex))? };
        tex.ok_or_else(|| MediaError::from("staging sem textura"))
    }

    pub fn srv(&self, tex: &ID3D11Texture2D, format: DXGI_FORMAT) -> Result<ID3D11ShaderResourceView> {
        let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: format,
            ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 } },
        };
        let mut v = None;
        unsafe { self.device.CreateShaderResourceView(tex, Some(&desc), Some(&mut v))? };
        v.ok_or_else(|| MediaError::from("sem SRV"))
    }

    pub fn rtv(&self, tex: &ID3D11Texture2D, format: DXGI_FORMAT) -> Result<ID3D11RenderTargetView> {
        let desc = D3D11_RENDER_TARGET_VIEW_DESC {
            Format: format,
            ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 } },
        };
        let mut v = None;
        unsafe { self.device.CreateRenderTargetView(tex, Some(&desc), Some(&mut v))? };
        v.ok_or_else(|| MediaError::from("sem RTV"))
    }
}

/// Textura BGRA com SRV (o quadro capturado, já nosso).
pub struct BgraTexture {
    pub tex: ID3D11Texture2D,
    pub srv: ID3D11ShaderResourceView,
    pub width: u32,
    pub height: u32,
}

impl BgraTexture {
    pub fn new(dev: &Device, width: u32, height: u32) -> Result<Self> {
        let tex = dev.texture(width, height, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_BIND_SHADER_RESOURCE)?;
        let srv = dev.srv(&tex, DXGI_FORMAT_B8G8R8A8_UNORM)?;
        Ok(Self { tex, srv, width, height })
    }

    /// Textura já preenchida com `bgra` (`width*4` por linha) — imagens de teste.
    pub fn from_pixels(dev: &Device, width: u32, height: u32, bgra: &[u8]) -> Result<Self> {
        if bgra.len() < (width * height * 4) as usize {
            return Err("pixels a menos que o tamanho".into());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: bgra.as_ptr() as *const _, SysMemPitch: width * 4, SysMemSlicePitch: 0 };
        let mut tex = None;
        unsafe { dev.device.CreateTexture2D(&desc, Some(&init), Some(&mut tex))? };
        let tex = tex.ok_or("CreateTexture2D sem textura")?;
        let srv = dev.srv(&tex, DXGI_FORMAT_B8G8R8A8_UNORM)?;
        Ok(Self { tex, srv, width, height })
    }
}

/// Leitura de uma textura BGRA pela CPU (staging do mesmo tamanho).
/// Copia de `src` pra `dst` só `regions` (`None` = tudo).
pub fn copy_regions(dev: &Device, dst: &ID3D11Texture2D, src: &ID3D11Texture2D, regions: Option<&[crate::media::damage::Region]>) {
    unsafe {
        match regions {
            None => dev.ctx.CopyResource(dst, src),
            Some(list) => {
                for r in list.iter().filter(|r| !r.is_empty()) {
                    let b = D3D11_BOX { left: r.left, top: r.top, front: 0, right: r.right, bottom: r.bottom, back: 1 };
                    dev.ctx.CopySubresourceRegion(dst, 0, r.left, r.top, 0, src, 0, Some(&b));
                }
            }
        }
    }
}

pub struct BgraReadback {
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
}

impl BgraReadback {
    pub fn new(dev: &Device, like: &BgraTexture) -> Result<Self> {
        Ok(Self { staging: dev.staging_like(&like.tex)?, width: like.width, height: like.height })
    }

    /// Copia `src` (só `regions`, se dado: o resto da cópia anterior continua
    /// valendo) e entrega os bytes (com o stride da linha) a `f`.
    pub fn read<R>(&self, dev: &Device, src: &ID3D11Texture2D, regions: Option<&[crate::media::damage::Region]>, f: impl FnOnce(&[u8], usize) -> R) -> Result<R> {
        copy_regions(dev, &self.staging, src, regions);
        unsafe {
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            dev.ctx.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let pitch = m.RowPitch as usize;
            let len = pitch * (self.height as usize - 1) + self.width as usize * 4;
            // SAFETY: mapeado pra leitura até o Unmap; `len` cabe no recurso.
            let bytes = std::slice::from_raw_parts(m.pData as *const u8, len);
            let r = f(bytes, pitch);
            dev.ctx.Unmap(&self.staging, 0);
            Ok(r)
        }
    }
}

/// Um triângulo cobrindo a viewport (sem vertex buffer); `uv` 0..1.
const HLSL: &str = r#"
Texture2D tex : register(t0);
SamplerState smp : register(s0);
struct V { float4 pos : SV_Position; float2 uv : TEXCOORD0; };
V vs_main(uint id : SV_VertexID) {
    V o;
    float2 t = float2((id << 1) & 2, id & 2);
    o.pos = float4(t * float2(2, -2) + float2(-1, 1), 0, 1);
    o.uv = t;
    return o;
}
// BT.709, faixa limitada (16-235 / 16-240) — os coeficientes do OBS.
float ps_y(V i) : SV_Target {
    float3 c = tex.Sample(smp, i.uv).rgb;
    return dot(c, float3(0.182586, 0.614231, 0.062007)) + 0.0627451;
}
// Viewport com metade do tamanho: a amostragem bilinear tira a média (4:2:0).
float2 ps_uv(V i) : SV_Target {
    float3 c = tex.Sample(smp, i.uv).rgb;
    return float2(dot(c, float3(-0.100644, -0.338572, 0.439216)), dot(c, float3(0.439216, -0.398942, -0.040274))) + 0.501961;
}
"#;

fn compile(entry: PCSTR, target: PCSTR) -> Result<ID3DBlob> {
    let mut code = None;
    let mut errors = None;
    let r = unsafe { D3DCompile(HLSL.as_ptr() as *const _, HLSL.len(), s!("reflex-nv12"), None, None, entry, target, D3DCOMPILE_OPTIMIZATION_LEVEL3, 0, &mut code, Some(&mut errors)) };
    if let Err(e) = r {
        let msg = errors.map(|b: ID3DBlob| String::from_utf8_lossy(blob_bytes(&b)).into_owned()).unwrap_or_default();
        return Err(MediaError::Msg(format!("D3DCompile: {e} {msg}")));
    }
    code.ok_or_else(|| MediaError::from("D3DCompile sem código"))
}

fn blob_bytes(b: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()) }
}

/// BGRA (qualquer tamanho) → NV12 (tamanho do conversor).
pub struct Nv12Converter {
    vs: ID3D11VertexShader,
    ps_y: ID3D11PixelShader,
    ps_uv: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    pub width: u32,
    pub height: u32,
    pub nv12: ID3D11Texture2D,
    rtv_y: ID3D11RenderTargetView,
    rtv_uv: ID3D11RenderTargetView,
}

impl Nv12Converter {
    pub fn new(dev: &Device, width: u32, height: u32) -> Result<Self> {
        if !width.is_multiple_of(2) || !height.is_multiple_of(2) || width == 0 || height == 0 {
            return Err(MediaError::Msg(format!("NV12 exige dimensões pares ({width}x{height})")));
        }
        let vs_code = compile(s!("vs_main"), s!("vs_5_0"))?;
        let mut vs = None;
        unsafe { dev.device.CreateVertexShader(blob_bytes(&vs_code), None, Some(&mut vs))? };
        let ps = |entry| -> Result<ID3D11PixelShader> {
            let code = compile(entry, s!("ps_5_0"))?;
            let mut p = None;
            unsafe { dev.device.CreatePixelShader(blob_bytes(&code), None, Some(&mut p))? };
            p.ok_or_else(|| MediaError::from("sem pixel shader"))
        };
        let sdesc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut sampler = None;
        unsafe { dev.device.CreateSamplerState(&sdesc, Some(&mut sampler))? };
        let nv12 = dev.texture(width, height, DXGI_FORMAT_NV12, D3D11_BIND_RENDER_TARGET)?;
        Ok(Self {
            vs: vs.ok_or("sem vertex shader")?,
            ps_y: ps(s!("ps_y"))?,
            ps_uv: ps(s!("ps_uv"))?,
            sampler: sampler.ok_or("sem sampler")?,
            width,
            height,
            rtv_y: dev.rtv(&nv12, DXGI_FORMAT_R8_UNORM)?,
            rtv_uv: dev.rtv(&nv12, DXGI_FORMAT_R8G8_UNORM)?,
            nv12,
        })
    }

    fn pass(&self, dev: &Device, ps: &ID3D11PixelShader, rtv: &ID3D11RenderTargetView, src: &ID3D11ShaderResourceView, w: f32, h: f32) {
        let c = &dev.ctx;
        unsafe {
            c.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            c.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: w, Height: h, MinDepth: 0.0, MaxDepth: 1.0 }]));
            c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.IASetInputLayout(None);
            c.VSSetShader(&self.vs, None);
            c.PSSetShader(ps, None);
            c.PSSetShaderResources(0, Some(&[Some(src.clone())]));
            c.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            c.Draw(3, 0);
            c.PSSetShaderResources(0, Some(&[None]));
            c.OMSetRenderTargets(None, None);
        }
    }

    /// Converte (e escala) `src` pro NV12 do conversor.
    pub fn convert(&self, dev: &Device, src: &ID3D11ShaderResourceView) {
        let (w, h) = (self.width as f32, self.height as f32);
        self.pass(dev, &self.ps_y, &self.rtv_y, src, w, h);
        self.pass(dev, &self.ps_uv, &self.rtv_uv, src, w / 2.0, h / 2.0);
    }
}

/// Tamanho de saída: cabe em `max_w`×`max_h` (0 = sem limite), mantém a
/// proporção e arredonda pra par (NV12).
pub fn fit_even(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let mut scale = 1.0f64;
    if max_w > 0 && src_w > max_w {
        scale = scale.min(max_w as f64 / src_w as f64);
    }
    if max_h > 0 && src_h > max_h {
        scale = scale.min(max_h as f64 / src_h as f64);
    }
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(src_w as f64 * scale), even(src_h as f64 * scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_size_without_limits() {
        assert_eq!(fit_even(1920, 1080, 0, 0), (1920, 1080));
    }

    #[test]
    fn fit_rounds_odd_sizes_to_even() {
        assert_eq!(fit_even(1365, 767, 0, 0), (1364, 766));
    }

    #[test]
    fn fit_scales_down_keeping_aspect() {
        assert_eq!(fit_even(3840, 2160, 1920, 0), (1920, 1080));
        assert_eq!(fit_even(3840, 2160, 1920, 900), (1600, 900));
    }

    #[test]
    #[ignore = "precisa de GPU"]
    fn converter_compiles_shaders() {
        let dev = Device::create().unwrap();
        Nv12Converter::new(&dev, 64, 64).unwrap();
    }
}
