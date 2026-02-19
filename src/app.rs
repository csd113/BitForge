// src/app.rs
//
// The main application struct and egui rendering loop.
//
// Architecture overview
// ─────────────────────
// • `BitcoinCompilerApp` is stored on the main (UI) thread.
// • Background tasks live on the tokio runtime (`Arc<Runtime>`).
// • Two mpsc channels bridge the worlds:
//     msg_rx / msg_tx   – background → UI  (AppMessage)
//     confirm_rx         – background → UI  (ConfirmRequest — needs Yes/No)
// • When `update()` runs it:
//     1. Drains both channels into local state.
//     2. If a confirmation is pending it renders a modal overlay.
//     3. Renders all other UI.
//     4. Requests a repaint in 50 ms while busy so the log scrolls smoothly.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use tokio::runtime::Runtime;

use crate::compiler::{compile_bitcoin, compile_electrs};
use crate::deps::check_dependencies_task;
use crate::env_setup::{brew_prefix, find_brew, macos_version, setup_build_environment};
use crate::github::{fetch_bitcoin_versions, fetch_electrs_versions};
use crate::messages::{AppMessage, ConfirmRequest};

// Maximum log lines retained in memory to avoid unbounded growth.
const MAX_LOG_LINES: usize = 4_000;

// ─── Modal state ─────────────────────────────────────────────────────────────

enum Modal {
    /// Simple info or error alert — user clicks OK.
    Alert {
        title: String,
        message: String,
        is_error: bool,
    },
    /// Yes / No confirmation — sends answer via oneshot channel.
    Confirm {
        title: String,
        message: String,
        response_tx: tokio::sync::oneshot::Sender<bool>,
    },
}

// Local enum used to communicate user interactions out of the modal rendering
// closure without holding a borrow on `self.modal` at the same time.
enum ModalAction {
    Close,
    Confirm(bool),
}

// ─── App state ────────────────────────────────────────────────────────────────

pub struct BitcoinCompilerApp {
    // ── Configuration ─────────────────────────────────────────────────────────
    target: String,                  // "Bitcoin" | "Electrs" | "Both"
    cores: usize,
    max_cores: usize,
    build_dir: String,

    // ── Version lists ─────────────────────────────────────────────────────────
    bitcoin_versions: Vec<String>,
    selected_bitcoin: String,
    electrs_versions: Vec<String>,
    selected_electrs: String,

    // ── UI state ──────────────────────────────────────────────────────────────
    log_buffer: String,              // append-only terminal log text
    progress: f32,                   // 0.0 – 1.0
    is_busy: bool,                   // disables buttons during a task
    status_bar: String,              // bottom status bar text

    // ── Modal overlay ─────────────────────────────────────────────────────────
    modal: Option<Modal>,

    // ── Channels ──────────────────────────────────────────────────────────────
    msg_rx: Receiver<AppMessage>,
    msg_tx: Sender<AppMessage>,
    confirm_rx: Receiver<ConfirmRequest>,
    confirm_tx: Sender<ConfirmRequest>,

    // ── Tokio runtime ─────────────────────────────────────────────────────────
    runtime: Arc<Runtime>,

    // ── Detected environment ──────────────────────────────────────────────────
    brew: Option<String>,
    brew_pfx: Option<String>,
}

