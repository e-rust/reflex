//! Arquivos virtuais pra colar (§10.8.1). Quando o viewer (ou o host) recebe
//! uma oferta de arquivos, põe no clipboard um `IDataObject` com:
//! - `CFSTR_FILEDESCRIPTORW`: a lista (recortada aqui em bytes — [`file_group_descriptor`]);
//! - `CFSTR_FILECONTENTS`: um `IStream` por arquivo, que lê da rede sob demanda.
//!
//! O layout do `FILEGROUPDESCRIPTORW` é montado e testado sem COM
//! ([`file_group_descriptor`]). [`VirtualDataObject`] é o objeto COM: o
//! Explorer pede a lista e um `IStream` por arquivo e copia **com a janela de
//! progresso do próprio Windows** (`FD_PROGRESSUI` + modo assíncrono, numa
//! thread dele). De onde vêm os bytes é [`VirtualFiles`] — a rede
//! ([`crate::files::RemoteFiles`]) ou memória, nos testes.

use std::io::{self, Read};
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use reflex_proto::msg::FileEntry;
use windows::core::{implement, IUnknown, Ref, BOOL, HRESULT, PCWSTR};
use windows::Win32::Foundation::{DATA_S_SAMEFORMATETC, DV_E_FORMATETC, E_FAIL, E_NOTIMPL, FILETIME, HGLOBAL, OLE_E_ADVISENOTSUPPORTED, STG_E_ACCESSDENIED, S_FALSE, S_OK};
use windows::Win32::System::Com::{
    IAdviseSink, IBindCtx, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, ISequentialStream_Impl, IStream, IStream_Impl, DATADIR_GET, DVASPECT_CONTENT, FORMATETC, LOCKTYPE, STATFLAG, STATSTG,
    STGC, STGMEDIUM, STGMEDIUM_0, STGTY_STREAM, STREAM_SEEK, STREAM_SEEK_CUR, STREAM_SEEK_END, STREAM_SEEK_SET, TYMED_HGLOBAL, TYMED_ISTREAM,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GHND};
use windows::Win32::System::Ole::ReleaseStgMedium;
use windows::Win32::UI::Shell::{IDataObjectAsyncCapability, IDataObjectAsyncCapability_Impl, SHCreateStdEnumFmtEtc, CFSTR_FILECONTENTS, CFSTR_FILEDESCRIPTORW};

/// Flags de `FILEDESCRIPTORW.dwFlags` que vamos preencher.
const FD_ATTRIBUTES: u32 = 0x04;
const FD_FILESIZE: u32 = 0x40;
const FD_WRITESTIME: u32 = 0x20;
const FD_PROGRESSUI: u32 = 0x4000;
const FD_UNICODE: u32 = 0x80000000;

/// Tamanho de `FILEDESCRIPTORW` (Win64): dwFlags, CLSID, SIZEL, POINTL,
/// FILETIME×3, 2×DWORD size, cFileName[260] UTF-16.
const FILEDESCRIPTORW_SIZE: usize = 592;
const CFILENAME_OFFSET: usize = 72;
/// Sem isso no lugar certo o Explorer trata pasta como arquivo (e pede o "conteúdo" dela).
const ATTRIBUTES_OFFSET: usize = 36;
const CFILENAME_CHARS: usize = 260;

