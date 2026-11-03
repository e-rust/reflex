//! Resolução e escala de um monitor (resolução remota "igual à janela",
//! como o RDP). O host troca o modo de vídeo do monitor capturado pro maior
//! que cabe na janela do viewer e a escala do Windows pra a dele; no fim da
//! sessão [`DisplayOverride`] desfaz tudo.
//!
//! A escala usa `DisplayConfigGetDeviceInfo`/`SetDeviceInfo` com os tipos -3
//! e -4, não documentados (os mesmos do "Configurações > Tela"): o valor é
//! relativo à escala recomendada do monitor, em passos de [`DPI_STEPS`].

use windows::core::PCWSTR;
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_DEVICE_INFO_TYPE, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Gdi::{ChangeDisplaySettingsExW, EnumDisplaySettingsExW, CDS_TYPE, DEVMODEW, DISP_CHANGE_SUCCESSFUL, DM_PELSHEIGHT, DM_PELSWIDTH, ENUM_DISPLAY_SETTINGS_FLAGS, ENUM_DISPLAY_SETTINGS_MODE};

use crate::{from_wide, wide};

/// Escalas que o Windows oferece, em %.
pub const DPI_STEPS: [u16; 12] = [100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500];

/// O maior modo (área) que cabe em `want`; nenhum cabe → o menor. Pura.
pub fn best_mode(modes: &[(u32, u32)], want: (u32, u32)) -> Option<(u32, u32)> {
    let area = |m: &(u32, u32)| m.0 as u64 * m.1 as u64;
    modes.iter().filter(|m| m.0 <= want.0 && m.1 <= want.1).max_by_key(|m| area(m)).or_else(|| modes.iter().min_by_key(|m| area(m))).copied()
}

/// Índice em [`DPI_STEPS`] mais perto de `pct`. Pura.
pub fn dpi_index(pct: u16) -> usize {
    DPI_STEPS.iter().enumerate().min_by_key(|(_, s)| (**s as i32 - pct as i32).abs()).map(|(i, _)| i).unwrap_or(0)
}

/// Modos de vídeo (largura, altura) do monitor `device` (`\\.\DISPLAY1`), sem repetir.
pub fn modes(device: &str) -> Vec<(u32, u32)> {
    let name = wide(device);
    let mut out = Vec::new();
    for i in 0u32.. {
        let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        if !unsafe { EnumDisplaySettingsExW(PCWSTR(name.as_ptr()), ENUM_DISPLAY_SETTINGS_MODE(i), &mut dm, ENUM_DISPLAY_SETTINGS_FLAGS(0)) }.as_bool() {
            break;
        }
        let m = (dm.dmPelsWidth, dm.dmPelsHeight);
        if !out.contains(&m) {
            out.push(m);
        }
    }
    out
}

/// Troca a resolução (sem gravar no registro: some ao reiniciar ou com [`reset_size`]).
pub fn set_size(device: &str, size: (u32, u32)) -> Result<(), String> {
    let name = wide(device);
    let dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, dmFields: DM_PELSWIDTH | DM_PELSHEIGHT, dmPelsWidth: size.0, dmPelsHeight: size.1, ..Default::default() };
    let r = unsafe { ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), Some(&dm), None, CDS_TYPE(0), None) };
    if r == DISP_CHANGE_SUCCESSFUL {
        Ok(())
    } else {
        Err(format!("{device} {}x{}: ChangeDisplaySettingsEx {}", size.0, size.1, r.0))
    }
}

/// Volta à resolução gravada no registro (a de antes).
pub fn reset_size(device: &str) {
    let name = wide(device);
    unsafe {
        let _ = ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), None, None, CDS_TYPE(0), None);
    }
}

// Pacotes não documentados da escala (ver o topo do módulo).
const GET_DPI: DISPLAYCONFIG_DEVICE_INFO_TYPE = DISPLAYCONFIG_DEVICE_INFO_TYPE(-3);
const SET_DPI: DISPLAYCONFIG_DEVICE_INFO_TYPE = DISPLAYCONFIG_DEVICE_INFO_TYPE(-4);

#[repr(C)]
#[derive(Default)]
struct DpiGet {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    min_rel: i32,
    cur_rel: i32,
    max_rel: i32,
}

#[repr(C)]
#[derive(Default)]
struct DpiSet {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    scale_rel: i32,
}

/// Placa + origem do caminho ativo que mostra o monitor `device`.
fn source_of(device: &str) -> Option<(LUID, u32)> {
    let (mut np, mut nm) = (0u32, 0u32);
    unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) }.ok().ok()?;
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
    unsafe { QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None) }.ok().ok()?;
    paths.truncate(np as usize);
    paths.iter().map(|p| (p.sourceInfo.adapterId, p.sourceInfo.id)).find(|&(adapter, id)| {
        let mut name = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32, adapterId: adapter, id },
            ..Default::default()
        };
        let found = unsafe { DisplayConfigGetDeviceInfo(&mut name.header) };
        found == 0 && from_wide(&name.viewGdiDeviceName).eq_ignore_ascii_case(device)
    })
}

