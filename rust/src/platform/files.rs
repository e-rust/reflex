//! Árvore e leitura dos arquivos copiados (§10.8). O lado que COPIOU (host via
//! `reflex.exe --user`, ou o viewer) expande a lista do clipboard numa árvore
//! de caminhos relativos e serve os blocos; o lado que COLA recria tudo.
//!
//! A parte de montar a árvore é pura (testável); só `read_tree`/`open` tocam o
//! disco, com as permissões do usuário logado (quem roda este processo).
//!
//! Na rede (§10.8.3), cada pedido abre o próprio stream: [`serve`] responde do
//! lado que copiou; [`RemoteFiles`] pede do lado que cola (um `std::io::Read`
//! por arquivo, que o `IStream` do Explorer consome).

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};

use crate::proto::frame::{read_known, write_msg};
use crate::proto::msg::{FileEntry, Reply, Request, StreamError, FILE_CHUNK};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite};

/// `SystemTime` → FILETIME (100 ns desde 1601).
fn to_filetime(t: std::time::SystemTime) -> u64 {
    const EPOCH_DIFF_100NS: u64 = 116_444_736_000_000_000;
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => EPOCH_DIFF_100NS + d.as_nanos() as u64 / 100,
        Err(_) => EPOCH_DIFF_100NS,
    }
}

/// Monta os `FileEntry` (caminho relativo com `\`) a partir de caminhos
/// absolutos escolhidos. A ordem põe cada pasta antes do seu conteúdo (o
/// Explorer recria assim). `root_name` de cada raiz é o próprio nome do item.
pub fn build_tree(roots: &[PathBuf]) -> std::io::Result<Vec<FileEntry>> {
    let mut out = Vec::new();
    for root in roots {
        let base = root.file_name().map(PathBuf::from).unwrap_or_else(|| root.clone());
        walk(root, &base, &mut out)?;
    }
    Ok(out)
}

fn walk(abs: &Path, rel: &Path, out: &mut Vec<FileEntry>) -> std::io::Result<()> {
    let meta = std::fs::metadata(abs)?;
    let rel_path = rel.to_string_lossy().replace('/', "\\");
    let modified = meta.modified().map(to_filetime).unwrap_or(0);
    if meta.is_dir() {
        out.push(FileEntry { rel_path, is_dir: true, size: 0, modified, attributes: 0x10 });
        let mut entries: Vec<_> = std::fs::read_dir(abs)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            walk(&e.path(), &rel.join(e.file_name()), out)?;
        }
    } else {
        out.push(FileEntry { rel_path, is_dir: false, size: meta.len(), modified, attributes: 0x80 });
    }
    Ok(())
}

/// Resolve o caminho absoluto de uma entrada (índice na árvore de `roots`),
/// garantindo que não escapa das raízes escolhidas (sem `..`).
pub fn resolve(roots: &[PathBuf], tree: &[FileEntry], index: usize) -> Option<PathBuf> {
    let entry = tree.get(index)?;
    let rel = PathBuf::from(entry.rel_path.replace('\\', "/"));
    let first = rel.components().next()?.as_os_str();
    // Achar a raiz cujo nome bate com o 1º componente.
    let root = roots.iter().find(|r| r.file_name() == Some(first))?;
    let rest: PathBuf = rel.components().skip(1).collect();
    // Arquivo copiado sozinho: é a própria raiz. (`join("")` põe uma barra no
    // fim — "logo.png\" — e abrir dá "nome do diretório inválido".)
    let full = if rest.as_os_str().is_empty() { root.clone() } else { root.join(&rest) };
    // Nenhum `..` pode ter sobrado.
    (!entry.rel_path.contains("..")).then_some(full)
}

/// Arquivos que ESTE lado copiou, pro outro colar. A árvore é montada no 1º
/// pedido (pasta grande copiada e nunca colada não custa nada).
#[derive(Debug)]
pub struct FileOffer {
    pub seq: u64,
    pub roots: Vec<PathBuf>,
    tree: OnceLock<Result<Vec<FileEntry>, String>>,
}

