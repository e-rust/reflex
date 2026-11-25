//! O que uma captura de tela entrega a cada espera, igual em todos os
//! sistemas (DXGI no Windows, PipeWire no Linux).

use crate::media::cursor::RawCursorShape;
use crate::media::damage::Region;

/// O que veio num `acquire`.
#[derive(Debug, Default)]
pub struct FrameInfo {
    /// A imagem mudou (já está na captura).
    pub new_image: bool,
    /// Posição do cursor relativa ao monitor, se mudou.
    pub cursor_pos: Option<(i32, i32, bool)>,
    /// Forma nova do cursor, se mudou.
    pub cursor_shape: Option<RawCursorShape>,
    /// O que a imagem nova mudou (`None` = não se sabe: tudo).
    pub dirty: Option<Vec<Region>>,
    /// Dessa área, quanto só mudou de lugar (os *move rects*: rolagem, arrasto).
    pub moved_px: u64,
}
