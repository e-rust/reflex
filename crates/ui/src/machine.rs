//! Página "Esta máquina" (§5.2, §5.3). Mostra o estado do serviço, a impressão
//! digital (o ID a conferir no viewer), os endereços, a lista de usuários e o
//! botão "Compartilhar acesso" (convite de uso único). Fala com o serviço pelo
//! pipe de configuração; alterações exigem administrador (o serviço confere).

use winsafe::co;
use winsafe::gui;
use winsafe::prelude::*;

use crate::configclient;
use crate::theme;
use reflex_platform::service_ctrl::{self, ServiceState};
use reflex_proto::msg::{AdminRequest, AdminResponse};
use reflex_proto::perms::Level;

#[derive(Clone)]
pub struct MachinePage {
    wnd: gui::WindowControl,
    title: gui::Label,
    service: gui::Label,
    fingerprint: gui::Label,
    addresses: gui::Label,
    users: gui::ListView,
    share: gui::Button,
    invite: gui::Label,
    refresh: gui::Button,
}

impl MachinePage {
    pub fn new(parent: &(impl GuiParent + 'static), position: (i32, i32), size: (i32, i32)) -> Self {
        let wnd = gui::WindowControl::new(parent, theme::page_opts(position, size));
        let title = gui::Label::new(&wnd, gui::LabelOpts { text: "Esta máquina", position: gui::dpi(24, 18), size: gui::dpi(300, 34), ..Default::default() });
        let refresh = gui::Button::new(&wnd, gui::ButtonOpts { text: "&Atualizar", position: gui::dpi(334, 20), width: gui::dpi_x(110), height: gui::dpi_y(32), ..Default::default() });
        let service = gui::Label::new(&wnd, gui::LabelOpts { text: "Verificando o serviço…", position: gui::dpi(24, 64), size: gui::dpi(420, 20), ..Default::default() });
        let fingerprint = gui::Label::new(&wnd, gui::LabelOpts { text: "ID desta máquina: —", position: gui::dpi(24, 88), size: gui::dpi(420, 20), ..Default::default() });
        let addresses = gui::Label::new(&wnd, gui::LabelOpts { text: "Endereços: —", position: gui::dpi(24, 112), size: gui::dpi(420, 20), ..Default::default() });
        let users_hint = gui::Label::new(&wnd, gui::LabelOpts { text: "Usuários com acesso", position: gui::dpi(24, 144), size: gui::dpi(420, 20), ..Default::default() });
        let users = gui::ListView::new(
            &wnd,
            gui::ListViewOpts {
                position: gui::dpi(24, 166),
                size: gui::dpi(420, 112),
                control_style: co::LVS::REPORT | co::LVS::SHOWSELALWAYS | co::LVS::SINGLESEL | co::LVS::NOSORTHEADER,
                control_ex_style: co::LVS_EX::FULLROWSELECT | co::LVS_EX::DOUBLEBUFFER,
                columns: &[("Nome", 140), ("Nível", 110), ("ID de acesso", 160)],
                ..Default::default()
            },
        );
        let share = gui::Button::new(&wnd, gui::ButtonOpts { text: "&Compartilhar acesso", position: gui::dpi(24, 292), width: gui::dpi_x(170), height: gui::dpi_y(32), ..Default::default() });
        let invite = gui::Label::new(&wnd, gui::LabelOpts { text: "", position: gui::dpi(206, 290), size: gui::dpi(238, 40), ..Default::default() });

        theme::white_page(&wnd, vec![addresses.clone(), users_hint, invite.clone()]);
        let me = Self { wnd, title, service, fingerprint, addresses, users, share, invite, refresh };
        me.events();
        me
    }

    pub fn set_visible(&self, visible: bool) {
        theme::set_visible(&self.wnd, visible);
    }

    /// Fonte de título (chamada depois da fonte de texto, que vale pra tudo).
    pub fn apply_titles(&self) {
        theme::apply_title_font(&self.title);
    }

    fn events(&self) {
        let me = self.clone();
        self.wnd.on().wm_create(move |_| {
            me.reload();
            Ok(0)
        });
        let me = self.clone();
        self.refresh.on().bn_clicked(move || {
            me.reload();
            Ok(())
        });
        let me = self.clone();
        self.share.on().bn_clicked(move || {
            me.toggle_share();
            Ok(())
        });
    }

    fn set(&self, label: &gui::Label, text: &str) {
        let _ = label.hwnd().SetWindowText(text);
    }

    /// Relê tudo do serviço e atualiza a tela.
    fn reload(&self) {
        let state = service_ctrl::state();
        self.set(&self.service, &format!("Serviço: {}", state.label()));
        if state == ServiceState::NotInstalled {
            self.set(&self.fingerprint, "ID desta máquina: — (ative o acesso para gerar)");
            self.set(&self.addresses, "Endereços: —");
            self.set(&self.invite, "");
            let _ = self.users.items().delete_all();
            return;
        }
        match configclient::status() {
            Ok(s) => {
                self.set(&self.fingerprint, &format!("ID desta máquina: {}", s.fingerprint));
                let addrs = if s.addresses.is_empty() { format!("{}:{}", s.machine_name, s.port) } else { s.addresses.join("   ") };
                self.set(&self.addresses, &format!("Endereços: {addrs}"));
                self.fill_users();
                self.show_invite(s.invite);
                if !s.has_master {
                    self.set(&self.service, &format!("Serviço: {} — sem master (cadastre um administrador)", state.label()));
                }
            }
            Err(e) => {
                self.set(&self.fingerprint, &format!("Não foi possível ler o serviço: {e}"));
            }
        }
    }

    fn fill_users(&self) {
        let _ = self.users.items().delete_all();
        if let Ok(AdminResponse::Users(users)) = configclient::admin(AdminRequest::ListUsers) {
            for u in users {
                let level = match u.level {
                    Level::Master => "Master",
                    Level::FullControl => "Controle total",
                    Level::View => "Visualização",
                };
                let _ = self.users.items().add(&[u.name.as_str(), level, u.access_id.as_str()], None, ());
            }
        }
    }

    fn show_invite(&self, invite: Option<reflex_proto::msg::InviteInfo>) {
        match invite {
            Some(inv) => {
                let _ = self.share.hwnd().SetWindowText("Parar de &compartilhar");
                self.set(&self.invite, &format!("Código: {}\nnível {:?} — troca a cada 10 min", inv.code, inv.level));
            }
            None => {
                let _ = self.share.hwnd().SetWindowText("&Compartilhar acesso");
                self.set(&self.invite, "");
            }
        }
    }

    /// Liga/desliga o compartilhamento de acesso (convite de uso único).
    fn toggle_share(&self) {
        let currently_on = configclient::status().ok().and_then(|s| s.invite).is_some();
        let req = AdminRequest::SetSharing { enabled: !currently_on, level: Level::FullControl };
        match configclient::admin(req) {
            Ok(AdminResponse::Ok) => self.reload(),
            Ok(AdminResponse::Err(e)) => self.set(&self.invite, &e),
            Ok(_) => self.reload(),
            Err(e) => self.set(&self.invite, &e.to_string()),
        }
    }
}

