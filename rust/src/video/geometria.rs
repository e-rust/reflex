//! A conta da janela de vídeo, sem tela: onde a imagem fica, onde o mouse cai
//! na tela remota, o tamanho da janela e o cursor remoto. Tudo em pixels reais
//! da janela (o form converte pra pontos lógicos do Aylis). Puras.

use crate::proto::msg::{CursorShape, DesiredDisplay};
use crate::viewer::hosts::ViewScale;

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
/// se não couber.
pub fn image_rect(scale: ViewScale, img_w: u32, img_h: u32, win_w: i32, win_h: i32) -> Rect {
    match scale {
        ViewScale::Native if img_w as i32 <= win_w && img_h as i32 <= win_h => {
            let (w, h) = (img_w as i32, img_h as i32);
            Rect { x: (win_w - w) / 2, y: (win_h - h) / 2, w, h }
        }
        _ => fit_rect(img_w, img_h, win_w, win_h),
    }
}

/// Maior retângulo com a proporção da imagem que cabe na janela, centralizado.
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

/// Posição do mouse (pixels na janela) → normalizada 0..=65535 dentro da
/// imagem (as faixas pretas encostam na borda mais próxima).
pub fn norm(px: i32, py: i32, image: Rect) -> (u16, u16) {
    let axis = |p: i32, start: i32, len: i32| -> u16 {
        if len <= 1 {
            return 0;
        }
        ((p - start).clamp(0, len - 1) as i64 * 65535 / (len as i64 - 1)) as u16
    };
    (axis(px, image.x, image.w), axis(py, image.y, image.h))
}

/// O `WHEEL_DELTA` do Windows: um dente da roda no protocolo.
const WHEEL_DELTA: f32 = 120.0;

/// A roda do Aylis (`delta` em pontos, positivo = pra cima / pra esquerda,
/// como no winit; um dente = `line`) → dentes do protocolo (120 = um,
/// positivo pra cima / pra direita, como no Windows). O que sobra de um dente
/// fica em `rest` pro próximo (touchpad). Pura.
pub fn wheel_notches(rest: &mut (f32, f32), delta: (f32, f32), line: f32) -> (i16, i16) {
    let line = line.max(1.0);
    rest.0 -= delta.0 / line * WHEEL_DELTA;
    rest.1 += delta.1 / line * WHEEL_DELTA;
    let out = (rest.0.trunc(), rest.1.trunc());
    rest.0 -= out.0;
    rest.1 -= out.1;
    (out.0.clamp(i16::MIN as f32, i16::MAX as f32) as i16, out.1.clamp(i16::MIN as f32, i16::MAX as f32) as i16)
}

/// Onde o cursor vai na janela: a posição e o tamanho remotos na escala da
/// imagem desenhada.
pub fn cursor_rect(image: Rect, frame: (u32, u32), pos: (i32, i32), size: (u32, u32)) -> Rect {
    let (fw, fh) = (frame.0.max(1) as i64, frame.1.max(1) as i64);
    let sx = |v: i64| (v * image.w as i64 / fw) as i32;
    let sy = |v: i64| (v * image.h as i64 / fh) as i32;
    Rect { x: image.x + sx(pos.0 as i64), y: image.y + sy(pos.1 as i64), w: sx(size.0 as i64).max(1), h: sy(size.1 as i64).max(1) }
}

/// O que pedir pra tela remota: a área da janela e a escala do Windows
/// deste monitor (96 DPI = 100%).
pub fn desired_for(client: (i32, i32), dpi: u32) -> Option<DesiredDisplay> {
    let (w, h) = client;
    // Minimizada (0x0) ou minúscula: não pede nada.
    (w >= 320 && h >= 240).then(|| DesiredDisplay { width: w as u32, height: h as u32, scale_pct: (dpi.max(96) * 100 / 96) as u16 })
}

/// Tamanho da tela remota na escala do monitor daqui (96 DPI = 100%): com
/// 150%, 1024x768 vira 1536x1152 — o mesmo que o console do Hyper-V e o resto
/// do Windows mostram. Em pixel 1:1 a VM ficava 1/3 menor.
pub fn scaled_to_dpi(remote: (u32, u32), dpi: u32) -> (u32, u32) {
    let dpi = dpi.max(96) as u64;
    ((remote.0 as u64 * dpi / 96) as u32, (remote.1 as u64 * dpi / 96) as u32)
}

/// Janela (borda e título inclusos) com área de imagem `content`, limitada à
/// área útil do monitor `work` (sem a barra de tarefas) e centralizada nela;
/// não cabendo, encolhe mantendo a proporção. `frame` é o quanto a borda e o
/// título somam em largura e altura.
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

/// Alfa pré-multiplicado (o que a imagem do Aylis quer).
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
        self.shape.as_ref().is_some_and(shape_ok) && self.known && self.visible
    }
}

/// A forma tem pixels pra todo o tamanho dela.
pub fn shape_ok(s: &CursorShape) -> bool {
    s.width > 0 && s.height > 0 && s.bgra.len() >= (s.width * s.height * 4) as usize
}

/// Que cursor mostrar sobre a imagem no modo "só o remoto".
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
        Some(s) if shape_ok(s) => CursorPick::Remote(s.id),
        _ => CursorPick::Local,
    }
}

#[cfg(test)]
mod wheel_tests {
    use super::*;

    #[test]
    fn a_notch_down_is_minus_120_and_touchpad_fractions_add_up() {
        let mut rest = (0.0, 0.0);
        // Pra baixo no Aylis é negativo; no protocolo também.
        assert_eq!(wheel_notches(&mut rest, (0.0, -100.0), 100.0), (0, -120));
        // Pra direita no Aylis é negativo; no protocolo, positivo.
        assert_eq!(wheel_notches(&mut rest, (-25.0, 0.5), 100.0), (30, 0));
        assert_eq!(wheel_notches(&mut rest, (0.0, 0.5), 100.0), (0, 1));
    }
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
