//! Captura de tela no Linux: o fluxo do PipeWire que o Mutter entrega
//! ([`crate::platform::mutter`]). Uma thread roda o laço do PipeWire e copia
//! cada quadro (só as áreas alteradas, pelo `SPA_META_VideoDamage`) para a
//! imagem compartilhada; o cursor vem à parte (`SPA_META_Cursor`). O motor
//! espera por [`PwCapture::acquire`] e lê os pixels por [`PwCapture::read`].

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use pipewire as pw;
use pw::spa;
use spa::buffer::meta::{MetaCursor, MetaVideoDamage};
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::pod::serialize::PodSerializer;
use spa::pod::{Object, Pod, Property, Value};

use crate::media::cursor::{RawCursorShape, SHAPE_COLOR};
use crate::media::damage::Region;
use crate::media::frame_info::FrameInfo;
use crate::media::{MediaError, Result};

/// Quantas áreas alteradas por quadro o Mutter pode mandar.
const DAMAGE_REGIONS: usize = 16;
/// O maior cursor que cabe no metadado (o Mutter oferece um tamanho fixo
/// dentro disso; fora da faixa pedida, ele larga o metadado do cursor).
const CURSOR_MAX: usize = 1024;

/// A imagem mais nova e o que mudou desde a última espera.
#[derive(Default)]
struct Latest {
    /// BGRA (o 4º byte não importa), `width * 4` por linha.
    bgra: Vec<u8>,
    width: u32,
    height: u32,
    new_image: bool,
    /// O que mudou desde a última espera (`None` = tudo).
    damage: Option<Vec<Region>>,
    cursor_pos: Option<(i32, i32, bool)>,
    cursor_shape: Option<RawCursorShape>,
    /// O fluxo acabou (a sessão do Mutter fechou).
    ended: Option<String>,
}

type Shared = Arc<(Mutex<Latest>, Condvar)>;

/// A captura de um fluxo do PipeWire.
pub struct PwCapture {
    shared: Shared,
    quit: pw::channel::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PwCapture {
    /// Liga no nó `node` e espera o 1º quadro (até 3 s).
    pub fn open(node: u32) -> Result<Self> {
        let shared: Shared = Arc::default();
        let (quit, quit_rx) = pw::channel::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("pipewire".into())
            .spawn(move || {
                if let Err(e) = run(node, s.clone(), quit_rx, ready_tx.clone()) {
                    let _ = ready_tx.send(Err(e.clone()));
                    end(&s, e);
                }
            })
            .map_err(|e| MediaError::Msg(format!("thread do PipeWire: {e}")))?;
        let capture = Self { shared, quit, thread: Some(thread) };
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(MediaError::Msg(format!("PipeWire: {e}"))),
            Err(_) => return Err(MediaError::Msg("PipeWire: nenhum quadro em 3 s".into())),
        }
        Ok(capture)
    }

    /// O tamanho da imagem.
    pub fn size(&self) -> (u32, u32) {
        self.shared.0.lock().map(|l| (l.width, l.height)).unwrap_or((0, 0))
    }

    /// Espera até `wait_ms` por imagem nova ou cursor. O fluxo acabou:
    /// [`MediaError::AccessLost`] (o motor recria).
    pub fn acquire(&mut self, wait_ms: u32) -> Result<Option<FrameInfo>> {
        let (lock, cvar) = &*self.shared;
        let mut l = lock.lock().map_err(|_| MediaError::AccessLost)?;
        if !has_news(&l) {
            l = cvar.wait_timeout(l, Duration::from_millis(wait_ms as u64)).map_err(|_| MediaError::AccessLost)?.0;
        }
        if let Some(e) = l.ended.as_ref() {
            log::info!("PipeWire: {e}");
            return Err(MediaError::AccessLost);
        }
        if !has_news(&l) {
            return Ok(None);
        }
        let new_image = std::mem::take(&mut l.new_image);
        Ok(Some(FrameInfo {
            new_image,
            cursor_pos: l.cursor_pos.take(),
            cursor_shape: l.cursor_shape.take(),
            dirty: if new_image { l.damage.take() } else { None },
            moved_px: 0,
        }))
    }

