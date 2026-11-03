//! Monitor virtual (§4.2). Sem monitor ligado — servidor sem tela, VM Hyper-V
//! com o console fechado — o Windows para de desenhar e a captura só vê
//! preto (medido na VM). O Virtual Display Driver (`third_party/vdd`, MIT,
//! assinado) cria um monitor que não existe; o Windows desenha nele e o DXGI
//! captura como sempre.
//!
//! - [`install`]/[`uninstall`]: o `--install`/`--uninstall` põem e tiram o
//!   driver (embutido no exe) e a configuração;
//! - [`Watcher`]: o serviço liga o monitor virtual quando não há monitor de
//!   verdade e desliga quando volta um (senão viraria um 2º monitor).

use std::path::PathBuf;

use reflex_platform::devices::{self, Device, GUID_DEVCLASS_DISPLAY};
use reflex_platform::driver_trust::TrustedSigner;
use reflex_platform::{paths, registry};

/// Hardware ID do dispositivo raiz que o `.inf` atende.
const HARDWARE_ID: &str = r"Root\MttVDD";
/// Onde o driver procura o `vdd_settings.xml` (valor `VDDPATH`).
const SETTINGS_KEY: &str = r"SOFTWARE\MikeTheTech\VirtualDisplayDriver";

const FILES: [(&str, &[u8]); 3] = [
    ("MttVDD.inf", include_bytes!("../../third_party/vdd/MttVDD.inf")),
    ("mttvdd.cat", include_bytes!("../../third_party/vdd/mttvdd.cat")),
    ("MttVDD.dll", include_bytes!("../../third_party/vdd/MttVDD.dll")),
];

/// `%ProgramData%\Reflex\vdd`: configuração do driver (e o pacote, em `driver\`).
fn dir() -> PathBuf {
    paths::machine_dir().join("vdd")
}

/// Modos do monitor virtual. Com a resolução "igual à janela" o host escolhe
/// o maior que cabe na janela do viewer: quanto mais tamanhos, mais perto.
/// 16:9, 16:10, 4:3/5:4 e 3:2, de 1024 a 3840 de largura.
pub const MODES: [(u32, u32); 26] = [
    (1024, 576), (1280, 720), (1366, 768), (1536, 864), (1600, 900), (1920, 1080), (2048, 1152), (2304, 1296), (2560, 1440), (3200, 1800), (3840, 2160),
    (1280, 800), (1440, 900), (1680, 1050), (1920, 1200), (2560, 1600),
    (1024, 768), (1152, 864), (1280, 960), (1280, 1024), (1440, 1080), (1600, 1200),
    (1440, 960), (1620, 1080), (1920, 1280), (2160, 1440),
];

/// Modo inicial do monitor virtual: 1080p com placa de vídeo; sem ela, 720p
/// — o Windows desenha a tela na CPU e o x264 também usa a CPU (medido numa
/// VM de 1 núcleo: 1080p ficava em 8–17 quadros/s no uso normal). Pura.
pub fn preferred_mode(hardware_gpu: bool) -> (u32, u32) {
    if hardware_gpu {
        (1920, 1080)
    } else {
        (1280, 720)
    }
}

/// Configuração do driver: um monitor, 60 Hz, `first` primeiro (o modo que o
/// Windows escolhe ao ligar); os outros ficam disponíveis. Pura.
pub fn settings_xml(first: (u32, u32)) -> String {
    let modes: String = std::iter::once(first)
        .chain(MODES.into_iter().filter(|&m| m != first))
        .map(|(w, h)| format!("        <resolution><width>{w}</width><height>{h}</height><refresh_rate>60</refresh_rate></resolution>\n"))
        .collect();
    format!(
        "<?xml version='1.0' encoding='utf-8'?>\n<vdd_settings>\n    <monitors><count>1</count></monitors>\n    <gpu><friendlyname>default</friendlyname></gpu>\n    <global><g_refresh_rate>60</g_refresh_rate></global>\n    <resolutions>\n{modes}    </resolutions>\n    <options>\n        <CustomEdid>false</CustomEdid>\n        <HardwareCursor>true</HardwareCursor>\n        <SDR10bit>false</SDR10bit>\n        <HDRPlus>false</HDRPlus>\n        <logging>false</logging>\n        <debuglogging>false</debuglogging>\n    </options>\n</vdd_settings>\n"
    )
}

/// Instala o driver e deixa o monitor virtual no estado certo (exige admin).
pub fn install() -> Result<(), String> {
    let root = dir();
    let pkg = root.join("driver");
    std::fs::create_dir_all(&pkg).map_err(|e| format!("{}: {e}", pkg.display()))?;
    for (name, bytes) in FILES {
        std::fs::write(pkg.join(name), bytes).map_err(|e| format!("{name}: {e}"))?;
    }
    std::fs::write(root.join("vdd_settings.xml"), settings_xml(preferred_mode(reflex_media::capture::has_hardware_gpu()))).map_err(|e| format!("vdd_settings.xml: {e}"))?;
    registry::set_string(SETTINGS_KEY, "VDDPATH", &root.to_string_lossy())?;

    // Confia no assinante só durante a instalação (ver `driver_trust`).
    let trust = TrustedSigner::for_catalog(&pkg.join("mttvdd.cat"))?;
    let reboot = devices::install_root_device(&GUID_DEVCLASS_DISPLAY, "Display", HARDWARE_ID, &pkg.join("MttVDD.inf"))?;
    drop(trust);
    if reboot {
        log::warn!("monitor virtual: o Windows pediu reinício");
    }
    // Instalado já liga; com monitor de verdade, desliga na hora.
    Watcher::default().apply_now();
    Ok(())
}

