//! A janela de vídeo de uma conexão (uma por conexão; [`abrir`] cria). A
//! conexão ([`crate::sessao`]) entrega quadros e cursor e pede [`pintar`]; o
//! resto (entrada, cursor do Windows, tamanho) está em [`crate::video`].

use std::cell::RefCell;
use std::rc::Rc;

#[cfg(windows)]
use aylis::platform::win::{MessageReply, on_message, remove_message_hook};
use aylis::prelude::{ComponentId, EventArgs, FrameView, ImageView, RectF, Ui};

use crate::generated::{FrmVideo, FrmVideoComponents};
use crate::sessao::{Abertura, Janela};
use crate::video::geometria::{cursor_rect, image_rect, premultiply, Rect};
use crate::video::{Estado, SHOW_ANYWAY};

/// O estado do form: cada janela aberta dele tem o seu (`self.data`).
#[derive(Default)]
pub struct Data {
    estado: Option<Rc<RefCell<Estado>>>,
    /// O gancho das mensagens do Windows desta janela.
    #[cfg(windows)]
    gancho: Option<u64>,
}

/// Como a conexão abre, pinta e titula a janela.
pub const JANELA: Janela = Janela { abrir, pintar, titular };

/// O estado da janela `form` (fora dos handlers dela).
fn estado_de(ui: &Ui, form: ComponentId) -> Option<Rc<RefCell<Estado>>> {
    ui.form_instance::<FrmVideo>(form)?.data.estado.clone()
}

/// Cria a janela de uma conexão. Ela aparece com o 1º quadro (já no tamanho
/// da tela remota) ou, sem imagem, depois de [`SHOW_ANYWAY`].
pub fn abrir(ui: &mut Ui, a: Abertura) {
    let saida = a.saida.clone();
    let form = match ui.create_form_instance("frmVideo") {
        Ok(f) => f,
        Err(e) => {
            log::error!("janela de vídeo: {e}");
            saida.fechar();
            return;
        }
    };
    if let Some(c) = FrmVideo::components_of(ui, form) {
        c.form.set_caption(ui, a.titulo.as_str());
        c.tmrVideo.set_enabled(ui, true);
        // Linux: Super, Alt+Tab e cia. vão pra lá com a janela em primeiro
        // plano. No Windows quem prende é o gancho do `crate::video::teclado`.
        #[cfg(not(windows))]
        c.kcTeclado.set_active(ui, true);
    }
    let estado = Rc::new(RefCell::new(Estado::new(a)));
    // Mouse, roda, travas e cursor: pelas mensagens da janela. Ocupado (um
    // handler mexendo no estado e o Windows mandando mensagem no meio): segue.
    #[cfg(windows)]
    let gancho = {
        let e = estado.clone();
        on_message(form, move |m| e.try_borrow_mut().map_or(MessageReply::Default, |mut e| e.mensagem(m)))
    };
    if let Some(f) = ui.form_instance_mut::<FrmVideo>(form) {
        f.data.estado = Some(estado);
        #[cfg(windows)]
        {
            f.data.gancho = Some(gancho);
        }
    }
    saida.ligar(form);
}

/// Pinta o que a conexão entregou: o quadro novo (se houver) e o cursor.
pub fn pintar(ui: &mut Ui, form: ComponentId) {
    let (Some(estado), Some(c)) = (estado_de(ui, form), FrmVideo::components_of(ui, form)) else { return };
    let mostrar = {
        let mut e = estado.borrow_mut();
        e.saida.pintado();
        let quadro = e.saida.quadro.lock().ok().and_then(|mut q| q.take());
        let mut mostrar = false;
        if let Some(f) = quadro {
            e.quadro = Some((f.width, f.height));
            e.ultima = Some(aylis::core::Image::bgra(f.width, f.height, f.data));
            // 1º quadro: a janela aparece (o timer a ajusta à tela remota).
            mostrar = !std::mem::replace(&mut e.mostrada, true);
        }
        mostrar
    };
    // Fora do estado: mostrar já roda o `on_show`, que mexe nele.
    if mostrar {
        let _ = ui.show(form);
    }
    desenhar(ui, &c, &mut estado.borrow_mut());
}

