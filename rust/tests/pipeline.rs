//! GPU → encode → decode de verdade, em cada codec que a máquina tiver.
//! Precisa de GPU e das DLLs do FFmpeg (`runtime\`), no Windows:
//! `cargo test --test pipeline -- --ignored --nocapture`
#![cfg(windows)]

use std::time::Instant;

use reflex::media::caps::{decoders, probe_encoders};
use reflex::media::capture::ScreenCapture;
use reflex::media::decode::{AvDecoder, VideoDecoder};
use reflex::media::encode::{open_encoder, EncoderConfig, FULL_CHROMA_BACKEND};
use reflex::media::gpu::{BgraReadback, BgraTexture, Device, Nv12Converter};
use reflex::proto::codec::Codec;

/// Quatro faixas: branco, vermelho, verde, azul.
fn pattern(w: u32, h: u32) -> Vec<u8> {
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for _y in 0..h {
        for x in 0..w {
            let c = match x * 4 / w {
                0 => [255, 255, 255, 255],
                1 => [0, 0, 255, 255],
                2 => [0, 255, 0, 255],
                _ => [255, 0, 0, 255],
            };
            px.extend_from_slice(&c);
        }
    }
    px
}

fn close(a: &[u8], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(&x, y)| (x as i32 - y as i32).abs() < 40)
}

#[test]
#[ignore = "precisa de GPU e FFmpeg"]
fn encode_decode_every_codec_preserves_colors() {
    let dev = Device::create().unwrap();
    let (w, h) = (1280u32, 720u32);
    let src = BgraTexture::from_pixels(&dev, w, h, &pattern(w, h)).unwrap();
    let conv = Nv12Converter::new(&dev, w, h).unwrap();
    conv.convert(&dev, &src.srv);

    let decs = decoders();
    println!("decoders: {decs:?}");
    let encs = probe_encoders(&dev);
    assert!(!encs.is_empty());
    for choice in encs {
        let cfg = EncoderConfig { codec: choice.cap.codec, backend: choice.backend.to_string(), width: w, height: h, fps: 60, bitrate_kbps: 8000, vbv_frames: 1, chroma444: false };
        let mut enc = open_encoder(&dev, &cfg).unwrap();
        let hw_dec = decs.iter().any(|d| d.codec == choice.cap.codec && d.hw);
        let mut dec = AvDecoder::open(choice.cap.codec, hw_dec).unwrap();
        let mut decoded = None;
        let mut total = 0usize;
        let t0 = Instant::now();
        for i in 0..30 {
            for p in enc.encode(&dev, &conv.nv12, i == 0 || i == 20).unwrap() {
                total += p.data.len();
                if let Some(f) = dec.decode(&p.data).unwrap() {
                    decoded = Some(f);
                }
            }
        }
        let f = decoded.unwrap_or_else(|| panic!("{}: nada decodificado", choice.backend));
        assert_eq!((f.width, f.height), (w, h));
        let at = |x: u32| &f.data[((h / 2 * w + x) * 4) as usize..][..3];
        println!("{}: {} KB/30 quadros, {:?}, decode hw={}, amostras {:?} {:?} {:?} {:?}", choice.backend, total / 1024, t0.elapsed(), dec.is_hardware(), at(w / 8), at(3 * w / 8), at(5 * w / 8), at(7 * w / 8));
        assert!(close(at(w / 8), [255, 255, 255]), "{}: branco", choice.backend);
        assert!(close(at(3 * w / 8), [0, 0, 255]), "{}: vermelho", choice.backend);
        assert!(close(at(5 * w / 8), [0, 255, 0]), "{}: verde", choice.backend);
        assert!(close(at(7 * w / 8), [255, 0, 0]), "{}: azul", choice.backend);
    }
}

#[test]
#[ignore = "precisa de GPU e FFmpeg"]
fn cpu_h264_from_bgra_preserves_colors_and_is_fast() {
    // O caminho das placas de software: BGRA lido → I420 na CPU → openh264.
    let dev = Device::create().unwrap();
    let (w, h) = (1024u32, 768u32);
    let src = BgraTexture::from_pixels(&dev, w, h, &pattern(w, h)).unwrap();
    let readback = BgraReadback::new(&dev, &src).unwrap();
    let cfg = EncoderConfig { codec: Codec::H264, backend: "openh264".into(), width: w, height: h, fps: 30, bitrate_kbps: 6000, vbv_frames: 1, chroma444: false };
    let mut enc = open_encoder(&dev, &cfg).unwrap();
    assert!(enc.accepts_bgra());
    let mut dec = AvDecoder::open(Codec::H264, false).unwrap();
    let (mut decoded, mut upload_us) = (None, 0u32);
    for i in 0..30 {
        let packets = readback.read(&dev, &src.tex, None, |bgra, stride| enc.encode_bgra(bgra, stride, None, i == 0)).unwrap().unwrap();
        upload_us = upload_us.max(enc.last_timing().upload_us);
        for p in packets {
            if let Some(f) = dec.decode(&p.data).unwrap() {
                decoded = Some(f);
            }
        }
    }
    let f = decoded.expect("nada decodificado");
    let at = |x: u32| &f.data[((h / 2 * w + x) * 4) as usize..][..3];
    println!("conversão na CPU: pior quadro {:.1} ms", upload_us as f64 / 1000.0);
    assert!(close(at(w / 8), [255, 255, 255]));
    assert!(close(at(3 * w / 8), [0, 0, 255]));
    assert!(close(at(5 * w / 8), [0, 255, 0]));
    assert!(close(at(7 * w / 8), [255, 0, 0]));
}

