//! Janela de vídeo (§5.1, caminho de MVP): uma janela Win32 que pinta o último
//! quadro BGRA por `StretchDIBits` e captura mouse/teclado pro viewer. Roda a
//! própria bomba de mensagens numa thread, alimentada por um buffer
//! compartilhado; a swapchain D3D11 substitui o `StretchDIBits` depois sem
//! mexer na captura de input nem no protocolo.

use std::sync::{Arc, Mutex};

use reflex_media::decode::BgraFrame;
use reflex_proto::msg::{CursorShape, DesiredDisplay, Input, MouseButtons};
use reflex_viewer::hosts::{CursorMode, ViewScale};
use tokio::sync::mpsc::UnboundedSender;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, FillRect, GetStockObject, InvalidateRect, SetBrushOrgEx, SetStretchBltMode, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLACK_BRUSH,
    DIB_RGB_COLORS, HALFTONE, HBRUSH, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Quadro mais novo a pintar (o laço do viewer escreve, o `WM_PAINT` lê).
#[derive(Default)]
pub struct FrameBuffer {
    pub frame: Mutex<Option<BgraFrame>>,
    pub cursor: Mutex<RemoteCursor>,
    /// A janela de vídeo (0 antes de abrir / depois de fechar).
    window: std::sync::atomic::AtomicIsize,
}

impl FrameBuffer {
    /// A janela de vídeo ainda está aberta.
    pub fn is_open(&self) -> bool {
        self.window.load(std::sync::atomic::Ordering::Acquire) != 0
    }

    /// Troca o título da janela (de qualquer thread).
    pub fn set_title(&self, title: &str) {
        let h = self.window.load(std::sync::atomic::Ordering::Acquire);
        if h != 0 {
            let w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: a mensagem vai pra thread da janela; janela fechada só falha.
            unsafe {
                let _ = SetWindowTextW(HWND(h as *mut _), PCWSTR(w.as_ptr()));
            }
        }
    }

    /// Chegou quadro novo: pinta já (um timer do Windows de 16 ms dispara a
    /// cada ~31 ms — limitava a tela a ~32 quadros/s, com trancos).
    pub fn notify(&self) {
        let h = self.window.load(std::sync::atomic::Ordering::Acquire);
        if h != 0 {
            // SAFETY: InvalidateRect aceita qualquer thread; janela que já
            // fechou só faz a chamada falhar.
            unsafe {
                let _ = InvalidateRect(Some(HWND(h as *mut _)), None, false);
            }
        }
    }
}

/// Cursor da máquina remota. A captura (DXGI) não põe o cursor na imagem: o
/// host manda a forma (BGRA com alfa) e a posição à parte.
#[derive(Default)]
pub struct RemoteCursor {
    pub shape: Option<CursorShape>,
    /// Canto de cima à esquerda da forma, em pixels da tela remota.
    pub x: i32,
    pub y: i32,
    pub visible: bool,
    /// Já chegou alguma posição (antes disso não há onde desenhar).
    pub known: bool,
}

impl RemoteCursor {
    /// Há forma e posição, e o cursor está aparecendo lá.
    pub fn drawable(&self) -> bool {
        self.shape.is_some() && self.known && self.visible
    }
}

/// Alfa pré-multiplicado (o que o `AlphaBlend` quer). Pura.
pub fn premultiply(bgra: &[u8]) -> Vec<u8> {
    bgra.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = p[3] as u32;
            let m = |c: u8| ((c as u32 * a + 127) / 255) as u8;
            [m(p[0]), m(p[1]), m(p[2]), p[3]]
        })
        .collect()
}

/// Onde o cursor vai na janela: a posição e o tamanho remotos na escala da
/// imagem desenhada. Pura.
pub fn cursor_rect(image: Rect, frame: (u32, u32), pos: (i32, i32), size: (u32, u32)) -> Rect {
    let (fw, fh) = (frame.0.max(1) as i64, frame.1.max(1) as i64);
    let sx = |v: i64| (v * image.w as i64 / fw) as i32;
    let sy = |v: i64| (v * image.h as i64 / fh) as i32;
    Rect { x: image.x + sx(pos.0 as i64), y: image.y + sy(pos.1 as i64), w: sx(size.0 as i64).max(1), h: sy(size.1 as i64).max(1) }
}

/// Bitmap do cursor remoto atual (refeito quando a forma muda).
struct CursorBitmap {
    id: u32,
    bmp: windows::Win32::Graphics::Gdi::HBITMAP,
    width: i32,
    height: i32,
}