pub fn titular(ui: &mut Ui, form: ComponentId, titulo: &str) {
    if let Some(c) = FrmVideo::components_of(ui, form) {
        c.form.set_caption(ui, titulo);
    }
}

/// Põe o quadro onde ele vai (proporção mantida, faixas pretas em volta) e o
/// cursor desenhado por cima.
fn desenhar(ui: &mut Ui, c: &FrmVideoComponents, e: &mut Estado) {
    let k = e.escala();
    let tela = ui.bounds(c.fvTela.id).map_or((0.0, 0.0), |r| (r.width, r.height));
    e.layout_para = tela;
    if let (Some((w, h)), Some(img)) = (e.quadro, e.ultima.clone()) {
        e.imagem = image_rect(e.opcoes.effective_scale(), w, h, (tela.0 * k).round() as i32, (tela.1 * k).round() as i32);
        if let Some(fv) = ui.get_mut::<FrameView>(c.fvTela.id) {
            fv.set_frame(Some((img, logico(e.imagem, k))));
        }
    }
    desenhar_cursor(ui, c, e, k);
    ui.invalidate();
}

/// O cursor remoto desenhado sobre a imagem (a `Image` por cima do quadro).
fn desenhar_cursor(ui: &mut Ui, c: &FrmVideoComponents, e: &mut Estado, k: f32) {
    let alvo = {
        let Ok(cur) = e.saida.cursor.lock() else { return };
        match (cur.shape.as_ref(), e.quadro) {
            (Some(s), Some(quadro)) if e.desenha_cursor() && cur.drawable() => {
                let picture = (e.cursor_img != Some(s.id)).then(|| aylis::core::Image::bgra(s.width, s.height, premultiply(&s.bgra[..(s.width * s.height * 4) as usize])));
                Some((s.id, picture, cursor_rect(e.imagem, quadro, (cur.x, cur.y), (s.width, s.height))))
            }
            _ => None,
        }
    };
    let Some((id, picture, r)) = alvo else {
        c.imgCursor.set_visible(ui, false);
        return;
    };
    if let Some(p) = picture {
        if let Some(img) = ui.get_mut::<ImageView>(c.imgCursor.id) {
            img.set_picture(Some(p));
        }
        e.cursor_img = Some(id);
    }
    let r = logico(r, k);
    c.imgCursor.set_left(ui, r.x as f64);
    c.imgCursor.set_top(ui, r.y as f64);
    c.imgCursor.set_width(ui, r.width as f64);
    c.imgCursor.set_height(ui, r.height as f64);
    c.imgCursor.set_visible(ui, true);
}

/// Pixels reais da janela → pontos do Aylis.
fn logico(r: Rect, k: f32) -> RectF {
    RectF::new(r.x as f32 / k, r.y as f32 / k, r.w as f32 / k, r.h as f32 / k)
}

/// A área útil do monitor da janela, em pixels. Sem saber o monitor dela (o
/// Wayland não diz onde a janela está), a do principal.
#[cfg(not(windows))]
fn area_util(ui: &Ui, form: ComponentId) -> Option<(u32, u32)> {
    let ms = ui.monitors();
    let m = ui.form_monitor(form).and_then(|i| ms.get(i)).or_else(|| ms.iter().find(|m| m.primary)).or(ms.first());
    if m.is_none() {
        log::debug!("janela de vídeo: o sistema não disse os monitores");
    }
    m.map(|m| (m.work_width, m.work_height))
}

/// Quanto a barra de título ocupa (pontos): a janela ajustada cabe na área útil com ela.
#[cfg(not(windows))]
const TITULO: f32 = 48.0;

