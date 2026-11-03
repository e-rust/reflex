//! Embute o manifesto (Common Controls 6, DPI, UTF-8) no reflex.exe e põe as
//! DLLs de `third_party\ffmpeg` (FFmpeg + vcruntime140) ao lado dele: o FFmpeg é carregado
//! em tempo de execução e o Windows só procura na pasta do exe ou no PATH.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=reflex.rc");
    println!("cargo:rerun-if-changed=reflex.manifest");
    embed_resource::compile("reflex.rc", embed_resource::NONE).manifest_required().expect("compilar reflex.rc");

    let dlls = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("third_party").join("ffmpeg");
    println!("cargo:rerun-if-changed={}", dlls.display());
    if let Some(dest) = exe_dir() {
        place_dlls(&dlls, &dest);
    }
}

/// Pasta onde o Cargo põe o reflex.exe (`target\release`, `target\debug`…):
/// o `OUT_DIR` é `<pasta>\build\reflex-<hash>\out`.
fn exe_dir() -> Option<PathBuf> {
    let out = PathBuf::from(std::env::var_os("OUT_DIR")?);
    out.ancestors().nth(3).map(Path::to_path_buf)
}

/// Hard link (não ocupa espaço de novo; são 42 MB) ou, se não der, cópia.
/// Só mexe no que mudou.
fn place_dlls(src_dir: &Path, dest_dir: &Path) {
    let Ok(entries) = fs::read_dir(src_dir) else {
        println!("cargo:warning=pasta de DLLs não encontrada: {}", src_dir.display());
        return;
    };
    for entry in entries.flatten() {
        let src = entry.path();
        if !src.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll")) {
            continue;
        }
        let dest = dest_dir.join(entry.file_name());
        if same_file(&src, &dest) {
            continue;
        }
        let _ = fs::remove_file(&dest);
        if fs::hard_link(&src, &dest).is_err() {
            if let Err(e) = fs::copy(&src, &dest) {
                println!("cargo:warning=não deu pra pôr {} em {}: {e}", src.display(), dest_dir.display());
            }
        }
    }
}

/// Mesmo tamanho e mesma data de modificação: já é o arquivo certo.
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.len() == b.len() && a.modified().ok() == b.modified().ok(),
        _ => false,
    }
}
