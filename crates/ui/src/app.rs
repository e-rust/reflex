//! Janela principal (§5.2): o seletor de seções no estilo do Windows 11 e as
//! páginas "Conectar" e "Esta máquina", com os retoques de [`crate::theme`].

use winsafe::co;
use winsafe::gui;
use winsafe::prelude::*;

use crate::connect::ConnectPage;
use crate::machine::MachinePage;
use crate::nav::SelectorBar;
use crate::theme;

#[derive(Clone)]
pub struct MainWindow {
    wnd: gui::WindowMain,
}

impl MainWindow {
    pub fn new() -> winsafe::AnyResult<Self> {
        let wnd = gui::WindowMain::new(gui::WindowMainOpts {
            title: "Reflex",
            size: gui::dpi(480, 532),
            class_bg_brush: gui::Brush::Color(co::COLOR::WINDOW),
            ..Default::default()
        });

        let nav = SelectorBar::new(&wnd, gui::dpi(20, 6), gui::dpi(440, 40), vec!["Conectar", "Esta máquina"]);
        let connect = ConnectPage::new(&wnd, gui::dpi(0, 48), gui::dpi(480, 484));
        let machine = MachinePage::new(&wnd, gui::dpi(0, 48), gui::dpi(480, 484));

        let (c, m) = (connect.clone(), machine.clone());
        nav.on_select(move |i| {
            c.set_visible(i == 0);
            m.set_visible(i == 1);
        });

        // Depois que tudo foi criado: moldura, fonte de texto em todos os
        // controles (o EnumChildWindows desce até os netos), títulos e a 1ª página.
        let wnd2 = wnd.clone();
        wnd.on().wm_create(move |_| {
            theme::decorate_window(wnd2.hwnd());
            theme::apply_text_font(wnd2.hwnd());
            connect.apply_titles();
            machine.apply_titles();
            machine.set_visible(false);
            Ok(0)
        });

        Ok(Self { wnd })
    }

    pub fn run(&self) -> winsafe::AnyResult<i32> {
        self.wnd.run_main(None)
    }
}
