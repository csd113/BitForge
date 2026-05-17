// src/app.rs
//
// BitForge — main application state and egui render loop.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use tokio::runtime::Runtime;

use crate::compiler::{compile_bitcoin, compile_electrs};
use crate::deps::check_dependencies_task;
use crate::env_setup::{
    brew_prefix, default_build_dir, find_brew, is_supported_platform, platform_summary,
    setup_build_environment, supported_platforms_message,
};
use crate::github::{fetch_bitcoin_versions, fetch_electrs_versions};
use crate::messages::{log_msg, AppMessage, ConfirmRequest};

/// Maximum log lines retained in memory.
const MAX_LOG_LINES: usize = 4_000;
/// Drop to this many lines when the cap is hit.
const TRIM_TO_LINES: usize = MAX_LOG_LINES / 2;
/// Fixed pixel height for the build log terminal panel.
const TERMINAL_HEIGHT: f32 = 260.0;
/// Max width for the centred content column.
const CONTENT_WIDTH: f32 = 860.0;
/// Shared card corner radius.
const CARD_RADIUS: u8 = 8;
/// Shared horizontal spacing inside form-like rows.
const ROW_GAP: f32 = 12.0;

// ─── Colour palette (macOS light mode) ───────────────────────────────────────

mod pal {
    use egui::Color32;
    pub const ACCENT: Color32 = Color32::from_rgb(0, 122, 255); // macOS blue
    pub const ACCENT_TEXT: Color32 = Color32::WHITE;
    pub const SURFACE: Color32 = Color32::from_rgb(250, 250, 252); // card bg
    pub const SURFACE_DARK: Color32 = Color32::from_rgb(31, 32, 36);
    pub const BORDER: Color32 = Color32::from_rgb(212, 212, 218);
    pub const BORDER_DARK: Color32 = Color32::from_rgb(64, 66, 74);
    pub const LABEL_MUTED: Color32 = Color32::from_rgb(128, 128, 138);
    pub const LABEL_MUTED_DARK: Color32 = Color32::from_rgb(170, 172, 182);
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(20, 20, 25);
    pub const TEXT_PRIMARY_DARK: Color32 = Color32::from_rgb(238, 239, 244);
    pub const SUCCESS: Color32 = Color32::from_rgb(52, 199, 89); // macOS green
    pub const WARNING: Color32 = Color32::from_rgb(255, 149, 0); // macOS orange
    pub const DANGER: Color32 = Color32::from_rgb(255, 59, 48); // macOS red
    pub const PAGE_BG: Color32 = Color32::from_rgb(236, 236, 240); // window bg
    pub const PAGE_BG_DARK: Color32 = Color32::from_rgb(22, 23, 27);
    pub const STATUS_BG: Color32 = Color32::from_rgb(242, 242, 246);
    pub const STATUS_BG_DARK: Color32 = Color32::from_rgb(28, 29, 33);

    // Terminal stays dark
    pub const TERM_BG: Color32 = Color32::from_rgb(18, 18, 18);
    pub const TERM_TEXT: Color32 = Color32::from_rgb(0, 215, 0);
    pub const TERM_BORDER: Color32 = Color32::from_rgb(55, 55, 55);
}

// ─── Modal ────────────────────────────────────────────────────────────────────

enum Modal {
    Alert {
        title: String,
        message: String,
        is_error: bool,
    },
    Confirm {
        title: String,
        message: String,
        response_tx: tokio::sync::oneshot::Sender<bool>,
    },
}

enum ModalAction {
    Close,
    Confirm(bool),
}

// ─── App state ────────────────────────────────────────────────────────────────

pub struct BitForgeApp {
    // Configuration
    target: String,
    cores: usize,
    max_cores: usize,
    build_dir: String,

    // Version lists
    bitcoin_versions: Vec<String>,
    selected_bitcoin: String,
    electrs_versions: Vec<String>,
    selected_electrs: String,

    // UI state
    log_buffer: String,
    log_line_count: usize,
    progress: f32,
    is_busy: bool,
    status_bar: String,

    // Modal
    modal: Option<Modal>,

    // Channels
    msg_rx: Receiver<AppMessage>,
    msg_tx: Sender<AppMessage>,
    confirm_rx: Receiver<ConfirmRequest>,
    confirm_tx: Sender<ConfirmRequest>,

    // Runtime
    runtime: Arc<Runtime>,

    // Environment
    brew: Option<String>,
    brew_pfx: Option<String>,
}

