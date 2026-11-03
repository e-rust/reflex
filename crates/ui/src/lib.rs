//! Interface do Reflex em winsafe (§5.1, §5.2). Controles nativos do Windows;
//! o vídeo decodificado é apresentado numa janela por `StretchDIBits` (o
//! caminho de MVP do §5.1 — a swapchain D3D11 entra depois sem mexer no resto).
//!
//! - [`addr`]: normalização de endereço e o resumo de status (puro, testado);
//! - [`present`]: a janela de vídeo que pinta o último quadro BGRA;
//! - [`app`]: a janela principal (Conectar) e o laço do viewer.

pub mod addr;
mod identity;
mod keyhook;
mod connect;
mod machine;
mod configclient;
mod app;
mod nav;
mod present;
mod theme;

use winsafe::prelude::*;

/// Abre direto a janela de vídeo conectada em `address` (`reflex --connect`),
/// sem a janela principal. Devolve o código de saída do processo.
pub fn connect(address: &str) -> i32 {
    let Some(normalized) = addr::normalize_address(address) else {
        return 2;
    };
    connect::run_viewer(&normalized, connect::ViewOptions::saved(), std::sync::Arc::new(|_: &str| {}))
}

/// Sobe a janela principal. Devolve o código de saída do processo.
pub fn run() -> i32 {
    match app::MainWindow::new().and_then(|w| w.run()) {
        Ok(code) => code,
        Err(e) => {
            let _ = winsafe::HWND::NULL.MessageBox(&e.to_string(), "Reflex", winsafe::co::MB::ICONERROR);
            1
        }
    }
}
