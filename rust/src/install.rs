//! Instalar e administrar esta máquina pela linha de comando (Windows):
//!
//! | `--install`   | instala e inicia o serviço, política do Ctrl+Alt+Del, firewall (admin) |
//! | `--uninstall` | para e remove o serviço e as regras de firewall (admin) |
//! | `--baixar-openh264 [pasta]` | baixa o openh264 da Cisco |
//!
//! `--add-user` e `--status` (os dados do host) estão no [`crate::cli`].

use crate::platform::{elevation, paths, sas, service_ctrl};

use crate::hoststore;
use crate::vdd;

/// Nome das regras de firewall (o mesmo pra criar e remover).
const FIREWALL_RULE: &str = "Reflex";

pub fn require_admin() -> Result<(), String> {
    if elevation::is_elevated() {
        Ok(())
    } else {
        Err("este comando precisa de um terminal como administrador".into())
    }
}

/// Roda `netsh` e devolve erro legível se falhar.
fn netsh(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("netsh").args(args).output().map_err(|e| format!("netsh: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("netsh {}: {}", args.join(" "), String::from_utf8_lossy(&out.stdout).trim()))
    }
}

/// Liberação de entrada pro exe (UDP do QUIC e TCP da reserva).
fn add_firewall_rules(exe: &std::path::Path) -> Result<(), String> {
    let program = format!("program={}", exe.display());
    for proto in ["UDP", "TCP"] {
        netsh(&["advfirewall", "firewall", "add", "rule", &format!("name={FIREWALL_RULE}"), "dir=in", "action=allow", &program, &format!("protocol={proto}"), "profile=any"])?;
    }
    Ok(())
}

fn remove_firewall_rules() {
    let _ = netsh(&["advfirewall", "firewall", "delete", "rule", &format!("name={FIREWALL_RULE}")]);
}

pub fn install() -> Result<(), String> {
    require_admin()?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Gera identidade e store já como admin, pra o --status funcionar antes da 1ª conexão.
    hoststore::open_store()?;
    let fp = hoststore::machine_fingerprint()?;
    remove_firewall_rules();
    add_firewall_rules(&exe)?;
    if let Err(e) = sas::enable_policy() {
        println!("aviso: política do Ctrl+Alt+Del não gravada ({e})");
    }
    // H.264 pela CPU (máquina sem placa de vídeo): o openh264 baixado da Cisco
    // agora (a licença de patente só vale assim). Com placa, não faz falta.
    if let Some(dir) = exe.parent() {
        match crate::media::openh264enc::download(dir) {
            Ok(path) => println!("Encoder H.264 por CPU: {}", path.display()),
            Err(e) => println!("aviso: openh264 não baixado ({e}); sem placa de vídeo, esta máquina não vai conseguir mandar vídeo"),
        }
    }
    // Opcional: sem ele a máquina sem monitor só mostra preto, mas o resto funciona.
    match vdd::install() {
        Ok(()) => println!("Monitor virtual: {}", vdd::status_label()),
        Err(e) => println!("aviso: monitor virtual não instalado ({e})"),
    }
    service_ctrl::install(&exe)?;
    match service_ctrl::start() {
        Ok(()) => {}
        // 1056 = ERROR_SERVICE_ALREADY_RUNNING.
        Err(e) if e.contains("1056") => {}
        Err(e) => return Err(format!("serviço instalado mas não iniciou: {e}")),
    }
    println!("Serviço instalado e iniciado.");
    println!("ID desta máquina: {fp}");
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    require_admin()?;
    service_ctrl::uninstall()?;
    remove_firewall_rules();
    if let Err(e) = vdd::uninstall() {
        println!("aviso: monitor virtual não removido ({e})");
    }
    println!("Serviço removido. Os dados ficam em {}", paths::machine_dir().display());
    Ok(())
}

/// `--baixar-openh264 [pasta]`: baixa o openh264 da Cisco (padrão: a pasta do
/// exe). Em desenvolvimento: `runtime\`, ao lado do exe que a IDE compila.
pub fn download_openh264(args: &[String]) -> Result<(), String> {
    let dir = match args.first() {
        Some(d) => std::path::PathBuf::from(d),
        None => std::env::current_exe().map_err(|e| e.to_string())?.parent().ok_or("pasta do exe")?.to_path_buf(),
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    println!("Baixando de {}…", crate::media::openh264enc::DOWNLOAD_URL);
    let path = crate::media::openh264enc::download(&dir)?;
    println!("Pronto (hash conferido): {}", path.display());
    Ok(())
}

/// O monitor virtual (a linha do `--status`).
pub fn virtual_monitor_label() -> String {
    vdd::status_label()
}
