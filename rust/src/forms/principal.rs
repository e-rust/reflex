//! A janela principal (§5.2): a aba "Conectar" (endereço, Meu ID, opções de
//! imagem) e a aba "Esta máquina" (o serviço daqui, usuários, convite).
//!
//! Os handlers só leem e escrevem a tela: as regras ficam em [`crate::escolha`],
//! [`crate::maquina`] e no viewer; o que bloqueia (pipe do serviço, a conexão)
//! roda fora da thread da tela e volta pelo `post`.

use std::sync::{Arc, OnceLock};

use aylis::prelude::*;
use crate::proto::msg::{Chroma, Refine, RemoteSize};
use crate::viewer::hosts::{ClipboardMode, CursorMode, ViewScale};

use crate::escolha;
use crate::generated::{ComboBox, FrmPrincipal, FrmPrincipalComponents};
use crate::maquina;
use crate::sessao::{self, Report, ViewOptions};

/// O estado do form: cada janela aberta dele tem o seu (`self.data`).
#[derive(Default)]
pub struct Data {}

/// `reflex --connect <endereço>`: a janela abre já conectando, e fechar o
/// vídeo fecha o app.
static DIRETO: OnceLock<String> = OnceLock::new();

/// Liga o modo `--connect` (antes de subir a interface).
pub fn conectar_direto(endereco: &str) {
    let _ = DIRETO.set(endereco.to_string());
}

/// Uma janela de vídeo fechou: no modo `--connect`, o app termina junto.
pub fn video_fechou(ui: &mut Ui) {
    if DIRETO.get().is_some()
        && let Some(c) = FrmPrincipal::components(ui)
    {
        ui.close(c.form.id);
    }
}

/// Uma lista de opções: os itens (de `all`) e o valor atual escolhido.
fn preencher<T: Copy + PartialEq>(ui: &mut Ui, combo: ComboBox, all: &[T], label: fn(T) -> &'static str, atual: T) {
    combo.set_items(ui, &escolha::items(all, label));
    combo.set_item_index(ui, escolha::index_of(all, &atual));
}

/// O ID de acesso deste viewer (o que se cadastra na outra máquina).
fn meu_id() -> String {
    crate::identity::viewer_cert()
        .map(|c| crate::auth::identity::AccessId::of_cert(&c.cert_der).to_string())
        .unwrap_or_else(|e| format!("indisponível: {e}"))
}

/// Mostra o resumo da aba "Esta máquina".
fn mostrar_maquina(ui: &mut Ui, c: &FrmPrincipalComponents, r: &maquina::Resumo) {
    c.lblServico.set_text(ui, r.servico.as_str());
    c.lblIdMaquina.set_text(ui, r.id.as_str());
    c.lblEnderecos.set_text(ui, r.enderecos.as_str());
    c.lvUsuarios.set_items(ui, &r.usuarios);
    c.lblConvite.set_text(ui, r.convite.as_str());
    c.btnCompartilhar.set_text(ui, r.botao_compartilhar);
    c.btnCompartilhar.set_enabled(ui, r.pode_compartilhar);
    c.btnAtualizar.set_enabled(ui, true);
}

// Os handlers têm o nome do componente (`btnSalvar_click`).
#[allow(non_snake_case)]
impl FrmPrincipal {
    pub fn frmPrincipal_show(&mut self, ui: &mut Ui, _sender: ComponentId, _args: &EventArgs) {
        self.edtEndereco.set_text(ui, DIRETO.get().cloned().unwrap_or_else(sessao::last_address));
        self.edtMeuId.set_text(ui, meu_id());
        let o = ViewOptions::saved();
        preencher(ui, self.cmbNitidez, &Refine::ALL, Refine::label, o.refine);
        preencher(ui, self.cmbEscala, &ViewScale::ALL, ViewScale::label, o.scale);
        preencher(ui, self.cmbResolucao, &RemoteSize::ALL, RemoteSize::label, o.remote_size);
        preencher(ui, self.cmbCor, &Chroma::ALL, Chroma::label, o.chroma);
        preencher(ui, self.cmbCursor, &CursorMode::ALL, CursorMode::label, o.cursor);
        preencher(ui, self.cmbClipboard, &ClipboardMode::ALL, ClipboardMode::label, o.clipboard);
        recarregar_maquina(ui, self.form.id);
        if DIRETO.get().is_some() {
            self.btnConectar_click(ui, self.btnConectar.id, &EventArgs::None);
        }
    }

