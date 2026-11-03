//! Integração com o Windows (docs/levantamento.md §4, §10.7, §10.8).
//!
//! Cada módulo embrulha uma área da API Win32 atrás de funções/tipos
//! pequenos, com a parte de decisão separada (e testada) da chamada ao sistema:
//! - [`desktop`]: seguir a área de trabalho de entrada (Default ↔ Winlogon);
//! - [`input`]: `SendInput` a partir das mensagens do protocolo;
//! - [`session`]: sessões do Windows e lançar processos nelas;
//! - [`sas`]: Ctrl+Alt+Del (`SendSAS`);
//! - [`power`]: manter a tela acesa durante a sessão;
//! - [`clipboard`]: ler/escrever o clipboard e vigiar mudanças;
//! - [`vfiles`]: arquivos virtuais pro Explorer colar (`IDataObject`);
//! - [`files`]: árvore de arquivos copiados e leitura em blocos;
//! - [`pipe`]: pipes nomeados com ACL e conferência de administrador;
//! - [`dpapi`]: proteger segredos em disco;
//! - [`elevation`]: privilégio de administrador;
//! - [`paths`]: pastas do Reflex.

pub mod clipboard;
pub mod cpu;
pub mod desktop;
pub mod devices;
pub mod display;
pub mod dpapi;
pub mod driver_trust;
pub mod elevation;
pub mod files;
pub mod input;
pub mod paths;
pub mod pipe;
pub mod power;
pub mod registry;
pub mod sas;
pub mod service_ctrl;
pub mod session;
pub mod timer;
pub mod vfiles;

/// Texto → UTF-16 terminado em zero.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 (até o primeiro zero) → texto.
pub fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

#[cfg(test)]
mod tests {
    #[test]
    fn wide_roundtrip() {
        assert_eq!(super::from_wide(&super::wide("ação")), "ação");
        assert_eq!(*super::wide("a").last().unwrap(), 0);
    }
}
