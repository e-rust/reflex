//! Dispositivos PnP (SetupAPI/cfgmgr32): instalar um driver num dispositivo
//! "raiz" — sem hardware, como o `devcon install` —, ligar/desligar e saber
//! se há monitor conectado. Usado pelo monitor virtual (§4.2).

use std::path::Path;

use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Devices::Properties::{DEVPKEY_Device_DriverInfPath, DEVPROPTYPE};
use windows::core::BOOL;

use crate::{from_wide, wide};

pub use windows::Win32::Devices::DeviceAndDriverInstallation::{GUID_DEVCLASS_DISPLAY, GUID_DEVCLASS_MONITOR};

/// Um dispositivo encontrado pelo hardware ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub instance_id: String,
    pub enabled: bool,
}

/// Lista de dispositivos da SetupAPI (destruída ao sair).
struct DevList(HDEVINFO);

impl Drop for DevList {
    fn drop(&mut self) {
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

fn new_data() -> SP_DEVINFO_DATA {
    SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() }
}

/// Texto → REG_MULTI_SZ com um item (UTF-16, dois zeros no fim), em bytes. Pura.
pub fn multi_sz(item: &str) -> Vec<u8> {
    item.encode_utf16().chain([0, 0]).flat_map(u16::to_le_bytes).collect()
}

/// REG_MULTI_SZ em bytes → itens (para no primeiro item vazio). Pura.
pub fn parse_multi_sz(bytes: &[u8]) -> Vec<String> {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
    units.split(|&u| u == 0).take_while(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
}

fn present(class: &GUID) -> Result<DevList, String> {
    unsafe { SetupDiGetClassDevsW(Some(class), PCWSTR::null(), None, DIGCF_PRESENT) }.map(DevList).map_err(|e| format!("SetupDiGetClassDevs: {e}"))
}

fn devices(list: &DevList) -> impl Iterator<Item = SP_DEVINFO_DATA> + '_ {
    (0u32..).map_while(move |i| {
        let mut data = new_data();
        unsafe { SetupDiEnumDeviceInfo(list.0, i, &mut data) }.ok().map(|_| data)
    })
}

fn hardware_ids(list: &DevList, data: &SP_DEVINFO_DATA) -> Vec<String> {
    let mut buf = [0u8; 2048];
    match unsafe { SetupDiGetDeviceRegistryPropertyW(list.0, data, SPDRP_HARDWAREID, None, Some(&mut buf), None) } {
        Ok(()) => parse_multi_sz(&buf),
        Err(_) => Vec::new(),
    }
}

fn instance_id(devinst: u32) -> String {
    let mut buf = [0u16; MAX_DEVICE_ID_LEN as usize + 1];
    unsafe { CM_Get_Device_IDW(devinst, &mut buf, 0) };
    from_wide(&buf)
}

fn is_enabled(devinst: u32) -> bool {
    let (mut status, mut problem) = (CM_DEVNODE_STATUS_FLAGS(0), CM_PROB(0));
    let r = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, devinst, 0) };
    !(r == CR_SUCCESS && (status.0 & DN_HAS_PROBLEM.0) != 0 && problem == CM_PROB_DISABLED)
}

/// Acha o 1º dispositivo presente da `class` com o hardware ID `hwid` e chama `f`.
fn with_device<R>(class: &GUID, hwid: &str, f: impl FnOnce(&DevList, &SP_DEVINFO_DATA) -> R) -> Result<Option<R>, String> {
    let list = present(class)?;
    let found = devices(&list).find(|d| hardware_ids(&list, d).iter().any(|h| h.eq_ignore_ascii_case(hwid)));
    Ok(found.map(|d| f(&list, &d)))
}

/// O dispositivo com esse hardware ID, se existir.
pub fn find(class: &GUID, hwid: &str) -> Option<Device> {
    with_device(class, hwid, |_, d| Device { instance_id: instance_id(d.DevInst), enabled: is_enabled(d.DevInst) }).ok().flatten()
}

/// Cria (se ainda não existe) o dispositivo raiz `hwid` e instala o driver do
/// `inf` nele. Devolve se o Windows pediu reinício.
pub fn install_root_device(class: &GUID, class_name: &str, hwid: &str, inf: &Path) -> Result<bool, String> {
    if find(class, hwid).is_none() {
        let list = DevList(unsafe { SetupDiCreateDeviceInfoList(Some(class), None) }.map_err(|e| format!("SetupDiCreateDeviceInfoList: {e}"))?);
        let mut data = new_data();
        let name = wide(class_name);
        unsafe { SetupDiCreateDeviceInfoW(list.0, PCWSTR(name.as_ptr()), class, PCWSTR::null(), None, DICD_GENERATE_ID, Some(&mut data)) }.map_err(|e| format!("SetupDiCreateDeviceInfo: {e}"))?;
        unsafe { SetupDiSetDeviceRegistryPropertyW(list.0, &mut data, SPDRP_HARDWAREID, Some(&multi_sz(hwid))) }.map_err(|e| format!("hardware ID: {e}"))?;
        unsafe { SetupDiCallClassInstaller(DIF_REGISTERDEVICE, list.0, Some(&data)) }.map_err(|e| format!("registrar o dispositivo: {e}"))?;
    }
    let (hw, path) = (wide(hwid), wide(&inf.to_string_lossy()));
    let mut reboot = BOOL(0);
    unsafe { UpdateDriverForPlugAndPlayDevicesW(None, PCWSTR(hw.as_ptr()), PCWSTR(path.as_ptr()), INSTALLFLAG_FORCE, Some(&mut reboot)) }.map_err(|e| format!("instalar o driver: {e}"))?;
    Ok(reboot.as_bool())
}