impl BitcoinCompilerApp {
    pub fn new(
        _cc: &eframe::CreationContext<'_>,
        runtime: Arc<Runtime>,
        msg_rx: Receiver<AppMessage>,
        msg_tx: Sender<AppMessage>,
        confirm_rx: Receiver<ConfirmRequest>,
        confirm_tx: Sender<ConfirmRequest>,
    ) -> Self {
        let max_cores = num_cpus::get();
        let default_cores = max_cores.saturating_sub(1).max(1);

        let brew = find_brew();
        let brew_pfx = brew.as_deref().map(brew_prefix);

        let macos = macos_version();
        let status_bar = format!(
            "System: macOS {macos}  |  Homebrew: {}  |  CPUs: {max_cores}",
            brew_pfx.as_deref().unwrap_or("Not Found"),
        );

        let default_build_dir = dirs_home()
            .map(|h| h.join("Downloads/bitcoin_builds").to_string_lossy().to_string())
            .unwrap_or_else(|| "/tmp/bitcoin_builds".to_string());

        let mut app = Self {
            target: "Bitcoin".to_string(),
            cores: default_cores,
            max_cores,
            build_dir: default_build_dir,

            bitcoin_versions: vec!["Loading...".to_string()],
            selected_bitcoin: "Loading...".to_string(),
            electrs_versions: vec!["Loading...".to_string()],
            selected_electrs: "Loading...".to_string(),

            log_buffer: String::new(),
            progress: 0.0,
            is_busy: false,
            status_bar,

            modal: None,

            msg_rx,
            msg_tx,
            confirm_rx,
            confirm_tx,

            runtime,

            brew,
            brew_pfx,
        };

        // ── Initial log splash ─────────────────────────────────────────────────
        let sep = "=".repeat(60);
        let macos_str = macos_version();
        let brew_str = app.brew_pfx.clone().unwrap_or_else(|| "Not Found".to_string());
        let cpu_count = app.max_cores;

        app.append_log(&format!("{sep}\nBitcoin Core & Electrs Compiler\n{sep}\n"));
        app.append_log(&format!("System: macOS {macos_str}\n"));
        app.append_log(&format!("Homebrew: {brew_str}\n"));
        app.append_log(&format!("CPU Cores: {cpu_count}\n"));
        app.append_log(&format!("{sep}\n\n"));
        app.append_log("👉 Click 'Check & Install Dependencies' to begin\n\n");
        app.append_log("📝 Note: Both Bitcoin and Electrs pull source from GitHub\n\n");

        // ── Load version lists in the background ──────────────────────────────
        app.spawn_refresh_all_versions();

        app
    }

    // ─── Log helpers ──────────────────────────────────────────────────────────

    fn append_log(&mut self, msg: &str) {
        self.log_buffer.push_str(msg);

        // Trim oldest lines when the buffer exceeds MAX_LOG_LINES.
        let newline_count = self.log_buffer.chars().filter(|&c| c == '\n').count();
        if newline_count > MAX_LOG_LINES {
            // Drop the oldest half of lines.
            let keep = MAX_LOG_LINES / 2;
            let drop_count = newline_count.saturating_sub(keep);
            if let Some(split_pos) = self
                .log_buffer
                .char_indices()
                .filter_map(|(i, c)| if c == '\n' { Some(i) } else { None })
                .nth(drop_count)
            {
                self.log_buffer = self.log_buffer[split_pos + 1..].to_string();
            }
        }
    }

    // ─── Message channel drain ────────────────────────────────────────────────

