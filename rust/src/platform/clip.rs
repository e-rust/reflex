//! Uma cópia do clipboard como viaja entre as máquinas (§10.7), igual em todos
//! os sistemas. Os formatos padrão vão pelo número do Windows
//! (`CF_UNICODETEXT` = 13, `CF_DIB` = 8...); os registrados, pelo nome. Quem
//! lê e escreve o clipboard de cada sistema é o `clipboard` da plataforma.

use crate::proto::msg::{ClipFormatId, ClipItem, ClipboardOffer};

/// Acima disso uma cópia não é sincronizada (imagem 4K sem compressão ~33 MB).
pub const MAX_CLIP_BYTES: usize = 16 * 1024 * 1024;

/// Texto Unicode (UTF-16LE terminado em zero), o número do Windows.
pub const CF_UNICODETEXT: u32 = 13;

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
    /// Arquivos/pastas copiados aqui. Só os nomes vão na oferta; o conteúdo é
    /// servido por [`crate::platform::files::serve`] quando o outro cola.
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

    /// Só texto.
    pub fn text(text: &str) -> Self {
        let data = text.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
        Self { items: vec![ClipItem { format: ClipFormatId::Standard(CF_UNICODETEXT), data }], files: Vec::new() }
    }

    /// O texto Unicode da cópia, se houver.
    pub fn as_text(&self) -> Option<String> {
        self.items.iter().find(|i| i.format == ClipFormatId::Standard(CF_UNICODETEXT)).map(|i| utf16_text(&i.data))
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

/// Texto UTF-16 (até o primeiro zero) em bytes → String. Pura.
pub fn utf16_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_roundtrips_through_the_offer() {
        let snap = ClipSnapshot::text("ação");
        assert_eq!(snap.as_text().as_deref(), Some("ação"));
        assert_eq!(ClipSnapshot::from_offer(&snap.to_offer(7)), snap);
    }

    #[test]
    fn utf16_stops_at_the_terminator() {
        assert_eq!(utf16_text(&[b'o', 0, b'i', 0, 0, 0, b'x', 0]), "oi");
    }
}
