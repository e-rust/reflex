//! Cursor fora do vídeo (§2.2): o host converte a forma do Windows pra BGRA
//! com alfa; o viewer desenha por cima do quadro na posição recebida.

/// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE`.
pub const SHAPE_MONOCHROME: u32 = 1;
pub const SHAPE_COLOR: u32 = 2;
pub const SHAPE_MASKED_COLOR: u32 = 4;

/// Forma como a DXGI entrega.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCursorShape {
    pub kind: u32,
    pub width: u32,
    /// No monocromático é o DOBRO da altura visível (máscara AND + XOR).
    pub height: u32,
    pub pitch: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub data: Vec<u8>,
}

/// Forma pronta: BGRA com alfa (não pré-multiplicado).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorImage {
    pub width: u32,
    pub height: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub bgra: Vec<u8>,
}

fn bit(row: &[u8], x: usize) -> bool {
    row.get(x / 8).is_some_and(|b| b & (0x80 >> (x % 8)) != 0)
}

/// Converte. Os pixels "inverter a tela" (XOR) não têm equivalente com alfa:
/// viram preto opaco (o caso comum é o cursor de texto sobre fundo claro).
pub fn to_image(raw: &RawCursorShape) -> Option<CursorImage> {
    let w = raw.width as usize;
    let pitch = raw.pitch as usize;
    let (h, mut out) = match raw.kind {
        SHAPE_MONOCHROME => {
            let h = (raw.height / 2) as usize;
            let mut out = vec![0u8; w * h * 4];
            for y in 0..h {
                let and_row = raw.data.get(y * pitch..(y + 1) * pitch)?;
                let xor_row = raw.data.get((y + h) * pitch..(y + h + 1) * pitch)?;
                for x in 0..w {
                    let px = match (bit(and_row, x), bit(xor_row, x)) {
                        (false, false) => [0, 0, 0, 255],
                        (false, true) => [255, 255, 255, 255],
                        (true, false) => [0, 0, 0, 0],
                        (true, true) => [0, 0, 0, 255],
                    };
                    out[(y * w + x) * 4..][..4].copy_from_slice(&px);
                }
            }
            (h, out)
        }
        SHAPE_COLOR | SHAPE_MASKED_COLOR => {
            let h = raw.height as usize;
            let mut out = vec![0u8; w * h * 4];
            for y in 0..h {
                let row = raw.data.get(y * pitch..y * pitch + w * 4)?;
                out[y * w * 4..(y + 1) * w * 4].copy_from_slice(row);
            }
            if raw.kind == SHAPE_MASKED_COLOR {
                // Alfa = máscara: 0 → pixel opaco; 0xFF → XOR com a tela
                // (preto = transparente; outra cor = opaca, aproximação).
                let count = out.len() / 4;
                for i in 0..count {
                    let px = &mut out[i * 4..i * 4 + 4];
                    let xor = px[3] == 0xFF;
                    px[3] = if !xor || px[..3] != [0, 0, 0] { 255 } else { 0 };
                }
            }
            (h, out)
        }
        _ => return None,
    };
    if w == 0 || h == 0 {
        out.clear();
        return None;
    }
    Some(CursorImage { width: raw.width, height: h as u32, hot_x: raw.hot_x, hot_y: raw.hot_y, bgra: out })
}

/// Desenha o cursor no quadro BGRA (`stride` em bytes) com o ponto quente em (`x`, `y`).
pub fn blend(frame: &mut [u8], frame_w: u32, frame_h: u32, stride: usize, cursor: &CursorImage, x: i32, y: i32) {
    let left = x - cursor.hot_x as i32;
    let top = y - cursor.hot_y as i32;
    for cy in 0..cursor.height as i32 {
        let fy = top + cy;
        if fy < 0 || fy >= frame_h as i32 {
            continue;
        }
        for cx in 0..cursor.width as i32 {
            let fx = left + cx;
            if fx < 0 || fx >= frame_w as i32 {
                continue;
            }
            let s = ((cy as u32 * cursor.width + cx as u32) * 4) as usize;
            let src = &cursor.bgra[s..s + 4];
            let a = src[3] as u32;
            if a == 0 {
                continue;
            }
            let d = fy as usize * stride + fx as usize * 4;
            let dst = &mut frame[d..d + 4];
            for i in 0..3 {
                dst[i] = ((src[i] as u32 * a + dst[i] as u32 * (255 - a)) / 255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monochrome_masks_are_decoded() {
        // 4x1 visível: 1ª linha = máscara AND, 2ª = XOR (1 bit por pixel).
        let raw = RawCursorShape { kind: SHAPE_MONOCHROME, width: 4, height: 2, pitch: 1, hot_x: 0, hot_y: 0, data: vec![0b1100_0000, 0b0101_0000] };
        let img = to_image(&raw).unwrap();
        assert_eq!((img.width, img.height), (4, 1));
        let px: Vec<&[u8]> = img.bgra.chunks(4).collect();
        assert_eq!(px[0], &[0, 0, 0, 0]); // AND=1 XOR=0 → transparente
        assert_eq!(px[1], &[0, 0, 0, 255]); // AND=1 XOR=1 → inverte ≈ preto
        assert_eq!(px[2], &[0, 0, 0, 255]); // AND=0 XOR=0 → preto
        assert_eq!(px[3], &[255, 255, 255, 255]); // AND=0 XOR=1 → branco
    }

    #[test]
    fn color_shape_respects_pitch() {
        let raw = RawCursorShape { kind: SHAPE_COLOR, width: 1, height: 2, pitch: 8, hot_x: 0, hot_y: 0, data: vec![1, 2, 3, 4, 9, 9, 9, 9, 5, 6, 7, 8, 9, 9, 9, 9] };
        assert_eq!(to_image(&raw).unwrap().bgra, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn masked_color_alpha_from_mask() {
        let raw = RawCursorShape { kind: SHAPE_MASKED_COLOR, width: 2, height: 1, pitch: 8, hot_x: 0, hot_y: 0, data: vec![10, 20, 30, 0, 0, 0, 0, 0xFF] };
        let img = to_image(&raw).unwrap();
        assert_eq!(&img.bgra[..4], &[10, 20, 30, 255]);
        assert_eq!(img.bgra[7], 0);
    }

    #[test]
    fn blend_uses_hotspot_and_clips() {
        let cursor = CursorImage { width: 2, height: 2, hot_x: 1, hot_y: 1, bgra: vec![255; 16] };
        let mut frame = vec![0u8; 4 * 4 * 4];
        blend(&mut frame, 4, 4, 16, &cursor, 0, 0);
        // Só o pixel (0,0) cai dentro do quadro.
        assert_eq!(&frame[..4], &[255, 255, 255, 0]);
        assert_eq!(&frame[4..8], &[0, 0, 0, 0]);
    }

    #[test]
    fn blend_half_alpha() {
        let cursor = CursorImage { width: 1, height: 1, hot_x: 0, hot_y: 0, bgra: vec![200, 200, 200, 128] };
        let mut frame = vec![0u8; 4];
        blend(&mut frame, 1, 1, 4, &cursor, 0, 0);
        assert_eq!(frame[0], 100);
    }

    #[test]
    fn truncated_data_is_rejected() {
        let raw = RawCursorShape { kind: SHAPE_COLOR, width: 4, height: 4, pitch: 16, hot_x: 0, hot_y: 0, data: vec![0; 10] };
        assert!(to_image(&raw).is_none());
    }
}