/// Linux: a área da imagem do tamanho da tela remota (como no Windows), sem
/// passar da área útil do monitor; maximizada, fica como está.
#[cfg(not(windows))]
fn ajustar_a_remota(ui: &mut Ui, form: crate::generated::Form, remota: (u32, u32), e: &Estado) {
    use crate::video::geometria::{scaled_to_dpi, window_rect_for};
    use crate::viewer::hosts::ViewScale;
    let Some((aw, ah)) = area_util(ui, form.id).filter(|_| !ui.is_maximized(form.id)) else { return };
    let k = e.escala();
    let conteudo = match e.opcoes.effective_scale() {
        ViewScale::Windows => scaled_to_dpi(remota, (k * 96.0).round() as u32),
        ViewScale::Native => remota,
    };
    let titulo = (TITULO * k).round() as i32;
    let r = window_rect_for(conteudo, Rect { x: 0, y: 0, w: aw as i32, h: ah as i32 }, (0, titulo));
    form.set_client_width(ui, (r.w as f32 / k) as f64);
    form.set_client_height(ui, ((r.h - titulo) as f32 / k) as f64);
    log::info!("janela ajustada à tela remota {}x{}: área de dentro {}x{} pixels (escala {k})", remota.0, remota.1, r.w, r.h - titulo);
}

/// Linux, resolução "igual à janela": abre grande (4/5 da área útil), e a
/// tela remota acompanha.
#[cfg(not(windows))]
fn abrir_grande(ui: &mut Ui, form: crate::generated::Form, k: f32) {
    if let Some((aw, ah)) = area_util(ui, form.id) {
        form.set_client_width(ui, (aw as f32 * 0.8 / k) as f64);
        form.set_client_height(ui, (ah as f32 * 0.8 / k - TITULO) as f64);
    }
}

/// Linux, "só o remoto": o cursor daqui em cima da imagem é o de lá (a forma
/// dele), some quando lá some e é o normal enquanto a forma não chegou.
#[cfg(not(windows))]
fn por_cursor(ui: &mut Ui, tela: crate::generated::FrameView, pick: crate::video::geometria::CursorPick, imagem: Option<crate::video::CursorImagem>) {
    use crate::video::geometria::CursorPick;
    use aylis::prelude::CursorShape;
    ui.clear_cursor_image(tela.id);
    match (pick, imagem) {
        (CursorPick::Remote(_), Some((img, x, y))) => match ui.set_cursor_image(tela.id, &img, x, y) {
            Ok(()) => tela.set_cursor(ui, CursorShape::Default),
            Err(err) => {
                log::debug!("cursor remoto como cursor daqui: {err}");
                tela.set_cursor(ui, CursorShape::Default);
            }
        },
        (CursorPick::Hidden, _) => tela.set_cursor(ui, CursorShape::None),
        _ => tela.set_cursor(ui, CursorShape::Default),
    }
}

// Os handlers têm o nome do componente (`btnSalvar_click`).
#[allow(non_snake_case)]
impl FrmVideo {
    pub fn frmVideo_show(&mut self, _ui: &mut Ui, _sender: ComponentId, _args: &EventArgs) {
        self.janela_pronta();
    }

    pub fn frmVideo_close(&mut self, ui: &mut Ui, _sender: ComponentId, _args: &EventArgs) {
        #[cfg(windows)]
        if let Some(g) = self.data.gancho.take() {
            remove_message_hook(self.form.id, g);
        }
        if let Some(e) = self.data.estado.take() {
            e.borrow_mut().fechar();
        }
        crate::generated::principal::video_fechou(ui);
    }