impl FileOffer {
    pub fn new(seq: u64, roots: Vec<PathBuf>) -> Self {
        Self { seq, roots, tree: OnceLock::new() }
    }

    /// Nomes do 1º nível (vão na oferta). Pura.
    pub fn top_names(&self) -> Vec<String> {
        self.roots.iter().filter_map(|r| r.file_name()).map(|n| n.to_string_lossy().into_owned()).collect()
    }

    pub fn tree(&self) -> Result<&[FileEntry], String> {
        self.tree.get_or_init(|| build_tree(&self.roots).map_err(|e| e.to_string())).as_deref().map_err(String::clone)
    }
}

/// A oferta atual deste lado (troca a cada cópia; `None` = nenhuma).
pub type SharedOffer = Arc<Mutex<Option<Arc<FileOffer>>>>;

/// A oferta pedida, se ainda for a atual. Pura.
fn current(offer: &SharedOffer, seq: u64) -> Result<Arc<FileOffer>, StreamError> {
    offer.lock().ok().and_then(|o| o.clone()).filter(|o| o.seq == seq).ok_or(StreamError::Gone)
}

/// Responde um pedido de arquivos (o stream já aberto pelo outro lado).
pub async fn serve<R, W>(offer: &SharedOffer, read: &mut R, write: &mut W) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let req: Request = read_known(read).await.map_err(io::Error::other)?;
    let send = |r: Reply| r;
    let reply = match req {
        Request::FileTree { seq } => match current(offer, seq).and_then(|o| o.tree().map(<[FileEntry]>::to_vec).map_err(StreamError::Io)) {
            Ok(tree) => send(Reply::Tree(tree)),
            Err(e) => send(Reply::Error(e)),
        },
        Request::FileRead { seq, index, offset } => {
            let path = current(offer, seq).and_then(|o| {
                let tree = o.tree().map_err(StreamError::Io)?;
                resolve(&o.roots, tree, index as usize).ok_or(StreamError::NotAllowed)
            });
            match path {
                Ok(path) => return send_file(&path, offset, write).await,
                Err(e) => send(Reply::Error(e)),
            }
        }
        Request::ClipboardText { .. } => send(Reply::Error(StreamError::NotAllowed)),
    };
    write_msg(write, &reply).await.map_err(io::Error::other)
}

/// `FileHeader`, blocos de [`FILE_CHUNK`] a partir de `offset`, `Done`.
async fn send_file<W: AsyncWrite + Unpin>(path: &Path, offset: u64, write: &mut W) -> io::Result<()> {
    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => return write_msg(write, &Reply::Error(StreamError::Io(e.to_string()))).await.map_err(io::Error::other),
    };
    let size = file.metadata().await?.len();
    write_msg(write, &Reply::FileHeader { size }).await.map_err(io::Error::other)?;
    file.seek(io::SeekFrom::Start(offset.min(size))).await?;
    let mut buf = vec![0u8; FILE_CHUNK];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        write_msg(write, &Reply::Chunk(buf[..n].to_vec())).await.map_err(io::Error::other)?;
    }
    write_msg(write, &Reply::Done).await.map_err(io::Error::other)
}

/// Um stream novo pra o outro lado (escrita, leitura).
pub type BoxWrite = Box<dyn AsyncWrite + Send + Unpin>;
pub type BoxRead = Box<dyn AsyncRead + Send + Unpin>;
pub type StreamFuture = Pin<Box<dyn std::future::Future<Output = io::Result<(BoxWrite, BoxRead)>> + Send>>;
pub type Opener = Arc<dyn Fn() -> StreamFuture + Send + Sync>;

fn stream_err(e: StreamError) -> io::Error {
    match e {
        StreamError::Gone => io::Error::new(io::ErrorKind::NotFound, "a cópia mudou na outra máquina; copie de novo"),
        StreamError::NotAllowed => io::Error::new(io::ErrorKind::PermissionDenied, "não permitido"),
        StreamError::Io(m) => io::Error::other(m),
    }
}

