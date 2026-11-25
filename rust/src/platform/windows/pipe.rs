//! Pipe nomeado da configuração (§5.3): a aba "Esta máquina" fala com o
//! serviço por aqui. O serviço lê o estado pra qualquer um e só aceita
//! alterações de um cliente administrador (`ImpersonateNamedPipeClient` +
//! [`crate::platform::elevation::token_is_admin`]).
//!
//! Enquadramento: `u32` (tamanho) + corpo. A camada de cima (serviço)
//! decide o conteúdo; aqui só entrega os bytes e diz se o cliente é admin.

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

use crate::proto::frame::MAX_FRAME;

/// Lê uma mensagem enquadrada.
pub async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "mensagem grande demais"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(body)
}

/// Escreve uma mensagem enquadrada.
pub async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, body: &[u8]) -> io::Result<()> {
    w.write_all(&(body.len() as u32).to_le_bytes()).await?;
    w.write_all(body).await?;
    w.flush().await
}

/// ACL do pipe de configuração: SYSTEM e administradores com tudo; usuários
/// interativos lêem e escrevem (mandam pedidos). Quem pode ALTERAR é decidido
/// pelo serviço por pedido ([`client_is_admin`]), não pela ACL.
pub const CONFIG_PIPE_SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)";

/// ACL dos pipes do agente com o processo do usuário: SYSTEM e usuários
/// interativos. O agente confere a sessão e o processo de cada cliente
/// ([`client_session_process`]).
pub const USER_PIPE_SDDL: &str = "D:(A;;GA;;;SY)(A;;GRGW;;;IU)";

/// Cria a próxima instância do servidor de pipe com a ACL `sddl` (o serviço aceita em laço).
pub fn create_server(name: &str, first: bool, sddl: &str) -> io::Result<NamedPipeServer> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

    let sddl_w = crate::platform::wide(sddl);
    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(windows::core::PCWSTR(sddl_w.as_ptr()), SDDL_REVISION_1, &mut sd, None) }.map_err(io::Error::other)?;
    let mut attrs = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: false.into() };
    // SAFETY: `attrs` aponta pra um descritor válido até o LocalFree abaixo;
    // o Windows copia o descritor na criação do pipe.
    let server = unsafe { ServerOptions::new().first_pipe_instance(first).create_with_security_attributes_raw(name, &mut attrs as *mut _ as *mut _) };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(sd.0)));
    }
    server
}

/// Conecta como cliente. Todas as instâncias ocupadas (o servidor ainda não
/// criou a próxima): tenta de novo por um instante.
pub async fn connect(name: &str) -> io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    const ERROR_PIPE_BUSY: i32 = 231;
    let mut tries = 0;
    loop {
        match ClientOptions::new().open(name) {
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && tries < 50 => {
                tries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            other => return other,
        }
    }
}

/// Sessão do Windows e processo do cliente conectado.
pub fn client_session_process(server: &NamedPipeServer) -> Option<(u32, u32)> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeClientSessionId};

    let handle = HANDLE(server.as_raw_handle() as *mut _);
    let (mut session, mut pid) = (0u32, 0u32);
    // SAFETY: handle de um pipe aberto (vive enquanto `server`).
    unsafe {
        GetNamedPipeClientSessionId(handle, &mut session).ok()?;
        GetNamedPipeClientProcessId(handle, &mut pid).ok()?;
    }
    Some((session, pid))
}

/// Processo que criou o servidor do pipe (pra conferir com quem se esperava).
pub fn server_process(client: &tokio::net::windows::named_pipe::NamedPipeClient) -> Option<u32> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Pipes::GetNamedPipeServerProcessId;

    let mut pid = 0u32;
    // SAFETY: handle de um pipe aberto (vive enquanto `client`).
    unsafe { GetNamedPipeServerProcessId(HANDLE(client.as_raw_handle() as *mut _), &mut pid) }.ok()?;
    Some(pid)
}

/// Executa `f` na identidade do cliente do pipe e devolve se ele é admin.
///
/// # Safety
/// Chamar só no servidor com um cliente conectado.
pub fn client_is_admin(server: &NamedPipeServer) -> bool {
    use std::os::windows::io::AsRawHandle;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::RevertToSelf;
    use windows::Win32::Security::TOKEN_QUERY;
    use windows::Win32::System::Pipes::ImpersonateNamedPipeClient;
    use windows::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

    // O handle do pipe do tokio: usamos o próprio OS handle.
    let handle = HANDLE(server.as_raw_handle() as *mut _);
    unsafe {
        if ImpersonateNamedPipeClient(handle).is_err() {
            return false;
        }
        let mut token = HANDLE::default();
        let got = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token).is_ok();
        let admin = got && crate::platform::elevation::token_is_admin(token);
        if got {
            let _ = windows::Win32::Foundation::CloseHandle(token);
        }
        let _ = RevertToSelf();
        admin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn framed_roundtrip_over_pipe() {
        let name = format!(r"\\.\pipe\reflex-test-{}", std::process::id());
        let server = create_server(&name, true, CONFIG_PIPE_SDDL).unwrap();
        let name2 = name.clone();
        let client = tokio::spawn(async move {
            let mut c = connect(&name2).await.unwrap();
            write_frame(&mut c, b"ping").await.unwrap();
            read_frame(&mut c).await.unwrap()
        });
        server.connect().await.unwrap();
        let mut server = server;
        let got = read_frame(&mut server).await.unwrap();
        assert_eq!(got, b"ping");
        write_frame(&mut server, b"pong").await.unwrap();
        assert_eq!(client.await.unwrap(), b"pong");
    }

    #[tokio::test]
    async fn both_ends_see_this_process() {
        let name = format!(r"\\.\pipe\reflex-test-pid-{}", std::process::id());
        let server = create_server(&name, true, USER_PIPE_SDDL).unwrap();
        let client = connect(&name).await.unwrap();
        server.connect().await.unwrap();
        let me = std::process::id();
        assert_eq!(server_process(&client), Some(me));
        assert_eq!(client_session_process(&server), Some((crate::platform::session::current_session(), me)));
    }
}