/// Monta os bytes de um `FILEGROUPDESCRIPTORW`: `u32` com a contagem, seguido
/// de um `FILEDESCRIPTORW` por entrada.
pub fn file_group_descriptor(tree: &[FileEntry]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + tree.len() * FILEDESCRIPTORW_SIZE);
    out.extend_from_slice(&(tree.len() as u32).to_le_bytes());
    for e in tree {
        let start = out.len();
        out.resize(start + FILEDESCRIPTORW_SIZE, 0);
        let desc = &mut out[start..start + FILEDESCRIPTORW_SIZE];

        let mut flags = FD_UNICODE | FD_ATTRIBUTES | FD_PROGRESSUI;
        if !e.is_dir {
            flags |= FD_FILESIZE;
        }
        if e.modified != 0 {
            flags |= FD_WRITESTIME;
        }
        desc[0..4].copy_from_slice(&flags.to_le_bytes());
        // dwFileAttributes depois de dwFlags(4) + CLSID(16) + SIZEL(8) + POINTL(8).
        desc[ATTRIBUTES_OFFSET..ATTRIBUTES_OFFSET + 4].copy_from_slice(&e.attributes.to_le_bytes());
        // ftLastWriteTime em 56..64 (dwFlags 4, CLSID 16, SIZEL 8, POINTL 8, 3 FILETIME de 8 = 56).
        desc[56..64].copy_from_slice(&e.modified.to_le_bytes());
        // nFileSizeHigh/Low em 64..72.
        desc[64..68].copy_from_slice(&((e.size >> 32) as u32).to_le_bytes());
        desc[68..72].copy_from_slice(&((e.size & 0xFFFF_FFFF) as u32).to_le_bytes());

        let name: Vec<u16> = e.rel_path.encode_utf16().take(CFILENAME_CHARS - 1).collect();
        for (i, c) in name.iter().enumerate() {
            let o = CFILENAME_OFFSET + i * 2;
            desc[o..o + 2].copy_from_slice(&c.to_le_bytes());
        }
    }
    out
}

/// Índices das entradas que são arquivos (os únicos pedidos por `FILECONTENTS`).
pub fn file_indices(tree: &[FileEntry]) -> Vec<usize> {
    tree.iter().enumerate().filter(|(_, e)| !e.is_dir).map(|(i, _)| i).collect()
}

/// De onde vêm os arquivos colados (a rede, na prática).
pub trait VirtualFiles: Send + Sync {
    /// A árvore inteira (pastas antes do conteúdo).
    fn tree(&self) -> io::Result<Vec<FileEntry>>;
    /// O arquivo `index` da árvore a partir de `offset`.
    fn open(&self, index: u32, offset: u64) -> io::Result<Box<dyn Read + Send>>;
}

impl VirtualFiles for crate::files::RemoteFiles {
    fn tree(&self) -> io::Result<Vec<FileEntry>> {
        crate::files::RemoteFiles::tree(self)
    }

    fn open(&self, index: u32, offset: u64) -> io::Result<Box<dyn Read + Send>> {
        crate::files::RemoteFiles::open(self, index, offset).map(|(_, r)| Box::new(r) as Box<dyn Read + Send>)
    }
}

fn register(name: PCWSTR) -> u16 {
    unsafe { RegisterClipboardFormatW(name) as u16 }
}

/// Bytes numa memória global (o que o clipboard/COM transporta).
fn to_hglobal(bytes: &[u8]) -> windows::core::Result<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GHND, bytes.len().max(1))?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            return Err(E_FAIL.into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        Ok(h)
    }
}

fn hglobal_medium(bytes: &[u8]) -> windows::core::Result<STGMEDIUM> {
    Ok(STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: to_hglobal(bytes)? }, pUnkForRelease: ManuallyDrop::new(None) })
}

/// O `IDataObject` dos arquivos colados: lista, conteúdo de cada arquivo e
/// os outros formatos da mesma cópia (texto etc., pra colar em editores).
#[implement(IDataObject, IDataObjectAsyncCapability)]
pub struct VirtualDataObject {
    /// Outros formatos da cópia, por número local.
    items: Vec<(u16, Vec<u8>)>,
    files: Arc<dyn VirtualFiles>,
    tree: Mutex<Option<Vec<FileEntry>>>,
    cf_descriptor: u16,
    cf_contents: u16,
    /// O que o Explorer grava de volta ("Performed DropEffect" e afins).
    extra: Mutex<Vec<(u16, Vec<u8>)>>,
    async_mode: AtomicBool,
    in_operation: AtomicBool,
}

