//! Ctrl+Alt+Del remoto (§4.1): `SendSAS` da `sas.dll`, chamado pelo serviço.
//! Exige a política `SoftwareSASGeneration` (o instalador grava com
//! [`enable_policy`]).

use windows::core::BOOL;

/// Manda o Ctrl+Alt+Del. `Err` = DLL/função ausente.
pub fn send_sas() -> Result<(), String> {
    unsafe {
        let lib = libloading::Library::new("sas.dll").map_err(|e| format!("sas.dll: {e}"))?;
        let f: libloading::Symbol<unsafe extern "system" fn(BOOL)> = lib.get(b"SendSAS\0").map_err(|e| format!("SendSAS: {e}"))?;
        // FALSE = chamado por um serviço.
        f(BOOL(0));
    }
    Ok(())
}

/// `SoftwareSASGeneration = 3` (serviços e aplicativos de acessibilidade). Exige admin.
pub fn enable_policy() -> Result<(), String> {
    crate::platform::registry::set_dword(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System", "SoftwareSASGeneration", 3)
}
