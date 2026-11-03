//! Retoques do Windows 11 sobre os controles Win32 (§5.1.1):
//! - moldura com Mica e cantos arredondados (`DwmSetWindowAttribute`);
//! - fonte Segoe UI Variable (texto) e Display Semibold (títulos), na escala
//!   do DPI da janela;
//! - fundo branco uniforme: rótulos e campos só-leitura sem a "caixa" cinza
//!   do `COLOR_BTNFACE`.
//!
//! O resto do visual moderno vem do manifesto (Common Controls 6, DPI por
//! monitor) embutido no exe pelo `build.rs` do app.

use std::cell::OnceCell;

use winsafe::co;
use winsafe::gui;
use winsafe::prelude::*;

/// Cor do texto secundário (rótulos de apoio, status).
pub const SECONDARY_TEXT: (u8, u8, u8) = (0x5F, 0x5F, 0x5F);

struct Fonts {
    text: winsafe::HFONT,
    title: winsafe::HFONT,
    nav: winsafe::HFONT,
}

thread_local! {
    static FONTS: OnceCell<Fonts> = const { OnceCell::new() };
}

/// Altura lógica de uma fonte em pontos → pixels no DPI dado (negativa = altura do caractere).
pub fn font_height(points: i32, dpi: u32) -> i32 {
    -((points * dpi as i32 + 36) / 72)
}

fn make_font(points: i32, dpi: u32, face: &str, weight: co::FW) -> winsafe::HFONT {
    let mut lf = winsafe::LOGFONT::new_face(font_height(points, dpi), face);
    lf.lfWeight = weight;
    lf.lfQuality = co::QUALITY::CLEARTYPE;
    match winsafe::HFONT::CreateFontIndirect(&lf) {
        // A fonte vive o processo inteiro (UI de uma thread): sem guard.
        Ok(guard) => unsafe { std::mem::ManuallyDrop::new(guard).raw_copy() },
        Err(_) => winsafe::HFONT::NULL,
    }
}

fn with_fonts<R>(dpi: u32, f: impl FnOnce(&Fonts) -> R) -> R {
    FONTS.with(|cell| {
        let fonts = cell.get_or_init(|| Fonts {
            text: make_font(10, dpi, "Segoe UI Variable Text", co::FW::NORMAL),
            title: make_font(18, dpi, "Segoe UI Variable Display", co::FW::SEMIBOLD),
            nav: make_font(11, dpi, "Segoe UI Variable Text", co::FW::NORMAL),
        });
        f(fonts)
    })
}

fn dpi_of(hwnd: &winsafe::HWND) -> u32 {
    let dpi = hwnd.GetDpiForWindow();
    if dpi == 0 {
        96
    } else {
        dpi
    }
}

fn set_font(hwnd: &winsafe::HWND, font: &winsafe::HFONT) {
    unsafe {
        hwnd.SendMessage(winsafe::msg::WmSetFont { hfont: font.raw_copy(), redraw: true });
    }
}

/// Moldura do Windows 11 na janela principal: Mica e cantos arredondados.
/// No Windows 10 as chamadas falham e fica a moldura padrão.
pub fn decorate_window(hwnd: &winsafe::HWND) {
    let _ = hwnd.DwmSetWindowAttribute(winsafe::DwmAttr::SystemBackdropType(co::DWMSBT::MAINWINDOW));
    let _ = hwnd.DwmSetWindowAttribute(winsafe::DwmAttr::WindowCornerPreference(co::DWMWCP::ROUND));
}

/// Fonte de texto em todos os controles filhos de `parent`.
pub fn apply_text_font(parent: &winsafe::HWND) {
    with_fonts(dpi_of(parent), |f| {
        parent.EnumChildWindows(|child| {
            set_font(&child, &f.text);
            true
        });
    });
}

/// Fonte de título num rótulo.
pub fn apply_title_font(label: &gui::Label) {
    with_fonts(dpi_of(label.hwnd()), |f| set_font(label.hwnd(), &f.title));
}

