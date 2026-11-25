//! Reflex: o aplicativo. A interface é um projeto do Aylis (`Reflex.aylisproj`: forms
//! em `forms/`, o código deles em `src/forms/`; o que a IDE refaz fica em `generated/`).
//! O mesmo exe é também o serviço, o agente, o processo do usuário e a linha de
//! comando ([`modes`]).

pub mod modes;

// O núcleo, sem tela (eram crates separados): protocolo, autenticação, rede,
// captura e codecs, Windows, o lado do host e o lado do viewer.
pub mod auth;
pub mod host;
pub mod media;
pub mod net;
pub mod platform;
pub mod proto;
pub mod viewer;

mod addr;
mod configclient;
mod escolha;
mod identity;
mod maquina;
mod sessao;
mod video;

mod admin;
mod agent;
mod cli;
mod hoststore;
mod identity_store;
// A tecla física no Linux (host e viewer); o Windows usa o scancode direto.
#[cfg_attr(windows, allow(dead_code))]
mod keymap;
mod logger;
mod probe;

// Instalar no Linux: o serviço do usuário no systemd.
#[cfg(target_os = "linux")]
mod install_linux;

// Só do host do Windows: o serviço (sessão 0), o processo do usuário, o pipe
// de configuração, a instalação e o monitor virtual.
#[cfg(windows)]
mod config_pipe;
#[cfg(windows)]
mod install;
#[cfg(windows)]
mod service;
#[cfg(windows)]
mod user;
#[cfg(windows)]
mod vdd;

/// O que a IDE gera: as structs dos forms, o acesso tipado e o registro (e os arquivos
/// dos forms, em `src/forms/`).
#[rustfmt::skip]
#[path = "../generated/mod.rs"]
pub mod generated;

/// Cria o aplicativo: tema, idioma, assets e os forms marcados como "criar no início",
/// com os handlers de todos os forms registrados.
///
/// No desenvolvimento os arquivos vêm da pasta do projeto; no aplicativo publicado,
/// de dentro do executável.
pub fn app() -> Result<aylis::Application, aylis::Error> {
    let mut app = aylis::Application::bundled(aylis::include_project!())?;
    generated::register(&mut app.ui);
    Ok(app)
}

/// Android: a NativeActivity carrega a biblioteca e chama esta função.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(android: aylis::platform::android::AndroidApp) {
    aylis::run_android(android, app);
}

/// Navegador: a página chama esta função depois de carregar o WebAssembly.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn aylis_start() {
    aylis::run_web(app);
}