    fn drain_messages(&mut self) {
        // Process all pending messages from background tasks.
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                AppMessage::Log(s) => self.append_log(&s),
                AppMessage::Progress(v) => self.progress = v.clamp(0.0, 1.0),
                AppMessage::BitcoinVersionsLoaded(versions) => {
                    if !versions.is_empty() {
                        self.selected_bitcoin = versions[0].clone();
                    }
                    self.bitcoin_versions = versions;
                }
                AppMessage::ElectrsVersionsLoaded(versions) => {
                    if !versions.is_empty() {
                        self.selected_electrs = versions[0].clone();
                    }
                    self.electrs_versions = versions;
                }
                AppMessage::ShowDialog { title, message, is_error } => {
                    self.modal = Some(Modal::Alert { title, message, is_error });
                }
                AppMessage::TaskDone => {
                    self.is_busy = false;
                    self.progress = 0.0;
                }
            }
        }

        // Check for a pending confirmation request (queue one at a time so that
        // a Confirm doesn't arrive while an Alert is still shown).
        if self.modal.is_none() {
            if let Ok(req) = self.confirm_rx.try_recv() {
                self.modal = Some(Modal::Confirm {
                    title: req.title,
                    message: req.message,
                    response_tx: req.response_tx,
                });
            }
        }
    }

    // ─── Background task spawners ─────────────────────────────────────────────

    fn spawn_check_deps(&mut self) {
        let brew = match &self.brew {
            Some(b) => b.clone(),
            None => {
                self.modal = Some(Modal::Alert {
                    title: "Missing Dependency".into(),
                    message: "Homebrew not found!\nPlease install from https://brew.sh".into(),
                    is_error: true,
                });
                return;
            }
        };

        let env = setup_build_environment(self.brew_pfx.as_deref());
        let log_tx = self.msg_tx.clone();
        let confirm_tx = self.confirm_tx.clone();
        let done_tx = self.msg_tx.clone();

        self.is_busy = true;
        self.append_log("\n>>> Starting dependency check...\n");

        self.runtime.spawn(async move {
            match check_dependencies_task(brew, env, log_tx, confirm_tx).await {
                Ok(_) => {}
                Err(e) => {
                    done_tx
                        .send(AppMessage::ShowDialog {
                            title: "Error".into(),
                            message: format!("Dependency check failed: {e}"),
                            is_error: true,
                        })
                        .ok();
                }
            }
            done_tx.send(AppMessage::TaskDone).ok();
        });
    }

    fn spawn_refresh_bitcoin_versions(&self) {
        let tx = self.msg_tx.clone();
        self.runtime.spawn(async move {
            tx.send(AppMessage::Log(
                "\n📡 Fetching Bitcoin versions from GitHub...\n".into(),
            ))
            .ok();
            match fetch_bitcoin_versions().await {
                Ok(versions) => {
                    tx.send(AppMessage::Log(format!(
                        "✓ Loaded {} Bitcoin versions\n",
                        versions.len()
                    )))
                    .ok();
                    tx.send(AppMessage::BitcoinVersionsLoaded(versions)).ok();
                }
                Err(e) => {
                    tx.send(AppMessage::Log(format!(
                        "⚠️  Could not fetch Bitcoin versions: {e}\n"
                    )))
                    .ok();
                    tx.send(AppMessage::ShowDialog {
                        title: "Network Error".into(),
                        message:
                            "Could not fetch Bitcoin versions.\nCheck your internet connection."
                                .into(),
                        is_error: false,
                    })
                    .ok();
                }
            }
        });
    }

    fn spawn_refresh_electrs_versions(&self) {
        let tx = self.msg_tx.clone();
        self.runtime.spawn(async move {
            tx.send(AppMessage::Log(
                "\n📡 Fetching Electrs versions from GitHub...\n".into(),
            ))
            .ok();
            match fetch_electrs_versions().await {
                Ok(versions) => {
                    tx.send(AppMessage::Log(format!(
                        "✓ Loaded {} Electrs versions\n",
                        versions.len()
                    )))
                    .ok();
                    tx.send(AppMessage::ElectrsVersionsLoaded(versions)).ok();
                }
                Err(e) => {
                    tx.send(AppMessage::Log(format!(
                        "⚠️  Could not fetch Electrs versions: {e}\n"
                    )))
                    .ok();
                    tx.send(AppMessage::ShowDialog {
                        title: "Network Error".into(),
                        message:
                            "Could not fetch Electrs versions.\nCheck your internet connection."
                                .into(),
                        is_error: false,
                    })
                    .ok();
                }
            }
        });
    }

    fn spawn_refresh_all_versions(&self) {
        self.spawn_refresh_bitcoin_versions();
        self.spawn_refresh_electrs_versions();
    }

    fn spawn_compile(&mut self) {
        let target = self.target.clone();
        let cores = self.cores;
        let build_dir = PathBuf::from(&self.build_dir);
        let bitcoin_ver = self.selected_bitcoin.clone();
        let electrs_ver = self.selected_electrs.clone();

        // Validate versions are loaded before starting.
        if (target == "Bitcoin" || target == "Both")
            && (bitcoin_ver.is_empty() || bitcoin_ver == "Loading...")
        {
            self.modal = Some(Modal::Alert {
                title: "Error".into(),
                message: "Please wait for Bitcoin versions to load, or click Refresh".into(),
                is_error: true,
            });
            return;
        }
        if (target == "Electrs" || target == "Both")
            && (electrs_ver.is_empty() || electrs_ver == "Loading...")
        {
            self.modal = Some(Modal::Alert {
                title: "Error".into(),
                message: "Please wait for Electrs versions to load, or click Refresh".into(),
                is_error: true,
            });
            return;
        }

        let env = setup_build_environment(self.brew_pfx.as_deref());
        let tx = self.msg_tx.clone();
        let done_tx = self.msg_tx.clone();

        self.is_busy = true;
        self.progress = 0.0;

        self.runtime.spawn(async move {
            tx.send(AppMessage::Progress(0.05)).ok();

            let mut output_dirs: Vec<String> = Vec::new();
            let mut error_occurred = false;

            // ── Bitcoin ────────────────────────────────────────────────────────
            if target == "Bitcoin" || target == "Both" {
                tx.send(AppMessage::Progress(0.1)).ok();
                match compile_bitcoin(&bitcoin_ver, &build_dir, cores, &env, &tx, &tx).await {
                    Ok(dir) => {
                        output_dirs.push(dir.to_string_lossy().to_string());
                        let next_progress = if target == "Both" { 0.5 } else { 0.95 };
                        tx.send(AppMessage::Progress(next_progress)).ok();
                    }
                    Err(e) => {
                        tx.send(AppMessage::Log(format!("\n❌ Compilation failed: {e}\n")))
                            .ok();
                        tx.send(AppMessage::ShowDialog {
                            title: "Compilation Failed".into(),
                            message: e.to_string(),
                            is_error: true,
                        })
                        .ok();
                        error_occurred = true;
                    }
                }
            }

            // ── Electrs ────────────────────────────────────────────────────────
            if !error_occurred && (target == "Electrs" || target == "Both") {
                let start_progress = if target == "Both" { 0.55 } else { 0.1 };
                tx.send(AppMessage::Progress(start_progress)).ok();

                match compile_electrs(&electrs_ver, &build_dir, cores, &env, &tx, &tx).await {
                    Ok(dir) => {
                        output_dirs.push(dir.to_string_lossy().to_string());
                        tx.send(AppMessage::Progress(1.0)).ok();
                    }
                    Err(e) => {
                        tx.send(AppMessage::Log(format!("\n❌ Compilation failed: {e}\n")))
                            .ok();
                        tx.send(AppMessage::ShowDialog {
                            title: "Compilation Failed".into(),
                            message: e.to_string(),
                            is_error: true,
                        })
                        .ok();
                        error_occurred = true;
                    }
                }
            }

            // ── Success dialog ─────────────────────────────────────────────────
            if !error_occurred {
                tx.send(AppMessage::Progress(1.0)).ok();
                let dirs_list = output_dirs
                    .iter()
                    .map(|d| format!("• {d}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                tx.send(AppMessage::ShowDialog {
                    title: "Compilation Complete".into(),
                    message: format!(
                        "✅ {target} compilation completed successfully!\n\nBinaries saved to:\n{dirs_list}"
                    ),
                    is_error: false,
                })
                .ok();
            }

            done_tx.send(AppMessage::TaskDone).ok();
        });
    }

    // ─── Modal rendering ──────────────────────────────────────────────────────
    // We extract data from `self.modal` as owned/copied values, render the
    // window inside the match arm (where the borrow is active), collect
    // the user's action into a local `Option<ModalAction>`, then drop the
    // match borrow and apply the action via mutable access.

    fn render_modal(&mut self, ctx: &egui::Context) {
        let action: Option<ModalAction> = match &self.modal {
            None => return,

            Some(Modal::Alert { title, message, is_error }) => {
                // Clone the data we need inside the closure.
                let title_str = title.clone();
                let msg_str = message.clone();
                let err = *is_error;
                let mut close = false;

                egui::Window::new(title_str.as_str())
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .collapsible(false)
                    .resizable(false)
                    .min_width(340.0)
                    .show(ctx, |ui| {
                        let color = if err {
                            egui::Color32::from_rgb(230, 90, 90)
                        } else {
                            egui::Color32::from_rgb(90, 190, 90)
                        };
                        ui.colored_label(color, if err { "⛔ Error" } else { "ℹ  Info" });
                        ui.separator();
                        ui.label(msg_str.as_str());
                        ui.add_space(8.0);
                        if ui.button("  OK  ").clicked() {
                            close = true;
                        }
                    });

                if close { Some(ModalAction::Close) } else { None }
            }

            Some(Modal::Confirm { title, message, .. }) => {
                let title_str = title.clone();
                let msg_str = message.clone();
                let mut answer: Option<bool> = None;

                egui::Window::new(title_str.as_str())
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .collapsible(false)
                    .resizable(false)
                    .min_width(360.0)
                    .show(ctx, |ui| {
                        ui.label(msg_str.as_str());
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("  Yes  ").clicked() {
                                answer = Some(true);
                            }
                            if ui.button("  No  ").clicked() {
                                answer = Some(false);
                            }
                        });
                    });

                answer.map(ModalAction::Confirm)
            }
        };

        // Apply the action — borrow of self.modal has ended by here.
        match action {
            None => {}
            Some(ModalAction::Close) => {
                self.modal = None;
            }
            Some(ModalAction::Confirm(answer)) => {
                if let Some(Modal::Confirm { response_tx, .. }) = self.modal.take() {
                    response_tx.send(answer).ok();
                }
            }
        }
    }
}