/// Retângulo de texto centralizado na vertical dentro da área cliente do
/// campo, com `pad` de respiro nas laterais. Pura.
pub fn centered_text_rect(client: winsafe::RECT, text_height: i32, pad: i32) -> winsafe::RECT {
    let top = ((client.bottom - client.top - text_height) / 2).max(0);
    winsafe::RECT { left: client.left + pad, top: client.top + top, right: client.right - pad, bottom: client.top + top + text_height }
}

/// Centraliza o texto de um campo (criado com `ES_MULTILINE`, o único tipo de
/// Edit que aceita retângulo de formatação) — o Edit de uma linha escreve
/// colado no topo. Chamar depois de aplicar a fonte (o `WM_SETFONT` desfaz o
/// retângulo).
pub fn center_edit_text(edit: &gui::Edit) {
    let h = edit.hwnd();
    let scale = (dpi_of(h) as i32 * 100) / 96;
    let text_h = (|| -> winsafe::SysResult<i32> {
        let dc = h.GetDC()?;
        let font = with_fonts(dpi_of(h), |f| unsafe { f.text.raw_copy() });
        let _old = dc.SelectObject(&font)?;
        Ok(dc.GetTextMetrics()?.tmHeight)
    })();
    let (Ok(text_h), Ok(client)) = (text_h, h.GetClientRect()) else { return };
    let rc = centered_text_rect(client, text_h, 8 * scale / 100);
    unsafe {
        h.SendMessage(winsafe::msg::EmSetRect { is_absolute_coords: true, rect: Some(&rc) });
    }
}

/// Fonte do seletor de seções (cópia do handle; a fonte vive o processo todo).
pub fn nav_font(hwnd: &winsafe::HWND) -> winsafe::HFONT {
    with_fonts(dpi_of(hwnd), |f| unsafe { f.nav.raw_copy() })
}

/// Página com fundo branco e rótulos sem caixa cinza. `secondary`: rótulos
/// que ficam na cor de texto secundário.
pub fn white_page(page: &gui::WindowControl, secondary: Vec<gui::Label>) {
    page.on().wm_ctl_color_static(move |p| {
        let _ = p.hdc.SetBkMode(co::BKMODE::TRANSPARENT);
        if secondary.iter().any(|l| *l.hwnd() == p.hwnd) {
            let (r, g, b) = SECONDARY_TEXT;
            let _ = p.hdc.SetTextColor(winsafe::COLORREF::from_rgb(r, g, b));
        }
        Ok(winsafe::HBRUSH::GetStockObject(co::STOCK_BRUSH::WHITE)?)
    });
}

/// Opções de uma página (painel) com fundo branco, que acompanha o tamanho da janela.
pub fn page_opts(position: (i32, i32), size: (i32, i32)) -> gui::WindowControlOpts<'static> {
    gui::WindowControlOpts {
        position,
        size,
        class_bg_brush: gui::Brush::Color(co::COLOR::WINDOW),
        resize_behavior: (gui::Horz::Resize, gui::Vert::Resize),
        ..Default::default()
    }
}

/// Mostra ou esconde um painel.
pub fn set_visible(page: &gui::WindowControl, visible: bool) {
    page.hwnd().ShowWindow(if visible { co::SW::SHOW } else { co::SW::HIDE });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_rect_is_vertically_centered() {
        let client = winsafe::RECT { left: 0, top: 0, right: 300, bottom: 44 };
        let rc = centered_text_rect(client, 24, 12);
        assert_eq!((rc.left, rc.top, rc.right, rc.bottom), (12, 10, 288, 34));
        // Texto maior que o campo: encosta no topo, sem valor negativo.
        assert_eq!(centered_text_rect(client, 60, 0).top, 0);
    }

    #[test]
    fn font_height_scales_with_dpi() {
        assert_eq!(font_height(9, 96), -12);
        assert_eq!(font_height(10, 96), -13);
        assert_eq!(font_height(10, 144), -20);
    }
}
