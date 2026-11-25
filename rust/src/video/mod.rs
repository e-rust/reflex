//! A janela de vídeo (§5.1). O form `frmVideo` mostra o quadro (`FrameView`)
//! e o cursor remoto (uma `Image` por cima); a entrada — mouse, roda, travas,
//! foco — e o cursor do Windows passam pelas mensagens da janela
//! ([`Estado::mensagem`]), e o teclado inteiro pelo gancho de baixo nível
//! ([`teclado`]).
//!
//! - [`geometria`]: a conta (onde a imagem fica, o mouse, o tamanho), pura;
//! - `janela` (Windows): o que se pede direto ao Windows pelo `HWND`;
//! - `teclado` (Windows): o gancho que manda Win, Alt+Tab e cia. pra lá.
//!
//! No Linux a entrada vem pelos eventos do Aylis no `FrameView` (mouse e roda)
//! e no form (as teclas, pela tecla física do evdev, que o [`crate::keymap`]
//! traduz pro scancode do PC; o `KeyboardCapture` do form traz Super, Alt+Tab
//! e cia.); o cursor remoto vira o cursor daqui pelo `set_cursor_image`.

pub mod geometria;
#[cfg(windows)]
pub mod janela;
#[cfg(windows)]
pub mod teclado;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::proto::msg::{DesiredDisplay, Input};
use tokio::sync::mpsc::UnboundedSender;

use crate::sessao::{Abertura, OnResize, Saida, ViewOptions};
use geometria::{CursorPick, Rect};

use crate::proto::msg::MouseButtons;
use geometria::norm;

#[cfg(windows)]
use aylis::platform::win::{MessageReply, WindowMessage};
use geometria::{cursor_pick, desired_for};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    HTCLIENT, SIZE_MAXIMIZED, SIZE_RESTORED, WM_EXITSIZEMOVE, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SETFOCUS, WM_SIZE, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// A forma do cursor remoto como cursor daqui: a imagem e o ponto de clique.
#[cfg(not(windows))]
pub type CursorImagem = (aylis::core::Image, u32, u32);

/// Sem imagem até aqui, a janela aparece mesmo assim (preta).
pub const SHOW_ANYWAY: Duration = Duration::from_secs(15);

/// O estado de uma janela de vídeo (uma por conexão), na thread da tela.
pub struct Estado {
    pub saida: Arc<Saida>,
    input: UnboundedSender<Input>,
    pub opcoes: ViewOptions,
    /// Resolução "igual à janela": pra onde mandar o tamanho, e o último mandado.
    pub pedir_tamanho: Option<OnResize>,
    ultimo_pedido: Option<DesiredDisplay>,
    /// A janela do Windows (0 até o Aylis criá-la; sempre 0 no Linux).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub hwnd: isize,
    /// Onde a imagem está desenhada, em pixels reais da janela: o mouse é
    /// mapeado pra este retângulo.
    pub imagem: Rect,
    /// O tamanho do último quadro (a resolução remota).
    pub quadro: Option<(u32, u32)>,
    /// A imagem mostrada (pra redesenhar quando a janela muda de tamanho).
    pub ultima: Option<aylis::core::Image>,
    /// Resolução remota pra qual a janela já foi ajustada.
    pub ajustada_para: Option<(u32, u32)>,
    /// O tamanho do `FrameView` (pontos) na última conta da imagem.
    pub layout_para: (f32, f32),
    botoes: u8,
    /// Linux: pedaço de dente da roda ainda não mandado (touchpad), por eixo.
    #[cfg(not(windows))]
    roda: (f32, f32),
    /// Linux: a escala da janela (o Aylis diz ao criar e a cada mudança).
    #[cfg(not(windows))]
    pub escala_janela: f32,
    /// Linux: o mouse está em cima da imagem (no Wayland, só os eventos dizem).
    #[cfg(not(windows))]
    pub dentro: bool,
    #[cfg(windows)]
    maximizada: bool,
    pub criada: Instant,
    /// Já pediu pro Aylis mostrar a janela (ela nasce escondida e aparece com
    /// o 1º quadro, ou depois de [`SHOW_ANYWAY`]).
    pub mostrada: bool,
    #[cfg(windows)]
    os_cursor: Option<janela::OsCursor>,
    /// O cursor daqui que está posto por último (troca de forma parada no lugar).
    cursor_mostrado: Option<CursorPick>,
    /// A forma que está na `Image` do cursor desenhado.
    pub cursor_img: Option<u32>,
    #[cfg(windows)]
    teclado: Option<teclado::KeyboardHook>,
}