/// O cursor remoto como cursor do Windows aqui (modo "só o remoto"): o
/// Windows o move junto com o mouse, sem esperar a posição voltar de lá — como
/// o RDP faz. A forma vem de lá; a posição é a daqui.
struct OsCursor {
    id: u32,
    handle: HCURSOR,
}

impl Drop for OsCursor {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyCursor(self.handle);
        }
    }
}

/// Que cursor mostrar sobre a imagem no modo "só o remoto". Pura.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorPick {
    /// Ainda sem a forma de lá: o daqui (a seta).
    Local,
    /// Lá o cursor está escondido (digitando, vídeo em tela cheia…).
    Hidden,
    /// A forma de lá, com este id.
    Remote(u32),
}

pub fn cursor_pick(c: &RemoteCursor) -> CursorPick {
    match &c.shape {
        Some(_) if c.known && !c.visible => CursorPick::Hidden,
        Some(s) if s.width > 0 && s.height > 0 && s.bgra.len() >= (s.width * s.height * 4) as usize => CursorPick::Remote(s.id),
        _ => CursorPick::Local,
    }
}

/// Cursor do Windows com a forma (BGRA, alfa não pré-multiplicado).
unsafe fn make_os_cursor(shape: &CursorShape) -> Option<HCURSOR> {
    use windows::Win32::Graphics::Gdi::{CreateBitmap, CreateDIBSection, DeleteObject};
    let (w, h) = (shape.width as i32, shape.height as i32);
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let color = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
    let n = (shape.width * shape.height * 4) as usize;
    std::ptr::copy_nonoverlapping(shape.bgra.as_ptr(), bits as *mut u8, n);
    // Máscara toda zero: com cor de 32 bits o Windows usa o alfa.
    let mask_bits = vec![0u8; ((w + 15) / 16 * 2 * h) as usize];
    let mask = CreateBitmap(w, h, 1, 1, Some(mask_bits.as_ptr() as *const _));
    let info = ICONINFO { fIcon: false.into(), xHotspot: shape.hot_x, yHotspot: shape.hot_y, hbmMask: mask, hbmColor: color };
    let icon = CreateIconIndirect(&info);
    let _ = DeleteObject(color.into());
    let _ = DeleteObject(mask.into());
    icon.ok().map(|i| HCURSOR(i.0))
}

/// O mouse daqui está sobre a área da janela (e não sobre outra por cima).
unsafe fn mouse_inside(hwnd: HWND) -> bool {
    let mut p = windows::Win32::Foundation::POINT::default();
    if GetCursorPos(&mut p).is_err() || WindowFromPoint(p) != hwnd {
        return false;
    }
    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut p);
    let (w, h) = client_size(hwnd);
    p.x >= 0 && p.y >= 0 && p.x < w && p.y < h
}

/// Põe o cursor certo (modo "só o remoto"); `false` = deixar o padrão.
unsafe fn apply_os_cursor(d: &mut WindowData) -> bool {
    let pick = match d.buffer.cursor.lock() {
        Ok(c) => {
            let pick = cursor_pick(&c);
            if let (CursorPick::Remote(id), Some(shape)) = (pick, c.shape.as_ref()) {
                if d.os_cursor.as_ref().map(|o| o.id) != Some(id) {
                    d.os_cursor = make_os_cursor(shape).map(|handle| OsCursor { id, handle });
                }
            }
            pick
        }
        Err(_) => CursorPick::Local,
    };
    d.cursor_shown = Some(pick);
    match (pick, d.os_cursor.as_ref()) {
        (CursorPick::Hidden, _) => {
            SetCursor(None);
            true
        }
        (CursorPick::Remote(id), Some(o)) if o.id == id => {
            SetCursor(Some(o.handle));
            true
        }
        _ => false,
    }
}

impl Drop for CursorBitmap {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::DeleteObject(self.bmp.into());
        }
    }
}