    /// Os pixels (BGRA, `width * 4` por linha) enquanto `f` roda.
    pub fn read<R>(&self, f: impl FnOnce(&[u8], usize) -> R) -> Result<R> {
        let l = self.shared.0.lock().map_err(|_| MediaError::AccessLost)?;
        Ok(f(&l.bgra, l.width as usize * 4))
    }
}

impl Drop for PwCapture {
    fn drop(&mut self) {
        let _ = self.quit.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn has_news(l: &Latest) -> bool {
    l.new_image || l.cursor_pos.is_some() || l.cursor_shape.is_some() || l.ended.is_some()
}

fn end(shared: &Shared, why: String) {
    if let Ok(mut l) = shared.0.lock() {
        l.ended = Some(why);
    }
    shared.1.notify_all();
}

/// O que a thread do PipeWire guarda entre os quadros.
struct State {
    shared: Shared,
    format: VideoInfoRaw,
    ready: Option<std::sync::mpsc::Sender<std::result::Result<(), String>>>,
}

/// O laço do PipeWire (na thread dele) até o `quit`.
fn run(node: u32, shared: Shared, quit: pw::channel::Receiver<()>, ready: std::sync::mpsc::Sender<std::result::Result<(), String>>) -> std::result::Result<(), String> {
    pw::init();
    let err = |e: pw::Error| e.to_string();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(err)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(err)?;
    let core = context.connect_rc(None).map_err(err)?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "reflex",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(err)?;
    let state = State { shared: shared.clone(), format: VideoInfoRaw::default(), ready: Some(ready) };
    let _listener = stream
        .add_local_listener_with_user_data(state)
        .state_changed(|_, st, _, new| {
            if let pw::stream::StreamState::Error(e) = new {
                end(&st.shared, format!("o fluxo da tela falhou: {e}"));
            } else if new == pw::stream::StreamState::Unconnected {
                end(&st.shared, "o fluxo da tela fechou".into());
            }
        })
        .param_changed(|stream, st, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() || st.format.parse(param).is_err() {
                return;
            }
            // Com o formato: pede o cursor e as áreas alteradas junto de cada quadro.
            let region = std::mem::size_of::<spa::sys::spa_meta_region>();
            let damage = meta(spa::sys::SPA_META_VideoDamage, region * DAMAGE_REGIONS, region, region * DAMAGE_REGIONS);
            let cursor = meta(spa::sys::SPA_META_Cursor, cursor_meta_size(64), cursor_meta_size(1), cursor_meta_size(CURSOR_MAX));
            if let (Some(d), Some(c)) = (Pod::from_bytes(&damage), Pod::from_bytes(&cursor))
                && let Err(e) = stream.update_params(&mut [d, c])
            {
                log::warn!("PipeWire: metadados não pedidos ({e})");
            }
        })
        .process(process)
        .register()
        .map_err(err)?;

    let format = enum_format();
    let mut params = [Pod::from_bytes(&format).ok_or("formato inválido")?];
    stream
        .connect(spa::utils::Direction::Input, Some(node), pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS, &mut params)
        .map_err(err)?;
    let ml = mainloop.clone();
    let _quit = quit.attach(mainloop.loop_(), move |_| ml.quit());
    mainloop.run();
    Ok(())
}

/// Um quadro (ou só o cursor) chegou.
fn process(stream: &pw::stream::Stream, st: &mut State) {
    let Some(mut buffer) = stream.dequeue_buffer() else { return };
    let mut cursor = None;
    if let Some(c) = buffer.find_meta::<MetaCursor>() {
        cursor = cursor_of(c);
    }
    let damage: Option<Vec<Region>> = buffer.find_meta::<MetaVideoDamage>().map(|d| {
        d.iter()
            .filter(|r| r.is_valid())
            .map(|r| {
                let (p, s) = (r.position(), r.size());
                region(p.x, p.y, s.width, s.height)
            })
            .collect()
    });
    let (w, h) = (st.format.size().width, st.format.size().height);
    let swap = matches!(st.format.format(), VideoFormat::RGBx | VideoFormat::RGBA);
    let datas = buffer.datas_mut();
    let frame = datas.first_mut().and_then(|d| {
        let (offset, stride, size) = (d.chunk().offset() as usize, d.chunk().stride().max(0) as usize, d.chunk().size() as usize);
        (size > 0 && stride >= w as usize * 4).then_some(())?;
        d.data().map(|bytes| (bytes.get(offset..offset + size).map(<[u8]>::to_vec), stride))
    });
    let (lock, cvar) = &*st.shared;
    let Ok(mut l) = lock.lock() else { return };
    if let Some((Some(src), stride)) = frame {
        let fresh = l.width != w || l.height != h || l.bgra.len() != (w * h * 4) as usize;
        if fresh {
            l.width = w;
            l.height = h;
            l.bgra = vec![0; (w * h * 4) as usize];
        }
        // A 1ª imagem (ou sem a lista do Mutter) vai inteira.
        let areas = damage.filter(|_| !fresh).unwrap_or_else(|| vec![Region { left: 0, top: 0, right: w, bottom: h }]);
        let dst_stride = w as usize * 4;
        let mut copied = Vec::with_capacity(areas.len());
        for r in areas.iter().map(|r| clip(r, w, h)).filter(|r| !r.is_empty()) {
            copy_region(&src, stride, &mut l.bgra, dst_stride, &r, swap);
            copied.push(r);
        }
        if fresh || !copied.is_empty() {
            l.new_image = true;
            l.damage = merge(if l.new_image && !fresh { l.damage.take() } else { None }, if fresh { None } else { Some(copied) }, fresh);
        }
        if let Some(tx) = st.ready.take() {
            let _ = tx.send(Ok(()));
        }
    }
    if let Some((pos, shape)) = cursor {
        l.cursor_pos = Some(pos);
        if shape.is_some() {
            l.cursor_shape = shape;
        }
    }
    drop(l);
    cvar.notify_all();
}

/// Junta o que já estava marcado com o que mudou agora (`None` = tudo). Pura.
fn merge(before: Option<Vec<Region>>, now: Option<Vec<Region>>, fresh: bool) -> Option<Vec<Region>> {
    if fresh {
        return None;
    }
    match (before, now) {
        (Some(mut a), Some(b)) => {
            a.extend(b);
            Some(a)
        }
        (None, b) => b,
        (Some(_), None) => None,
    }
}

/// Um retângulo do Mutter (pode vir com x/y negativos) → região. Pura.
fn region(x: i32, y: i32, w: u32, h: u32) -> Region {
    let (x0, y0) = (x.max(0) as u32, y.max(0) as u32);
    let right = (x as i64 + w as i64).max(0) as u32;
    let bottom = (y as i64 + h as i64).max(0) as u32;
    Region { left: x0, top: y0, right, bottom }
}

fn clip(r: &Region, w: u32, h: u32) -> Region {
    Region { left: r.left.min(w), top: r.top.min(h), right: r.right.min(w), bottom: r.bottom.min(h) }
}

/// Copia a região de `src` pra `dst` (4 bytes por pixel), trocando R e B se
/// `swap` (o fluxo veio RGB). Pura.
fn copy_region(src: &[u8], src_stride: usize, dst: &mut [u8], dst_stride: usize, r: &Region, swap: bool) {
    let (x0, x1) = (r.left as usize * 4, r.right as usize * 4);
    for y in r.top as usize..r.bottom as usize {
        let (Some(s), Some(d)) = (src.get(y * src_stride + x0..y * src_stride + x1), dst.get_mut(y * dst_stride + x0..y * dst_stride + x1)) else { break };
        d.copy_from_slice(s);
        if swap {
            d.as_chunks_mut::<4>().0.iter_mut().for_each(|px| px.swap(0, 2));
        }
    }
}

/// A posição do cursor e, se veio, a forma nova (BGRA). `None`: sem cursor no
/// metadado.
fn cursor_of(c: &MetaCursor) -> Option<((i32, i32, bool), Option<RawCursorShape>)> {
    if !c.is_valid() {
        return None;
    }
    let p = c.position();
    let hot = c.hotspot();
    let shape = c.bitmap().filter(|b| b.is_valid()).and_then(|b| {
        let size = b.size();
        let bytes = b.bitmap_data()?;
        let rgb = matches!(b.format(), VideoFormat::RGBA | VideoFormat::RGBx);
        Some(cursor_shape(bytes, size.width, size.height, b.stride().max(0) as usize, rgb, (hot.x.max(0) as u32, hot.y.max(0) as u32)))
    });
    // A posição do Reflex é a do canto da forma; o Mutter dá a do ponto de clique.
    Some(((p.x - hot.x, p.y - hot.y, true), shape))
}

/// A forma do cursor como o motor espera (a "colorida" da DXGI: BGRA). Pura.
fn cursor_shape(bytes: &[u8], w: u32, h: u32, stride: usize, rgb: bool, hot: (u32, u32)) -> RawCursorShape {
    let row = w as usize * 4;
    let mut data = vec![0u8; row * h as usize];
    for y in 0..h as usize {
        if let (Some(s), Some(d)) = (bytes.get(y * stride..y * stride + row), data.get_mut(y * row..(y + 1) * row)) {
            d.copy_from_slice(s);
            if rgb {
                d.as_chunks_mut::<4>().0.iter_mut().for_each(|px| px.swap(0, 2));
            }
        }
    }
    RawCursorShape { kind: SHAPE_COLOR, width: w, height: h, pitch: row as u32, hot_x: hot.0, hot_y: hot.1, data }
}

/// O metadado de um cursor de até `side`×`side` (a forma vai junto, BGRA).
fn cursor_meta_size(side: usize) -> usize {
    std::mem::size_of::<spa::sys::spa_meta_cursor>() + std::mem::size_of::<spa::sys::spa_meta_bitmap>() + side * side * 4
}

/// Um `SPA_PARAM_Meta` de `kind` com o tamanho entre `min` e `max` bytes (o
/// Mutter escolhe dentro da faixa; fora dela, larga o metadado).
fn meta(kind: u32, default: usize, min: usize, max: usize) -> Vec<u8> {
    use spa::utils::{Choice, ChoiceEnum, ChoiceFlags};
    let size = Value::Choice(spa::pod::ChoiceValue::Int(Choice(ChoiceFlags::empty(), ChoiceEnum::Range { default: default as i32, min: min as i32, max: max as i32 })));
    let obj = Object {
        type_: spa::sys::SPA_TYPE_OBJECT_ParamMeta,
        id: spa::sys::SPA_PARAM_Meta,
        properties: vec![Property::new(spa::sys::SPA_PARAM_META_type, Value::Id(spa::utils::Id(kind))), Property::new(spa::sys::SPA_PARAM_META_size, size)],
    };
    serialize(obj)
}

/// Os formatos aceitos: BGRx/BGRA (o do motor) ou RGBx/RGBA (troca na cópia),
/// em memória (sem modificador, o Mutter manda por memória compartilhada).
fn enum_format() -> Vec<u8> {
    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(spa::param::format::FormatProperties::MediaType, Id, spa::param::format::MediaType::Video),
        spa::pod::property!(spa::param::format::FormatProperties::MediaSubtype, Id, spa::param::format::MediaSubtype::Raw),
        spa::pod::property!(spa::param::format::FormatProperties::VideoFormat, Choice, Enum, Id, VideoFormat::BGRx, VideoFormat::BGRx, VideoFormat::BGRA, VideoFormat::RGBx, VideoFormat::RGBA),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle { width: 1920, height: 1080 },
            spa::utils::Rectangle { width: 1, height: 1 },
            spa::utils::Rectangle { width: 8192, height: 8192 }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 30, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: 60, denom: 1 }
        ),
    );
    serialize(obj)
}