impl BitForgeApp {
    pub fn new(
        _cc: &eframe::CreationContext<'_>,
        runtime: Arc<Runtime>,
        msg_rx: Receiver<AppMessage>,
        msg_tx: Sender<AppMessage>,
        confirm_rx: Receiver<ConfirmRequest>,
        confirm_tx: Sender<ConfirmRequest>,
    ) -> Self {
        let max_cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let default_cores = max_cores.saturating_sub(1).max(1);

        let brew = find_brew();
        let brew_pfx = brew.as_deref().map(brew_prefix);
        let platform = platform_summary();
        let dependency_source = if cfg!(target_os = "macos") {
            format!("Homebrew: {}", brew_pfx.as_deref().unwrap_or("not found"))
        } else {
            "Linux packages: system package manager".to_owned()
        };

        let status_bar = format!("{platform}   ·   {dependency_source}   ·   {max_cores} CPUs");

        let default_build_dir = default_build_dir().to_string_lossy().into_owned();

        let mut app = Self {
            target: "Bitcoin".to_owned(),
            cores: default_cores,
            max_cores,
            build_dir: default_build_dir,

            bitcoin_versions: vec!["Loading...".to_owned()],
            selected_bitcoin: "Loading...".to_owned(),
            electrs_versions: vec!["Loading...".to_owned()],
            selected_electrs: "Loading...".to_owned(),

            log_buffer: String::new(),
            log_line_count: 0,
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

        // Splash — borrow ends before first append_log call
        let sep = "=".repeat(60);
        let dependency_str = if cfg!(target_os = "macos") {
            app.brew_pfx.as_deref().unwrap_or("Not Found").to_owned()
        } else {
            "system package manager".to_owned()
        };
        let cpus = app.max_cores;

        app.append_log(&format!(
            "{sep}\nBitForge — Bitcoin Core & Electrs Compiler\n{sep}\n"
        ));
        app.append_log(&format!("System: {platform}\n"));
        app.append_log(&format!("Dependencies: {dependency_str}\n"));
        app.append_log(&format!("CPU Cores: {cpus}\n"));
        if !is_supported_platform() {
            app.append_log(supported_platforms_message());
            app.append_log("\n");
        }
        app.append_log(&format!("{sep}\n\n"));
        app.append_log("Next step: click \"Check Dependencies\" before compiling.\n\n");
        app.append_log("Bitcoin Core and Electrs are compiled from source via GitHub.\n\n");

        app.spawn_refresh_all_versions();
        app
    }

    // ─── Log helpers ──────────────────────────────────────────────────────────

    fn append_log(&mut self, msg: &str) {
        // Process character-by-character so that bare \r (carriage return)
        // gets true terminal semantics: go back to the start of the current
        // line and overwrite it.  This keeps cmake/make/git progress lines
        // as a single updating line instead of hundreds of stacked copies.
        for ch in msg.chars() {
            match ch {
                '\r' => {
                    // Carriage return: truncate back to the last newline so
                    // the next characters overwrite the current line.
                    if let Some(pos) = self.log_buffer.rfind('\n') {
                        self.log_buffer.truncate(pos + 1);
                    } else {
                        self.log_buffer.clear();
                    }
                    // Don't change log_line_count — we're still on the same line.
                }
                '\n' => {
                    self.log_buffer.push('\n');
                    self.log_line_count += 1;
                }
                c => {
                    self.log_buffer.push(c);
                }
            }
        }

        if self.log_line_count > MAX_LOG_LINES {
            let drop_count = self.log_line_count.saturating_sub(TRIM_TO_LINES);
            let mut remaining = drop_count;
            if let Some(split_pos) = self.log_buffer.char_indices().find_map(|(i, c)| {
                if c == '\n' {
                    if remaining == 0 {
                        return Some(i);
                    }
                    remaining -= 1;
                }
                None
            }) {
                self.log_buffer = self.log_buffer[split_pos + 1..].to_owned();
                self.log_line_count = TRIM_TO_LINES;
            }
        }
    }

    // ─── Message drain ────────────────────────────────────────────────────────

    fn drain_messages(&mut self) {
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                AppMessage::Log(s) => self.append_log(&s),
                AppMessage::Progress(v) => self.progress = v.clamp(0.0, 1.0),
                AppMessage::BitcoinVersionsLoaded(versions) => {
                    if let Some(first) = versions.first() {
                        self.selected_bitcoin = first.clone();
                    }
                    self.bitcoin_versions = versions;
                }
                AppMessage::BitcoinVersionsFailed => {
                    self.bitcoin_versions.clear();
                    "Unavailable".clone_into(&mut self.selected_bitcoin);
                }
                AppMessage::ElectrsVersionsLoaded(versions) => {
                    if let Some(first) = versions.first() {
                        self.selected_electrs = first.clone();
                    }
                    self.electrs_versions = versions;
                }
                AppMessage::ElectrsVersionsFailed => {
                    self.electrs_versions.clear();
                    "Unavailable".clone_into(&mut self.selected_electrs);
                }
                AppMessage::ShowDialog {
                    title,
                    message,
                    is_error,
                } => {
                    self.modal = Some(Modal::Alert {
                        title,
                        message,
                        is_error,
                    });
                }
                AppMessage::TaskDone => {
                    self.is_busy = false;
                    self.progress = 0.0;
                }
            }
        }

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
        if cfg!(target_os = "macos") && self.brew.is_none() {
            self.modal = Some(Modal::Alert {
                title: "Homebrew Not Found".into(),
                message:
                    "Homebrew is required on macOS Apple Silicon.\nInstall it from https://brew.sh then restart BitForge."
                        .into(),
                is_error: true,
            });
            return;
        }