/// Estado da janela, passado pelo ponteiro de usuário da janela.
struct WindowData {
    buffer: Arc<FrameBuffer>,
    input: UnboundedSender<Input>,
    /// Onde a imagem é desenhada (proporção mantida, faixas pretas em volta):
    /// o mouse é mapeado pra este retângulo.
    image: Rect,
    buttons: u8,
    /// Resolução remota pra qual a janela já foi ajustada.
    sized_for: Option<(u32, u32)>,
    scale: ViewScale,
    /// Resolução "igual à janela": pra onde mandar o tamanho que a tela
    /// remota deve ter, e o último mandado.
    on_resize: Option<OnResize>,
    last_desired: Option<DesiredDisplay>,
    maximized: bool,
    /// A janela nasce escondida e aparece com o 1º quadro (ou depois de
    /// [`SHOW_ANYWAY`] sem imagem, pra não sumir sem explicação).
    created: std::time::Instant,
    shown: bool,
    cursor_mode: CursorMode,
    cursor_bmp: Option<CursorBitmap>,
    os_cursor: Option<OsCursor>,
    /// O que [`apply_os_cursor`] pôs por último (troca de forma parada no lugar).
    cursor_shown: Option<CursorPick>,
}

/// Sem imagem até aqui, a janela aparece mesmo assim (preta).
const SHOW_ANYWAY: std::time::Duration = std::time::Duration::from_secs(15);

/// Recebe o tamanho (pixels reais) e a escala que a tela remota deve ter.
pub type OnResize = Box<dyn Fn(DesiredDisplay) + Send>;

/// O que pedir pra tela remota: a área da janela e a escala do Windows
/// deste monitor (96 DPI = 100%). Pura.
pub fn desired_for(client: (i32, i32), dpi: u32) -> Option<DesiredDisplay> {
    let (w, h) = client;
    // Minimizada (0x0) ou minúscula: não pede nada.
    (w >= 320 && h >= 240).then(|| DesiredDisplay { width: w as u32, height: h as u32, scale_pct: (dpi.max(96) * 100 / 96) as u16 })
}

/// Retângulo em pixels da janela: x, y, largura, altura.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Onde desenhar a imagem na janela. `Windows`: o maior retângulo com a
/// proporção dela (amplia). `Native`: tamanho real, centralizado; só encolhe
/// se não couber. Pura.
pub fn image_rect(scale: ViewScale, img_w: u32, img_h: u32, win_w: i32, win_h: i32) -> Rect {
    match scale {
        ViewScale::Native if img_w as i32 <= win_w && img_h as i32 <= win_h => {
            let (w, h) = (img_w as i32, img_h as i32);
            Rect { x: (win_w - w) / 2, y: (win_h - h) / 2, w, h }
        }
        _ => fit_rect(img_w, img_h, win_w, win_h),
    }
}

/// Maior retângulo com a proporção da imagem que cabe na janela, centralizado. Pura.
pub fn fit_rect(img_w: u32, img_h: u32, win_w: i32, win_h: i32) -> Rect {
    if img_w == 0 || img_h == 0 || win_w <= 0 || win_h <= 0 {
        return Rect { x: 0, y: 0, w: win_w.max(0), h: win_h.max(0) };
    }
    // Compara win_w/win_h com img_w/img_h sem ponto flutuante.
    let (w, h) = if win_w as i64 * img_h as i64 > win_h as i64 * img_w as i64 {
        ((win_h as i64 * img_w as i64 / img_h as i64) as i32, win_h)
    } else {
        (win_w, (win_w as i64 * img_h as i64 / img_w as i64) as i32)
    };
    Rect { x: (win_w - w) / 2, y: (win_h - h) / 2, w, h }
}

/// Abre a janela e roda a bomba de mensagens até fechar (bloqueante — chame
/// numa thread dedicada). `on_close` roda ao fim.
pub fn run(title: &str, buffer: Arc<FrameBuffer>, input: UnboundedSender<Input>, scale: ViewScale, cursor_mode: CursorMode, on_resize: Option<OnResize>) {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let class = w!("ReflexVideoWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            style: CS_HREDRAW | CS_VREDRAW,
            ..Default::default()
        };
        RegisterClassW(&wc);

        let mut data = Box::new(WindowData { buffer, input, image: Rect::default(), buttons: 0, sized_for: None, scale, on_resize, last_desired: None, maximized: false, created: std::time::Instant::now(), shown: false, cursor_mode, cursor_bmp: None, os_cursor: None, cursor_shown: None });
        let title_w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR(title_w.as_ptr()),
            // Sem WS_VISIBLE: aparece quando a imagem chegar (ver `show_once`).
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1280,
            760,
            None,
            None,
            Some(hinst.into()),
            Some(data.as_mut() as *mut _ as *mut _),
        )
        .unwrap_or_default();
        if hwnd.0.is_null() {
            return;
        }

        // Cada quadro novo pinta a janela (`FrameBuffer::notify`); o timer
        // cuida do resto (tamanho, aparecer, cursor que muda parado).
        data.buffer.window.store(hwnd.0 as isize, std::sync::atomic::Ordering::Release);
        SetTimer(Some(hwnd), 1, 50, None);
        // Resolução "igual à janela": abre grande (80% do monitor) e pede esse tamanho.
        if data.on_resize.is_some() {
            open_large(hwnd);
            send_desired(hwnd);
        }
        // Teclado inteiro (tecla Windows, Alt+Tab…) pro host com a janela em foco.
        let hook = crate::keyhook::KeyboardHook::install(hwnd, data.input.clone());
        if hook.is_none() {
            log::warn!("gancho de teclado indisponível: atalhos do Windows ficam neste PC");
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        drop(hook);
        data.buffer.window.store(0, std::sync::atomic::Ordering::Release);
        drop(data);
    }
}