impl Estado {
    pub fn new(a: Abertura) -> Self {
        Self {
            saida: a.saida,
            input: a.input,
            opcoes: a.opcoes,
            pedir_tamanho: a.pedir_tamanho,
            ultimo_pedido: None,
            hwnd: 0,
            imagem: Rect::default(),
            quadro: None,
            ultima: None,
            ajustada_para: None,
            layout_para: (0.0, 0.0),
            botoes: 0,
            #[cfg(not(windows))]
            roda: (0.0, 0.0),
            #[cfg(not(windows))]
            escala_janela: 1.0,
            #[cfg(not(windows))]
            dentro: false,
            #[cfg(windows)]
            maximizada: false,
            criada: Instant::now(),
            mostrada: false,
            #[cfg(windows)]
            os_cursor: None,
            cursor_mostrado: None,
            cursor_img: None,
            #[cfg(windows)]
            teclado: None,
        }
    }

    /// A janela fechou: solta as teclas apertadas lá.
    #[cfg(not(windows))]
    pub fn fechar(&mut self) {
        let _ = self.input.send(Input::ReleaseAll);
        self.saida.fechar();
    }

    /// Manda `client` (pixels da área da imagem, no `dpi` dela) como o tamanho
    /// da tela remota, se mudou (resolução "igual à janela").
    fn pedir(&mut self, client: (i32, i32), dpi: u32) {
        let Some(send) = self.pedir_tamanho.as_ref() else { return };
        if let Some(want) = desired_for(client, dpi).filter(|w| self.ultimo_pedido != Some(*w)) {
            self.ultimo_pedido = Some(want);
            send(want);
        }
    }

    /// A forma do cursor remoto mudou desde a última vez que o cursor daqui
    /// foi posto ("só o remoto"): o que pôr agora e, se for a de lá, a imagem
    /// dela e o ponto de clique. (No Windows quem põe é o `WM_SETCURSOR`.)
    #[cfg(not(windows))]
    pub fn cursor_trocado(&mut self) -> Option<(CursorPick, Option<CursorImagem>)> {
        let c = self.saida.cursor.lock().ok()?;
        let pick = cursor_pick(&c);
        if self.cursor_mostrado == Some(pick) {
            return None;
        }
        self.cursor_mostrado = Some(pick);
        let imagem = match (pick, c.shape.as_ref()) {
            (CursorPick::Remote(_), Some(s)) => {
                Some((aylis::core::Image::bgra(s.width, s.height, s.bgra[..(s.width * s.height * 4) as usize].to_vec()), s.hot_x.min(s.width - 1), s.hot_y.min(s.height - 1)))
            }
            _ => None,
        };
        Some((pick, imagem))
    }

    /// Escala da janela (1 = 96 DPI): pontos do Aylis × isto = pixels.
    #[cfg(not(windows))]
    pub fn escala(&self) -> f32 {
        self.escala_janela
    }

    /// Linux: o tamanho da área da imagem (`pontos` do `FrameView`) como o da
    /// tela remota.
    #[cfg(not(windows))]
    pub fn pedir_tamanho(&mut self, pontos: (f32, f32)) {
        let k = self.escala();
        self.pedir(((pontos.0 * k).round() as i32, (pontos.1 * k).round() as i32), (k * 96.0).round() as u32);
    }

    /// Linux: as travas de lá iguais às daqui.
    #[cfg(not(windows))]
    pub fn travas(&mut self, t: aylis::core::LockKeys) {
        let _ = self.input.send(Input::Locks { caps: t.caps_lock, num: t.num_lock, scroll: t.scroll_lock });
    }

    /// O cursor remoto vai desenhado na imagem: o modo pede, e o mouse daqui
    /// não está em cima fazendo as vezes dele ("só o remoto").
    #[cfg(not(windows))]
    pub fn desenha_cursor(&self) -> bool {
        self.opcoes.cursor.draws_remote() && !(self.opcoes.cursor.hides_local() && self.dentro)
    }

    /// Linux: o mouse em cima da imagem (`pos` em pontos do `FrameView`), com
    /// o botão que mudou, se mudou.
    #[cfg(not(windows))]
    pub fn ponteiro(&mut self, pos: (f32, f32), botao: Option<(aylis::prelude::PointerButton, bool)>) {
        use aylis::prelude::PointerButton;
        if let Some((b, down)) = botao {
            let bit = match b {
                PointerButton::Primary => MouseButtons::LEFT,
                PointerButton::Secondary => MouseButtons::RIGHT,
                PointerButton::Middle => MouseButtons::MIDDLE,
                PointerButton::Back => MouseButtons::X1,
                PointerButton::Forward => MouseButtons::X2,
            };
            if down { self.botoes |= bit } else { self.botoes &= !bit }
        }
        let k = self.escala();
        let (nx, ny) = norm((pos.0 * k).round() as i32, (pos.1 * k).round() as i32, self.imagem);
        let _ = self.input.send(Input::Mouse { x: nx, y: ny, buttons: MouseButtons(self.botoes) });
    }