impl VirtualDataObject {
    pub fn new(items: Vec<(u16, Vec<u8>)>, files: Arc<dyn VirtualFiles>) -> Self {
        Self::with_async(items, files, true)
    }

    /// `asynchronous = false`: quem cola copia na hora, na thread que chamou
    /// (testes; o Explorer usa o modo assíncrono).
    pub fn with_async(items: Vec<(u16, Vec<u8>)>, files: Arc<dyn VirtualFiles>, asynchronous: bool) -> Self {
        Self {
            items,
            files,
            tree: Mutex::new(None),
            cf_descriptor: register(CFSTR_FILEDESCRIPTORW),
            cf_contents: register(CFSTR_FILECONTENTS),
            extra: Mutex::new(Vec::new()),
            async_mode: AtomicBool::new(asynchronous),
            in_operation: AtomicBool::new(false),
        }
    }

    /// A árvore, buscada uma vez (a 1ª vez que o Explorer pede a lista).
    fn tree(&self) -> windows::core::Result<Vec<FileEntry>> {
        let mut t = self.tree.lock().map_err(|_| windows::core::Error::from(E_FAIL))?;
        if t.is_none() {
            *t = Some(self.files.tree().map_err(|e| {
                log::warn!("arquivos colados: lista: {e}");
                windows::core::Error::from(E_FAIL)
            })?);
        }
        Ok(t.clone().unwrap_or_default())
    }

    fn formats(&self) -> Vec<FORMATETC> {
        let fmt = |cf: u16, tymed: i32, lindex: i32| FORMATETC { cfFormat: cf, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex, tymed: tymed as u32 };
        let mut all = vec![fmt(self.cf_descriptor, TYMED_HGLOBAL.0, -1), fmt(self.cf_contents, TYMED_ISTREAM.0, -1)];
        all.extend(self.items.iter().map(|(cf, _)| fmt(*cf, TYMED_HGLOBAL.0, -1)));
        all
    }

    fn item(&self, cf: u16) -> Option<Vec<u8>> {
        let own = self.items.iter().find(|(c, _)| *c == cf).map(|(_, d)| d.clone());
        own.or_else(|| self.extra.lock().ok()?.iter().find(|(c, _)| *c == cf).map(|(_, d)| d.clone()))
    }
}

// Os ponteiros vêm do COM (contrato da interface) e são conferidos antes de usar.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
impl IDataObject_Impl for VirtualDataObject_Impl {
    fn GetData(&self, pformatetcin: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        let f = unsafe { pformatetcin.as_ref() }.ok_or(windows::core::Error::from(DV_E_FORMATETC))?;
        if f.cfFormat == self.cf_descriptor && f.tymed & TYMED_HGLOBAL.0 as u32 != 0 {
            return hglobal_medium(&file_group_descriptor(&self.tree()?));
        }
        if f.cfFormat == self.cf_contents && f.tymed & TYMED_ISTREAM.0 as u32 != 0 {
            let tree = self.tree()?;
            let entry = usize::try_from(f.lindex).ok().and_then(|i| tree.get(i)).filter(|e| !e.is_dir).ok_or(windows::core::Error::from(DV_E_FORMATETC))?;
            let stream: IStream = VirtualStream::new(self.files.clone(), f.lindex as u32, entry.size, &entry.rel_path).into();
            return Ok(STGMEDIUM { tymed: TYMED_ISTREAM.0 as u32, u: STGMEDIUM_0 { pstm: ManuallyDrop::new(Some(stream)) }, pUnkForRelease: ManuallyDrop::new(None) });
        }
        match self.item(f.cfFormat) {
            Some(bytes) if f.tymed & TYMED_HGLOBAL.0 as u32 != 0 => hglobal_medium(&bytes),
            _ => Err(DV_E_FORMATETC.into()),
        }
    }