// ─── eframe::App implementation ───────────────────────────────────────────────

impl eframe::App for BitcoinCompilerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── 1. Drain incoming messages ─────────────────────────────────────────
        self.drain_messages();

        // ── 2. Modal overlays (rendered on top of everything) ─────────────────
        self.render_modal(ctx);

        // ── 3. Status bar ─────────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&self.status_bar).small().weak());
            });
        });

        // ── 4. Main content panel ─────────────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.set_min_width(800.0);

            // Header
            ui.vertical_centered(|ui| {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Bitcoin Core & Electrs Compiler")
                        .size(20.0)
                        .strong(),
                );
                ui.add_space(6.0);
            });

            // ── Step 1: Dependency check ──────────────────────────────────────
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Step 1:").strong());
                let btn_enabled = !self.is_busy;
                if ui
                    .add_enabled(
                        btn_enabled,
                        egui::Button::new("Check & Install Dependencies"),
                    )
                    .clicked()
                {
                    self.spawn_check_deps();
                }
            });

            ui.separator();

            // ── Step 2: Build settings ────────────────────────────────────────
            ui.group(|ui| {
                ui.label(egui::RichText::new("Step 2: Select What to Compile").strong());
                ui.add_space(4.0);

                egui::Grid::new("settings_grid")
                    .num_columns(5)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        // Row 0: Target + Cores
                        ui.label("Target:");
                        egui::ComboBox::from_id_source("target_combo")
                            .selected_text(&self.target)
                            .width(130.0)
                            .show_ui(ui, |ui| {
                                for opt in &["Bitcoin", "Electrs", "Both"] {
                                    ui.selectable_value(
                                        &mut self.target,
                                        opt.to_string(),
                                        *opt,
                                    );
                                }
                            });

                        ui.label("CPU Cores:");
                        ui.add(
                            egui::DragValue::new(&mut self.cores)
                                .range(1..=self.max_cores)
                                .speed(1.0),
                        );
                        ui.label(
                            egui::RichText::new(format!("(max: {})", self.max_cores))
                                .small()
                                .weak(),
                        );
                        ui.end_row();

                        // Row 1: Build directory
                        ui.label("Build Directory:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.build_dir)
                                .desired_width(360.0),
                        );
                        ui.label(""); // spacer
                        ui.label(""); // spacer
                        if ui.button("Browse…").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                self.build_dir = folder.to_string_lossy().to_string();
                            }
                        }
                        ui.end_row();
                    });
            });

            ui.add_space(4.0);

            // ── Step 3: Version selection ─────────────────────────────────────
            ui.group(|ui| {
                ui.label(egui::RichText::new("Step 3: Select Versions").strong());
                ui.add_space(4.0);

                egui::Grid::new("versions_grid")
                    .num_columns(3)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        // Bitcoin
                        ui.label("Bitcoin Version:");
                        egui::ComboBox::from_id_source("bitcoin_combo")
                            .selected_text(&self.selected_bitcoin)
                            .width(180.0)
                            .show_ui(ui, |ui| {
                                for v in self.bitcoin_versions.clone() {
                                    ui.selectable_value(
                                        &mut self.selected_bitcoin,
                                        v.clone(),
                                        &v,
                                    );
                                }
                            });
                        if ui.button("Refresh").clicked() {
                            self.spawn_refresh_bitcoin_versions();
                        }
                        ui.end_row();

                        // Electrs
                        ui.label("Electrs Version:");
                        egui::ComboBox::from_id_source("electrs_combo")
                            .selected_text(&self.selected_electrs)
                            .width(180.0)
                            .show_ui(ui, |ui| {
                                for v in self.electrs_versions.clone() {
                                    ui.selectable_value(
                                        &mut self.selected_electrs,
                                        v.clone(),
                                        &v,
                                    );
                                }
                            });
                        if ui.button("Refresh").clicked() {
                            self.spawn_refresh_electrs_versions();
                        }
                        ui.end_row();
                    });
            });

            ui.add_space(6.0);

            // ── Progress bar ──────────────────────────────────────────────────
            ui.label("Progress:");
            ui.add(
                egui::ProgressBar::new(self.progress)
                    .desired_width(ui.available_width())
                    .animate(self.is_busy),
            );

            ui.add_space(6.0);

            // ── Build log terminal ────────────────────────────────────────────
            ui.label(egui::RichText::new("Build Log").strong());

            // Dark frame background to mimic a terminal.
            let log_frame = egui::Frame {
                fill: egui::Color32::from_rgb(18, 18, 18),
                inner_margin: egui::Margin::same(8.0),
                stroke: egui::Stroke::new(1.0, egui::Color32::from_gray(55)),
                ..Default::default()
            };

            // Reserve space for the Compile button below.
            let available_height = ui.available_height() - 56.0;

            log_frame.show(ui, |ui| {
                egui::ScrollArea::both()
                    .stick_to_bottom(true)
                    .max_height(available_height.max(120.0))
                    .min_scrolled_height(120.0)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(&self.log_buffer)
                                .color(egui::Color32::from_rgb(0, 215, 0))
                                .monospace()
                                .size(11.5),
                        );
                    });
            });

            ui.add_space(6.0);

            // ── Compile button ────────────────────────────────────────────────
            ui.vertical_centered(|ui| {
                if ui
                    .add_enabled(
                        !self.is_busy,
                        egui::Button::new(
                            egui::RichText::new("🚀  Start Compilation").size(14.0),
                        )
                        .min_size(egui::vec2(210.0, 36.0)),
                    )
                    .clicked()
                {
                    self.spawn_compile();
                }
            });
        });

        // ── 5. Repaint scheduling ─────────────────────────────────────────────
        // Frequent repaints while a task is running keep the log scrolling
        // smoothly.  When idle, poll less often to avoid wasting CPU.
        if self.is_busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}

// ─── Home directory helper ────────────────────────────────────────────────────

fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}
