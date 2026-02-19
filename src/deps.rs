// src/deps.rs
//
// Mirrors the Python `check_dependencies()` and `check_rust_installation()`
// functions.  Runs in a tokio background task.
//
// Key design points:
//   - Uses `run_command` from process.rs for any Homebrew install commands.
//   - Sends ConfirmRequest to the UI when it needs a Yes/No answer (e.g. to
//     install missing packages).  The background task awaits the oneshot
//     reply channel while the main thread shows the modal.
//   - All log output goes through AppMessage::Log so the UI terminal stays
//     up-to-date in real time.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use anyhow::Result;
use tokio::sync::oneshot;

use crate::messages::{AppMessage, ConfirmRequest};
use crate::process::{probe, run_command};

// Homebrew packages required for both Bitcoin Core (autotools + cmake) and
// Electrs (cargo).  Mirrors the Python `brew_packages` list exactly.
const BREW_PACKAGES: &[&str] = &[
    "automake", "libtool", "pkg-config", "boost",
    "miniupnpc", "zeromq", "sqlite", "python", "cmake",
    "llvm", "libevent", "rocksdb", "rust", "git",
];

// ─── Public entry point ───────────────────────────────────────────────────────

/// Background task: check and (optionally) install all dependencies.
///
/// `brew`        – path to the `brew` binary (e.g. "/opt/homebrew/bin/brew")
/// `brew_prefix` – Homebrew prefix (e.g. "/opt/homebrew")
/// `env`         – build environment from `setup_build_environment()`
/// `log_tx`      – log-line channel to the UI
/// `confirm_tx`  – channel for asking the user a Yes/No question
///
/// Returns `true` when everything (including Rust toolchain) is ready.
pub async fn check_dependencies_task(
    brew: String,
    env: HashMap<String, String>,
    log_tx: Sender<AppMessage>,
    confirm_tx: Sender<ConfirmRequest>,
) -> Result<bool> {
    log(&log_tx, "\n=== Checking System Dependencies ===\n");
    log(&log_tx, &format!("✓ Homebrew found at: {brew}\n"));

    // ── Check Homebrew packages ───────────────────────────────────────────────
    log(&log_tx, "\nChecking Homebrew packages...\n");

    let mut missing: Vec<&str> = Vec::new();
    for &pkg in BREW_PACKAGES {
        let result = std::process::Command::new(&brew)
            .args(["list", pkg])
            .env_clear()
            .envs(&env)
            .output();

        match result {
            Ok(o) if o.status.success() => {
                log(&log_tx, &format!("  ✓ {pkg}\n"));
            }
            _ => {
                log(&log_tx, &format!("  ❌ {pkg} - not installed\n"));
                missing.push(pkg);
            }
        }
    }

    // ── Offer to install missing packages ────────────────────────────────────
    if !missing.is_empty() {
        log(
            &log_tx,
            &format!(
                "\n⚠️  Missing Homebrew packages: {}\n",
                missing.join(", ")
            ),
        );

        let count = missing.len();
        let preview = missing
            .iter()
            .take(5)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        let extra = if count > 5 {
            format!(", and {} more", count - 5)
        } else {
            String::new()
        };

        let message = format!(
            "Found {count} missing package{}:\n\n{preview}{extra}\n\nInstall all missing packages now?",
            if count == 1 { "" } else { "s" }
        );

        let should_install = ask_confirm(
            &confirm_tx,
            "Install Missing Dependencies",
            &message,
        )
        .await;

        if should_install {
            for pkg in &missing {
                log(&log_tx, &format!("\n📦 Installing {pkg}...\n"));
                match run_command(
                    &format!("{brew} install {pkg}"),
                    None,
                    &env,
                    &log_tx,
                )
                .await
                {
                    Ok(()) => log(&log_tx, &format!("✓ {pkg} installed successfully\n")),
                    Err(e) => {
                        log(&log_tx, &format!("❌ Failed to install {pkg}: {e}\n"));
                        log_tx
                            .send(AppMessage::ShowDialog {
                                title: "Installation Failed".into(),
                                message: format!("Failed to install {pkg}:\n{e}"),
                                is_error: true,
                            })
                            .ok();
                    }
                }
            }
        } else {
            log(
                &log_tx,
                "\n⚠️  Dependencies not installed. Compilation may fail.\n",
            );
        }
    } else {
        log(&log_tx, "\n✓ All Homebrew packages are installed!\n");
    }

    // ── Check Rust toolchain ─────────────────────────────────────────────────
    let rust_ok = check_rust_installation(&brew, &env, &log_tx).await;

    log(&log_tx, "\n=== Dependency Check Complete ===\n");

    if rust_ok {
        log(&log_tx, "\n✓ Rust toolchain is ready!\n");
        log_tx
            .send(AppMessage::ShowDialog {
                title: "Dependency Check".into(),
                message: "✅ All dependencies are installed and ready!\n\nYou can now proceed with compilation.".into(),
                is_error: false,
            })
            .ok();
    } else {
        log(
            &log_tx,
            "\n⚠️  Rust toolchain needs attention (see messages above)\n",
        );
        log_tx
            .send(AppMessage::ShowDialog {
                title: "Dependency Check".into(),
                message: "⚠️  Some dependencies need attention.\n\nCheck the log for details.\nYou may need to restart the app after installing Rust.".into(),
                is_error: false,
            })
            .ok();
    }

    Ok(rust_ok)
}