    /// Enter no endereço também chega aqui (é o botão padrão).
    pub fn btnConectar_click(&mut self, ui: &mut Ui, _sender: ComponentId, _args: &EventArgs) {
        let Some(endereco) = crate::addr::normalize_address(&self.edtEndereco.text(ui)) else {
            self.lblStatus.set_text(ui, "Informe o endereço da máquina.");
            return;
        };
        self.lblStatus.set_text(ui, format!("Conectando a {endereco}…"));
        let opcoes = self.opcoes(ui);
        opcoes.save();
        // A linha de status mostra o que a conexão conta (de outra thread, pelo post).
        let (poster, form, status) = (ui.poster(), self.form.id, self.lblStatus);
        let report: Report = Arc::new(move |texto: &str| {
            let texto = texto.to_string();
            poster.post_to(form, move |ui| status.set_text(ui, texto));
        });
        let poster = ui.poster();
        std::thread::spawn(move || sessao::rodar(&endereco, opcoes, report, poster, crate::generated::video::JANELA));
    }

    pub fn btnCopiarId_click(&mut self, ui: &mut Ui, _sender: ComponentId, _args: &EventArgs) {
        let id = self.edtMeuId.text(ui);
        clipboard::set_text(ui, &id);
        self.lblStatus.set_text(ui, "Meu ID copiado.");
    }

    pub fn btnAtualizar_click(&mut self, ui: &mut Ui, _sender: ComponentId, _args: &EventArgs) {
        recarregar_maquina(ui, self.form.id);
    }

    pub fn btnCompartilhar_click(&mut self, ui: &mut Ui, _sender: ComponentId, _args: &EventArgs) {
        self.btnCompartilhar.set_enabled(ui, false);
        let form = self.form.id;
        ui.background(form, maquina::alternar_compartilhar, move |ui, feito| {
            if let Err(e) = feito {
                message_dlg(ui, &format!("Não foi possível mudar o compartilhamento.\n\n{e}"), DlgKind::Error, &[ModalResult::Ok], |_, _| {});
            }
            recarregar_maquina(ui, form);
        });
    }

    /// As opções de imagem escolhidas na tela.
    fn opcoes(&self, ui: &Ui) -> ViewOptions {
        ViewOptions {
            refine: escolha::at(&Refine::ALL, self.cmbNitidez.item_index(ui)),
            scale: escolha::at(&ViewScale::ALL, self.cmbEscala.item_index(ui)),
            remote_size: escolha::at(&RemoteSize::ALL, self.cmbResolucao.item_index(ui)),
            chroma: escolha::at(&Chroma::ALL, self.cmbCor.item_index(ui)),
            cursor: escolha::at(&CursorMode::ALL, self.cmbCursor.item_index(ui)),
            clipboard: escolha::at(&ClipboardMode::ALL, self.cmbClipboard.item_index(ui)),
        }
    }
}

/// Relê o serviço (fora da tela: o pipe pode demorar) e mostra na aba "Esta máquina"
/// do form `form`.
fn recarregar_maquina(ui: &mut Ui, form: ComponentId) {
    if let Some(c) = FrmPrincipal::components_of(ui, form) {
        c.btnAtualizar.set_enabled(ui, false);
    }
    ui.background(form, maquina::ler, move |ui, leitura| {
        if let Some(c) = FrmPrincipal::components_of(ui, form) {
            mostrar_maquina(ui, &c, &maquina::resumo(&leitura));
        }
    });
}
