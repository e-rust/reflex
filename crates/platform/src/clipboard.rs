//! Clipboard (§10.7): ler/escrever **todos** os formatos de uma cópia
//! (texto, HTML, RTF, imagem, metafile, formatos próprios dos programas) e um
//! observador que avisa quando alguém copia. A detecção de mudança e o "sem
//! eco" usam o número de sequência do clipboard: o que nós mesmos escrevemos
//! não volta como cópia.
//!
//! [`carry`] decide o que viaja: fica de fora o que só faz sentido nesta
//! máquina (objetos OLE, handles GDI) e o que o Windows recria sozinho do
//! outro lado (ANSI a partir do Unicode, bitmap a partir do DIB). Arquivos
//! copiados (`CF_HDROP`) são a transferência de arquivos (§10.8), à parte.

use std::sync::mpsc::{channel, Receiver, Sender};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use reflex_proto::msg::{ClipFormatId, ClipItem, ClipboardOffer};
use windows::Win32::Graphics::Gdi::{GetEnhMetaFileBits, SetEnhMetaFileBits, HENHMETAFILE};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardFormatNameW, GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW,
    RemoveClipboardFormatListener, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GHND};
use windows::Win32::System::Ole::{
    CF_BITMAP, CF_DIB, CF_DIBV5, CF_DSPENHMETAFILE, CF_DSPTEXT, CF_ENHMETAFILE, CF_GDIOBJLAST, CF_HDROP, CF_METAFILEPICT, CF_OEMTEXT, CF_OWNERDISPLAY, CF_PALETTE, CF_PRIVATEFIRST, CF_TEXT, CF_UNICODETEXT,
};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, GetWindowLongPtrW, KillTimer, PostMessageW, PostQuitMessage, RegisterClassW, SetTimer, SetWindowLongPtrW, GWLP_USERDATA, HWND_MESSAGE, MSG,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLIPBOARDUPDATE, WM_CLOSE, WM_DESTROY, WM_TIMER, WNDCLASSW,
};

/// Acima disso uma cópia não é sincronizada (imagem 4K sem compressão ~33 MB).
pub const MAX_CLIP_BYTES: usize = 16 * 1024 * 1024;

/// O que o clipboard tem agora (resumo pra oferta).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardContent {
    Empty,
    Text(String),
    Files(Vec<std::path::PathBuf>),
}

/// Uma cópia: os formatos que viajam, com o conteúdo de cada um.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClipSnapshot {
    pub items: Vec<ClipItem>,
    /// Arquivos/pastas copiados aqui (`CF_HDROP`). Só os nomes vão na oferta;
    /// o conteúdo é servido por [`crate::files::serve`] quando o outro cola.
    pub files: Vec<std::path::PathBuf>,
}

impl ClipSnapshot {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.files.is_empty()
    }

    /// Tamanho em bytes. Pura.
    pub fn size(&self) -> usize {
        self.items.iter().map(|i| i.data.len()).sum()
    }

    /// Tem algo e cabe no limite de sincronização. Pura.
    pub fn fits(&self) -> bool {
        !self.is_empty() && self.size() <= MAX_CLIP_BYTES
    }

    /// Só texto (testes, probe).
    pub fn text(text: &str) -> Self {
        let data = text.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
        Self { items: vec![ClipItem { format: ClipFormatId::Standard(CF_UNICODETEXT.0 as u32), data }], files: Vec::new() }
    }

    /// O texto Unicode da cópia, se houver.
    pub fn as_text(&self) -> Option<String> {
        self.items.iter().find(|i| i.format == ClipFormatId::Standard(CF_UNICODETEXT.0 as u32)).map(|i| utf16_text(&i.data))
    }

    /// Vira a oferta pro outro lado. Pura.
    pub fn to_offer(&self, seq: u64) -> ClipboardOffer {
        let files = self.files.iter().filter_map(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
        ClipboardOffer { seq, items: self.items.clone(), files }
    }

    /// O que veio do outro lado. Pura.
    pub fn from_offer(offer: &ClipboardOffer) -> Self {
        Self { items: offer.items.clone(), files: Vec::new() }
    }
}

/// O que fazer com um formato do clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carry {
    /// Memória global: vai como bytes.
    Bytes,
    /// `CF_ENHMETAFILE` (desenho do Office): handle GDI, vai serializado.
    EnhMetafile,
    Skip,
}