// ─── Rust toolchain check ─────────────────────────────────────────────────────

async fn check_rust_installation(
    brew: &str,
    env: &HashMap<String, String>,
    log_tx: &Sender<AppMessage>,
) -> bool {
    log(log_tx, "\n=== Checking Rust Toolchain ===\n");

    let rustc_ok = match probe(&["rustc", "--version"], env) {
        Some(v) => {
            log(log_tx, &format!("✓ rustc found: {v}\n"));
            true
        }
        None => {
            log(log_tx, "❌ rustc not found in PATH\n");
            false
        }
    };

    let cargo_ok = match probe(&["cargo", "--version"], env) {
        Some(v) => {
            log(log_tx, &format!("✓ cargo found: {v}\n"));
            true
        }
        None => {
            log(log_tx, "❌ cargo not found in PATH\n");
            false
        }
    };

    if rustc_ok && cargo_ok {
        return true;
    }

    // ── Try installing Rust via Homebrew ──────────────────────────────────────
    log(log_tx, "\n❌ Rust toolchain not found or incomplete!\n");
    log(log_tx, "Installing Rust via Homebrew...\n");

    // Check that brew knows about the rust formula first.
    let brew_knows_rust = std::process::Command::new(brew)
        .args(["info", "rust"])
        .env_clear()
        .envs(env)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !brew_knows_rust {
        log(log_tx, "❌ Rust formula not found in Homebrew\n");
        log(log_tx, "Attempting alternative installation method...\n");
        log_tx
            .send(AppMessage::ShowDialog {
                title: "Rust Installation Failed".into(),
                message: "Could not install Rust via Homebrew.\n\nPlease install manually:\n1. Visit https://rustup.rs\n2. Run: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh\n3. Restart this app".into(),
                is_error: true,
            })
            .ok();
        return false;
    }

    log(log_tx, "📦 Installing rust from Homebrew...\n");
    match run_command(&format!("{brew} install rust"), None, env, log_tx).await {
        Err(e) => {
            log(log_tx, &format!("❌ Failed to install Rust: {e}\n"));
            log_tx
                .send(AppMessage::ShowDialog {
                    title: "Installation Error".into(),
                    message: format!("Failed to install Rust: {e}\n\nPlease install manually from https://rustup.rs"),
                    is_error: true,
                })
                .ok();
            return false;
        }
        Ok(()) => {
            log(log_tx, "\nVerifying Rust installation...\n");
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    // Re-check after installation
    let rustc_v = probe(&["rustc", "--version"], env);
    let cargo_v = probe(&["cargo", "--version"], env);

    match (rustc_v, cargo_v) {
        (Some(r), Some(c)) => {
            log(log_tx, &format!("✓ rustc installed: {r}\n"));
            log(log_tx, &format!("✓ cargo installed: {c}\n"));
            true
        }
        _ => {
            log(
                log_tx,
                "⚠️  Rust installation may have succeeded but binaries not found in PATH\n",
            );
            log(
                log_tx,
                "You may need to restart the app or your terminal\n",
            );
            log_tx
                .send(AppMessage::ShowDialog {
                    title: "Rust Installation".into(),
                    message: "Rust was installed but may not be in PATH.\n\nPlease:\n1. Close and reopen this app\n2. OR manually add ~/.cargo/bin to your PATH".into(),
                    is_error: false,
                })
                .ok();
            false
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn log(tx: &Sender<AppMessage>, msg: &str) {
    tx.send(AppMessage::Log(msg.to_string())).ok();
}

/// Send a ConfirmRequest to the UI, then await the Yes/No answer.
async fn ask_confirm(
    tx: &Sender<ConfirmRequest>,
    title: &str,
    message: &str,
) -> bool {
    let (response_tx, response_rx) = oneshot::channel::<bool>();
    tx.send(ConfirmRequest {
        title: title.to_string(),
        message: message.to_string(),
        response_tx,
    })
    .ok();
    // Suspend this async task until the UI thread sends the response.
    response_rx.await.unwrap_or(false)
}
