// src/main.rs
//
// Entry point for the Bitcoin & Electrs Compiler.
//
// Responsibilities:
//   1. Create the tokio multi-thread runtime (for all background I/O).
//   2. Create the std::sync::mpsc channels (AppMessage) and the
//      ConfirmRequest channel that bridges the UI to async tasks.
//   3. Set a wide PATH in the process environment so child process spawns
//      can find Homebrew, Cargo, etc. (mirrors the Python path-patching at
//      the top of the script).
//   4. Launch the eframe event loop on the main thread.

mod app;
mod compiler;
mod deps;
mod env_setup;
mod github;
mod messages;
mod process;

use std::sync::Arc;

use app::BitcoinCompilerApp;
use env_setup::{brew_prefix, find_brew, setup_build_environment};

fn main() -> eframe::Result<()> {
    // ── 0. Widen PATH for child processes ─────────────────────────────────────
    // The Python script patches os.environ["PATH"] at module load time.
    // We replicate this so that every std::process::Command we spawn inherits
    // the correct PATH, even before setup_build_environment() is called for a
    // specific build task.
    {
        let brew = find_brew();
        let pfx = brew.as_deref().map(brew_prefix);
        let env = setup_build_environment(pfx.as_deref());
        if let Some(path) = env.get("PATH") {
            std::env::set_var("PATH", path);
        }
    }

    // ── 1. Tokio runtime ──────────────────────────────────────────────────────
    // Multi-thread runtime so HTTP + subprocess tasks can run concurrently.
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(4)
            .build()
            .expect("Failed to create tokio runtime"),
    );

    // ── 2. Channels ───────────────────────────────────────────────────────────
    // AppMessage: background tasks → UI
    let (msg_tx, msg_rx) = std::sync::mpsc::channel::<messages::AppMessage>();

    // ConfirmRequest: background tasks need a Yes/No answer from the UI
    let (confirm_tx, confirm_rx) = std::sync::mpsc::channel::<messages::ConfirmRequest>();

    // ── 3. eframe native window options ──────────────────────────────────────
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Bitcoin & Electrs Compiler for macOS")
            .with_inner_size([920.0, 820.0])
            .with_min_inner_size([700.0, 600.0]),
        // Use the wgpu (Metal) renderer on macOS for best performance.
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    // ── 4. Run eframe on the main thread ──────────────────────────────────────
    eframe::run_native(
        "Bitcoin & Electrs Compiler for macOS",
        native_options,
        Box::new(move |cc| {
            // Configure egui visuals for a slightly darker default theme so
            // the dark log terminal blends in better.
            cc.egui_ctx.set_visuals(egui::Visuals::dark());

            Ok(Box::new(BitcoinCompilerApp::new(
                cc,
                runtime,
                msg_rx,
                msg_tx,
                confirm_rx,
                confirm_tx,
            )))
        }),
    )
}
