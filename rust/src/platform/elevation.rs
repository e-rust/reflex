//! Privilégio de administrador (§5.3): a aba "Esta máquina" só altera a
//! configuração com elevação, e o serviço confere se o cliente do pipe é
//! administrador antes de aceitar uma alteração.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    CheckTokenMembership, CreateWellKnownSid, GetTokenInformation, TokenElevation, WinBuiltinAdministratorsSid, PSID, SECURITY_MAX_SID_SIZE, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// O processo atual está rodando elevado (como administrador)?
pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0u32;
        let ok = GetTokenInformation(token, TokenElevation, Some(&mut elevation as *mut _ as *mut _), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut size).is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// `token` pertence ao grupo de administradores? Usado pelo serviço depois de
/// `ImpersonateNamedPipeClient` pra decidir se aceita alterações daquele cliente.
pub fn token_is_admin(token: HANDLE) -> bool {
    unsafe {
        let mut sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
        let mut size = sid.len() as u32;
        let psid = PSID(sid.as_mut_ptr() as *mut _);
        if CreateWellKnownSid(WinBuiltinAdministratorsSid, None, Some(psid), &mut size).is_err() {
            return false;
        }
        let mut member = windows::core::BOOL(0);
        CheckTokenMembership(Some(token), psid, &mut member).is_ok() && member.as_bool()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn elevation_query_does_not_crash() {
        // O valor depende de como os testes foram iniciados; só não pode travar.
        let _ = super::is_elevated();
    }
}