    fn GetDataHere(&self, _pformatetc: *const FORMATETC, _pmedium: *mut STGMEDIUM) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> HRESULT {
        let Some(f) = (unsafe { pformatetc.as_ref() }) else { return DV_E_FORMATETC };
        let known = self.formats().iter().any(|k| k.cfFormat == f.cfFormat && k.tymed & f.tymed != 0) || self.item(f.cfFormat).is_some();
        if known {
            S_OK
        } else {
            DV_E_FORMATETC
        }
    }

    fn GetCanonicalFormatEtc(&self, _pformatectin: *const FORMATETC, pformatetcout: *mut FORMATETC) -> HRESULT {
        if let Some(out) = unsafe { pformatetcout.as_mut() } {
            out.ptd = std::ptr::null_mut();
        }
        DATA_S_SAMEFORMATETC
    }

    fn SetData(&self, pformatetc: *const FORMATETC, pmedium: *const STGMEDIUM, frelease: BOOL) -> windows::core::Result<()> {
        // O Explorer avisa o resultado ("Performed DropEffect", "Paste
        // Succeeded"…): guarda o que vier em memória global.
        unsafe {
            if let (Some(f), Some(m)) = (pformatetc.as_ref(), pmedium.as_ref()) {
                if m.tymed == TYMED_HGLOBAL.0 as u32 {
                    let h = m.u.hGlobal;
                    let p = GlobalLock(h) as *const u8;
                    if !p.is_null() {
                        let bytes = std::slice::from_raw_parts(p, GlobalSize(h)).to_vec();
                        let _ = GlobalUnlock(h);
                        if let Ok(mut extra) = self.extra.lock() {
                            extra.retain(|(c, _)| *c != f.cfFormat);
                            extra.push((f.cfFormat, bytes));
                        }
                    }
                }
            }
            if frelease.as_bool() {
                ReleaseStgMedium(pmedium as *mut STGMEDIUM);
            }
        }
        Ok(())
    }

    fn EnumFormatEtc(&self, dwdirection: u32) -> windows::core::Result<IEnumFORMATETC> {
        if dwdirection as i32 != DATADIR_GET.0 {
            return Err(E_NOTIMPL.into());
        }
        unsafe { SHCreateStdEnumFmtEtc(&self.formats()) }
    }

