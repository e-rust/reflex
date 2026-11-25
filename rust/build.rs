//! O build do reflex.exe: o projeto do Aylis (forms, temas, assets, ícone) e
//! o manifesto do Reflex (DPI por monitor, UTF-8, console destacado no
//! Windows 11 24H2+) vão dentro do exe. As DLLs (FFmpeg, openh264) ficam em
//! `runtime\`, onde a IDE põe o exe (`output` do projeto).

fn main() {
    aylis_build::project();

    // O manifesto é do Windows (o build roda no sistema de quem compila, então
    // vale o alvo, não o `cfg` do build.rs).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=reflex.rc");
        println!("cargo:rerun-if-changed=reflex.manifest");
        embed_resource::compile("reflex.rc", embed_resource::NONE).manifest_required().expect("compilar reflex.rc");
    }
}
