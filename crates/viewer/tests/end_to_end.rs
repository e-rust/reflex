//! Host + viewer de verdade, sobre QUIC na loopback, com captura e decode.
//! Precisa de GPU, monitor e FFmpeg (`exe\`):
//! `cargo test -p reflex-viewer --test end_to_end -- --ignored --nocapture`

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use reflex_auth::identity::Identity;
use reflex_auth::store::{Actor, User};
use reflex_host::server::{self, HostDeps};
use reflex_media::caps;
use reflex_net::{LocalCert, Listener};
use reflex_proto::codec::CodecChoice;
use reflex_proto::msg::SessionConfig;
use reflex_proto::perms::Level;
use reflex_viewer::client::{self, Credential, ViewerEvent};
use reflex_viewer::hosts::Hosts;
use tokio::sync::mpsc;

fn cert(name: &str) -> (LocalCert, Identity) {
    let id = Identity::generate(name).unwrap();
    (LocalCert { cert_der: id.cert_der.clone(), key_der: id.key_der.clone() }, id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "precisa de GPU, monitor e FFmpeg"]
async fn registered_viewer_sees_real_frames() {
    unsafe {
        windows::Win32::System::Power::SetThreadExecutionState(windows::Win32::System::Power::ES_DISPLAY_REQUIRED);
    }
    let (host_cert, host_id) = cert("host");
    let (viewer_cert, viewer_id) = cert("viewer");
    let viewer_access = viewer_id.access_id();

    // Host: viewer já cadastrado como controle total.
    let store = server::mem_store(b"segredo");
    store.update(|d| d.add_user(&Actor::LocalAdmin, User::new(viewer_access.clone(), "Teste", Level::FullControl))).unwrap();

    let listener = Listener::bind("127.0.0.1:0".parse().unwrap(), &host_cert).await.unwrap();
    let addr: std::net::SocketAddr = format!("127.0.0.1:{}", listener.port()).parse().unwrap();

    let deps = Arc::new(HostDeps {
        store,
        machine_name: "pc-teste".into(),
        app_version: "0.1".into(),
        encoders: caps::encoders().iter().map(|c| c.cap).collect(),
        monitors: Arc::new(reflex_media::capture::monitors),
        confirm: Arc::new(|_| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(true);
            rx
        }),
        next_session_id: Arc::new(AtomicU64::new(0)),
        user: None,
        hub: Arc::default(),
    });
    tokio::spawn(server::serve(listener, deps));

    // Viewer.
    let decoders = caps::decoders();
    let check = Hosts::verifier(host_id.access_id().to_string());
    let connected = client::connect_and_auth(addr, &viewer_cert, check, reflex_net::TransportMode::Quic, "0.1", decoders, Credential::Key, || {})
        .await
        .unwrap();
    assert_eq!(connected.level, Level::FullControl);

    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let (_in_tx, in_rx) = mpsc::unbounded_channel();
    let cfg = SessionConfig { codec: CodecChoice::Auto, max_fps: 30, bitrate_kbps: 6000, max_width: 1280, max_height: 720, monitor: 0, ..Default::default() };
    tokio::spawn(client::run(connected, cfg, ev_tx, in_rx, tokio::sync::mpsc::unbounded_channel().1, Default::default()));

    let mut frames = 0;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_secs(1), ev_rx.recv()).await {
            Ok(Some(ViewerEvent::Frame(f))) => {
                assert!(f.width > 0 && !f.data.is_empty());
                frames += 1;
                if frames >= 3 {
                    break;
                }
            }
            Ok(Some(ViewerEvent::Disconnected(e))) => panic!("desconectou: {e}"),
            Ok(Some(_)) => {}
            _ => {}
        }
    }
    let _ = (host_id, viewer_access);
    assert!(frames >= 1, "o viewer não recebeu quadro nenhum");
    println!("viewer recebeu {frames} quadros decodificados");
}

/// Conecta um viewer cadastrado e conta quadros decodificados por `secs`.
async fn watch(addr: std::net::SocketAddr, viewer_cert: LocalCert, host: String, secs: u64) -> usize {
    let check = Hosts::verifier(host);
    let connected = client::connect_and_auth(addr, &viewer_cert, check, reflex_net::TransportMode::Quic, "0.1", caps::decoders(), Credential::Key, || {}).await.unwrap();
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let (_in_tx, in_rx) = mpsc::unbounded_channel();
    let cfg = SessionConfig { codec: CodecChoice::Auto, max_fps: 30, bitrate_kbps: 6000, max_width: 1280, max_height: 720, monitor: 0, ..Default::default() };
    let session = tokio::spawn(client::run(connected, cfg, ev_tx, in_rx, mpsc::unbounded_channel().1, Default::default()));
    let mut frames = 0;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(500), ev_rx.recv()).await {
            Ok(Some(ViewerEvent::Frame(_))) => frames += 1,
            Ok(Some(ViewerEvent::Disconnected(e))) => panic!("desconectou: {e}"),
            _ => {}
        }
    }
    session.abort();
    frames
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "precisa de GPU, monitor e FFmpeg"]
async fn two_viewers_share_one_capture() {
    unsafe {
        windows::Win32::System::Power::SetThreadExecutionState(windows::Win32::System::Power::ES_DISPLAY_REQUIRED);
    }
    let (host_cert, host_id) = cert("host");
    let (a_cert, a_id) = cert("a");
    let (b_cert, b_id) = cert("b");
    let store = server::mem_store(b"segredo");
    for id in [&a_id, &b_id] {
        store.update(|d| d.add_user(&Actor::LocalAdmin, User::new(id.access_id(), "Teste", Level::FullControl))).unwrap();
    }
    let listener = Listener::bind("127.0.0.1:0".parse().unwrap(), &host_cert).await.unwrap();
    let addr: std::net::SocketAddr = format!("127.0.0.1:{}", listener.port()).parse().unwrap();
    let deps = Arc::new(HostDeps {
        store,
        machine_name: "pc-teste".into(),
        app_version: "0.1".into(),
        encoders: caps::encoders().iter().map(|c| c.cap).collect(),
        monitors: Arc::new(reflex_media::capture::monitors),
        confirm: Arc::new(|_| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(true);
            rx
        }),
        next_session_id: Arc::new(AtomicU64::new(0)),
        user: None,
        hub: Arc::default(),
    });
    tokio::spawn(server::serve(listener, deps));
    let host = host_id.access_id().to_string();

    // A entra; B entra no meio (antes a 2ª captura falhava: sem imagem).
    let a = tokio::spawn(watch(addr, a_cert, host.clone(), 6));
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let b = watch(addr, b_cert, host.clone(), 3).await;
    let a = a.await.unwrap();
    println!("A: {a} quadros, B: {b} quadros");
    assert!(a >= 1 && b >= 1, "as duas conexões têm de ver a tela (A {a}, B {b})");
}
