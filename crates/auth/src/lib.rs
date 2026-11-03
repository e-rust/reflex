//! Acesso (docs/levantamento.md §11).
//!
//! - [`identity`]: a chave/certificado de cada instalação e o ID de acesso;
//! - [`store`]: usuários, níveis, regras do master e a configuração do host,
//!   persistidos com assinatura (detecta edição por fora);
//! - [`invite`]: o código de acesso único rotativo;
//! - [`pake`]: a prova do código por SPAKE2, amarrada ao canal TLS;
//! - [`policy`]: quem pode entrar e com quais permissões;
//! - [`admin`]: a API de administração (local e remota).

pub mod admin;
pub mod identity;
pub mod invite;
pub mod pake;
pub mod policy;
pub mod store;

/// Agora em segundos Unix (as regras recebem o tempo por parâmetro pra serem testáveis).
pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