#[test]
#[ignore = "precisa de GPU e FFmpeg"]
fn only_the_changed_region_is_read_and_converted() {
    // A tela muda só num quadrado: copia, lê e converte só ele; o resto do
    // quadro convertido antes continua valendo.
    use reflex::media::damage::Region;
    let dev = Device::create().unwrap();
    let (w, h) = (1024u32, 768u32);
    let screen = BgraTexture::from_pixels(&dev, w, h, &pattern(w, h)).unwrap();
    let mut changed = pattern(w, h);
    for y in 100..300 {
        for x in 200..400 {
            changed[((y * w + x) * 4) as usize..][..3].copy_from_slice(&[0, 0, 0]);
        }
    }
    let next = BgraTexture::from_pixels(&dev, w, h, &changed).unwrap();
    let readback = BgraReadback::new(&dev, &screen).unwrap();
    let cfg = EncoderConfig { codec: Codec::H264, backend: "openh264".into(), width: w, height: h, fps: 30, bitrate_kbps: 8000, vbv_frames: 4, chroma444: false };
    let mut enc = open_encoder(&dev, &cfg).unwrap();
    let mut dec = AvDecoder::open(Codec::H264, false).unwrap();
    let mut last = None;
    let square = [Region { left: 200, top: 100, right: 400, bottom: 300 }];
    for i in 0..12 {
        // 0: tudo; 1: só o quadrado (a tela mudou); depois: nada mudou.
        let regions: Option<&[Region]> = match i {
            0 => None,
            1 => {
                reflex::media::gpu::copy_regions(&dev, &screen.tex, &next.tex, Some(&square));
                Some(&square)
            }
            _ => Some(&[]),
        };
        for p in readback.read(&dev, &screen.tex, regions, |bgra, stride| enc.encode_bgra(bgra, stride, regions, i == 0)).unwrap().unwrap() {
            if let Some(f) = dec.decode(&p.data).unwrap() {
                last = Some(f);
            }
        }
    }
    let f = last.expect("nada decodificado");
    let at = |x: u32, y: u32| &f.data[((y * w + x) * 4) as usize..][..3];
    assert!(close(at(300, 200), [0, 0, 0]), "o quadrado mudou: {:?}", at(300, 200));
    assert!(close(at(300, 500), [0, 0, 255]), "fora dele, o vermelho de antes: {:?}", at(300, 500));
    assert!(close(at(100, 200), [255, 255, 255]), "e o branco: {:?}", at(100, 200));
}

#[test]
#[ignore = "precisa de GPU e FFmpeg"]
fn full_chroma_keeps_one_pixel_color_stripes() {
    // Listras de 1 pixel vermelho/azul: no 4:2:0 a cor de cada par vira roxo.
    let dev = Device::create().unwrap();
    // 4:4:4 só com o x264, que a build LGPL do FFmpeg não tem.
    if !reflex::media::caps::has_full_chroma(&probe_encoders(&dev)) {
        println!("sem {FULL_CHROMA_BACKEND} nesta build do FFmpeg: 4:4:4 indisponível, nada a testar");
        return;
    }
    let (w, h) = (640u32, 360u32);
    let px: Vec<u8> = (0..w * h).flat_map(|i| if (i % w).is_multiple_of(2) { [0, 0, 255, 255] } else { [255, 0, 0, 255] }).collect();
    let src = BgraTexture::from_pixels(&dev, w, h, &px).unwrap();
    let readback = BgraReadback::new(&dev, &src).unwrap();
    let stripes_survive = |chroma444: bool| {
        let cfg = EncoderConfig { codec: Codec::H264, backend: FULL_CHROMA_BACKEND.into(), width: w, height: h, fps: 30, bitrate_kbps: 20000, vbv_frames: 8, chroma444 };
        let mut enc = open_encoder(&dev, &cfg).unwrap();
        // Pede a placa: no 4:4:4 ela não serve e o decoder cai pro software.
        let mut dec = AvDecoder::open(Codec::H264, true).unwrap();
        let mut last = None;
        for i in 0..20 {
            for p in readback.read(&dev, &src.tex, None, |bgra, stride| enc.encode_bgra(bgra, stride, None, i == 0)).unwrap().unwrap() {
                if let Some(f) = dec.decode(&p.data).unwrap() {
                    last = Some(f);
                }
            }
        }
        let f = last.expect("nada decodificado");
        let at = |x: u32| f.data[((h / 2 * w + x) * 4) as usize..][..3].to_vec();
        println!("4:4:4={chroma444}: pixel par {:?}, ímpar {:?}", at(100), at(101));
        close(&at(100), [0, 0, 255]) && close(&at(101), [255, 0, 0])
    };
    assert!(!stripes_survive(false), "4:2:0 não devia manter listras de 1 pixel");
    assert!(stripes_survive(true), "4:4:4 tem de manter a cor de cada pixel");
}

#[test]
#[ignore = "precisa de monitor (sessão desbloqueada) e GPU"]
fn screen_capture_delivers_frames() {
    // Com o monitor em economia de energia a DXGI não entrega quadro nenhum.
    unsafe {
        windows::Win32::System::Power::SetThreadExecutionState(windows::Win32::System::Power::ES_DISPLAY_REQUIRED);
    }
    let (dev, mut cap) = ScreenCapture::open(0).unwrap();
    let mut frames = 0;
    let t0 = Instant::now();
    while t0.elapsed().as_millis() < 1000 {
        if let Some(f) = cap.acquire(&dev, 50).unwrap() {
            frames += f.new_image as u32;
        }
    }
    assert!(frames > 0);
}
