//! Seletor de seções no estilo do Windows 11 (o `SelectorBar` do WinUI 3):
//! os nomes lado a lado, o selecionado em preto com uma barrinha na cor de
//! destaque embaixo, os outros em cinza. Desenhado por GDI num `WindowControl`
//! — o controle de abas nativo (`SysTabControl32`) não tem esse visual.

use std::cell::RefCell;
use std::rc::Rc;

use winsafe::co;
use winsafe::gui;
use winsafe::prelude::*;

use crate::theme;

/// Posições horizontais dos itens: começa em `pad` e separa por `gap`. Pura.
pub fn layout(widths: &[i32], pad: i32, gap: i32) -> Vec<(i32, i32)> {
    let mut x = pad;
    widths
        .iter()
        .map(|&w| {
            let item = (x, x + w);
            x += w + gap;
            item
        })
        .collect()
}

/// Índice do item sob `x` (a área clicável inclui metade do espaço entre itens).
pub fn hit(items: &[(i32, i32)], x: i32, gap: i32) -> Option<usize> {
    items.iter().position(|&(l, r)| x >= l - gap / 2 && x < r + gap / 2)
}

struct State {
    selected: usize,
    items: Vec<(i32, i32)>,
    on_select: Option<Box<dyn Fn(usize)>>,
}

#[derive(Clone)]
pub struct SelectorBar {
    wnd: gui::WindowControl,
    labels: Rc<Vec<&'static str>>,
    state: Rc<RefCell<State>>,
}

impl SelectorBar {
    pub fn new(parent: &(impl GuiParent + 'static), position: (i32, i32), size: (i32, i32), labels: Vec<&'static str>) -> Self {
        let wnd = gui::WindowControl::new(
            parent,
            gui::WindowControlOpts { position, size, class_bg_brush: gui::Brush::Color(co::COLOR::WINDOW), class_cursor: gui::Cursor::Idc(co::IDC::HAND), ..Default::default() },
        );
        let me = Self { wnd, labels: Rc::new(labels), state: Rc::new(RefCell::new(State { selected: 0, items: Vec::new(), on_select: None })) };
        me.events();
        me
    }

    /// O que fazer quando o usuário escolhe uma seção.
    pub fn on_select(&self, f: impl Fn(usize) + 'static) {
        self.state.borrow_mut().on_select = Some(Box::new(f));
    }

    fn scale(&self) -> i32 {
        (self.wnd.hwnd().GetDpiForWindow().max(96) as i32 * 100) / 96
    }

    fn events(&self) {
        let me = self.clone();
        self.wnd.on().wm_paint(move || {
            me.paint()?;
            Ok(())
        });
        let me = self.clone();
        self.wnd.on().wm_l_button_down(move |p| {
            let gap = 24 * me.scale() / 100;
            let index = hit(&me.state.borrow().items, p.coords.x, gap);
            if let Some(i) = index {
                if i != me.state.borrow().selected {
                    me.state.borrow_mut().selected = i;
                    me.wnd.hwnd().InvalidateRect(None, true)?;
                    if let Some(f) = me.state.borrow().on_select.as_ref() {
                        f(i);
                    }
                }
            }
            Ok(())
        });
    }

    fn paint(&self) -> winsafe::AnyResult<()> {
        let hwnd = self.wnd.hwnd();
        let hdc = hwnd.BeginPaint()?;
        let rc = hwnd.GetClientRect()?;
        hdc.FillRect(rc, &winsafe::HBRUSH::GetStockObject(co::STOCK_BRUSH::WHITE)?)?;

        let s = self.scale();
        let (pad, gap) = (4 * s / 100, 24 * s / 100);
        let font = theme::nav_font(hwnd);
        let _old_font = hdc.SelectObject(&font)?;
        hdc.SetBkMode(co::BKMODE::TRANSPARENT)?;

        let widths: Vec<i32> = self.labels.iter().map(|l| hdc.GetTextExtentPoint32(l).map(|sz| sz.cx).unwrap_or(0)).collect();
        let items = layout(&widths, pad, gap);
        let text_h = hdc.GetTextExtentPoint32("Ag").map(|sz| sz.cy).unwrap_or(16);
        let y = (rc.bottom - text_h) / 2 - 3 * s / 100;
        let selected = self.state.borrow().selected;
        for (i, (label, &(left, right))) in self.labels.iter().zip(&items).enumerate() {
            let (r, g, b) = if i == selected { (0x1A, 0x1A, 0x1A) } else { theme::SECONDARY_TEXT };
            hdc.SetTextColor(winsafe::COLORREF::from_rgb(r, g, b))?;
            hdc.TextOut(left, y, label)?;
            if i == selected {
                // Barrinha de destaque centrada sob o texto.
                let (w, h) = (16 * s / 100, (3 * s / 100).max(2));
                let cx = (left + right) / 2;
                let bar = winsafe::RECT { left: cx - w / 2, right: cx + w / 2, top: rc.bottom - h - 2 * s / 100, bottom: rc.bottom - 2 * s / 100 };
                let accent = winsafe::HBRUSH::CreateSolidBrush(winsafe::GetSysColor(co::COLOR::HIGHLIGHT))?;
                let _b = hdc.SelectObject(&*accent)?;
                let _p = hdc.SelectObject(&winsafe::HPEN::GetStockObject(co::STOCK_PEN::NULL)?)?;
                hdc.RoundRect(bar, winsafe::SIZE { cx: h, cy: h })?;
            }
        }
        self.state.borrow_mut().items = items;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_are_laid_out_left_to_right() {
        assert_eq!(layout(&[60, 90], 4, 24), vec![(4, 64), (88, 178)]);
    }

    #[test]
    fn hit_testing_includes_half_the_gap() {
        let items = layout(&[60, 90], 4, 24);
        assert_eq!(hit(&items, 10, 24), Some(0));
        assert_eq!(hit(&items, 75, 24), Some(0));
        assert_eq!(hit(&items, 77, 24), Some(1));
        assert_eq!(hit(&items, 500, 24), None);
    }
}
