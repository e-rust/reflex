//! Controle do serviço do Windows (§5.3): consultar o estado, instalar,
//! iniciar e parar o `reflex --service`. Consultar o estado não precisa de
//! administrador; instalar/iniciar/parar precisam (a interface pede elevação).

use windows::core::PCWSTR;
use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, CreateServiceW, DeleteService, OpenSCManagerW, OpenServiceW, QueryServiceStatus, StartServiceW, SC_HANDLE, SC_MANAGER_ALL_ACCESS, SC_MANAGER_CONNECT,
    SERVICE_ALL_ACCESS, SERVICE_AUTO_START, SERVICE_CONTROL_STOP, SERVICE_ERROR_NORMAL, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START, SERVICE_STATUS, SERVICE_STOP, SERVICE_STOPPED,
    SERVICE_WIN32_OWN_PROCESS,
};

use crate::platform::wide;

/// Nome interno do serviço.
pub const SERVICE_NAME: &str = "Reflex";
/// Nome exibido no services.msc.
pub const SERVICE_DISPLAY: &str = "Reflex (acesso remoto)";

/// Estado do serviço desta máquina.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// Não existe (acesso a esta máquina nunca foi ativado).
    NotInstalled,
    Running,
    Stopped,
    /// Instalado, mas num estado de transição (iniciando/parando).
    Other,
}

impl ServiceState {
    pub fn label(self) -> &'static str {
        match self {
            ServiceState::NotInstalled => "acesso a esta máquina desativado",
            ServiceState::Running => "em execução",
            ServiceState::Stopped => "instalado, parado",
            ServiceState::Other => "mudando de estado…",
        }
    }
}

struct Handle(SC_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

fn open_manager(access: u32) -> Result<Handle, String> {
    unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), access) }.map(Handle).map_err(|e| e.message())
}

fn open_service(mgr: &Handle, access: u32) -> Result<Option<Handle>, String> {
    let name = wide(SERVICE_NAME);
    match unsafe { OpenServiceW(mgr.0, PCWSTR(name.as_ptr()), access) } {
        Ok(h) => Ok(Some(Handle(h))),
        // 1060 = ERROR_SERVICE_DOES_NOT_EXIST.
        Err(e) if e.code().0 as u32 & 0xFFFF == 1060 => Ok(None),
        Err(e) => Err(e.message()),
    }
}

/// Estado atual. Só precisa de acesso de leitura.
pub fn state() -> ServiceState {
    let Ok(mgr) = open_manager(SC_MANAGER_CONNECT) else { return ServiceState::NotInstalled };
    let svc = match open_service(&mgr, SERVICE_QUERY_STATUS) {
        Ok(Some(s)) => s,
        Ok(None) => return ServiceState::NotInstalled,
        Err(_) => return ServiceState::NotInstalled,
    };
    let mut status = SERVICE_STATUS::default();
    if unsafe { QueryServiceStatus(svc.0, &mut status) }.is_err() {
        return ServiceState::Other;
    }
    match status.dwCurrentState {
        SERVICE_RUNNING => ServiceState::Running,
        SERVICE_STOPPED => ServiceState::Stopped,
        _ => ServiceState::Other,
    }
}

/// Instala o serviço apontando pra `exe --service` (início automático). Admin.
pub fn install(exe: &std::path::Path) -> Result<(), String> {
    let mgr = open_manager(SC_MANAGER_ALL_ACCESS)?;
    if open_service(&mgr, SERVICE_QUERY_STATUS)?.is_some() {
        return Ok(()); // já instalado
    }
    let name = wide(SERVICE_NAME);
    let display = wide(SERVICE_DISPLAY);
    let bin = wide(&format!("\"{}\" --service", exe.display()));
    let svc = unsafe {
        CreateServiceW(
            mgr.0,
            PCWSTR(name.as_ptr()),
            PCWSTR(display.as_ptr()),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            PCWSTR(bin.as_ptr()),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(), // LocalSystem
            PCWSTR::null(),
        )
    }
    .map_err(|e| e.message())?;
    unsafe {
        let _ = CloseServiceHandle(svc);
    }
    Ok(())
}

/// Inicia o serviço. Admin.
pub fn start() -> Result<(), String> {
    let mgr = open_manager(SC_MANAGER_CONNECT)?;
    let svc = open_service(&mgr, SERVICE_START | SERVICE_QUERY_STATUS)?.ok_or("serviço não instalado")?;
    unsafe { StartServiceW(svc.0, None) }.map_err(|e| e.message())
}

/// Para o serviço. Admin.
pub fn stop() -> Result<(), String> {
    let mgr = open_manager(SC_MANAGER_CONNECT)?;
    let svc = open_service(&mgr, SERVICE_STOP | SERVICE_QUERY_STATUS)?.ok_or("serviço não instalado")?;
    let mut status = SERVICE_STATUS::default();
    unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut status) }.map_err(|e| e.message())
}

/// Desinstala o serviço (para antes, se estiver rodando). Admin.
pub fn uninstall() -> Result<(), String> {
    let _ = stop();
    let mgr = open_manager(SC_MANAGER_ALL_ACCESS)?;
    let Some(svc) = open_service(&mgr, SERVICE_ALL_ACCESS)? else { return Ok(()) };
    unsafe { DeleteService(svc.0) }.map_err(|e| e.message())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_query_does_not_crash_without_admin() {
        // Sem o serviço instalado (ou sem permissão), devolve um estado, sem travar.
        let s = state();
        assert!(matches!(s, ServiceState::NotInstalled | ServiceState::Stopped | ServiceState::Running | ServiceState::Other));
    }

    #[test]
    fn labels_are_filled() {
        for s in [ServiceState::NotInstalled, ServiceState::Running, ServiceState::Stopped, ServiceState::Other] {
            assert!(!s.label().is_empty());
        }
    }
}
