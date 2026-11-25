//! Segredos em disco no Linux: a chave do viewer e o segredo da máquina ficam
//! num arquivo `0600` numa pasta `0700` (só o dono lê; a máquina é do root).
//! Não há cifra: é o que o `ssh` faz com as chaves dele. O chaveiro do
//! sistema (libsecret) fica pra depois.

use std::fs::{DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Do usuário (na pasta dele).
    User,
    /// Da máquina (na pasta do root).
    Machine,
}

/// O segredo gravado em `path` (nada se não existe ou não abre).
pub fn read(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// Grava `data` em `path` só para o dono (troca atômica pelo `.tmp`). O
/// `Scope` está na pasta escolhida por quem chama.
pub fn write(path: &Path, data: &[u8], _scope: Scope) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn only_the_owner_reads_it() {
        let dir = std::env::temp_dir().join(format!("reflex-secret-{}", std::process::id()));
        let path = dir.join("s.bin");
        write(&path, b"segredo", Scope::User).unwrap();
        assert_eq!(read(&path).unwrap(), b"segredo");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        // Regravar troca o conteúdo inteiro.
        write(&path, b"x", Scope::User).unwrap();
        assert_eq!(read(&path).unwrap(), b"x");
        std::fs::remove_dir_all(&dir).ok();
    }
}
