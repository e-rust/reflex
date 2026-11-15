//! O que a IDE do Aylis refaz a cada "Gerar código" (não edite): o acesso tipado aos
//! componentes, a struct de cada form e o registro. O código dos forms é seu, em
//! `src/forms/`.

use aylis::prelude::Ui;

pub mod components;
pub use components::*;

#[path = "../src/forms/principal.rs"]
pub mod principal;

#[path = "../src/forms/video.rs"]
pub mod video;

/// Registra as classes dos forms (cada form aberto ganha a instância dele), os handlers e
/// as classes dos pacotes de código.
pub fn register(ui: &mut Ui) {
    ui.register_form_class("frmPrincipal", FrmPrincipal::bind);
    ui.register_handler("frmPrincipal.frmPrincipal_show", aylis::prelude::instances::form_handler(FrmPrincipal::frmPrincipal_show));
    ui.register_handler("frmPrincipal.btnConectar_click", aylis::prelude::instances::form_handler(FrmPrincipal::btnConectar_click));
    ui.register_handler("frmPrincipal.btnCopiarId_click", aylis::prelude::instances::form_handler(FrmPrincipal::btnCopiarId_click));
    ui.register_handler("frmPrincipal.btnAtualizar_click", aylis::prelude::instances::form_handler(FrmPrincipal::btnAtualizar_click));
    ui.register_handler("frmPrincipal.btnCompartilhar_click", aylis::prelude::instances::form_handler(FrmPrincipal::btnCompartilhar_click));
    ui.register_form_class("frmVideo", FrmVideo::bind);
    ui.register_handler("frmVideo.frmVideo_show", aylis::prelude::instances::form_handler(FrmVideo::frmVideo_show));
    ui.register_handler("frmVideo.frmVideo_close", aylis::prelude::instances::form_handler(FrmVideo::frmVideo_close));
    ui.register_handler("frmVideo.tmrVideo_timer", aylis::prelude::instances::form_handler(FrmVideo::tmrVideo_timer));
}