        let env = setup_build_environment(self.brew_pfx.as_deref());
        let tx = self.msg_tx.clone();
        let confirm_tx = self.confirm_tx.clone();
        let brew = self.brew.clone();

        self.is_busy = true;
        self.append_log("\n>>> Starting dependency check...\n");

        self.runtime.spawn(async move {
            match check_dependencies_task(brew, env, tx.clone(), confirm_tx).await {
                Ok(_) => {}
                Err(e) => {
                    tx.send(AppMessage::ShowDialog {
                        title: "Error".into(),
                        message: format!("Dependency check failed:\n{e}"),
                        is_error: true,
                    })
                    .ok();
                }
            }
            tx.send(AppMessage::TaskDone).ok();
        });
    }

    fn spawn_refresh_bitcoin_versions(&mut self) {
        self.bitcoin_versions = vec!["Loading...".to_owned()];
        "Loading...".clone_into(&mut self.selected_bitcoin);

        let tx = self.msg_tx.clone();
        self.runtime.spawn(async move {
            log_msg(&tx, "\n📡 Fetching Bitcoin versions from GitHub...\n");
            match fetch_bitcoin_versions().await {
                Ok(versions) => {
                    log_msg(
                        &tx,
                        &format!("✓ Loaded {} Bitcoin versions\n", versions.len()),
                    );
                    tx.send(AppMessage::BitcoinVersionsLoaded(versions)).ok();
                }
                Err(e) => {
                    log_msg(&tx, &format!("⚠️  Could not fetch Bitcoin versions: {e}\n"));
                    tx.send(AppMessage::BitcoinVersionsFailed).ok();
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

    fn spawn_refresh_electrs_versions(&mut self) {
        self.electrs_versions = vec!["Loading...".to_owned()];
        "Loading...".clone_into(&mut self.selected_electrs);

        let tx = self.msg_tx.clone();
        self.runtime.spawn(async move {
            log_msg(&tx, "\n📡 Fetching Electrs versions from GitHub...\n");
            match fetch_electrs_versions().await {
                Ok(versions) => {
                    log_msg(
                        &tx,
                        &format!("✓ Loaded {} Electrs versions\n", versions.len()),
                    );
                    tx.send(AppMessage::ElectrsVersionsLoaded(versions)).ok();
                }
                Err(e) => {
                    log_msg(&tx, &format!("⚠️  Could not fetch Electrs versions: {e}\n"));
                    tx.send(AppMessage::ElectrsVersionsFailed).ok();
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

    fn spawn_refresh_all_versions(&mut self) {
        self.spawn_refresh_bitcoin_versions();
        self.spawn_refresh_electrs_versions();
    }

    fn spawn_compile(&mut self) {
        let target = self.target.clone();
        let cores = self.cores;
        let build_dir = PathBuf::from(&self.build_dir);
        let bitcoin_ver = self.selected_bitcoin.clone();
        let electrs_ver = self.selected_electrs.clone();

        let loading = |s: &str| s.is_empty() || !version_is_ready(s);
        if (target == "Bitcoin" || target == "Both") && loading(&bitcoin_ver) {
            self.modal = Some(Modal::Alert {
                title: "Not Ready".into(),
                message: "Please wait for Bitcoin versions to load, or click Refresh.".into(),
                is_error: true,
            });
            return;
        }
        if (target == "Electrs" || target == "Both") && loading(&electrs_ver) {
            self.modal = Some(Modal::Alert {
                title: "Not Ready".into(),
                message: "Please wait for Electrs versions to load, or click Refresh.".into(),
                is_error: true,
            });
            return;
        }

        let env = setup_build_environment(self.brew_pfx.as_deref());
        let tx = self.msg_tx.clone();

        self.is_busy = true;
        self.progress = 0.0;

        self.runtime.spawn(async move {
            tx.send(AppMessage::Progress(0.05)).ok();
            let mut output_dirs: Vec<String> = Vec::new();
            let mut error_occurred = false;

            if target == "Bitcoin" || target == "Both" {
                tx.send(AppMessage::Progress(0.1)).ok();
                match compile_bitcoin(&bitcoin_ver, &build_dir, cores, &env, &tx).await {
                    Ok(dir) => {
                        output_dirs.push(dir.to_string_lossy().into_owned());
                        tx.send(AppMessage::Progress(if target == "Both" {
                            0.5
                        } else {
                            0.95
                        }))
                        .ok();
                    }
                    Err(e) => {
                        log_msg(&tx, &format!("\n❌ Compilation failed: {e}\n"));
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

            if !error_occurred && (target == "Electrs" || target == "Both") {
                tx.send(AppMessage::Progress(if target == "Both" {
                    0.55
                } else {
                    0.1
                }))
                .ok();
                match compile_electrs(&electrs_ver, &build_dir, cores, &env, &tx).await {
                    Ok(dir) => {
                        output_dirs.push(dir.to_string_lossy().into_owned());
                        tx.send(AppMessage::Progress(1.0)).ok();
                    }
                    Err(e) => {
                        log_msg(&tx, &format!("\n❌ Compilation failed: {e}\n"));
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
                        "✅ {target} compiled successfully!\n\nBinaries saved to:\n{dirs_list}"
                    ),
                    is_error: false,
                })
                .ok();
            }

            tx.send(AppMessage::TaskDone).ok();
        });
    }

    // ─── Modal rendering ──────────────────────────────────────────────────────

    fn render_modal(&mut self, ctx: &egui::Context) {
        let action: Option<ModalAction> = match &self.modal {
            None => return,

            Some(Modal::Alert {
                title,
                message,
                is_error,
            }) => {
                let title_str = title.clone();
                let msg_str = message.clone();
                let err = *is_error;
                let mut close = false;

                egui::Window::new(title_str.as_str())
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .collapsible(false)
                    .resizable(false)
                    .min_width(360.0)
                    .max_width(480.0)
                    .show(ctx, |ui| {
                        ui.add_space(2.0);
                        let (state_label, color) = if err {
                            ("Error", pal::DANGER)
                        } else {
                            ("Ready", pal::SUCCESS)
                        };
                        status_pill(ui, state_label, color);
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.add(egui::Label::new(msg_str.as_str()).wrap());
                        ui.add_space(12.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            if ui.add(accent_button("OK")).clicked() {
                                close = true;
                            }
                        });
                        ui.add_space(2.0);
                    });

                if close {
                    Some(ModalAction::Close)
                } else {
                    None
                }
            }

            Some(Modal::Confirm { title, message, .. }) => {
                let title_str = title.clone();
                let msg_str = message.clone();
                let mut answer: Option<bool> = None;

                egui::Window::new(title_str.as_str())
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .collapsible(false)
                    .resizable(false)
                    .min_width(380.0)
                    .max_width(500.0)
                    .show(ctx, |ui| {
                        ui.add_space(6.0);
                        status_pill(ui, "Action Required", pal::WARNING);
                        ui.add_space(8.0);
                        ui.add(egui::Label::new(msg_str.as_str()).wrap());
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            if ui.add(accent_button("Install")).clicked() {
                                answer = Some(true);
                            }
                            ui.add_space(6.0);
                            if ui
                                .button(egui::RichText::new("Cancel").size(13.0))
                                .clicked()
                            {
                                answer = Some(false);
                            }
                        });
                        ui.add_space(2.0);
                    });

                answer.map(ModalAction::Confirm)
            }
        };

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

    // ─── Content renderer (called inside centred column) ──────────────────────

    fn render_content(&mut self, ui: &mut egui::Ui) {
        Self::render_header(ui);
        ui.add_space(20.0);
        self.render_dependency_section(ui);
        ui.add_space(10.0);
        self.render_build_settings_section(ui);
        ui.add_space(10.0);
        self.render_version_section(ui);
        ui.add_space(10.0);
        self.render_progress_section(ui);
        ui.add_space(10.0);
        self.render_log_section(ui);
        ui.add_space(18.0);
        self.render_compile_button(ui);
    }

    fn render_header(ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("⚙  BitForge")
                    .size(26.0)
                    .strong()
                    .color(text_primary(ui)),
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Bitcoin Core & Electrs Compiler")
                    .size(13.0)
                    .color(label_muted(ui)),
            );
        });
    }

    fn render_dependency_section(&mut self, ui: &mut egui::Ui) {
        section_card(ui, "Step 1 — Check Dependencies", |ui| {
            ui.horizontal_wrapped(|ui| {
                let (label, color) = if self.is_busy {
                    ("Running", pal::WARNING)
                } else if !is_supported_platform() {
                    ("Unsupported", pal::DANGER)
                } else if cfg!(target_os = "macos") && self.brew.is_none() {
                    ("Needs Homebrew", pal::DANGER)
                } else {
                    ("Ready to Check", pal::SUCCESS)
                };
                status_pill(ui, label, color);
                ui.add_space(ROW_GAP);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "Verifies native packages and Rust before long builds start.",
                        )
                        .size(12.5)
                        .color(label_muted(ui)),
                    )
                    .wrap(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(!self.is_busy, accent_button("Check Dependencies"))
                        .on_disabled_hover_text("Wait for the current task to finish.")
                        .clicked()
                    {
                        self.spawn_check_deps();
                    }
                });
            });
        });
    }

    fn render_build_settings_section(&mut self, ui: &mut egui::Ui) {
        section_card(ui, "Step 2 — Configure Build", |ui| {
            egui::Grid::new("settings_grid")
                .num_columns(4)
                .spacing([ROW_GAP, 10.0])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Target").color(label_muted(ui)));
                    egui::ComboBox::from_id_salt("target_combo")
                        .selected_text(&self.target)
                        .width(140.0)
                        .show_ui(ui, |ui: &mut egui::Ui| {
                            for opt in &["Bitcoin", "Electrs", "Both"] {
                                ui.selectable_value(&mut self.target, opt.to_string(), *opt);
                            }
                        });

                    ui.label(egui::RichText::new("CPU Cores").color(label_muted(ui)));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.cores)
                                .range(1..=self.max_cores)
                                .speed(1.0),
                        );
                        ui.label(
                            egui::RichText::new(format!("of {}", self.max_cores))
                                .small()
                                .color(label_muted(ui)),
                        );
                    });
                    ui.end_row();

                    ui.label(egui::RichText::new("Output Folder").color(label_muted(ui)));
                    let path_response = ui.add(
                        egui::TextEdit::singleline(&mut self.build_dir)
                            .desired_width((ui.available_width() - 112.0).max(260.0))
                            .font(egui::TextStyle::Monospace),
                    );
                    path_response.on_hover_text(&self.build_dir);
                    ui.label("");
                    if ui.button("Browse…").clicked() {
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            self.build_dir = folder.to_string_lossy().into_owned();
                        }
                    }
                    ui.end_row();
                });
        });
    }

    fn render_version_section(&mut self, ui: &mut egui::Ui) {
        section_card(ui, "Step 3 — Select Versions", |ui| {
            egui::Grid::new("versions_grid")
                .num_columns(4)
                .spacing([ROW_GAP, 10.0])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Bitcoin Core").color(label_muted(ui)));
                    egui::ComboBox::from_id_salt("bitcoin_combo")
                        .selected_text(&self.selected_bitcoin)
                        .width(200.0)
                        .show_ui(ui, |ui: &mut egui::Ui| {
                            for v in &self.bitcoin_versions {
                                ui.selectable_value(
                                    &mut self.selected_bitcoin,
                                    v.clone(),
                                    v.as_str(),
                                );
                            }
                        });
                    if ui.button("Refresh").clicked() {
                        self.spawn_refresh_bitcoin_versions();
                    }
                    version_status(ui, &self.selected_bitcoin, self.bitcoin_versions.len());
                    ui.end_row();

                    ui.label(egui::RichText::new("Electrs").color(label_muted(ui)));
                    egui::ComboBox::from_id_salt("electrs_combo")
                        .selected_text(&self.selected_electrs)
                        .width(200.0)
                        .show_ui(ui, |ui: &mut egui::Ui| {
                            for v in &self.electrs_versions {
                                ui.selectable_value(
                                    &mut self.selected_electrs,
                                    v.clone(),
                                    v.as_str(),
                                );
                            }
                        });
                    if ui.button("Refresh").clicked() {
                        self.spawn_refresh_electrs_versions();
                    }
                    version_status(ui, &self.selected_electrs, self.electrs_versions.len());
                    ui.end_row();
                });
        });
    }

    fn render_progress_section(&self, ui: &mut egui::Ui) {
        section_card(ui, "Build Progress", |ui| {
            let (label, color) = if self.is_busy {
                (
                    format!("Working · {:.0}%", self.progress * 100.0),
                    pal::WARNING,
                )
            } else if self.progress >= 1.0 {
                ("Complete".to_owned(), pal::SUCCESS)
            } else {
                ("Idle".to_owned(), label_muted(ui))
            };

            ui.horizontal(|ui| {
                ui.add(
                    egui::ProgressBar::new(self.progress)
                        .desired_width((ui.available_width() - 116.0).max(180.0))
                        .animate(self.is_busy)
                        .text(""),
                );
                ui.add_space(6.0);
                ui.label(egui::RichText::new(label).small().strong().color(color));
            });
        });
    }

    fn render_log_section(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Build Log")
                    .strong()
                    .color(text_primary(ui)),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!("{} lines retained", self.log_line_count))
                    .small()
                    .color(label_muted(ui)),
            );
        });
        ui.add_space(4.0);

        egui::Frame {
            fill: pal::TERM_BG,
            stroke: egui::Stroke::new(1.0, pal::TERM_BORDER),
            inner_margin: egui::Margin::same(10),
            corner_radius: egui::CornerRadius::same(CARD_RADIUS),
            outer_margin: egui::Margin::ZERO,
            ..Default::default()
        }
        .show(ui, |ui| {
            ui.set_min_height(TERMINAL_HEIGHT);
            ui.set_max_height(TERMINAL_HEIGHT);

            egui::ScrollArea::both()
                .id_salt("build_log")
                .stick_to_bottom(true)
                .max_height(TERMINAL_HEIGHT)
                .min_scrolled_height(TERMINAL_HEIGHT)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&self.log_buffer)
                                .color(pal::TERM_TEXT)
                                .monospace()
                                .size(11.5),
                        )
                        .selectable(false),
                    );
                });
        });
    }

    fn render_compile_button(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            let label = if self.is_busy {
                "Compiling…"
            } else {
                "Start Compilation"
            };
            let needs_bitcoin = self.target == "Bitcoin" || self.target == "Both";
            let needs_electrs = self.target == "Electrs" || self.target == "Both";
            let versions_ready = (!needs_bitcoin || version_is_ready(&self.selected_bitcoin))
                && (!needs_electrs || version_is_ready(&self.selected_electrs));
            let can_compile = !self.is_busy && versions_ready && !self.build_dir.trim().is_empty();
            if ui
                .add_enabled(
                    can_compile,
                    egui::Button::new(
                        egui::RichText::new(label)
                            .size(15.0)
                            .color(pal::ACCENT_TEXT)
                            .strong(),
                    )
                    .fill(pal::ACCENT)
                    .stroke(egui::Stroke::NONE)
                    .min_size(egui::vec2(220.0, 40.0)),
                )
                .on_disabled_hover_text(if self.is_busy {
                    "Wait for the current task to finish."
                } else if !versions_ready {
                    "Wait for version lists to load, or use Refresh."
                } else {
                    "Choose an output folder before compiling."
                })
                .clicked()
            {
                self.spawn_compile();
            }
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Outputs are copied into a versioned binaries folder.")
                    .small()
                    .color(label_muted(ui)),
            );
        });
    }
}