/// Formatos registrados que não fazem sentido em outra máquina: objetos OLE
/// e referências do Explorer/arrastar (arquivos vão pela §10.8).
const LOCAL_ONLY: [&str; 14] = [
    "DataObject",
    "Ole Private Data",
    "Shell IDList Array",
    "Shell Object Offsets",
    "FileName",
    "FileNameW",
    "FileGroupDescriptor",
    "FileGroupDescriptorW",
    "FileContents",
    "Preferred DropEffect",
    "DropDescription",
    "DragImageBits",
    "DragContext",
    "UsingDefaultDragImage",
];

/// Decide o que viaja de um formato (`format`, `name` se registrado) sabendo
/// quais outros estão presentes. Pura.
pub fn carry(format: u32, name: Option<&str>, present: &[u32]) -> Carry {
    let has = |f: windows::Win32::System::Ole::CLIPBOARD_FORMAT| present.contains(&(f.0 as u32));
    if let Some(n) = name {
        return if LOCAL_ONLY.iter().any(|l| l.eq_ignore_ascii_case(n)) { Carry::Skip } else { Carry::Bytes };
    }
    match format {
        // Handles GDI, desenho pelo dono, privados do programa.
        f if f == CF_BITMAP.0 as u32 || f == CF_PALETTE.0 as u32 || f == CF_METAFILEPICT.0 as u32 || f == CF_OWNERDISPLAY.0 as u32 => Carry::Skip,
        f if (CF_DSPTEXT.0 as u32..=CF_DSPENHMETAFILE.0 as u32).contains(&f) => Carry::Skip,
        f if (CF_PRIVATEFIRST.0 as u32..=CF_GDIOBJLAST.0 as u32).contains(&f) => Carry::Skip,
        f if f == CF_HDROP.0 as u32 => Carry::Skip,
        // O Windows recria do outro lado a partir do Unicode / do DIBV5.
        f if (f == CF_TEXT.0 as u32 || f == CF_OEMTEXT.0 as u32) && has(CF_UNICODETEXT) => Carry::Skip,
        f if f == CF_DIB.0 as u32 && has(CF_DIBV5) => Carry::Skip,
        f if f == CF_ENHMETAFILE.0 as u32 => Carry::EnhMetafile,
        _ => Carry::Bytes,
    }
}

