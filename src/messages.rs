// src/messages.rs
//
// All message types that flow between background tokio tasks and the main
// egui render thread.  Using typed enums (rather than raw strings) keeps
// the communication contract explicit and compiler-checked.

use tokio::sync::oneshot;

// ─── AppMessage ──────────────────────────────────────────────────────────────
// Sent FROM background tasks TO the UI via std::sync::mpsc::Sender<AppMessage>.
// std::sync::mpsc is used (not tokio's) because the receiving end lives on the
// synchronous egui main thread and must call try_recv() inside update().

#[derive(Debug)]
pub enum AppMessage {
    /// Append text to the dark terminal log widget
    Log(String),

    /// Set the progress bar value (0.0 – 1.0)
    Progress(f32),

    /// Populate the Bitcoin version combobox
    BitcoinVersionsLoaded(Vec<String>),

    /// Populate the Electrs version combobox
    ElectrsVersionsLoaded(Vec<String>),

    /// Show an informational / error overlay in the UI (no reply needed)
    ShowDialog {
        title: String,
        message: String,
        is_error: bool,
    },

    /// A background task completed — re-enable the "Start Compilation" button
    TaskDone,
}

// ─── ConfirmRequest ───────────────────────────────────────────────────────────
// Background tasks that need a Yes / No answer (e.g. "Install missing deps?")
// send a ConfirmRequest through a *separate* std::sync::mpsc channel.
//
// The background task creates a oneshot channel, passes the Sender to the
// request, and then `.await`s the Receiver.  The UI thread, on the next
// frame, shows the modal and sends the answer back through the oneshot Sender.

pub struct ConfirmRequest {
    pub title:       String,
    pub message:     String,
    /// Used by the UI to send true (Yes) or false (No) back
    pub response_tx: oneshot::Sender<bool>,
}