/// Liga ou desliga o dispositivo (como no Gerenciador de Dispositivos).
pub fn set_enabled(class: &GUID, hwid: &str, enabled: bool) -> Result<(), String> {
    with_device(class, hwid, |list, data| {
        let params = SP_PROPCHANGE_PARAMS {
            ClassInstallHeader: SP_CLASSINSTALL_HEADER { cbSize: std::mem::size_of::<SP_CLASSINSTALL_HEADER>() as u32, InstallFunction: DIF_PROPERTYCHANGE },
            StateChange: if enabled { DICS_ENABLE } else { DICS_DISABLE },
            Scope: DICS_FLAG_GLOBAL,
            HwProfile: 0,
        };
        unsafe {
            SetupDiSetClassInstallParamsW(list.0, Some(data), Some(&params.ClassInstallHeader), std::mem::size_of::<SP_PROPCHANGE_PARAMS>() as u32).map_err(|e| format!("parâmetros: {e}"))?;
            SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, list.0, Some(data)).map_err(|e| format!("{}: {e}", if enabled { "ligar" } else { "desligar" }))
        }
    })?
    .unwrap_or_else(|| Err(format!("dispositivo {hwid} não encontrado")))
}

/// Remove o dispositivo e tira o driver (`oemNN.inf`) do repositório do Windows.
pub fn remove(class: &GUID, hwid: &str) -> Result<(), String> {
    let inf = with_device(class, hwid, |list, data| {
        let inf = driver_inf_name(data.DevInst);
        unsafe { SetupDiCallClassInstaller(DIF_REMOVE, list.0, Some(data)) }.map_err(|e| format!("remover: {e}"))?;
        Ok::<_, String>(inf)
    })?
    .transpose()?
    .flatten();
    if let Some(inf) = inf.filter(|n| n.to_ascii_lowercase().starts_with("oem")) {
        let name = wide(&inf);
        let _ = unsafe { SetupUninstallOEMInfW(PCWSTR(name.as_ptr()), SUOI_FORCEDELETE, None) };
    }
    Ok(())
}

/// Nome do `.inf` publicado (`oem12.inf`) do driver do dispositivo.
fn driver_inf_name(devinst: u32) -> Option<String> {
    let mut buf = [0u16; 260];
    let mut size = (buf.len() * 2) as u32;
    let mut kind = DEVPROPTYPE(0);
    let r = unsafe { CM_Get_DevNode_PropertyW(devinst, &DEVPKEY_Device_DriverInfPath, &mut kind, Some(buf.as_mut_ptr() as *mut u8), &mut size, 0) };
    (r == CR_SUCCESS).then(|| from_wide(&buf)).filter(|s| !s.is_empty())
}

/// Monitores conectados agora, sem contar os que pertencem ao dispositivo
/// `except_parent` (instance ID — o monitor virtual).
pub fn monitors_present(except_parent: Option<&str>) -> usize {
    let Ok(list) = present(&GUID_DEVCLASS_MONITOR) else { return 0 };
    devices(&list)
        .filter(|d| {
            let mut parent = 0u32;
            let has_parent = unsafe { CM_Get_Parent(&mut parent, d.DevInst, 0) } == CR_SUCCESS;
            !(has_parent && except_parent.is_some_and(|p| instance_id(parent).eq_ignore_ascii_case(p)))
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_sz_roundtrip() {
        let b = multi_sz(r"Root\MttVDD");
        assert_eq!(&b[b.len() - 4..], &[0, 0, 0, 0]);
        assert_eq!(parse_multi_sz(&b), vec![r"Root\MttVDD".to_string()]);
        // Vários itens e lixo depois do fim.
        let mut two = multi_sz("A");
        two.truncate(two.len() - 2);
        two.extend(multi_sz("B"));
        two.extend([0, 0, 7, 0]);
        assert_eq!(parse_multi_sz(&two), vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn missing_device_is_not_found() {
        assert_eq!(find(&GUID_DEVCLASS_DISPLAY, r"Root\ReflexNaoExiste"), None);
        assert!(set_enabled(&GUID_DEVCLASS_DISPLAY, r"Root\ReflexNaoExiste", true).is_err());
    }
}
