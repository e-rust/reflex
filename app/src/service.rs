//! Modo `--service` (§4.1, §5.3): o serviço do Windows (sessão 0). Ele não
//! captura; supervisiona. Mantém um `--agent` (SYSTEM) e, quando há usuário
//! logado, um `--user` na sessão de console ativa, e os relança quando a
//! sessão troca (login, logoff, bloqueio, troca de usuário).
//!
//! A decisão "o que deve estar rodando agora" é um passo puro ([`plan`]),
//! separado das chamadas ao Windows, e por isso testável.

use std::path::PathBuf;

use reflex_platform::session::{self, Child};

/// O que o supervisor quer rodando na sessão de console.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Desired {
    pub session: Option<u32>,
    pub want_agent: bool,
    pub want_user: bool,
}

/// Decide o alvo a partir do estado do Windows. Puro.
pub fn plan(console_session: Option<u32>, has_user: bool) -> Desired {
    match console_session {
        // Há uma sessão de console (mesmo na tela de login): agente sempre;
        // o processo do usuário só quando há alguém logado.
        Some(s) => Desired { session: Some(s), want_agent: true, want_user: has_user },
        None => Desired { session: None, want_agent: false, want_user: false },
    }
}

/// Um processo supervisionado e em qual sessão foi lançado.
#[derive(Default)]
struct Slot {
    child: Option<Child>,
    session: Option<u32>,
}

impl Slot {
    /// Precisa (re)lançar? Sim se o alvo mudou de sessão, o processo morreu,
    /// ou não devia estar rodando.
    fn needs_restart(&self, want: bool, session: Option<u32>) -> bool {
        match (&self.child, want) {
            (None, true) => true,
            (Some(_), false) => true,
            (Some(c), true) => self.session != session || !c.is_running(),
            (None, false) => false,
        }
    }

    fn stop(&mut self) {
        if let Some(c) = self.child.take() {
            c.kill();
        }
        self.session = None;
    }
}

struct Supervisor {
    exe: PathBuf,
    agent: Slot,
    user: Slot,
}

impl Supervisor {
    fn new(exe: PathBuf) -> Self {
        Self { exe, agent: Slot::default(), user: Slot::default() }
    }

    /// Reavalia e aplica o plano (lança/mata o que precisar).
    fn reconcile(&mut self) {
        let d = plan(session::active_console_session(), console_has_user());
        let session = d.session;

        if self.agent.needs_restart(d.want_agent, session) {
            self.agent.stop();
            if let (true, Some(s)) = (d.want_agent, session) {
                match session::spawn_system_in_session(s, &self.exe, "--agent") {
                    Ok(c) => {
                        self.agent.child = Some(c);
                        self.agent.session = Some(s);
                        log::info!("agente lançado na sessão {s}");
                    }
                    Err(e) => log::warn!("lançar agente: {e}"),
                }
            }
        }
        if self.user.needs_restart(d.want_user, session) {
            self.user.stop();
            if let (true, Some(s)) = (d.want_user, session) {
                match session::spawn_user_in_session(s, &self.exe, "--user") {
                    Ok(c) => {
                        self.user.child = Some(c);
                        self.user.session = Some(s);
                        log::info!("processo do usuário lançado na sessão {s}");
                    }
                    Err(e) => log::warn!("lançar processo do usuário: {e}"),
                }
            }
        }
    }

    fn stop_all(&mut self) {
        self.agent.stop();
        self.user.stop();
    }
}

fn console_has_user() -> bool {
    session::active_console_session().map(session::has_logged_user).unwrap_or(false)
}

/// Laço do supervisor até `stop` sinalizar. Usado pelo serviço e dá pra rodar
/// em primeiro plano (depuração: `reflex --service` num terminal admin).
pub fn supervise(stop: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("reflex.exe"));
    let mut sup = Supervisor::new(exe);
    let mut screen = crate::vdd::Watcher::default();
    while !stop.load(Ordering::Relaxed) {
        sup.reconcile();
        screen.tick();
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    sup.stop_all();
}

windows_service::define_windows_service!(ffi_service_main, service_main);

/// `ERROR_FAILED_SERVICE_CONTROLLER_CONNECT`: o processo não foi iniciado pelo SCM.
const NOT_FROM_SCM: i32 = 1063;

/// Ponto de entrada do `--service`. Iniciado pelo SCM, registra o serviço;
/// rodado à mão (terminal admin, depuração), cai no modo primeiro plano.
pub fn run_service() -> i32 {
    match windows_service::service_dispatcher::start(reflex_platform::service_ctrl::SERVICE_NAME, ffi_service_main) {
        Ok(()) => 0,
        Err(windows_service::Error::Winapi(e)) if e.raw_os_error() == Some(NOT_FROM_SCM) => {
            log::info!("fora do SCM: supervisor em primeiro plano");
            run_supervisor(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
            0
        }
        Err(e) => {
            log::error!("dispatcher do serviço: {e}");
            1
        }
    }
}

/// Pipe de configuração numa thread e o supervisor na atual, até `stop`.
fn run_supervisor(stop: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    std::thread::spawn(crate::config_pipe::serve);
    log::info!("supervisor iniciado");
    supervise(stop);
    log::info!("supervisor encerrado");
}

/// Chamado pelo SCM numa thread dele.
fn service_main(_args: Vec<std::ffi::OsString>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use windows_service::service::{ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType};
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};

    let stop = Arc::new(AtomicBool::new(false));
    let stop_handler = stop.clone();
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            stop_handler.store(true, Ordering::Relaxed);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let status = match service_control_handler::register(reflex_platform::service_ctrl::SERVICE_NAME, handler) {
        Ok(s) => s,
        Err(e) => {
            log::error!("registrar o controle do serviço: {e}");
            return;
        }
    };
    let report = |state: ServiceState, accept: ServiceControlAccept| {
        let _ = status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: std::time::Duration::default(),
            process_id: None,
        });
    };
    report(ServiceState::Running, ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN);
    run_supervisor(stop);
    report(ServiceState::Stopped, ServiceControlAccept::empty());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_console_session_wants_nothing() {
        assert_eq!(plan(None, false), Desired { session: None, want_agent: false, want_user: false });
    }

    #[test]
    fn login_screen_wants_agent_only() {
        // Sessão de console existe (winlogon), mas ninguém logado.
        assert_eq!(plan(Some(1), false), Desired { session: Some(1), want_agent: true, want_user: false });
    }

    #[test]
    fn logged_in_wants_both() {
        assert_eq!(plan(Some(2), true), Desired { session: Some(2), want_agent: true, want_user: true });
    }

    #[test]
    fn slot_restart_rules() {
        let s = Slot::default();
        assert!(s.needs_restart(true, Some(1))); // nada rodando, quer
        assert!(!s.needs_restart(false, Some(1))); // nada rodando, não quer
        // sem Child, want=false → não precisa relançar.
        assert!(!s.needs_restart(false, None));
    }
}