/// Tamanho da tela remota na escala do monitor daqui (96 DPI = 100%): com
/// 150%, 1024x768 vira 1536x1152 — o mesmo que o console do Hyper-V e o resto
/// do Windows mostram. Em pixel 1:1 a VM ficava 1/3 menor. Pura.
pub fn scaled_to_dpi(remote: (u32, u32), dpi: u32) -> (u32, u32) {
    let dpi = dpi.max(96) as u64;
    ((remote.0 as u64 * dpi / 96) as u32, (remote.1 as u64 * dpi / 96) as u32)
}

/// Janela (borda e título inclusos) com área de imagem `content`, limitada à
/// área útil do monitor `work` (sem a barra de tarefas) e centralizada nela;
/// não cabendo, encolhe mantendo a proporção. `frame` é o quanto a borda e o
/// título somam em largura e altura. Pura.
pub fn window_rect_for(content: (u32, u32), work: Rect, frame: (i32, i32)) -> Rect {
    let (max_w, max_h) = ((work.w - frame.0).max(1), (work.h - frame.1).max(1));
    let (rw, rh) = (content.0 as i32, content.1 as i32);
    let (cw, ch) = if rw <= max_w && rh <= max_h {
        (rw, rh)
    } else {
        let r = fit_rect(content.0, content.1, max_w, max_h);
        (r.w, r.h)
    };
    let (w, h) = (cw + frame.0, ch + frame.1);
    Rect { x: work.x + (work.w - w) / 2, y: work.y + (work.h - h) / 2, w, h }
}

/// Ajusta a janela à resolução remota no monitor onde ela está. Maximizada,
/// fica como o usuário deixou.
unsafe fn fit_to_remote(hwnd: HWND, remote: (u32, u32), scale: ViewScale) {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
    if IsZoomed(hwnd).as_bool() {
        return;
    }
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
        return;
    }
    let w = info.rcWork;
    let work = Rect { x: w.left, y: w.top, w: w.right - w.left, h: w.bottom - w.top };
    // Quanto a borda e o título ocupam, na escala (DPI) deste monitor.
    let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
    let ex_style = WINDOW_EX_STYLE(GetWindowLongW(hwnd, GWL_EXSTYLE) as u32);
    let dpi = GetDpiForWindow(hwnd);
    let mut frame = RECT::default();
    if AdjustWindowRectExForDpi(&mut frame, style, false, ex_style, dpi).is_err() {
        return;
    }
    let content = match scale {
        ViewScale::Windows => scaled_to_dpi(remote, dpi),
        ViewScale::Native => remote,
    };
    let r = window_rect_for(content, work, (frame.right - frame.left, frame.bottom - frame.top));
    let _ = SetWindowPos(hwnd, None, r.x, r.y, r.w, r.h, SWP_NOZORDER | SWP_NOACTIVATE);
}

/// Mostra a janela (já no tamanho certo) e traz pra frente.
unsafe fn show_once(hwnd: HWND) {
    // O 1º ShowWindow do processo pode herdar "minimizado" de quem o abriu
    // (medido: aberto por um PowerShell, a janela nasceu minimizada).
    let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
    if IsIconic(hwnd).as_bool() {
        let _ = ShowWindow(hwnd, SW_RESTORE);
    }
    let _ = SetForegroundWindow(hwnd);
}

