//! Sessões do Windows e processos lançados nelas (§4.1, §10.8.2).
//!
//! O serviço (SYSTEM, sessão 0) lança na sessão do console:
//! - o **agente** como SYSTEM ([`spawn_system_in_session`]): o token do
//!   próprio serviço com o `SessionId` trocado — pode entrar na área de
//!   trabalho Winlogon;
//! - o **processo do usuário** com o token de quem está logado
//!   ([`spawn_user_in_session`]): clipboard e arquivos com as permissões dele.

use std::path::Path;

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Security::{DuplicateTokenEx, SecurityImpersonation, SetTokenInformation, TokenPrimary, TokenSessionId, TOKEN_ALL_ACCESS};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, GetCurrentProcessId, GetExitCodeProcess, OpenProcessToken, TerminateProcess, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT,
    PROCESS_INFORMATION, STARTUPINFOW,
};

use crate::platform::wide;

/// Sessão ligada ao monitor/teclado físicos. `None` = nenhuma (troca em andamento).
pub fn active_console_session() -> Option<u32> {
    let id = unsafe { WTSGetActiveConsoleSessionId() };
    (id != u32::MAX).then_some(id)
}

/// Sessão deste processo.
pub fn current_session() -> u32 {
    let mut id = 0u32;
    unsafe {
        let _ = ProcessIdToSessionId(GetCurrentProcessId(), &mut id);
    }
    id
}

/// Processo filho; mata no `kill` (não no Drop: quem decide é o supervisor).
pub struct Child {
    process: HANDLE,
    pub pid: u32,
}

// SAFETY: um HANDLE de processo pode ser usado de qualquer thread.
unsafe impl Send for Child {}

impl Child {
    pub fn is_running(&self) -> bool {
        unsafe { WaitForSingleObject(self.process, 0) == WAIT_TIMEOUT }
    }

    /// Espera até `ms`; `true` = terminou.
    pub fn wait(&self, ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.process, ms) == WAIT_OBJECT_0 }
    }

    pub fn exit_code(&self) -> Option<u32> {
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(self.process, &mut code) }.ok()?;
        (code != 259).then_some(code) // 259 = STILL_ACTIVE
    }

    pub fn kill(&self) {
        unsafe {
            let _ = TerminateProcess(self.process, 1);
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.process);
        }
    }
}

struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn primary_copy(src: HANDLE) -> windows::core::Result<Token> {
    let mut dup = HANDLE::default();
    unsafe { DuplicateTokenEx(src, TOKEN_ALL_ACCESS, None, SecurityImpersonation, TokenPrimary, &mut dup)? };
    Ok(Token(dup))
}

/// Linha de comando com o exe entre aspas.
pub fn command_line(exe: &Path, args: &str) -> String {
    format!("\"{}\" {}", exe.display(), args).trim_end().to_string()
}

fn create(token: &Token, exe: &Path, args: &str, desktop: &str) -> windows::core::Result<Child> {
    let mut env: *mut core::ffi::c_void = std::ptr::null_mut();
    unsafe { CreateEnvironmentBlock(&mut env, Some(token.0), false)? };
    let mut cmd = wide(&command_line(exe, args));
    let mut desk = wide(desktop);
    let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, lpDesktop: PWSTR(desk.as_mut_ptr()), ..Default::default() };
    let mut pi = PROCESS_INFORMATION::default();
    let app = wide(&exe.display().to_string());
    let r = unsafe {
        CreateProcessAsUserW(
            Some(token.0),
            windows::core::PCWSTR(app.as_ptr()),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            Some(env),
            None,
            &si,
            &mut pi,
        )
    };
    unsafe {
        let _ = DestroyEnvironmentBlock(env);
    }
    r?;
    unsafe {
        let _ = CloseHandle(pi.hThread);
    }
    Ok(Child { process: pi.hProcess, pid: pi.dwProcessId })
}

/// Lança `exe` como SYSTEM na sessão `session` (só funciona a partir de SYSTEM).
pub fn spawn_system_in_session(session: u32, exe: &Path, args: &str) -> windows::core::Result<Child> {
    let mut own = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut own)? };
    let own = Token(own);
    let token = primary_copy(own.0)?;
    unsafe { SetTokenInformation(token.0, TokenSessionId, &session as *const u32 as *const _, 4)? };
    create(&token, exe, args, "winsta0\\default")
}

/// Lança `exe` como o usuário logado na sessão `session` (exige SYSTEM).
/// Falha sem usuário logado (tela de login).
pub fn spawn_user_in_session(session: u32, exe: &Path, args: &str) -> windows::core::Result<Child> {
    let mut user = HANDLE::default();
    unsafe { WTSQueryUserToken(session, &mut user)? };
    let user = Token(user);
    let token = primary_copy(user.0)?;
    create(&token, exe, args, "winsta0\\default")
}

/// Há alguém logado na sessão?
pub fn has_logged_user(session: u32) -> bool {
    let mut user = HANDLE::default();
    let ok = unsafe { WTSQueryUserToken(session, &mut user) }.is_ok();
    if ok {
        drop(Token(user));
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_quotes_exe() {
        assert_eq!(command_line(Path::new(r"C:\Program Files\Reflex\reflex.exe"), "--agent"), r#""C:\Program Files\Reflex\reflex.exe" --agent"#);
        assert_eq!(command_line(Path::new("a.exe"), ""), "\"a.exe\"");
    }

    #[test]
    fn current_session_is_known() {
        // Testes rodam numa sessão interativa (≥ 1) ou no CI (0); só não pode travar.
        let _ = current_session();
        let _ = active_console_session();
    }
}