/// Nome de um formato registrado (≥ 0xC000); os padrão não têm.
fn format_name(format: u32) -> Option<String> {
    if format < 0xC000 {
        return None;
    }
    let mut buf = [0u16; 256];
    let n = unsafe { GetClipboardFormatNameW(format, &mut buf) };
    (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
}

/// Número de sequência atual (muda a cada escrita, de quem quer que seja).
pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

struct ClipboardGuard;

impl ClipboardGuard {
    fn open(owner: Option<HWND>) -> Option<Self> {
        // Algumas apps seguram o clipboard por um instante; tenta algumas vezes.
        for _ in 0..10 {
            if unsafe { OpenClipboard(owner) }.is_ok() {
                return Some(Self);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        None
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// Bytes de um handle de memória global do clipboard.
unsafe fn global_bytes(h: HANDLE) -> Option<Vec<u8>> {
    let g = HGLOBAL(h.0);
    let ptr = GlobalLock(g) as *const u8;
    if ptr.is_null() {
        return None;
    }
    let bytes = std::slice::from_raw_parts(ptr, GlobalSize(g)).to_vec();
    let _ = GlobalUnlock(g);
    Some(bytes)
}

/// Texto UTF-16 (até o primeiro zero) em bytes → String. Pura.
pub fn utf16_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// Lê todos os formatos que viajam ([`carry`]), até [`MAX_CLIP_BYTES`] no
/// total (um formato que estoura fica de fora; os outros vão).
pub fn read_snapshot() -> ClipSnapshot {
    let Some(_g) = ClipboardGuard::open(None) else { return ClipSnapshot::default() };
    let mut present = Vec::new();
    let mut f = 0;
    loop {
        f = unsafe { EnumClipboardFormats(f) };
        if f == 0 {
            break;
        }
        present.push(f);
    }
    let mut items = Vec::new();
    let mut total = 0usize;
    for &format in &present {
        let name = format_name(format);
        let data = match carry(format, name.as_deref(), &present) {
            Carry::Skip => continue,
            Carry::Bytes => unsafe { GetClipboardData(format).ok().and_then(|h| global_bytes(h)) },
            Carry::EnhMetafile => unsafe { GetClipboardData(format).ok().and_then(|h| emf_bytes(HENHMETAFILE(h.0))) },
        };
        let Some(data) = data else { continue };
        if total + data.len() > MAX_CLIP_BYTES {
            log::info!("clipboard: formato {} ({} bytes) passa do limite, fica de fora", name.as_deref().unwrap_or(&format.to_string()), data.len());
            continue;
        }
        total += data.len();
        let format = match name {
            Some(n) => ClipFormatId::Named(n),
            None => ClipFormatId::Standard(format),
        };
        items.push(ClipItem { format, data });
    }
    let files = if present.contains(&(CF_HDROP.0 as u32)) { unsafe { hdrop_paths() } } else { Vec::new() };
    ClipSnapshot { items, files }
}

/// Caminhos de `CF_HDROP` (clipboard já aberto).
unsafe fn hdrop_paths() -> Vec<std::path::PathBuf> {
    let Ok(h) = GetClipboardData(CF_HDROP.0 as u32) else { return Vec::new() };
    let hdrop = HDROP(h.0);
    (0..DragQueryFileW(hdrop, u32::MAX, None))
        .map(|i| {
            let len = DragQueryFileW(hdrop, i, None);
            let mut buf = vec![0u16; len as usize + 1];
            let got = DragQueryFileW(hdrop, i, Some(&mut buf));
            std::path::PathBuf::from(crate::from_wide(&buf[..got as usize]))
        })
        .collect()
}

/// Bytes de um metafile (pra recriar do outro lado).
unsafe fn emf_bytes(h: HENHMETAFILE) -> Option<Vec<u8>> {
    let n = GetEnhMetaFileBits(h, None);
    if n == 0 {
        return None;
    }
    let mut buf = vec![0u8; n as usize];
    (GetEnhMetaFileBits(h, Some(&mut buf)) == n).then_some(buf)
}

/// Lê texto e lista de arquivos do clipboard.
pub fn read() -> ClipboardContent {
    let Some(_g) = ClipboardGuard::open(None) else { return ClipboardContent::Empty };
    unsafe {
        let files = hdrop_paths();
        if !files.is_empty() {
            return ClipboardContent::Files(files);
        }
        if let Ok(h) = GetClipboardData(CF_UNICODETEXT.0 as u32) {
            if let Some(b) = global_bytes(h) {
                return ClipboardContent::Text(utf16_text(&b));
            }
        }
    }
    ClipboardContent::Empty
}

/// Põe `bytes` no clipboard no formato `format` (já aberto e esvaziado).
unsafe fn set_bytes(format: u32, bytes: &[u8]) -> Option<()> {
    let h = GlobalAlloc(GHND, bytes.len().max(1)).ok()?;
    let dst = GlobalLock(h) as *mut u8;
    if dst.is_null() {
        return None;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
    let _ = GlobalUnlock(h);
    SetClipboardData(format, Some(HANDLE(h.0))).ok().map(|_| ())
}

/// Escreve todos os formatos da cópia (substitui o que havia). Devolve o
/// número de sequência resultante (pra ignorar o próprio eco).
pub fn write_snapshot(owner: HWND, snap: &ClipSnapshot) -> Option<u32> {
    {
        let _g = ClipboardGuard::open(Some(owner))?;
        write_items(snap)?;
    }
    // Só depois de fechar: é no CloseClipboard que o número muda (e o Windows
    // acrescenta CF_LOCALE). Lido antes, o "sem eco" falhava e a cópia voltava.
    Some(sequence())
}

/// Esvazia o clipboard (já aberto) e põe todos os formatos.
fn write_items(snap: &ClipSnapshot) -> Option<()> {
    unsafe {
        EmptyClipboard().ok()?;
        for item in &snap.items {
            let format = match &item.format {
                ClipFormatId::Standard(n) => *n,
                ClipFormatId::Named(name) => {
                    let w = crate::wide(name);
                    RegisterClipboardFormatW(PCWSTR(w.as_ptr()))
                }
            };
            if format == 0 {
                continue;
            }
            if format == CF_ENHMETAFILE.0 as u32 {
                let emf = SetEnhMetaFileBits(&item.data);
                if !emf.is_invalid() {
                    // O clipboard fica dono do metafile.
                    let _ = SetClipboardData(format, Some(HANDLE(emf.0)));
                }
            } else if set_bytes(format, &item.data).is_none() {
                log::warn!("clipboard: não deu pra pôr o formato {:?}", item.format);
            }
        }
    }
    Some(())
}

/// Escreve texto no clipboard. Devolve o número de sequência resultante.
pub fn write_text(owner: HWND, text: &str) -> Option<u32> {
    write_snapshot(owner, &ClipSnapshot::text(text))
}

/// Os formatos da cópia pelos números desta máquina (o metafile fica de fora:
/// no objeto COM ele precisaria de outro meio que não memória global).
pub(crate) fn local_items(snap: &ClipSnapshot) -> Vec<(u16, Vec<u8>)> {
    snap.items
        .iter()
        .filter_map(|i| {
            let cf = match &i.format {
                ClipFormatId::Standard(n) if *n == CF_ENHMETAFILE.0 as u32 => return None,
                ClipFormatId::Standard(n) => *n,
                ClipFormatId::Named(name) => {
                    let w = crate::wide(name);
                    unsafe { RegisterClipboardFormatW(PCWSTR(w.as_ptr())) }
                }
            };
            (cf != 0).then(|| (cf as u16, i.data.clone()))
        })
        .collect()
}

/// Decide se uma mudança no clipboard é "nossa" (pra não ecoar). Pura.
pub fn is_own_change(current_seq: u32, last_written_seq: Option<u32>) -> bool {
    last_written_seq == Some(current_seq)
}

/// Observa o clipboard numa thread própria (janela oculta só de mensagens):
/// chama `on_change` a cada cópia de outro programa (e uma vez no início, com o
/// que já estava copiado) e escreve o que chegar por [`ClipboardWatcher::set`]
/// sem ecoar de volta.
pub struct ClipboardWatcher {
    hwnd: isize,
    tx: Sender<Pending>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct WatcherState {
    on_change: Box<dyn Fn(ClipSnapshot) + Send>,
    rx: Receiver<Pending>,
    last_written: Option<u32>,
    /// O objeto de arquivos virtuais que pusemos no clipboard (sai no fim).
    virtual_files: Option<windows::Win32::System::Com::IDataObject>,
    /// A última cópia avisada: há programas que mexem no clipboard duas vezes
    /// por cópia (o `Set-Clipboard`, p.ex.) e a 2ª invalidaria a 1ª do outro lado.
    last_sent: Option<ClipSnapshot>,
}

impl WatcherState {
    fn report(&mut self, snap: ClipSnapshot) {
        if self.last_sent.as_ref() == Some(&snap) {
            return;
        }
        self.last_sent = Some(snap.clone());
        (self.on_change)(snap);
    }
}

/// O que escrever: a cópia do outro lado, com ou sem arquivos.
enum Pending {
    Plain(ClipSnapshot),
    WithFiles(ClipSnapshot, std::sync::Arc<dyn crate::vfiles::VirtualFiles>),
}

/// Mensagem "chegou algo pra escrever".
const WM_WRITE: u32 = WM_APP + 1;

/// Timer da leitura adiada (ver `WM_CLIPBOARDUPDATE`).
const READ_TIMER: usize = 1;
const READ_DELAY_MS: u32 = 150;

impl ClipboardWatcher {
    pub fn spawn(on_change: impl Fn(ClipSnapshot) + Send + 'static) -> Option<Self> {
        let (tx, rx) = channel();
        let (ready_tx, ready_rx) = channel::<isize>();
        let thread = std::thread::Builder::new()
            .name("reflex-clipboard".into())
            .spawn(move || unsafe {
                // STA: o Explorer chama o objeto de arquivos virtuais nesta thread.
                let _ = windows::Win32::System::Ole::OleInitialize(None);
                let hinst = GetModuleHandleW(None).unwrap_or_default();
                let class = w!("ReflexClipboard");
                let wc = WNDCLASSW { lpfnWndProc: Some(watcher_proc), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
                RegisterClassW(&wc); // já registrada (2ª sessão) não é erro
                let state = Box::into_raw(Box::new(WatcherState { on_change: Box::new(on_change), rx, last_written: None, virtual_files: None, last_sent: None }));
                let hwnd = CreateWindowExW(WINDOW_EX_STYLE::default(), class, PCWSTR::null(), WINDOW_STYLE::default(), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(hinst.into()), None);
                let Ok(hwnd) = hwnd else {
                    drop(Box::from_raw(state));
                    let _ = ready_tx.send(0);
                    return;
                };
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
                let _ = AddClipboardFormatListener(hwnd);
                let _ = ready_tx.send(hwnd.0 as isize);
                // O que já estava copiado antes da conexão também vale.
                let now = read_snapshot();
                if now.fits() {
                    (*state).report(now);
                }
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    DispatchMessageW(&msg);
                }
                let state = Box::from_raw(state);
                // Arquivos virtuais nossos ainda no clipboard: a conexão acabou, tira
                // (colar depois daria erro de rede).
                if let Some(obj) = &state.virtual_files {
                    if windows::Win32::System::Ole::OleIsCurrentClipboard(obj).is_ok() {
                        let _ = windows::Win32::System::Ole::OleSetClipboard(None);
                    }
                }
                drop(state);
                windows::Win32::System::Ole::OleUninitialize();
            })
            .ok()?;
        match ready_rx.recv() {
            Ok(hwnd) if hwnd != 0 => Some(Self { hwnd, tx, thread: Some(thread) }),
            _ => {
                let _ = thread.join();
                None
            }
        }
    }

    /// Escreve no clipboard (o que veio do outro lado).
    pub fn set(&self, snap: ClipSnapshot) {
        self.send(Pending::Plain(snap));
    }

    /// Escreve a cópia do outro lado com arquivos: colar no Explorer copia
    /// de `files` com a janela de progresso do Windows.
    pub fn set_with_files(&self, snap: ClipSnapshot, files: std::sync::Arc<dyn crate::vfiles::VirtualFiles>) {
        self.send(Pending::WithFiles(snap, files));
    }

    fn send(&self, p: Pending) {
        if self.tx.send(p).is_ok() {
            unsafe {
                let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_WRITE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

impl Drop for ClipboardWatcher {
    fn drop(&mut self) {
        unsafe {
            let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

unsafe extern "system" fn watcher_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WatcherState;
    match msg {
        // Lê só um instante depois da última mudança: há programas que gravam
        // em etapas (Office, .NET: OleSetClipboard e depois OleFlushClipboard)
        // e falham se o clipboard estiver aberto por outro no meio.
        WM_CLIPBOARDUPDATE if !state.is_null() => {
            SetTimer(Some(hwnd), READ_TIMER, READ_DELAY_MS, None);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == READ_TIMER && !state.is_null() => {
            let _ = KillTimer(Some(hwnd), READ_TIMER);
            let s = &mut *state;
            if !is_own_change(sequence(), s.last_written) {
                let snap = read_snapshot();
                if snap.fits() {
                    s.report(snap);
                } else if !snap.is_empty() {
                    log::info!("clipboard com {} bytes: acima do limite, não sincronizado", snap.size());
                }
            }
            LRESULT(0)
        }
        WM_WRITE if !state.is_null() => {
            let s = &mut *state;
            while let Ok(p) = s.rx.try_recv() {
                // Copiar de novo aqui o que foi avisado antes já não é repetição.
                s.last_sent = None;
                let seq = match p {
                    Pending::Plain(snap) => {
                        s.virtual_files = None;
                        write_snapshot(hwnd, &snap)
                    }
                    Pending::WithFiles(snap, files) => {
                        let obj = crate::vfiles::data_object(local_items(&snap), files);
                        match windows::Win32::System::Ole::OleSetClipboard(&obj) {
                            Ok(()) => {
                                s.virtual_files = Some(obj);
                                Some(sequence())
                            }
                            Err(e) => {
                                log::warn!("clipboard: arquivos virtuais: {e}");
                                None
                            }
                        }
                    }
                };
                s.last_written = seq.or(s.last_written);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = RemoveClipboardFormatListener(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_change_is_detected_by_sequence() {
        assert!(is_own_change(5, Some(5)));
        assert!(!is_own_change(6, Some(5)));
        assert!(!is_own_change(5, None));
    }

    #[test]
    fn utf16_text_stops_at_the_terminator() {
        let mut b: Vec<u8> = "olá".encode_utf16().flat_map(u16::to_le_bytes).collect();
        b.extend([0, 0, b'x', 0]);
        assert_eq!(utf16_text(&b), "olá");
    }

    #[test]
    fn snapshot_roundtrips_through_the_offer() {
        let mut snap = ClipSnapshot::text("olá");
        snap.items.push(ClipItem { format: ClipFormatId::Named("HTML Format".into()), data: b"<b>ol\xc3\xa1</b>".to_vec() });
        let offer = snap.to_offer(7);
        assert_eq!(offer.seq, 7);
        assert_eq!(ClipSnapshot::from_offer(&offer), snap);
        assert_eq!(snap.as_text().as_deref(), Some("olá"));
    }

    #[test]
    fn custom_and_rich_formats_travel_local_ones_do_not() {
        let present = [13, 1, 7, 16, 0xC00F, 0xC010];
        assert_eq!(carry(0xC00F, Some("HTML Format"), &present), Carry::Bytes);
        assert_eq!(carry(0xC010, Some("Biff12"), &present), Carry::Bytes);
        assert_eq!(carry(0xC011, Some("Ole Private Data"), &present), Carry::Skip);
        assert_eq!(carry(0xC012, Some("FileGroupDescriptorW"), &present), Carry::Skip);
        // Unicode vai; ANSI/OEM o Windows recria.
        assert_eq!(carry(13, None, &present), Carry::Bytes);
        assert_eq!(carry(1, None, &present), Carry::Skip);
        assert_eq!(carry(7, None, &present), Carry::Skip);
        // Só ANSI (programa antigo): vai.
        assert_eq!(carry(1, None, &[1]), Carry::Bytes);
        assert_eq!(carry(16, None, &present), Carry::Bytes); // CF_LOCALE
    }

    #[test]
    fn images_and_gdi_handles() {
        assert_eq!(carry(17, None, &[8, 17, 2]), Carry::Bytes); // DIBV5
        assert_eq!(carry(8, None, &[8, 17, 2]), Carry::Skip); // DIB recriado do DIBV5
        assert_eq!(carry(8, None, &[8, 2]), Carry::Bytes);
        assert_eq!(carry(2, None, &[8, 2]), Carry::Skip); // HBITMAP
        assert_eq!(carry(14, None, &[14, 3]), Carry::EnhMetafile);
        assert_eq!(carry(3, None, &[14, 3]), Carry::Skip);
        assert_eq!(carry(15, None, &[15]), Carry::Skip); // arquivos: §10.8
        assert_eq!(carry(0x200, None, &[0x200]), Carry::Skip); // privado
    }

    #[test]
    fn snapshot_size_limit() {
        assert!(!ClipSnapshot::default().fits());
        assert!(ClipSnapshot::text("a").fits());
        let big = ClipSnapshot { items: vec![ClipItem { format: ClipFormatId::Standard(8), data: vec![0; MAX_CLIP_BYTES + 1] }], files: Vec::new() };
        assert!(!big.fits());
    }

    #[test]
    #[ignore = "mexe no clipboard real da máquina"]
    fn watcher_does_not_echo_what_it_writes() {
        use std::sync::{Arc, Mutex};
        let before = read_snapshot();
        let seen: Arc<Mutex<Vec<ClipSnapshot>>> = Arc::default();
        let log = seen.clone();
        let w = ClipboardWatcher::spawn(move |s| log.lock().unwrap().push(s)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let initial = seen.lock().unwrap().len(); // o que já estava copiado
        w.set(ClipSnapshot::text("reflex-sem-eco"));
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert_eq!(seen.lock().unwrap().len(), initial, "a própria escrita voltou como cópia");
        assert_eq!(read_snapshot().as_text().as_deref(), Some("reflex-sem-eco"));
        drop(w);
        if !before.is_empty() {
            write_snapshot(HWND::default(), &before);
        }
    }

    #[test]
    #[ignore = "mexe no clipboard real da máquina"]
    fn text_roundtrip() {
        let hwnd = HWND::default();
        let before = read();
        write_text(hwnd, "reflex-clip-teste").unwrap();
        assert_eq!(read(), ClipboardContent::Text("reflex-clip-teste".into()));
        // Restaura texto anterior se havia.
        if let ClipboardContent::Text(t) = before {
            write_text(hwnd, &t);
        }
    }
}