    /// A cada 50 ms: a janela aparece e se ajusta à tela remota, acompanha o
    /// tamanho dela e a forma do cursor.
    pub fn tmrVideo_timer(&mut self, ui: &mut Ui, _sender: ComponentId, _args: &EventArgs) {
        let Some(estado) = self.data.estado.clone() else { return };
        let form = self.form.id;
        let mostrar = {
            let mut e = estado.borrow_mut();
            !e.mostrada && e.criada.elapsed() >= SHOW_ANYWAY && !std::mem::replace(&mut e.mostrada, true)
        };
        if mostrar {
            let _ = ui.show(form);
        }
        self.janela_pronta();
        let mut e = estado.borrow_mut();
        #[cfg(windows)]
        if e.hwnd != 0 {
            // 1º quadro, ou a resolução remota mudou: ajusta a janela (o
            // `WM_SIZE` que isso gera passa pelo gancho com o estado ocupado).
            if let Some(q) = e.quadro.filter(|q| e.ajustada_para != Some(*q)) {
                e.ajustada_para = Some(q);
                crate::video::janela::fit_to_remote(e.hwnd, q, e.opcoes.effective_scale());
            }
            e.conferir_cursor();
        }
        #[cfg(not(windows))]
        {
            if e.mostrada
                && let Some(q) = e.quadro.filter(|q| e.ajustada_para != Some(*q))
            {
                e.ajustada_para = Some(q);
                ajustar_a_remota(ui, self.form, q, &e);
            }
            if e.opcoes.cursor.hides_local()
                && let Some((pick, imagem)) = e.cursor_trocado()
            {
                por_cursor(ui, self.fvTela, pick, imagem);
            }
        }
        // A janela mudou de tamanho, ou o cursor desenhado pode ter de sumir/aparecer.
        let tela = ui.bounds(self.fvTela.id).map_or((0.0, 0.0), |r| (r.width, r.height));
        let mudou = tela != e.layout_para;
        if mudou || e.opcoes.cursor.draws_remote() {
            let c = FrmVideo::components_of(ui, form);
            if let Some(c) = c {
                if mudou {
                    desenhar(ui, &c, &mut e);
                } else {
                    let k = e.escala();
                    desenhar_cursor(ui, &c, &mut e, k);
                }
            }
        }
    }

    /// Pega a janela do Windows assim que o Aylis a cria.
    #[cfg(not(windows))]
    fn janela_pronta(&mut self) {}

    /// Pega a janela do Windows assim que o Aylis a cria.
    #[cfg(windows)]
    fn janela_pronta(&mut self) {
        let Some(estado) = self.data.estado.as_ref() else { return };
        let mut e = estado.borrow_mut();
        if e.mostrada
            && e.hwnd == 0
            && let Some(hwnd) = aylis::platform::win::hwnd(self.form.id)
        {
            e.janela_pronta(hwnd);
        }
    }

    // A entrada pelos eventos do Aylis (Linux). No Windows ela vem pelas
    // mensagens da janela (o gancho as consome antes do Aylis): estes não chegam.

    pub fn frmVideo_deactivate(&mut self, _ui: &mut Ui, _sender: crate::generated::Form) {
        #[cfg(not(windows))]
        self.com_estado(|e| e.perdeu_foco());
    }

    /// `on_key_down` (com o `key_preview` do form: todas as teclas da janela).
    pub fn frmVideo_key_down(&mut self, _ui: &mut Ui, _sender: crate::generated::Form, value: aylis::prelude::KeyEvent) -> Result<(), String> {
        #[cfg(not(windows))]
        self.com_estado(|e| e.tecla(value.scancode, true, value.modifiers));
        #[cfg(windows)]
        let _ = value;
        Ok(())
    }

    pub fn frmVideo_key_up(&mut self, _ui: &mut Ui, _sender: crate::generated::Form, value: aylis::prelude::KeyEvent) {
        #[cfg(not(windows))]
        self.com_estado(|e| e.tecla(value.scancode, false, value.modifiers));
        #[cfg(windows)]
        let _ = value;
    }

    pub fn fvTela_pointer_down(&mut self, _ui: &mut Ui, _sender: crate::generated::FrameView, value: aylis::prelude::PointerEvent) {
        #[cfg(not(windows))]
        self.com_estado(|e| e.ponteiro((value.position.x, value.position.y), Some((value.button, true))));
        #[cfg(windows)]
        let _ = value;
    }

    pub fn fvTela_pointer_up(&mut self, _ui: &mut Ui, _sender: crate::generated::FrameView, value: aylis::prelude::PointerEvent) {
        #[cfg(not(windows))]
        self.com_estado(|e| e.ponteiro((value.position.x, value.position.y), Some((value.button, false))));
        #[cfg(windows)]
        let _ = value;
    }

