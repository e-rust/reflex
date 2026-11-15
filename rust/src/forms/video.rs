//! A janela de vídeo de uma conexão (uma por conexão; [`abrir`] cria). A
//! conexão ([`crate::sessao`]) entrega quadros e cursor e pede [`pintar`]; o
//! resto (entrada, cursor do Windows, tamanho) está em [`crate::video`].

use std::cell::RefCell;
use std::rc::Rc;

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
    }
    let estado = Rc::new(RefCell::new(Estado::new(a)));
    // Mouse, roda, travas e cursor: pelas mensagens da janela. Ocupado (um
    // handler mexendo no estado e o Windows mandando mensagem no meio): segue.
    let e = estado.clone();
    let gancho = on_message(form, move |m| e.try_borrow_mut().map_or(MessageReply::Default, |mut e| e.mensagem(m)));
    if let Some(f) = ui.form_instance_mut::<FrmVideo>(form) {
        f.data.estado = Some(estado);
        f.data.gancho = Some(gancho);
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

// Os handlers têm o nome do componente (`btnSalvar_click`).
#[allow(non_snake_case)]
impl FrmVideo {
    pub fn frmVideo_show(&mut self, _ui: &mut Ui, _sender: ComponentId, _args: &EventArgs) {
        self.janela_pronta();
    }

    pub fn frmVideo_close(&mut self, ui: &mut Ui, _sender: ComponentId, _args: &EventArgs) {
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
        if e.hwnd != 0 {
            // 1º quadro, ou a resolução remota mudou: ajusta a janela (o
            // `WM_SIZE` que isso gera passa pelo gancho com o estado ocupado).
            if let Some(q) = e.quadro.filter(|q| e.ajustada_para != Some(*q)) {
                e.ajustada_para = Some(q);
                crate::video::janela::fit_to_remote(e.hwnd, q, e.opcoes.effective_scale());
            }
            e.conferir_cursor();
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
}
