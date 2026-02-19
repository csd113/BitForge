// src/process.rs
//
// Equivalent of the Python `run_command()` helper.
//
// `run_command` spawns a child process via `/bin/sh -c <cmd>`, merges its
// stdout and stderr into a single stream, and sends every line to the UI log
// through the `AppMessage::Log` channel.  The function is fully async so that
// the tokio runtime can drive the UI repaint events while bytes arrive.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::Sender;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::messages::AppMessage;

/// Execute `cmd` in a shell, streaming every output line to `log_tx`.
///
/// * `cwd`  – optional working directory for the child process
/// * `env`  – complete environment HashMap (replaces the child's environment)
///
/// Returns `Ok(())` when the process exits with code 0.
/// Returns `Err(…)` on non-zero exit or spawn failure.
pub async fn run_command(
    cmd: &str,
    cwd: Option<&Path>,
    env: &HashMap<String, String>,
    log_tx: &Sender<AppMessage>,
) -> Result<()> {
    // Echo the command itself to the log, just as Python does.
    log_tx
        .send(AppMessage::Log(format!("\n$ {cmd}\n")))
        .ok(); // UI may have gone away — silently ignore SendError

    let mut builder = Command::new("sh");
    builder
        .arg("-c")
        .arg(cmd)
        // Replace the entire child environment so PATH etc. are controlled.
        .env_clear()
        .envs(env)
        // Capture both streams.
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Avoid inheriting a signal-handler disposition that could cause
        // the child to be killed when the parent receives Ctrl-C.
        .kill_on_drop(true);

    if let Some(dir) = cwd {
        builder.current_dir(dir);
    }

    let mut child = builder
        .spawn()
        .with_context(|| format!("Failed to spawn: {cmd}"))?;

    // Take ownership of the stream handles before awaiting.
    let stdout = child.stdout.take().context("stdout not captured")?;
    let stderr = child.stderr.take().context("stderr not captured")?;

    // Drain stdout on one task, stderr on another — both forward to the same
    // log channel.  This prevents deadlocks from full OS pipe buffers.
    let tx_out = log_tx.clone();
    let tx_err = log_tx.clone();

    let stdout_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tx_out
                .send(AppMessage::Log(format!("{line}\n")))
                .ok();
        }
    });

    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tx_err
                .send(AppMessage::Log(format!("{line}\n")))
                .ok();
        }
    });

    // Wait for the process to exit, then let the reader tasks finish draining.
    let status = child
        .wait()
        .await
        .with_context(|| format!("Failed to wait for: {cmd}"))?;

    // Await the reader tasks so we capture every byte before checking exit.
    stdout_task.await.ok();
    stderr_task.await.ok();

    if !status.success() {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());
        bail!("Command failed (exit {code}): {cmd}");
    }

    Ok(())
}

/// Convenience: run a command and capture its stdout as a String (no logging).
/// Used for probe commands such as `rustc --version`.
pub fn probe(cmd: &[&str], env: &HashMap<String, String>) -> Option<String> {
    std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .env_clear()
        .envs(env)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}