/// Tira o dispositivo, o driver e a configuração (exige admin).
pub fn uninstall() -> Result<(), String> {
    devices::remove(&GUID_DEVCLASS_DISPLAY, HARDWARE_ID)?;
    registry::delete_tree(SETTINGS_KEY);
    let _ = std::fs::remove_dir_all(dir());
    Ok(())
}

/// O monitor virtual, se instalado.
pub fn state() -> Option<Device> {
    devices::find(&GUID_DEVCLASS_DISPLAY, HARDWARE_ID)
}

/// Texto pro `--status`.
pub fn status_label() -> String {
    match state() {
        None => "não instalado".into(),
        Some(d) if d.enabled => "ligado (sem monitor de verdade)".into(),
        Some(_) => "desligado (há monitor de verdade)".into(),
    }
}

/// O que fazer com o monitor virtual: `Some(true)` ligar, `Some(false)`
/// desligar, `None` nada. Liga só quando não há monitor de verdade. Pura.
pub fn decide(real_monitors: usize, virtual_enabled: Option<bool>) -> Option<bool> {
    let enabled = virtual_enabled?;
    let want = real_monitors == 0;
    (want != enabled).then_some(want)
}

/// Mesma decisão em [`Watcher::CONFIRM`] voltas seguidas antes de agir: ao
/// abrir/fechar o console do Hyper-V o monitor some e volta por instantes.
#[derive(Debug, Default)]
pub struct Watcher {
    pending: Option<(bool, u32)>,
}

impl Watcher {
    pub const CONFIRM: u32 = 2;

    /// Uma volta do supervisor (a cada 2 s). Pura com relação ao Windows:
    /// recebe o que observou e devolve se deve agir agora.
    pub fn step(&mut self, decision: Option<bool>) -> Option<bool> {
        let Some(want) = decision else {
            self.pending = None;
            return None;
        };
        let seen = match self.pending {
            Some((w, n)) if w == want => n + 1,
            _ => 1,
        };
        if seen >= Self::CONFIRM {
            self.pending = None;
            Some(want)
        } else {
            self.pending = Some((want, seen));
            None
        }
    }

    /// Observa o Windows e liga/desliga se for o caso.
    pub fn tick(&mut self) {
        let Some(dev) = state() else { return };
        let real = devices::monitors_present(Some(&dev.instance_id));
        if let Some(on) = self.step(decide(real, Some(dev.enabled))) {
            switch(on, real);
        }
    }

    /// Sem esperar confirmação (logo depois de instalar).
    fn apply_now(&mut self) {
        let Some(dev) = state() else { return };
        let real = devices::monitors_present(Some(&dev.instance_id));
        if let Some(on) = decide(real, Some(dev.enabled)) {
            switch(on, real);
        }
    }
}

fn switch(on: bool, real: usize) {
    match devices::set_enabled(&GUID_DEVCLASS_DISPLAY, HARDWARE_ID, on) {
        Ok(()) if on => log::info!("monitor virtual ligado (nenhum monitor de verdade)"),
        Ok(()) => log::info!("monitor virtual desligado ({real} monitor(es) de verdade)"),
        Err(e) => log::warn!("monitor virtual: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_monitor_only_without_a_real_one() {
        assert_eq!(decide(0, Some(false)), Some(true));
        assert_eq!(decide(0, Some(true)), None);
        assert_eq!(decide(1, Some(true)), Some(false));
        assert_eq!(decide(2, Some(false)), None);
        // Driver não instalado: nada a fazer.
        assert_eq!(decide(0, None), None);
    }

    #[test]
    fn watcher_waits_for_a_stable_decision() {
        let mut w = Watcher::default();
        assert_eq!(w.step(Some(true)), None);
        assert_eq!(w.step(Some(true)), Some(true));
        // Pisca (some e volta): nada.
        assert_eq!(w.step(Some(false)), None);
        assert_eq!(w.step(None), None);
        assert_eq!(w.step(Some(false)), None);
        assert_eq!(w.step(Some(true)), None);
        assert_eq!(w.step(Some(true)), Some(true));
    }

    #[test]
    fn settings_have_one_monitor_and_the_preferred_mode_first() {
        let first_width = |xml: &str| {
            let i = xml.find("<width>").unwrap();
            xml[i + 7..xml[i..].find("</width>").unwrap() + i].to_string()
        };
        let gpu = settings_xml(preferred_mode(true));
        assert!(gpu.contains("<count>1</count>"));
        assert_eq!(first_width(&gpu), "1920");
        assert_eq!(gpu.matches("<resolution>").count(), MODES.len());
        let cpu = settings_xml(preferred_mode(false));
        assert_eq!(first_width(&cpu), "1280");
        // Sem repetir o modo escolhido.
        assert_eq!(cpu.matches("<width>1280</width><height>720</height>").count(), 1);
        assert_eq!(cpu.matches("<resolution>").count(), MODES.len());
    }

    #[test]
    fn bundled_driver_is_the_signed_release() {
        // O .cat assina o .inf (UTF-16 com BOM, CRLF): um byte trocado e o Windows recusa.
        let utf16 = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(u16::to_le_bytes).collect() };
        let inf = FILES[0].1;
        assert_eq!(&inf[..2], &[0xFF, 0xFE]);
        let hwid = utf16(HARDWARE_ID);
        assert!(inf.windows(hwid.len()).any(|w| w == hwid));
        assert!(inf.windows(4).any(|w| w == utf16("\r\n")), "o .inf perdeu o CRLF original");
        assert_eq!(FILES[1].1.len(), 13017);
    }
}