/// Arquivos que o OUTRO lado copiou, buscados sob demanda. As chamadas são
/// bloqueantes (o Explorer chama de uma thread COM, fora do tokio).
#[derive(Clone)]
pub struct RemoteFiles {
    pub seq: u64,
    opener: Opener,
    rt: tokio::runtime::Handle,
}

impl RemoteFiles {
    pub fn new(seq: u64, opener: Opener, rt: tokio::runtime::Handle) -> Self {
        Self { seq, opener, rt }
    }

    /// A árvore inteira (pastas antes do conteúdo).
    pub fn tree(&self) -> io::Result<Vec<FileEntry>> {
        let (seq, opener) = (self.seq, self.opener.clone());
        self.rt.block_on(async move {
            let (mut w, mut r) = opener().await?;
            write_msg(&mut w, &Request::FileTree { seq }).await.map_err(io::Error::other)?;
            match read_known::<_, Reply>(&mut r).await.map_err(io::Error::other)? {
                Reply::Tree(t) => Ok(t),
                Reply::Error(e) => Err(stream_err(e)),
                _ => Err(io::Error::other("resposta inesperada")),
            }
        })
    }

    /// O arquivo `index` a partir de `offset`: tamanho total e um leitor.
    pub fn open(&self, index: u32, offset: u64) -> io::Result<(u64, RemoteFile)> {
        let (seq, opener) = (self.seq, self.opener.clone());
        let (size, read) = self.rt.block_on(async move {
            let (mut w, mut r) = opener().await?;
            write_msg(&mut w, &Request::FileRead { seq, index, offset }).await.map_err(io::Error::other)?;
            match read_known::<_, Reply>(&mut r).await.map_err(io::Error::other)? {
                Reply::FileHeader { size } => Ok((size, r)),
                Reply::Error(e) => Err(stream_err(e)),
                _ => Err(io::Error::other("resposta inesperada")),
            }
        })?;
        Ok((size, RemoteFile { rt: self.rt.clone(), read, pending: Vec::new(), done: false }))
    }
}

/// Leitor de um arquivo remoto (blocos de 256 KB, guardando a sobra).
pub struct RemoteFile {
    rt: tokio::runtime::Handle,
    read: BoxRead,
    pending: Vec<u8>,
    done: bool,
}

impl Read for RemoteFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.pending.is_empty() && !self.done {
            let reply = self.rt.block_on(read_known::<_, Reply>(&mut self.read)).map_err(io::Error::other)?;
            match reply {
                Reply::Chunk(c) => self.pending = c,
                Reply::Done => self.done = true,
                Reply::Error(e) => return Err(stream_err(e)),
                _ => return Err(io::Error::other("resposta inesperada")),
            }
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("reflex-files-{}-{}", std::process::id(), fastrand()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn fastrand() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
    }