    /// Linux: a roda (`delta` em pontos do Aylis; um dente = `WHEEL_LINE`).
    /// Vai em dentes do Windows (120 = um), positivo pra cima / pra direita.
    #[cfg(not(windows))]
    pub fn roda(&mut self, delta: (f32, f32)) {
        let (dx, dy) = geometria::wheel_notches(&mut self.roda, delta, aylis::prelude::WHEEL_LINE);
        if dx != 0 || dy != 0 {
            let _ = self.input.send(Input::Wheel { dx, dy });
        }
    }

    /// Linux: uma tecla física (`scancode` do evdev). As travas não vão como
    /// tecla: ao soltar, vai o estado delas (`modifiers`).
    #[cfg(not(windows))]
    pub fn tecla(&mut self, scancode: u32, down: bool, modifiers: aylis::prelude::Modifiers) {
        if crate::keymap::EVDEV_LOCKS.contains(&scancode) {
            if !down {
                let _ = self.input.send(Input::Locks { caps: modifiers.caps_lock, num: modifiers.num_lock, scroll: modifiers.scroll_lock });
            }
            return;
        }
        match crate::keymap::pc_scancode(scancode) {
            Some((scancode, extended)) => {
                let _ = self.input.send(Input::Key { scancode, extended, down });
            }
            None => log::debug!("tecla sem equivalente: evdev {scancode}"),
        }
    }

    /// Linux: a janela perdeu o foco: solta tudo lá.
    #[cfg(not(windows))]
    pub fn perdeu_foco(&mut self) {
        self.botoes = 0;
        let _ = self.input.send(Input::ReleaseAll);
    }
}

/// O que se pede direto ao Windows pelo `HWND` e as mensagens da janela.
#[cfg(windows)]
impl Estado {
    /// A janela do Windows passou a existir: teclado inteiro pra lá, o tamanho
    /// pedido (resolução "igual à janela") e as travas iguais às daqui.
    pub fn janela_pronta(&mut self, hwnd: isize) {
        self.hwnd = hwnd;
        log::info!("janela de vídeo pronta (hwnd {hwnd:#x}, {} DPI)", janela::dpi(hwnd));
        self.teclado = teclado::KeyboardHook::install(hwnd, self.input.clone());
        if self.teclado.is_none() {
            log::warn!("gancho de teclado indisponível: atalhos do Windows ficam neste PC");
        }
        if self.pedir_tamanho.is_some() {
            janela::open_large(hwnd);
            self.pedir_tamanho();
        }
        janela::bring_to_front(hwnd);
        let _ = self.input.send(janela::locks());
    }

    /// A janela fechou: solta o teclado e as teclas apertadas lá.
    pub fn fechar(&mut self) {
        self.teclado = None;
        let _ = self.input.send(Input::ReleaseAll);
        self.saida.fechar();
    }

    /// Escala do monitor da janela (1 = 96 DPI): pontos do Aylis × isto = pixels.
    pub fn escala(&self) -> f32 {
        if self.hwnd == 0 { 1.0 } else { janela::dpi(self.hwnd) as f32 / 96.0 }
    }

    /// Manda o tamanho atual da janela como o da tela remota (se mudou).
    fn pedir_tamanho(&mut self) {
        self.pedir(janela::client_size(self.hwnd), janela::dpi(self.hwnd));
    }