// ─── UI helpers ───────────────────────────────────────────────────────────────

fn text_primary(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        pal::TEXT_PRIMARY_DARK
    } else {
        pal::TEXT_PRIMARY
    }
}

fn label_muted(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        pal::LABEL_MUTED_DARK
    } else {
        pal::LABEL_MUTED
    }
}

fn surface(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        pal::SURFACE_DARK
    } else {
        pal::SURFACE
    }
}

fn border(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        pal::BORDER_DARK
    } else {
        pal::BORDER
    }
}

fn page_bg(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        pal::PAGE_BG_DARK
    } else {
        pal::PAGE_BG
    }
}

fn status_bg(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        pal::STATUS_BG_DARK
    } else {
        pal::STATUS_BG
    }
}

fn status_pill(ui: &mut egui::Ui, label: &str, color: egui::Color32) {
    egui::Frame {
        fill: color.gamma_multiply(if ui.visuals().dark_mode { 0.22 } else { 0.14 }),
        stroke: egui::Stroke::new(1.0, color.gamma_multiply(0.78)),
        corner_radius: egui::CornerRadius::same(u8::MAX),
        inner_margin: egui::Margin::symmetric(9, 3),
        ..Default::default()
    }
    .show(ui, |ui| {
        ui.label(
            egui::RichText::new(label)
                .small()
                .strong()
                .color(if ui.visuals().dark_mode {
                    egui::Color32::WHITE
                } else {
                    color
                }),
        );
    });
}

