// src/main.rs — BitForge entry point.

mod app;
mod compiler;
mod deps;
mod env_setup;
mod github;
mod messages;
mod process;

use std::io::Write as _;
use std::sync::Arc;

use anyhow::Result;
use app::BitForgeApp;
use env_setup::{brew_prefix, find_brew, is_supported_platform, setup_build_environment};

#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
)))]
compile_error!("BitForge supports macOS Apple Silicon, Linux x86_64, and Linux ARM64 only.");

fn main() {
    if let Err(err) = run() {
        let message = format!("BitForge failed to start:\n\n{err}");
        let _ = rfd::MessageDialog::new()
            .set_title("BitForge Startup Failed")
            .set_description(&message)
            .set_buttons(rfd::MessageButtons::Ok)
            .set_level(rfd::MessageLevel::Error)
            .show();
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "{message}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    if !is_supported_platform() {
        return Err(anyhow::anyhow!(
            "{}\nCurrent platform: {} {}",
            env_setup::supported_platforms_message(),
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    }

    // ── 0. Widen PATH for child processes ─────────────────────────────────────
    let brew = find_brew();
    let pfx = brew.as_deref().map(brew_prefix);
    let env = setup_build_environment(pfx.as_deref());
    if let Some(path) = env.get("PATH") {
        std::env::set_var("PATH", path);
    }

    // ── 1. Tokio runtime ──────────────────────────────────────────────────────
    let worker_threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(worker_threads)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create tokio runtime: {e}"))?,
    );

    // ── 2. Channels ───────────────────────────────────────────────────────────
    let (msg_tx, msg_rx) = std::sync::mpsc::channel::<messages::AppMessage>();
    let (confirm_tx, confirm_rx) = std::sync::mpsc::channel::<messages::ConfirmRequest>();

    // ── 3. Window ─────────────────────────────────────────────────────────────
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("BitForge")
            .with_inner_size([960.0, 840.0])
            .with_min_inner_size([720.0, 620.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    // ── 4. Run on main thread ─────────────────────────────────────────────────
    eframe::run_native(
        "BitForge",
        native_options,
        Box::new(move |cc| {
            let mut visuals = cc.egui_ctx.global_style().visuals.clone();

            // ── Button / widget contrast ───────────────────────────────────────
            // Keep the user's light/dark theme, but tune neutral controls so
            // secondary buttons and inputs remain visible inside BitForge cards.
            let (idle_fill, hover_fill, click_fill, btn_stroke) = if visuals.dark_mode {
                (
                    egui::Color32::from_rgb(54, 56, 64),
                    egui::Color32::from_rgb(68, 70, 80),
                    egui::Color32::from_rgb(82, 84, 96),
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(92, 94, 106)),
                )
            } else {
                (
                    egui::Color32::from_rgb(196, 196, 202),
                    egui::Color32::from_rgb(176, 176, 186),
                    egui::Color32::from_rgb(156, 156, 166),
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(160, 160, 170)),
                )
            };

            visuals.widgets.inactive.bg_fill = idle_fill;
            visuals.widgets.inactive.weak_bg_fill = idle_fill;
            visuals.widgets.inactive.bg_stroke = btn_stroke;
            visuals.widgets.hovered.bg_fill = hover_fill;
            visuals.widgets.hovered.weak_bg_fill = hover_fill;
            visuals.widgets.hovered.bg_stroke = btn_stroke;
            visuals.widgets.active.bg_fill = click_fill;
            visuals.widgets.active.weak_bg_fill = click_fill;

            // ── Selection / accent ─────────────────────────────────────────────
            visuals.selection.bg_fill = egui::Color32::from_rgb(0, 122, 255);
            visuals.selection.stroke = egui::Stroke::NONE;
            visuals.hyperlink_color = egui::Color32::from_rgb(0, 122, 255);

            // ── Subtle window shadow ───────────────────────────────────────────
            visuals.popup_shadow = egui::Shadow::NONE;
            visuals.window_shadow = egui::Shadow {
                offset: [0, 4],
                blur: 16,
                spread: 0,
                color: egui::Color32::from_black_alpha(40),
            };

            cc.egui_ctx.set_visuals(visuals);

            Ok(Box::new(BitForgeApp::new(
                cc, runtime, msg_rx, msg_tx, confirm_rx, confirm_tx,
            )))
        }),
    )?;

    Ok(())
}