    /// Uma mensagem do Windows pra janela. Mouse, roda e teclado vão pra lá
    /// e param aqui (o Aylis não precisa deles nesta janela).
    pub fn mensagem(&mut self, m: &WindowMessage) -> MessageReply {
        let lparam = m.lparam;
        match m.message {
            // O usuário terminou de arrastar a borda: a tela remota acompanha.
            WM_EXITSIZEMOVE => self.pedir_tamanho(),
            // Maximizar e restaurar não passam pelo arrasto.
            WM_SIZE => {
                let max = m.wparam as u32 == SIZE_MAXIMIZED;
                if max || (self.maximizada && m.wparam as u32 == SIZE_RESTORED) {
                    self.maximizada = max;
                    self.pedir_tamanho();
                }
            }
            // Sobre a imagem, só o cursor remoto (se for a opção): a forma de
            // lá como cursor do Windows aqui.
            WM_SETCURSOR if (lparam & 0xFFFF) as u32 == HTCLIENT && self.opcoes.cursor.hides_local() => {
                if self.aplicar_cursor() {
                    return MessageReply::Handled(1);
                }
            }
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONUP => {
                let x = (lparam & 0xFFFF) as i16 as i32;
                let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
                match m.message {
                    WM_LBUTTONDOWN => self.botoes |= MouseButtons::LEFT,
                    WM_LBUTTONUP => self.botoes &= !MouseButtons::LEFT,
                    WM_RBUTTONDOWN => self.botoes |= MouseButtons::RIGHT,
                    WM_RBUTTONUP => self.botoes &= !MouseButtons::RIGHT,
                    WM_MBUTTONDOWN => self.botoes |= MouseButtons::MIDDLE,
                    WM_MBUTTONUP => self.botoes &= !MouseButtons::MIDDLE,
                    _ => {}
                }
                let (nx, ny) = norm(x, y, self.imagem);
                let _ = self.input.send(Input::Mouse { x: nx, y: ny, buttons: MouseButtons(self.botoes) });
                return MessageReply::Handled(0);
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let delta = ((m.wparam >> 16) & 0xFFFF) as i16;
                let wheel = if m.message == WM_MOUSEWHEEL { Input::Wheel { dx: 0, dy: delta } } else { Input::Wheel { dx: delta, dy: 0 } };
                let _ = self.input.send(wheel);
                return MessageReply::Handled(0);
            }
            // Com o gancho, só as travas chegam aqui (sem ele, todas as teclas).
            WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
                let up = m.message == WM_KEYUP || m.message == WM_SYSKEYUP;
                if teclado::is_lock_key(m.wparam as u32) {
                    // Trava: já mudou aqui (no aperto); manda o estado novo.
                    if up {
                        let _ = self.input.send(janela::locks());
                    }
                } else {
                    let scancode = ((lparam >> 16) & 0xFF) as u16;
                    let extended = (lparam >> 24) & 1 != 0;
                    let _ = self.input.send(Input::Key { scancode, extended, down: !up });
                }
                return MessageReply::Handled(0);
            }
            // Entrou na janela: as travas de lá ficam iguais às daqui.
            WM_SETFOCUS => {
                let _ = self.input.send(janela::locks());
            }
            WM_KILLFOCUS => {
                self.botoes = 0;
                let _ = self.input.send(Input::ReleaseAll);
            }
            _ => {}
        }
        MessageReply::Default
    }

    /// Põe o cursor certo no modo "só o remoto"; `false` = deixar o padrão.
    fn aplicar_cursor(&mut self) -> bool {
        let pick = match self.saida.cursor.lock() {
            Ok(c) => {
                let pick = cursor_pick(&c);
                if let (CursorPick::Remote(id), Some(shape)) = (pick, c.shape.as_ref())
                    && self.os_cursor.as_ref().map(|o| o.id) != Some(id)
                {
                    self.os_cursor = janela::OsCursor::new(shape);
                }
                pick
            }
            Err(_) => CursorPick::Local,
        };
        self.cursor_mostrado = Some(pick);
        match (pick, self.os_cursor.as_ref()) {
            (CursorPick::Hidden, _) => {
                janela::hide_cursor();
                true
            }
            (CursorPick::Remote(id), Some(o)) if o.id == id => {
                o.apply();
                true
            }
            _ => false,
        }
    }

    /// No timer: a forma do cursor mudou com o mouse parado em cima (o Windows
    /// não manda `WM_SETCURSOR` sozinho).
    pub fn conferir_cursor(&mut self) {
        if self.hwnd == 0 || !self.opcoes.cursor.hides_local() || !janela::mouse_inside(self.hwnd) {
            return;
        }
        let agora = self.saida.cursor.lock().ok().map(|c| cursor_pick(&c));
        if agora != self.cursor_mostrado {
            self.aplicar_cursor();
        }
    }

    /// O cursor remoto vai desenhado na imagem: o modo pede, e o mouse daqui
    /// não está em cima fazendo as vezes dele ("só o remoto").
    pub fn desenha_cursor(&self) -> bool {
        let os = self.opcoes.cursor.hides_local() && self.hwnd != 0 && janela::mouse_inside(self.hwnd);
        self.opcoes.cursor.draws_remote() && !os
    }
}
