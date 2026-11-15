//! O que a janela de vídeo pede direto ao Windows (pelo `HWND` que o Aylis
//! dá): tamanho e posição, escala do monitor, o mouse estar em cima, o cursor
//! com a forma remota e o estado das travas do teclado. A conta fica em
//! [`super::geometria`].

use crate::proto::msg::{CursorShape, Input};
use crate::viewer::hosts::ViewScale;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    ScreenToClient,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyCursor, GWL_EXSTYLE, GWL_STYLE, GetClientRect, GetCursorPos, GetWindowLongW, HCURSOR, ICONINFO, IsZoomed, SWP_NOACTIVATE, SWP_NOZORDER,
    SetCursor, SetForegroundWindow, SetWindowPos, WINDOW_EX_STYLE, WINDOW_STYLE, WindowFromPoint,
};

use super::geometria::{Rect, scaled_to_dpi, window_rect_for};

fn h(hwnd: isize) -> HWND {
    HWND(hwnd as *mut _)
}

/// DPI do monitor da janela (96 = 100%).
pub fn dpi(hwnd: isize) -> u32 {
    unsafe { GetDpiForWindow(h(hwnd)) }.max(96)
}

/// Tamanho da área de dentro, em pixels reais.
pub fn client_size(hwnd: isize) -> (i32, i32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(h(hwnd), &mut r);
    }
    (r.right - r.left, r.bottom - r.top)
}

/// A janela está maximizada.
pub fn maximized(hwnd: isize) -> bool {
    unsafe { IsZoomed(h(hwnd)) }.as_bool()
}

/// A área útil do monitor da janela (sem a barra de tarefas).
fn work_area(hwnd: isize) -> Option<Rect> {
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe { GetMonitorInfoW(MonitorFromWindow(h(hwnd), MONITOR_DEFAULTTONEAREST), &mut info) }.as_bool().then(|| {
        let w = info.rcWork;
        Rect { x: w.left, y: w.top, w: w.right - w.left, h: w.bottom - w.top }
    })
}

/// Ajusta a janela à resolução remota no monitor onde ela está. Maximizada,
/// fica como o usuário deixou.
pub fn fit_to_remote(hwnd: isize, remote: (u32, u32), scale: ViewScale) {
    if maximized(hwnd) {
        return;
    }
    let Some(work) = work_area(hwnd) else { return };
    // Quanto a borda e o título ocupam, na escala (DPI) deste monitor.
    let (style, ex_style) = unsafe { (WINDOW_STYLE(GetWindowLongW(h(hwnd), GWL_STYLE) as u32), WINDOW_EX_STYLE(GetWindowLongW(h(hwnd), GWL_EXSTYLE) as u32)) };
    let dpi = dpi(hwnd);
    let mut frame = RECT::default();
    if unsafe { AdjustWindowRectExForDpi(&mut frame, style, false, ex_style, dpi) }.is_err() {
        return;
    }
    let content = match scale {
        ViewScale::Windows => scaled_to_dpi(remote, dpi),
        ViewScale::Native => remote,
    };
    let r = window_rect_for(content, work, (frame.right - frame.left, frame.bottom - frame.top));
    unsafe {
        let _ = SetWindowPos(h(hwnd), None, r.x, r.y, r.w, r.h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Abre a janela com 80% da área útil do monitor, centralizada.
pub fn open_large(hwnd: isize) {
    if let Some(w) = work_area(hwnd) {
        let (cw, ch) = (w.w * 4 / 5, w.h * 4 / 5);
        unsafe {
            let _ = SetWindowPos(h(hwnd), None, w.x + (w.w - cw) / 2, w.y + (w.h - ch) / 2, cw, ch, SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
}

/// Traz a janela pra frente (pra receber o teclado já).
pub fn bring_to_front(hwnd: isize) {
    unsafe {
        let _ = SetForegroundWindow(h(hwnd));
    }
}

/// O mouse daqui está sobre a área da janela (e não sobre outra por cima).
pub fn mouse_inside(hwnd: isize) -> bool {
    let mut p = POINT::default();
    if unsafe { GetCursorPos(&mut p) }.is_err() || unsafe { WindowFromPoint(p) } != h(hwnd) {
        return false;
    }
    let _ = unsafe { ScreenToClient(h(hwnd), &mut p) };
    let (w, ht) = client_size(hwnd);
    p.x >= 0 && p.y >= 0 && p.x < w && p.y < ht
}

/// O estado das travas daqui (Caps, Num, Scroll Lock), pra mandar pra lá.
pub fn locks() -> Input {
    let on = |vk: i32| unsafe { GetKeyState(vk) } & 1 != 0;
    Input::Locks { caps: on(0x14), num: on(0x90), scroll: on(0x91) }
}

/// O cursor remoto como cursor do Windows aqui (modo "só o remoto"): o
/// Windows o move junto com o mouse, sem esperar a posição voltar de lá — como
/// o RDP faz. A forma vem de lá; a posição é a daqui.
pub struct OsCursor {
    pub id: u32,
    handle: HCURSOR,
}

impl OsCursor {
    /// Cursor do Windows com a forma (BGRA, alfa não pré-multiplicado).
    pub fn new(shape: &CursorShape) -> Option<Self> {
        let (w, ht) = (shape.width as i32, shape.height as i32);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -ht, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        unsafe {
            let mut bits = std::ptr::null_mut();
            let color = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let n = (shape.width * shape.height * 4) as usize;
            std::ptr::copy_nonoverlapping(shape.bgra.as_ptr(), bits as *mut u8, n);
            // Máscara toda zero: com cor de 32 bits o Windows usa o alfa.
            let mask_bits = vec![0u8; ((w + 15) / 16 * 2 * ht) as usize];
            let mask = CreateBitmap(w, ht, 1, 1, Some(mask_bits.as_ptr() as *const _));
            let info = ICONINFO { fIcon: false.into(), xHotspot: shape.hot_x, yHotspot: shape.hot_y, hbmMask: mask, hbmColor: color };
            let icon = CreateIconIndirect(&info);
            let _ = DeleteObject(color.into());
            let _ = DeleteObject(mask.into());
            icon.ok().map(|i| Self { id: shape.id, handle: HCURSOR(i.0) })
        }
    }

    pub fn apply(&self) {
        unsafe {
            SetCursor(Some(self.handle));
        }
    }
}

impl Drop for OsCursor {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyCursor(self.handle);
        }
    }
}

/// Esconde o cursor (lá ele está escondido).
pub fn hide_cursor() {
    unsafe {
        SetCursor(None);
    }
}