fn version_status(ui: &mut egui::Ui, selected: &str, count: usize) {
    if selected == "Loading..." {
        status_pill(ui, "Loading", pal::WARNING);
    } else if selected == "Unavailable" || count == 0 {
        status_pill(ui, "Unavailable", pal::DANGER);
    } else {
        status_pill(ui, "Ready", pal::SUCCESS);
    }
}

fn version_is_ready(selected: &str) -> bool {
    !matches!(selected, "" | "Loading..." | "Unavailable")
}

/// macOS-style filled accent button.
fn accent_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(
        egui::RichText::new(label)
            .color(pal::ACCENT_TEXT)
            .strong()
            .size(13.0),
    )
    .fill(pal::ACCENT)
    .stroke(egui::Stroke::NONE)
    .min_size(egui::vec2(100.0, 28.0))
}

/// Render a titled card section.
fn section_card(ui: &mut egui::Ui, heading: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame {
        fill: surface(ui),
        stroke: egui::Stroke::new(1.0, border(ui)),
        corner_radius: egui::CornerRadius::same(CARD_RADIUS),
        inner_margin: egui::Margin::symmetric(16, 12),
        outer_margin: egui::Margin::ZERO,
        ..Default::default()
    }
    .show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(
            egui::RichText::new(heading)
                .strong()
                .size(13.0)
                .color(text_primary(ui)),
        );
        ui.add_space(8.0);
        body(ui);
    });
}