    fn DAdvise(&self, _pformatetc: *const FORMATETC, _advf: u32, _padvsink: Ref<'_, IAdviseSink>) -> windows::core::Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _dwconnection: u32) -> windows::core::Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

/// Modo assíncrono: o Explorer copia numa thread dele (não trava a janela)
/// e mostra o progresso.
impl IDataObjectAsyncCapability_Impl for VirtualDataObject_Impl {
    fn SetAsyncMode(&self, fdoopasync: BOOL) -> windows::core::Result<()> {
        self.async_mode.store(fdoopasync.as_bool(), Ordering::Relaxed);
        Ok(())
    }

    fn GetAsyncMode(&self) -> windows::core::Result<BOOL> {
        Ok(self.async_mode.load(Ordering::Relaxed).into())
    }

    fn StartOperation(&self, _pbcreserved: Ref<'_, IBindCtx>) -> windows::core::Result<()> {
        self.in_operation.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn InOperation(&self) -> windows::core::Result<BOOL> {
        Ok(self.in_operation.load(Ordering::Relaxed).into())
    }

    fn EndOperation(&self, _hresult: HRESULT, _pbcreserved: Ref<'_, IBindCtx>, _dweffects: u32) -> windows::core::Result<()> {
        self.in_operation.store(false, Ordering::Relaxed);
        Ok(())
    }
}

/// O conteúdo de um arquivo colado: lê da origem à medida que o Explorer grava.
#[implement(IStream)]
struct VirtualStream {
    files: Arc<dyn VirtualFiles>,
    index: u32,
    size: u64,
    name: String,
    state: Mutex<StreamState>,
}

#[derive(Default)]
struct StreamState {
    pos: u64,
    /// Aberto na posição `pos`; um Seek pra outro lugar fecha e reabre lá.
    reader: Option<Box<dyn Read + Send>>,
}

impl VirtualStream {
    fn new(files: Arc<dyn VirtualFiles>, index: u32, size: u64, name: &str) -> Self {
        Self { files, index, size, name: name.to_string(), state: Mutex::new(StreamState::default()) }
    }
}

#[allow(clippy::not_unsafe_ptr_arg_deref)]
impl ISequentialStream_Impl for VirtualStream_Impl {
    fn Read(&self, pv: *mut core::ffi::c_void, cb: u32, pcbread: *mut u32) -> HRESULT {
        let Ok(mut st) = self.state.lock() else { return E_FAIL };
        if st.reader.is_none() {
            match self.files.open(self.index, st.pos) {
                Ok(r) => st.reader = Some(r),
                Err(e) => {
                    log::warn!("arquivo colado {}: {e}", self.name);
                    return E_FAIL;
                }
            }
        }
        let buf = unsafe { std::slice::from_raw_parts_mut(pv as *mut u8, cb as usize) };
        let mut got = 0usize;
        while got < buf.len() {
            match st.reader.as_mut().map(|r| r.read(&mut buf[got..])) {
                Some(Ok(0)) | None => break,
                Some(Ok(n)) => got += n,
                Some(Err(e)) => {
                    log::warn!("arquivo colado {}: {e}", self.name);
                    st.reader = None;
                    return E_FAIL;
                }
            }
        }
        st.pos += got as u64;
        if let Some(out) = unsafe { pcbread.as_mut() } {
            *out = got as u32;
        }
        if got == buf.len() {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn Write(&self, _pv: *const core::ffi::c_void, _cb: u32, _pcbwritten: *mut u32) -> HRESULT {
        STG_E_ACCESSDENIED
    }
}

/// Nova posição de um Seek, sem passar do fim nem ficar antes do começo. Pura.
pub fn seek_target(pos: u64, size: u64, origin: u32, delta: i64) -> u64 {
    let base = match origin {
        o if o == STREAM_SEEK_SET.0 => 0i128,
        o if o == STREAM_SEEK_CUR.0 => pos as i128,
        o if o == STREAM_SEEK_END.0 => size as i128,
        _ => pos as i128,
    };
    (base + delta as i128).clamp(0, size as i128) as u64
}

#[allow(clippy::not_unsafe_ptr_arg_deref)]
impl IStream_Impl for VirtualStream_Impl {
    fn Seek(&self, dlibmove: i64, dworigin: STREAM_SEEK, plibnewposition: *mut u64) -> windows::core::Result<()> {
        let mut st = self.state.lock().map_err(|_| windows::core::Error::from(E_FAIL))?;
        let target = seek_target(st.pos, self.size, dworigin.0, dlibmove);
        if target != st.pos {
            st.reader = None;
            st.pos = target;
        }
        if let Some(out) = unsafe { plibnewposition.as_mut() } {
            *out = st.pos;
        }
        Ok(())
    }

    fn SetSize(&self, _libnewsize: u64) -> windows::core::Result<()> {
        Err(STG_E_ACCESSDENIED.into())
    }

    fn CopyTo(&self, _pstm: Ref<'_, IStream>, _cb: u64, _pcbread: *mut u64, _pcbwritten: *mut u64) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Commit(&self, _grfcommitflags: &STGC) -> windows::core::Result<()> {
        Ok(())
    }

    fn Revert(&self) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn LockRegion(&self, _liboffset: u64, _cb: u64, _dwlocktype: &LOCKTYPE) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn UnlockRegion(&self, _liboffset: u64, _cb: u64, _dwlocktype: u32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Stat(&self, pstatstg: *mut STATSTG, _grfstatflag: &STATFLAG) -> windows::core::Result<()> {
        let out = unsafe { pstatstg.as_mut() }.ok_or(windows::core::Error::from(E_FAIL))?;
        *out = STATSTG { r#type: STGTY_STREAM.0 as u32, cbSize: self.size, mtime: FILETIME::default(), ..Default::default() };
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IStream> {
        Err(E_NOTIMPL.into())
    }
}

/// Monta o objeto pra pôr no clipboard (`OleSetClipboard`).
pub fn data_object(items: Vec<(u16, Vec<u8>)>, files: Arc<dyn VirtualFiles>) -> IDataObject {
    VirtualDataObject::new(items, files).into()
}

// Garante que IUnknown está no escopo do `implement` (usado pelo macro).
#[allow(dead_code)]
type _Unknown = IUnknown;

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64) -> FileEntry {
        FileEntry { rel_path: path.into(), is_dir: false, size, modified: 100, attributes: 0x80 }
    }

    fn dir(path: &str) -> FileEntry {
        FileEntry { rel_path: path.into(), is_dir: true, size: 0, modified: 0, attributes: 0x10 }
    }

    #[test]
    fn descriptor_has_count_and_one_record_each() {
        let tree = [dir("f"), file("f\\a.txt", 5)];
        let bytes = file_group_descriptor(&tree);
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 2);
        assert_eq!(bytes.len(), 4 + 2 * FILEDESCRIPTORW_SIZE);
    }

    #[test]
    fn file_record_carries_size_and_name() {
        let bytes = file_group_descriptor(&[file("a.txt", 0x1_0000_0002)]);
        let rec = &bytes[4..];
        let flags = u32::from_le_bytes(rec[0..4].try_into().unwrap());
        assert!(flags & FD_FILESIZE != 0 && flags & FD_UNICODE != 0);
        assert_eq!(u32::from_le_bytes(rec[64..68].try_into().unwrap()), 1); // high
        assert_eq!(u32::from_le_bytes(rec[68..72].try_into().unwrap()), 2); // low
        // Nome em UTF-16 a partir de 72.
        let name: Vec<u16> = (0..5).map(|i| u16::from_le_bytes(rec[72 + i * 2..74 + i * 2].try_into().unwrap())).collect();
        assert_eq!(String::from_utf16_lossy(&name), "a.txt");
    }

    #[test]
    fn attributes_are_where_windows_reads_them() {
        let bytes = file_group_descriptor(&[dir("f")]);
        let rec = &bytes[4..];
        assert_eq!(u32::from_le_bytes(rec[36..40].try_into().unwrap()), 0x10, "FILE_ATTRIBUTE_DIRECTORY");
        assert!(rec[20..28].iter().all(|&b| b == 0), "SIZEL fica zerado");
    }

    #[test]
    fn directory_has_no_filesize_flag() {
        let bytes = file_group_descriptor(&[dir("f")]);
        let flags = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        assert_eq!(flags & FD_FILESIZE, 0);
    }

    #[test]
    fn only_files_get_contents_indices() {
        let tree = [dir("f"), file("f\\a", 1), dir("f\\sub"), file("f\\sub\\b", 2)];
        assert_eq!(file_indices(&tree), vec![1, 3]);
    }

    #[test]
    fn seek_targets_stay_inside_the_file() {
        assert_eq!(seek_target(10, 100, STREAM_SEEK_SET.0, 40), 40);
        assert_eq!(seek_target(10, 100, STREAM_SEEK_CUR.0, 5), 15);
        assert_eq!(seek_target(10, 100, STREAM_SEEK_END.0, -1), 99);
        assert_eq!(seek_target(10, 100, STREAM_SEEK_CUR.0, -50), 0);
        assert_eq!(seek_target(10, 100, STREAM_SEEK_SET.0, 500), 100);
    }

    /// Arquivos em memória no lugar da rede.
    struct MemFiles(Vec<FileEntry>, Vec<Vec<u8>>);

    impl VirtualFiles for MemFiles {
        fn tree(&self) -> io::Result<Vec<FileEntry>> {
            Ok(self.0.clone())
        }

        fn open(&self, index: u32, offset: u64) -> io::Result<Box<dyn Read + Send>> {
            let data = self.1.get(index as usize).cloned().unwrap_or_default();
            Ok(Box::new(io::Cursor::new(data[offset as usize..].to_vec())))
        }
    }

    fn mem_files() -> Arc<MemFiles> {
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
        Arc::new(MemFiles(vec![dir("pasta"), file("pasta\\grande.bin", big.len() as u64), file("pasta\\oi.txt", 3)], vec![Vec::new(), big, b"oi!".to_vec()]))
    }

    #[test]
    #[ignore = "COM/Shell de verdade: cola numa pasta temporária como o Explorer"]
    fn shell_pastes_virtual_files_like_explorer() {
        use windows::Win32::Foundation::POINTL;
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        use windows::Win32::System::Ole::{IDropTarget, DROPEFFECT, DROPEFFECT_COPY};
        use windows::Win32::System::SystemServices::MK_LBUTTON;
        use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName, BHID_SFUIObject};
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let files = mem_files();
            // Síncrono: a cópia termina dentro do Drop (o Explorer usaria o assíncrono).
            let dobj: IDataObject = VirtualDataObject::with_async(Vec::new(), files.clone(), false).into();
            let dest = std::env::temp_dir().join(format!("reflex-colar-{}", std::process::id()));
            std::fs::create_dir_all(&dest).unwrap();
            let dest_w = crate::wide(&dest.to_string_lossy());
            // Colar = entregar o objeto ao alvo de soltura da pasta (o que o Explorer faz).
            let folder: IShellItem = SHCreateItemFromParsingName(PCWSTR(dest_w.as_ptr()), None).unwrap();
            let target: IDropTarget = folder.BindToHandler(None, &BHID_SFUIObject).unwrap();
            let mut effect = DROPEFFECT_COPY;
            target.DragEnter(&dobj, MK_LBUTTON, POINTL::default(), &mut effect).unwrap();
            assert_eq!(effect & DROPEFFECT_COPY, DROPEFFECT_COPY, "a pasta não aceitou copiar");
            let mut effect: DROPEFFECT = DROPEFFECT_COPY;
            target.Drop(&dobj, MK_LBUTTON, POINTL::default(), &mut effect).unwrap();
            assert_eq!(std::fs::read(dest.join("pasta").join("oi.txt")).unwrap(), b"oi!");
            assert_eq!(std::fs::read(dest.join("pasta").join("grande.bin")).unwrap(), files.1[1]);
            std::fs::remove_dir_all(&dest).ok();
        }
    }

    #[test]
    #[ignore = "COM/Shell e o clipboard de verdade"]
    fn pasting_with_the_formats_of_an_explorer_copy() {
        // Copiar no Explorer põe no clipboard formatos do Shell além do
        // CF_HDROP; os que viajam vão junto no objeto de arquivos virtuais do
        // outro lado. Colar tem de continuar funcionando.
        use windows::Win32::Foundation::POINTL;
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        use windows::Win32::System::Ole::{IDropTarget, OleFlushClipboard, OleInitialize, OleSetClipboard, DROPEFFECT, DROPEFFECT_COPY};
        use windows::Win32::System::SystemServices::MK_LBUTTON;
        use windows::Win32::UI::Shell::{BHID_DataObject, BHID_SFUIObject, IShellItem, SHCreateItemFromParsingName};
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let _ = OleInitialize(None);
            let base = std::env::temp_dir().join(format!("reflex-explorer-{}", std::process::id()));
            let (src, dest) = (base.join("origem"), base.join("destino"));
            std::fs::create_dir_all(&src).unwrap();
            std::fs::create_dir_all(&dest).unwrap();
            std::fs::write(src.join("oi.txt"), b"oi!").unwrap();
            let src_w = crate::wide(&src.join("oi.txt").to_string_lossy());
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(src_w.as_ptr()), None).unwrap();
            let shell_obj: IDataObject = item.BindToHandler(None, &BHID_DataObject).unwrap();
            OleSetClipboard(&shell_obj).unwrap();
            OleFlushClipboard().unwrap();
            let snap = crate::clipboard::read_snapshot();
            let names: Vec<String> = snap.items.iter().map(|i| format!("{:?}", i.format)).collect();
            println!("formatos que viajam: {names:?}; arquivos {:?}", snap.files);
            let files = mem_files();
            let dobj: IDataObject = VirtualDataObject::with_async(crate::clipboard::local_items(&snap), files.clone(), false).into();
            let dest_w = crate::wide(&dest.to_string_lossy());
            let folder: IShellItem = SHCreateItemFromParsingName(PCWSTR(dest_w.as_ptr()), None).unwrap();
            let target: IDropTarget = folder.BindToHandler(None, &BHID_SFUIObject).unwrap();
            let mut effect = DROPEFFECT_COPY;
            target.DragEnter(&dobj, MK_LBUTTON, POINTL::default(), &mut effect).unwrap();
            let mut effect: DROPEFFECT = DROPEFFECT_COPY;
            let dropped = target.Drop(&dobj, MK_LBUTTON, POINTL::default(), &mut effect);
            let ok = std::fs::read(dest.join("pasta").join("oi.txt")).ok();
            std::fs::remove_dir_all(&base).ok();
            dropped.unwrap();
            assert_eq!(ok.as_deref(), Some(&b"oi!"[..]), "nada colado");
        }
    }

    #[test]
    #[ignore = "cola o clipboard atual numa pasta temporária (diagnóstico)"]
    fn paste_current_clipboard() {
        use windows::Win32::Foundation::POINTL;
        use windows::Win32::System::Ole::{IDropTarget, OleGetClipboard, OleInitialize, DROPEFFECT, DROPEFFECT_COPY};
        use windows::Win32::System::SystemServices::MK_LBUTTON;
        use windows::Win32::UI::Shell::{BHID_SFUIObject, IShellItem, SHCreateItemFromParsingName};
        unsafe {
            let _ = OleInitialize(None);
            let dest = std::env::temp_dir().join(format!("reflex-colado-{}", std::process::id()));
            std::fs::create_dir_all(&dest).unwrap();
            let dobj = OleGetClipboard().unwrap();
            let dest_w = crate::wide(&dest.to_string_lossy());
            let folder: IShellItem = SHCreateItemFromParsingName(PCWSTR(dest_w.as_ptr()), None).unwrap();
            let target: IDropTarget = folder.BindToHandler(None, &BHID_SFUIObject).unwrap();
            let mut effect = DROPEFFECT_COPY;
            let enter = target.DragEnter(&dobj, MK_LBUTTON, POINTL::default(), &mut effect);
            println!("DragEnter: {enter:?}, efeito {effect:?}");
            let mut effect: DROPEFFECT = DROPEFFECT_COPY;
            let r = target.Drop(&dobj, MK_LBUTTON, POINTL::default(), &mut effect);
            println!("Drop: {r:?}");
            // Assíncrono: a cópia segue numa thread do Shell; bombeia mensagens enquanto isso.
            let t = std::time::Instant::now();
            while t.elapsed() < std::time::Duration::from_secs(20) {
                let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
                while windows::Win32::UI::WindowsAndMessaging::PeekMessageW(&mut msg, None, 0, 0, windows::Win32::UI::WindowsAndMessaging::PM_REMOVE).as_bool() {
                    windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            for e in walk_all(&dest) {
                println!("colado: {e}");
            }
        }
    }

    fn walk_all(dir: &std::path::Path) -> Vec<String> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk_all(&p));
            } else {
                out.push(format!("{} ({} bytes)", p.display(), p.metadata().map(|m| m.len()).unwrap_or(0)));
            }
        }
        out
    }

    #[test]
    fn very_long_names_are_truncated_safely() {
        let long = "x".repeat(400);
        let bytes = file_group_descriptor(&[file(&long, 1)]);
        assert_eq!(bytes.len(), 4 + FILEDESCRIPTORW_SIZE);
    }
}