/// Abre a janela com 80% da área útil do monitor, centralizada.
unsafe fn open_large(hwnd: HWND) {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
        let w = info.rcWork;
        let (ww, wh) = (w.right - w.left, w.bottom - w.top);
        let (cw, ch) = (ww * 4 / 5, wh * 4 / 5);
        let _ = SetWindowPos(hwnd, None, w.left + (ww - cw) / 2, w.top + (wh - ch) / 2, cw, ch, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Manda o tamanho atual da janela como o da tela remota (se mudou).
unsafe fn send_desired(hwnd: HWND) {
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    let Some(d) = data_of(hwnd) else { return };
    let Some(send) = d.on_resize.as_ref() else { return };
    if let Some(want) = desired_for(client_size(hwnd), GetDpiForWindow(hwnd)).filter(|w| d.last_desired != Some(*w)) {
        d.last_desired = Some(want);
        send(want);
    }
}

unsafe fn data_of<'a>(hwnd: HWND) -> Option<&'a mut WindowData> {
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowData;
    p.as_mut()
}

unsafe fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut r = RECT::default();
    let _ = GetClientRect(hwnd, &mut r);
    (r.right - r.left, r.bottom - r.top)
}

/// Posição do mouse (pixels na janela) → normalizada 0..=65535 dentro da
/// imagem (as faixas pretas encostam na borda mais próxima). Pura.
fn norm(px: i32, py: i32, image: Rect) -> (u16, u16) {
    let axis = |p: i32, start: i32, len: i32| -> u16 {
        if len <= 1 {
            return 0;
        }
        ((p - start).clamp(0, len - 1) as i64 * 65535 / (len as i64 - 1)) as u16
    };
    (axis(px, image.x, image.w), axis(py, image.y, image.h))
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            LRESULT(0)
        }
        // O usuário terminou de arrastar a borda: a tela remota acompanha.
        WM_EXITSIZEMOVE => {
            send_desired(hwnd);
            LRESULT(0)
        }
        // Maximizar e restaurar não passam pelo arrasto.
        WM_SIZE => {
            if let Some(d) = data_of(hwnd) {
                let max = wparam.0 as u32 == SIZE_MAXIMIZED;
                if max || (d.maximized && wparam.0 as u32 == SIZE_RESTORED) {
                    d.maximized = max;
                    send_desired(hwnd);
                }
            }
            LRESULT(0)
        }
        // Sobre a imagem, só o cursor remoto (se for a opção): a forma de lá
        // como cursor do Windows aqui.
        WM_SETCURSOR => {
            if (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
                if let Some(d) = data_of(hwnd).filter(|d| d.cursor_mode.hides_local()) {
                    if apply_os_cursor(d) {
                        return LRESULT(1);
                    }
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_TIMER => {
            if let Some(d) = data_of(hwnd) {
                if d.cursor_mode.hides_local() {
                    if mouse_inside(hwnd) {
                        // A forma mudou com o mouse parado: o Windows não manda
                        // WM_SETCURSOR sozinho.
                        let now = d.buffer.cursor.lock().ok().map(|c| cursor_pick(&c));
                        if now != d.cursor_shown {
                            apply_os_cursor(d);
                        }
                    } else if d.buffer.cursor.lock().is_ok_and(|c| c.drawable()) {
                        // Fora da janela o remoto é desenhado na imagem (se mexer lá).
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
            }
            if let Some(d) = data_of(hwnd) {
                // 1º quadro, ou a resolução remota mudou: ajusta a janela.
                let remote = d.buffer.frame.lock().ok().and_then(|g| g.as_ref().map(|f| (f.width, f.height)));
                if let Some(remote) = remote.filter(|r| d.sized_for != Some(*r)) {
                    d.sized_for = Some(remote);
                    fit_to_remote(hwnd, remote, d.scale);
                }
                if !d.shown && (remote.is_some() || d.created.elapsed() >= SHOW_ANYWAY) {
                    d.shown = true;
                    show_once(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if let Some(d) = data_of(hwnd) {
                let size = client_size(hwnd);
                // Cópia do Arc: o quadro fica travado enquanto `d` desenha o cursor.
                let buffer = d.buffer.clone();
                let guard = buffer.frame.lock();
                match guard.as_ref().ok().and_then(|g| g.as_ref()) {
                    Some(f) => {
                        d.image = image_rect(d.scale, f.width, f.height, size.0, size.1);
                        paint_frame(hdc, f, size, d.image);
                        // "Só o remoto" com o mouse aqui dentro: o cursor do
                        // Windows já é o de lá (ver WM_SETCURSOR).
                        let os = d.cursor_mode.hides_local() && mouse_inside(hwnd);
                        if d.cursor_mode.draws_remote() && !os {
                            paint_remote_cursor(hdc, d, (f.width, f.height));
                        }
                    }
                    // Ainda sem imagem: preto, não o branco do fundo vazio.
                    None => {
                        FillRect(hdc, &RECT { left: 0, top: 0, right: size.0, bottom: size.1 }, HBRUSH(GetStockObject(BLACK_BRUSH).0));
                    }
                }
                drop(guard);
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONUP => {
            if let Some(d) = data_of(hwnd) {
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                match msg {
                    WM_LBUTTONDOWN => d.buttons |= MouseButtons::LEFT,
                    WM_LBUTTONUP => d.buttons &= !MouseButtons::LEFT,
                    WM_RBUTTONDOWN => d.buttons |= MouseButtons::RIGHT,
                    WM_RBUTTONUP => d.buttons &= !MouseButtons::RIGHT,
                    WM_MBUTTONDOWN => d.buttons |= MouseButtons::MIDDLE,
                    WM_MBUTTONUP => d.buttons &= !MouseButtons::MIDDLE,
                    _ => {}
                }
                let (nx, ny) = norm(x, y, d.image);
                let _ = d.input.send(Input::Mouse { x: nx, y: ny, buttons: MouseButtons(d.buttons) });
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            if let Some(d) = data_of(hwnd) {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
                let _ = d.input.send(Input::Wheel { dx: 0, dy: delta });
            }
            LRESULT(0)
        }
        WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
            if let Some(d) = data_of(hwnd) {
                // Trava: já mudou aqui (no aperto); manda o estado novo.
                if crate::keyhook::is_lock_key(wparam.0 as u32) {
                    if msg == WM_KEYUP || msg == WM_SYSKEYUP {
                        send_locks(d);
                    }
                    return LRESULT(0);
                }
                let scancode = ((lparam.0 >> 16) & 0xFF) as u16;
                let extended = (lparam.0 >> 24) & 1 != 0;
                let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
                let _ = d.input.send(Input::Key { scancode, extended, down });
            }
            LRESULT(0)
        }
        // Entrou na janela: as travas de lá ficam iguais às daqui.
        WM_SETFOCUS => {
            if let Some(d) = data_of(hwnd) {
                send_locks(d);
            }
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            if let Some(d) = data_of(hwnd) {
                d.buttons = 0;
                let _ = d.input.send(Input::ReleaseAll);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            KillTimer(Some(hwnd), 1).ok();
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Manda o estado das travas daqui (Caps, Num, Scroll Lock).
unsafe fn send_locks(d: &WindowData) {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
    let on = |vk: i32| GetKeyState(vk) & 1 != 0;
    let _ = d.input.send(Input::Locks { caps: on(0x14), num: on(0x90), scroll: on(0x91) });
}

/// Desenha o cursor remoto por cima do quadro (alfa), na escala da imagem.
unsafe fn paint_remote_cursor(hdc: windows::Win32::Graphics::Gdi::HDC, d: &mut WindowData, frame: (u32, u32)) {
    use windows::Win32::Graphics::Gdi::{AlphaBlend, CreateCompatibleDC, CreateDIBSection, DeleteDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
    let Ok(c) = d.buffer.cursor.lock() else { return };
    let Some(shape) = c.shape.as_ref().filter(|_| c.visible && c.known) else { return };
    if shape.width == 0 || shape.height == 0 || shape.bgra.len() < (shape.width * shape.height * 4) as usize {
        return;
    }
    // Forma nova: bitmap novo (alfa pré-multiplicado, de cima pra baixo).
    if d.cursor_bmp.as_ref().map(|b| b.id) != Some(shape.id) {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: shape.width as i32,
                biHeight: -(shape.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else { return };
        let pre = premultiply(&shape.bgra[..(shape.width * shape.height * 4) as usize]);
        std::ptr::copy_nonoverlapping(pre.as_ptr(), bits as *mut u8, pre.len());
        d.cursor_bmp = Some(CursorBitmap { id: shape.id, bmp, width: shape.width as i32, height: shape.height as i32 });
    }
    let Some(b) = d.cursor_bmp.as_ref() else { return };
    let r = cursor_rect(d.image, frame, (c.x, c.y), (b.width as u32, b.height as u32));
    let mem = CreateCompatibleDC(Some(hdc));
    let old = SelectObject(mem, b.bmp.into());
    let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
    let _ = AlphaBlend(hdc, r.x, r.y, r.w, r.h, mem, 0, 0, b.width, b.height, blend);
    SelectObject(mem, old);
    let _ = DeleteDC(mem);
}

/// Desenha o quadro em `image` (proporção mantida) e pinta de preto o resto.
/// HALFTONE: o modo padrão do StretchDIBits (BLACKONWHITE) junta as linhas que
/// sobram com um E lógico ao reduzir — eram os tracinhos escuros na imagem.
unsafe fn paint_frame(hdc: windows::Win32::Graphics::Gdi::HDC, f: &BgraFrame, size: (i32, i32), image: Rect) {
    let black = GetStockObject(BLACK_BRUSH);
    let bars = [
        RECT { left: 0, top: 0, right: size.0, bottom: image.y },
        RECT { left: 0, top: image.y + image.h, right: size.0, bottom: size.1 },
        RECT { left: 0, top: image.y, right: image.x, bottom: image.y + image.h },
        RECT { left: image.x + image.w, top: image.y, right: size.0, bottom: image.y + image.h },
    ];
    for r in bars {
        if r.right > r.left && r.bottom > r.top {
            FillRect(hdc, &r, HBRUSH(black.0));
        }
    }
    SetStretchBltMode(hdc, HALFTONE);
    let _ = SetBrushOrgEx(hdc, 0, 0, None);
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: f.width as i32,
            // Altura negativa: top-down (nossa imagem começa no topo).
            biHeight: -(f.height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    StretchDIBits(
        hdc,
        image.x,
        image.y,
        image.w,
        image.h,
        0,
        0,
        f.width as i32,
        f.height as i32,
        Some(f.data.as_ptr() as *const _),
        &bi,
        DIB_RGB_COLORS,
        SRCCOPY,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // Monitor 1920x1080 com a barra de tarefas embaixo (área útil 1920x1032);
    // borda + título de 16 x 39 pixels.
    const WORK: Rect = Rect { x: 0, y: 0, w: 1920, h: 1032 };
    const FRAME: (i32, i32) = (16, 39);

    #[test]
    fn remote_that_fits_opens_at_real_size_centered() {
        let r = window_rect_for((1024, 768), WORK, FRAME);
        assert_eq!((r.w - FRAME.0, r.h - FRAME.1), (1024, 768));
        assert_eq!((r.x, r.y), ((1920 - 1040) / 2, (1032 - 807) / 2));
    }

    #[test]
    fn remote_bigger_than_the_monitor_shrinks_keeping_aspect() {
        let r = window_rect_for((1920, 1080), WORK, FRAME);
        let (cw, ch) = (r.w - FRAME.0, r.h - FRAME.1);
        assert_eq!(ch, 1032 - 39);
        assert!((cw * 1080 - ch * 1920).abs() < 1920, "{cw}x{ch} fora da proporção");
        assert!(r.x >= 0 && r.y == 0 && r.w <= 1920);
    }

    #[test]
    fn remote_follows_the_monitor_scale_like_the_hyperv_console() {
        assert_eq!(scaled_to_dpi((1024, 768), 96), (1024, 768));
        assert_eq!(scaled_to_dpi((1024, 768), 144), (1536, 1152));
        assert_eq!(scaled_to_dpi((1920, 1080), 120), (2400, 1350));
        // DPI inválido (0) não encolhe.
        assert_eq!(scaled_to_dpi((800, 600), 0), (800, 600));
    }

    #[test]
    fn native_scale_shows_real_pixels_and_only_shrinks() {
        // Cabe: 1:1 no meio, sem ampliar.
        assert_eq!(image_rect(ViewScale::Native, 1024, 768, 1536, 1152), Rect { x: 256, y: 192, w: 1024, h: 768 });
        // Não cabe: encolhe como o modo Windows.
        assert_eq!(image_rect(ViewScale::Native, 1920, 1080, 960, 540), fit_rect(1920, 1080, 960, 540));
        // Escala do Windows: amplia até a janela.
        assert_eq!(image_rect(ViewScale::Windows, 1024, 768, 1536, 1152), Rect { x: 0, y: 0, w: 1536, h: 1152 });
    }

    #[test]
    fn window_size_and_scale_become_the_remote_request() {
        assert_eq!(desired_for((1536, 1100), 144), Some(DesiredDisplay { width: 1536, height: 1100, scale_pct: 150 }));
        assert_eq!(desired_for((1280, 720), 96).map(|d| d.scale_pct), Some(100));
        // Minimizada não pede nada.
        assert_eq!(desired_for((0, 0), 144), None);
    }

    #[test]
    fn remote_cursor_needs_shape_position_and_visibility() {
        let shape = CursorShape { id: 1, width: 1, height: 1, hot_x: 0, hot_y: 0, bgra: vec![0; 4] };
        let mut c = RemoteCursor { shape: Some(shape), ..Default::default() };
        assert!(!c.drawable(), "sem posição ainda");
        (c.known, c.visible) = (true, true);
        assert!(c.drawable());
        c.visible = false;
        assert!(!c.drawable());
    }

    #[test]
    fn os_cursor_pick_follows_the_remote_state() {
        let shape = CursorShape { id: 7, width: 1, height: 1, hot_x: 0, hot_y: 0, bgra: vec![0; 4] };
        assert_eq!(cursor_pick(&RemoteCursor::default()), CursorPick::Local);
        // A forma basta (a posição é a daqui).
        let c = RemoteCursor { shape: Some(shape.clone()), ..Default::default() };
        assert_eq!(cursor_pick(&c), CursorPick::Remote(7));
        let hidden = RemoteCursor { shape: Some(shape.clone()), known: true, visible: false, ..Default::default() };
        assert_eq!(cursor_pick(&hidden), CursorPick::Hidden);
        let broken = RemoteCursor { shape: Some(CursorShape { bgra: Vec::new(), ..shape }), ..Default::default() };
        assert_eq!(cursor_pick(&broken), CursorPick::Local);
    }

    #[test]
    fn cursor_alpha_is_premultiplied() {
        assert_eq!(premultiply(&[255, 255, 255, 255, 200, 100, 50, 0, 255, 0, 0, 128]), vec![255, 255, 255, 255, 0, 0, 0, 0, 128, 0, 0, 128]);
    }

    #[test]
    fn cursor_follows_the_image_scale() {
        // Imagem 1920x1080 desenhada em 960x540 com 100 px de faixa à esquerda.
        let img = Rect { x: 100, y: 0, w: 960, h: 540 };
        assert_eq!(cursor_rect(img, (1920, 1080), (400, 200), (32, 32)), Rect { x: 300, y: 100, w: 16, h: 16 });
        // Pixels reais: tamanho igual.
        let real = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        assert_eq!(cursor_rect(real, (1920, 1080), (10, 20), (32, 32)), Rect { x: 10, y: 20, w: 32, h: 32 });
    }

    #[test]
    fn second_monitor_offsets_are_kept() {
        let right = Rect { x: 1920, y: 0, w: 2560, h: 1400 };
        let r = window_rect_for((1280, 720), right, FRAME);
        assert!(r.x > 1920 && r.x + r.w < 1920 + 2560);
    }

    #[test]
    fn normalization_hits_corners() {
        let img = Rect { x: 0, y: 0, w: 1280, h: 720 };
        assert_eq!(norm(0, 0, img), (0, 0));
        assert_eq!(norm(1279, 719, img), (65535, 65535));
        let (x, _) = norm(640, 0, img);
        assert!((32000..=33000).contains(&x));
    }

    #[test]
    fn letterbox_keeps_aspect_and_centers() {
        // 4:3 numa janela larga: faixas nas laterais.
        assert_eq!(fit_rect(1024, 768, 1262, 705), Rect { x: 161, y: 0, w: 940, h: 705 });
        // 16:9 numa janela alta: faixas em cima e embaixo.
        assert_eq!(fit_rect(1920, 1080, 800, 800), Rect { x: 0, y: 175, w: 800, h: 450 });
    }

    #[test]
    fn mouse_on_the_bars_clamps_to_image_edge() {
        let img = fit_rect(1024, 768, 1262, 705);
        assert_eq!(norm(10, 300, img).0, 0); // faixa da esquerda
        assert_eq!(norm(1255, 300, img).0, 65535); // faixa da direita
        assert_eq!(norm(img.x + img.w / 2, 0, img).0 / 1000, 32);
    }

    #[test]
    fn out_of_bounds_is_clamped() {
        assert_eq!(norm(-10, 9999, Rect { x: 0, y: 0, w: 100, h: 100 }), (0, 65535));
    }
}