// ─── eframe::App ──────────────────────────────────────────────────────────────

impl eframe::App for BitForgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.drain_messages();
        self.render_modal(&ctx);

        // ── Status bar ────────────────────────────────────────────────────────
        egui::Panel::bottom("status_bar")
            .frame(egui::Frame {
                fill: status_bg(&ctx),
                stroke: egui::Stroke::new(
                    1.0,
                    if ctx.global_style().visuals.dark_mode {
                        pal::BORDER_DARK
                    } else {
                        pal::BORDER
                    },
                ),
                inner_margin: egui::Margin::symmetric(16, 5),
                ..Default::default()
            })
            .show_inside(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    status_pill(
                        ui,
                        if self.is_busy { "Working" } else { "Ready" },
                        if self.is_busy {
                            pal::WARNING
                        } else {
                            pal::SUCCESS
                        },
                    );
                    ui.add_space(8.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&self.status_bar)
                                .small()
                                .color(label_muted(ui)),
                        )
                        .wrap(),
                    );
                });
            });

        // ── Main window ───────────────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(egui::Frame {
                fill: page_bg(&ctx),
                inner_margin: egui::Margin::ZERO,
                ..Default::default()
            })
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        // Horizontal centering: equal padding on both sides.
                        let total = ui.available_width();
                        let pad = ((total - CONTENT_WIDTH) / 2.0).max(16.0);

                        ui.add_space(20.0);
                        ui.horizontal(|ui| {
                            ui.add_space(pad);
                            ui.vertical(|ui| {
                                ui.set_width(CONTENT_WIDTH.min(total - pad * 2.0));
                                self.render_content(ui);
                            });
                        });
                        ui.add_space(28.0);
                    });
            });

        ctx.request_repaint_after(if self.is_busy {
            std::time::Duration::from_millis(50)
        } else {
            std::time::Duration::from_millis(250)
        });
    }
}