/// (escala atual em %, índice da recomendada, limites relativos min/máx).
fn dpi_info(adapter: LUID, id: u32) -> Option<(u16, usize, i32, i32)> {
    let mut get = DpiGet { header: DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: GET_DPI, size: std::mem::size_of::<DpiGet>() as u32, adapterId: adapter, id }, ..Default::default() };
    if unsafe { DisplayConfigGetDeviceInfo(&mut get.header) } != 0 {
        return None;
    }
    let recommended = get.min_rel.unsigned_abs() as usize;
    let current = (recommended as i32 + get.cur_rel).clamp(0, DPI_STEPS.len() as i32 - 1) as usize;
    Some((DPI_STEPS[current], recommended, get.min_rel, get.max_rel))
}

/// Escala atual do monitor, em %.
pub fn scale(device: &str) -> Option<u16> {
    let (adapter, id) = source_of(device)?;
    dpi_info(adapter, id).map(|(pct, ..)| pct)
}

/// Troca a escala do monitor (o mais perto de `pct` que ele aceita).
pub fn set_scale(device: &str, pct: u16) -> Result<(), String> {
    let (adapter, id) = source_of(device).ok_or_else(|| format!("{device}: caminho de vídeo não encontrado"))?;
    let (_, recommended, min_rel, max_rel) = dpi_info(adapter, id).ok_or("escala do monitor ilegível")?;
    let rel = (dpi_index(pct) as i32 - recommended as i32).clamp(min_rel, max_rel);
    let set = DpiSet { header: DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: SET_DPI, size: std::mem::size_of::<DpiSet>() as u32, adapterId: adapter, id }, scale_rel: rel };
    match unsafe { DisplayConfigSetDeviceInfo(&set.header) } {
        0 => Ok(()),
        e => Err(format!("{device}: escala {pct}%: erro {e}")),
    }
}

/// O que a sessão mudou num monitor, pra desfazer no fim.
#[derive(Debug, Default)]
pub struct DisplayOverride {
    device: Option<String>,
    /// Escala de antes, se a sessão mudou a escala.
    scale_before: Option<u16>,
    /// Último pedido atendido (a captura é recriada várias vezes; reaplicar
    /// o mesmo modo faria a tela piscar à toa).
    last: Option<Applied>,
}

/// Um pedido (tamanho, escala) e o modo que ele virou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Applied {
    size: (u32, u32),
    scale_pct: u16,
    mode: (u32, u32),
}

impl DisplayOverride {
    /// Põe `device` no maior modo que cabe em `size` e na escala `scale_pct`.
    /// Devolve o modo aplicado. A escala é melhor-esforço (log se falhar).
    pub fn apply(&mut self, device: &str, size: (u32, u32), scale_pct: u16) -> Result<(u32, u32), String> {
        let same_device = self.device.as_deref().is_some_and(|d| d.eq_ignore_ascii_case(device));
        if let Some(a) = self.last.filter(|a| same_device && a.size == size && a.scale_pct == scale_pct) {
            return Ok(a.mode);
        }
        if self.device.is_some() && !same_device {
            self.restore();
        }
        let mode = best_mode(&modes(device), size).ok_or_else(|| format!("{device} sem modos de vídeo"))?;
        set_size(device, mode)?;
        self.device = Some(device.to_string());
        let before = scale(device);
        if before != Some(DPI_STEPS[dpi_index(scale_pct)]) {
            match set_scale(device, scale_pct) {
                Ok(()) => {
                    // Lembra a escala original só na 1ª troca.
                    self.scale_before = self.scale_before.or(before);
                }
                Err(e) => log::warn!("escala remota: {e}"),
            }
        }
        self.last = Some(Applied { size, scale_pct, mode });
        Ok(mode)
    }

    /// Desfaz (resolução do registro e escala de antes).
    pub fn restore(&mut self) {
        self.last = None;
        if let Some(device) = self.device.take() {
            reset_size(&device);
            if let Some(pct) = self.scale_before.take() {
                if let Err(e) = set_scale(&device, pct) {
                    log::warn!("voltar a escala: {e}");
                }
            }
            log::info!("{device}: resolução e escala de volta ao original");
        }
    }
}

impl Drop for DisplayOverride {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODES: [(u32, u32); 5] = [(1024, 768), (1280, 720), (1366, 768), (1920, 1080), (2560, 1440)];

    #[test]
    fn biggest_mode_that_fits_the_window() {
        assert_eq!(best_mode(&MODES, (1500, 900)), Some((1366, 768)));
        assert_eq!(best_mode(&MODES, (1920, 1080)), Some((1920, 1080)));
        assert_eq!(best_mode(&MODES, (4000, 3000)), Some((2560, 1440)));
        // Janela menor que tudo: o menor modo.
        assert_eq!(best_mode(&MODES, (800, 600)), Some((1024, 768)));
        assert_eq!(best_mode(&[], (800, 600)), None);
    }

    #[test]
    fn scale_snaps_to_windows_steps() {
        assert_eq!(DPI_STEPS[dpi_index(150)], 150);
        assert_eq!(DPI_STEPS[dpi_index(140)], 150);
        assert_eq!(DPI_STEPS[dpi_index(96)], 100);
        assert_eq!(DPI_STEPS[dpi_index(1000)], 500);
    }

    #[test]
    fn current_monitor_has_modes_and_a_scale() {
        // Nesta máquina (com monitor): lê sem mudar nada.
        let dev = r"\\.\DISPLAY1";
        if modes(dev).is_empty() {
            return; // sem monitor (CI)
        }
        assert!(scale(dev).is_some_and(|s| DPI_STEPS.contains(&s)));
    }
}