    #[test]
    fn single_file_tree() {
        let d = tmp();
        std::fs::write(d.join("a.txt"), b"hello").unwrap();
        let tree = build_tree(&[d.join("a.txt")]).unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].rel_path, "a.txt");
        assert_eq!(tree[0].size, 5);
        assert!(!tree[0].is_dir);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn folder_comes_before_its_contents() {
        let d = tmp();
        std::fs::create_dir(d.join("f")).unwrap();
        std::fs::write(d.join("f").join("x.txt"), b"x").unwrap();
        std::fs::create_dir(d.join("f").join("sub")).unwrap();
        std::fs::write(d.join("f").join("sub").join("y.txt"), b"yy").unwrap();
        let tree = build_tree(&[d.join("f")]).unwrap();
        let paths: Vec<&str> = tree.iter().map(|e| e.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["f", "f\\sub", "f\\sub\\y.txt", "f\\x.txt"]);
        assert!(tree[0].is_dir);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn resolve_maps_index_back_to_disk() {
        let d = tmp();
        std::fs::create_dir(d.join("f")).unwrap();
        std::fs::write(d.join("f").join("x.txt"), b"x").unwrap();
        let roots = vec![d.join("f")];
        let tree = build_tree(&roots).unwrap();
        let file_idx = tree.iter().position(|e| e.rel_path == "f\\x.txt").unwrap();
        assert_eq!(resolve(&roots, &tree, file_idx), Some(d.join("f").join("x.txt")));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_file_copied_alone_resolves_to_itself() {
        let d = tmp();
        std::fs::write(d.join("logo.png"), b"png").unwrap();
        let roots = vec![d.join("logo.png")];
        let tree = build_tree(&roots).unwrap();
        assert_eq!(tree[0].rel_path, "logo.png");
        let path = resolve(&roots, &tree, 0).unwrap();
        assert_eq!(path, d.join("logo.png"));
        assert_eq!(std::fs::read(&path).unwrap(), b"png");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn resolve_refuses_dotdot() {
        let roots = vec![PathBuf::from(r"C:\base")];
        let tree = vec![FileEntry { rel_path: r"base\..\secret.txt".into(), is_dir: false, size: 0, modified: 0, attributes: 0 }];
        assert_eq!(resolve(&roots, &tree, 0), None);
    }

    /// Um "outro lado" em memória: cada stream aberto vira um `serve` do `offer`.
    fn loopback(offer: SharedOffer, rt: &tokio::runtime::Runtime) -> Opener {
        let handle = rt.handle().clone();
        Arc::new(move || {
            let offer = offer.clone();
            let handle = handle.clone();
            Box::pin(async move {
                let (client, server) = tokio::io::duplex(64 * 1024);
                let (cr, cw) = tokio::io::split(client);
                let (mut sr, mut sw) = tokio::io::split(server);
                handle.spawn(async move {
                    let _ = serve(&offer, &mut sr, &mut sw).await;
                });
                Ok((Box::new(cw) as BoxWrite, Box::new(cr) as BoxRead))
            })
        })
    }

    #[test]
    fn remote_tree_and_file_contents_roundtrip() {
        let d = tmp();
        std::fs::create_dir(d.join("f")).unwrap();
        let big: Vec<u8> = (0..(FILE_CHUNK * 2 + 123)).map(|i| (i % 251) as u8).collect();
        std::fs::write(d.join("f").join("big.bin"), &big).unwrap();
        let offer: SharedOffer = Arc::new(Mutex::new(Some(Arc::new(FileOffer::new(5, vec![d.join("f")])))));
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let remote = RemoteFiles::new(5, loopback(offer.clone(), &rt), rt.handle().clone());
        let tree = remote.tree().unwrap();
        assert_eq!(tree.iter().map(|e| e.rel_path.as_str()).collect::<Vec<_>>(), vec!["f", "f\\big.bin"]);
        let (size, mut file) = remote.open(1, 0).unwrap();
        assert_eq!(size, big.len() as u64);
        let mut got = Vec::new();
        file.read_to_end(&mut got).unwrap();
        assert_eq!(got, big);
        // Do meio (Seek do Explorer).
        let (_, mut tail) = remote.open(1, 1000).unwrap();
        let mut rest = Vec::new();
        tail.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, big[1000..]);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn old_copy_is_gone_and_paths_cannot_escape() {
        let d = tmp();
        std::fs::write(d.join("a.txt"), b"a").unwrap();
        let offer: SharedOffer = Arc::new(Mutex::new(Some(Arc::new(FileOffer::new(2, vec![d.join("a.txt")])))));
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let stale = RemoteFiles::new(1, loopback(offer.clone(), &rt), rt.handle().clone());
        assert_eq!(stale.tree().unwrap_err().kind(), io::ErrorKind::NotFound);
        let ok = RemoteFiles::new(2, loopback(offer, &rt), rt.handle().clone());
        assert!(ok.open(99, 0).is_err(), "índice fora da árvore");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn top_names_are_the_copied_items() {
        let o = FileOffer::new(1, vec![PathBuf::from(r"C:\x\Relatório.pdf"), PathBuf::from(r"C:\y\Pasta")]);
        assert_eq!(o.top_names(), vec!["Relatório.pdf".to_string(), "Pasta".to_string()]);
    }

    #[test]
    fn filetime_is_after_1601_epoch() {
        assert!(to_filetime(std::time::SystemTime::UNIX_EPOCH) >= 116_444_736_000_000_000);
    }
}