fn serialize(obj: Object) -> Vec<u8> {
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(obj)).map(|(c, _)| c.into_inner()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_region_is_copied_and_rgb_becomes_bgr() {
        // 2x2 de origem com pitch 12 (4 de sobra por linha).
        let src: Vec<u8> = (0..24).collect();
        let mut dst = vec![0u8; 16];
        copy_region(&src, 12, &mut dst, 8, &Region { left: 1, top: 1, right: 2, bottom: 2 }, true);
        assert_eq!(&dst[..12], &[0; 12]);
        assert_eq!(&dst[12..], &[18, 17, 16, 19]);
    }

    #[test]
    fn mutter_rectangles_are_clamped_to_the_screen() {
        assert_eq!(region(-5, 10, 20, 4), Region { left: 0, top: 10, right: 15, bottom: 14 });
        assert_eq!(clip(&Region { left: 100, top: 0, right: 3000, bottom: 50 }, 1920, 1080), Region { left: 100, top: 0, right: 1920, bottom: 50 });
    }

    #[test]
    fn pending_damage_accumulates_until_read() {
        let a = Region { left: 0, top: 0, right: 2, bottom: 2 };
        let b = Region { left: 4, top: 4, right: 6, bottom: 6 };
        assert_eq!(merge(Some(vec![a]), Some(vec![b]), false), Some(vec![a, b]));
        // Tudo antes, ou a 1ª imagem: tudo.
        assert_eq!(merge(None, Some(vec![b]), false), Some(vec![b]));
        assert_eq!(merge(Some(vec![a]), Some(vec![b]), true), None);
        assert_eq!(merge(Some(vec![a]), None, false), None);
    }

    #[test]
    #[ignore = "precisa da sessão gráfica do GNOME, desbloqueada"]
    fn captures_the_gnome_screen_and_follows_the_mouse() {
        let session = crate::platform::mutter::shared().unwrap();
        let mut capture = PwCapture::open(session.stream.node).unwrap();
        assert_eq!(capture.size(), session.stream.size);
        let started = std::time::Instant::now();
        let (mut images, mut cursor) = (0, None);
        let mut x = 100.0;
        while started.elapsed() < Duration::from_secs(3) {
            x += 7.0;
            session.pointer_motion(x, 200.0);
            if let Some(info) = capture.acquire(100).unwrap() {
                images += info.new_image as u32;
                cursor = info.cursor_pos.or(cursor);
            }
        }
        println!("{images} imagens, cursor {cursor:?}");
        assert!(capture.read(|bgra, _| bgra.iter().any(|&b| b != 0)).unwrap(), "imagem toda preta");
        assert!(cursor.is_some(), "o cursor não veio no metadado");
    }

    #[test]
    fn cursor_bitmap_becomes_a_bgra_color_shape() {
        // 1x2, pitch 8 (4 de sobra), RGBA.
        let bytes = [1, 2, 3, 255, 0, 0, 0, 0, 10, 20, 30, 128, 0, 0, 0, 0];
        let s = cursor_shape(&bytes, 1, 2, 8, true, (0, 1));
        assert_eq!((s.kind, s.width, s.height, s.pitch, s.hot_y), (SHAPE_COLOR, 1, 2, 4, 1));
        assert_eq!(s.data, vec![3, 2, 1, 255, 30, 20, 10, 128]);
    }
}
