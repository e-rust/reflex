//! `reflex.exe` — um exe, vários modos (§5.3). O modo vem da linha de comando:
//!
//! | sem argumento | janela principal (abas Conectar / Esta máquina) |
//! | `--service`   | serviço do Windows (sessão 0): supervisor + pipe + SAS |
//! | `--agent`     | sessão do console, SYSTEM: servidor, captura, input |
//! | `--user`      | sessão do console, usuário logado: clipboard e arquivos |
//! | `--install`, `--uninstall`, `--my-id`, `--add-user`, `--status` | ver [`cli`] |
//! | `--admin <ip> …` | administração remota por linha de comando |
//!
//! O supervisor (serviço) mantém um `--agent` e um `--user` na sessão de
//! console ativa e os relança quando a sessão troca (§4.1).

mod admin;
mod agent;
mod cli;
mod config_pipe;
mod hoststore;
mod identity_store;
mod logger;
mod probe;
mod service;
mod user;
mod vdd;

fn main() {
    // Saída do console em UTF-8 (acentos certos no cmd/PowerShell).
    unsafe {
        let _ = windows::Win32::System::Console::SetConsoleOutputCP(65001);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str);
    let rc = match mode {
        Some("--service") => {
            logger::init("service", true);
            service::run_service()
        }
        Some("--agent") => {
            logger::init("agent", true);
            agent::run()
        }
        Some("--user") => {
            logger::init("user", false);
            user::run()
        }
        Some("--install") => report(cli::install()),
        Some("--uninstall") => report(cli::uninstall()),
        Some("--my-id") => report(cli::my_id()),
        Some("--add-user") => report(cli::add_user(&args[1..])),
        Some("--status") => report(cli::status()),
        Some("--probe") => report(probe::run(&args[1..])),
        Some("--connect") => {
            logger::init("ui", false);
            release_own_console();
            match args.get(1) {
                Some(address) => reflex_ui::connect(address),
                None => report(Err("uso: reflex --connect <ip ou nome[:porta]>".into())),
            }
        }
        Some("--admin") => report(admin::run(&args[1..])),
        Some("--help") | Some("-h") => {
            print_help();
            0
        }
        None => {
            logger::init("ui", false);
            release_own_console();
            reflex_ui::run()
        }
        Some(other) => {
            eprintln!("modo desconhecido: {other}");
            print_help();
            2
        }
    };
    log::logger().flush();
    std::process::exit(rc);
}

/// Interface sem janela de console. No Windows 11 24H2+ o manifesto
/// (`consoleAllocationPolicy`) já evita o console; em versões anteriores, se o
/// console foi criado só pra este processo (aberto pelo Explorer), solta.
/// Aberto num terminal, o console é do terminal e fica.
fn release_own_console() {
    use windows::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
    let mut pids = [0u32; 2];
    let attached = unsafe { GetConsoleProcessList(&mut pids) };
    if attached == 1 {
        unsafe {
            let _ = FreeConsole();
        }
    }
}

fn report(r: Result<(), String>) -> i32 {
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("erro: {e}");
            1
        }
    }
}

fn print_help() {
    println!(
        "reflex {}\n
  reflex                                   janela principal
  reflex --my-id                           ID de acesso deste viewer
  reflex --install                         instala e inicia o serviço (admin)
  reflex --uninstall                       remove o serviço (admin)
  reflex --add-user <ID> <nome> <nível>    cadastra usuário: master | full | view (admin)
  reflex --status                          estado, ID da máquina e usuários (admin)
  reflex --admin <endereço> <comando>      administra outra máquina como master (users list/add/remove…)
  reflex --connect <endereço>              abre direto a janela de vídeo conectada
  reflex --probe <endereço> [seg] [png]    conecta sem janela, mede e salva o último quadro
        [--drag | --motion | --typing]    animação contínua, menu Iniciar ou digitação
        [--refine=off|leve|padrao|max]    nitidez com a tela parada
        [--cor=444] [--tamanho=1536x864@150]  cor completa; resolução remota igual à janela
        [--copiar=TEXTO]                  copia TEXTO pro clipboard de lá

Os modos --service, --agent e --user são iniciados pelo próprio Reflex.",
        env!("CARGO_PKG_VERSION")
    );
}