    pub fn fvTela_pointer_move(&mut self, _ui: &mut Ui, _sender: crate::generated::FrameView, value: aylis::prelude::PointerEvent) {
        #[cfg(not(windows))]
        self.com_estado(|e| e.ponteiro((value.position.x, value.position.y), None));
        #[cfg(windows)]
        let _ = value;
    }

    pub fn fvTela_wheel(&mut self, _ui: &mut Ui, _sender: crate::generated::FrameView, value: aylis::prelude::WheelEvent) {
        #[cfg(not(windows))]
        self.com_estado(|e| e.roda((value.delta.x, value.delta.y)));
        #[cfg(windows)]
        let _ = value;
    }

    /// Roda `f` no estado da janela (se ela ainda tem conexão).
    #[cfg(not(windows))]
    fn com_estado(&self, f: impl FnOnce(&mut Estado)) {
        if let Some(e) = self.data.estado.as_ref() {
            f(&mut e.borrow_mut());
        }
    }

    /// Linux: a janela passou a existir: a escala dela, as travas iguais às
    /// daqui, o tamanho pedido (resolução "igual à janela") e para a frente.
    pub fn frmVideo_window_created(&mut self, ui: &mut Ui, sender: crate::generated::Form) {
        #[cfg(not(windows))]
        {
            let Some(estado) = self.data.estado.clone() else { return };
            let grande = {
                let mut e = estado.borrow_mut();
                e.escala_janela = ui.form_scale(sender.id);
                e.travas(ui.lock_keys());
                log::info!("janela de vídeo pronta (escala {})", e.escala_janela);
                e.pedir_tamanho.is_some().then_some(e.escala_janela)
            };
            if let Some(k) = grande {
                abrir_grande(ui, sender, k);
            }
            if let Err(err) = sender.bring_to_front(ui) {
                log::debug!("janela de vídeo para a frente: {err}");
            }
        }
        #[cfg(windows)]
        let _ = (ui, sender);
    }

    /// Linux: entrou na janela: as travas de lá ficam iguais às daqui (no
    /// Wayland sem XWayland o Aylis não as sabe e diz todas desligadas).
    pub fn frmVideo_activate(&mut self, ui: &mut Ui, _sender: crate::generated::Form) {
        #[cfg(not(windows))]
        {
            let travas = ui.lock_keys();
            self.com_estado(|e| e.travas(travas));
        }
        #[cfg(windows)]
        let _ = ui;
    }

    /// Linux: a janela foi para um monitor de outra escala: a imagem e o mouse
    /// passam a contar com ela, e a tela remota acompanha.
    pub fn frmVideo_scale_change(&mut self, ui: &mut Ui, _sender: crate::generated::Form, value: f64) {
        #[cfg(not(windows))]
        {
            let tela = ui.bounds(self.fvTela.id).map_or((0.0, 0.0), |r| (r.width, r.height));
            self.com_estado(|e| {
                e.escala_janela = value as f32;
                // O timer refaz a conta da imagem.
                e.layout_para = (0.0, 0.0);
                e.pedir_tamanho(tela);
            });
        }
        #[cfg(windows)]
        let _ = (ui, value);
    }

    /// Linux: o usuário terminou de mudar o tamanho (ou maximizou): a tela
    /// remota acompanha. No Windows quem avisa é o `WM_EXITSIZEMOVE`.
    pub fn frmVideo_resize_end(&mut self, ui: &mut Ui, _sender: crate::generated::Form) {
        #[cfg(not(windows))]
        {
            let tela = ui.bounds(self.fvTela.id).map_or((0.0, 0.0), |r| (r.width, r.height));
            self.com_estado(|e| e.pedir_tamanho(tela));
        }
        #[cfg(windows)]
        let _ = ui;
    }

    pub fn fvTela_pointer_enter(&mut self, _ui: &mut Ui, _sender: crate::generated::FrameView) {
        #[cfg(not(windows))]
        self.com_estado(|e| e.dentro = true);
    }

    pub fn fvTela_pointer_leave(&mut self, _ui: &mut Ui, _sender: crate::generated::FrameView) {
        #[cfg(not(windows))]
        self.com_estado(|e| e.dentro = false);
    }
}
