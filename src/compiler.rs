// src/compiler.rs
//
// Provides `compile_bitcoin` and `compile_electrs`, async functions that
// mirror the Python `compile_bitcoin_source()` and `compile_electrs_source()`
// exactly.
//
// Both functions:
//   - git-clone (or update) the source repository
//   - Set up the build environment (PATH, LLVM, etc.)
//   - Run the appropriate build tool (CMake ≥ v25, Autotools for older Bitcoin;
//     `cargo build --release` for Electrs)
//   - Copy produced binaries into an output directory
//   - Send real-time log output via `log_tx`

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use anyhow::{Context, Result};
use regex::Regex;

use crate::messages::AppMessage;
use crate::process::{probe, run_command};

const BITCOIN_REPO: &str = "https://github.com/bitcoin/bitcoin.git";
const ELECTRS_REPO: &str = "https://github.com/romanz/electrs.git";

// ─── Public compile functions ─────────────────────────────────────────────────

/// Compile Bitcoin Core from source.
/// Returns the path of the output binaries directory.
pub async fn compile_bitcoin(
    version: &str,
    build_dir: &Path,
    cores: usize,
    env: &HashMap<String, String>,
    log_tx: &Sender<AppMessage>,
    progress_tx: &Sender<AppMessage>,
) -> Result<PathBuf> {
    let sep = "=".repeat(60);
    log(log_tx, &format!("\n{sep}\nCOMPILING BITCOIN CORE {version}\n{sep}\n"));

    let version_clean = version.trim_start_matches('v');
    let src_dir = build_dir.join(format!("bitcoin-{version_clean}"));

    // Ensure the parent build directory exists.
    std::fs::create_dir_all(build_dir).context("Failed to create build directory")?;

    // ── Clone or update source ────────────────────────────────────────────────
    clone_or_update(&src_dir, build_dir, version, BITCOIN_REPO, log_tx, env).await?;

    let path_preview = env
        .get("PATH")
        .map(|p| &p[..p.len().min(150)])
        .unwrap_or("");
    log(
        log_tx,
        &format!("\nEnvironment setup:\n  PATH: {path_preview}...\n  Building node-only (wallet support disabled)\n"),
    );

    progress_tx.send(AppMessage::Progress(0.3)).ok();

    // ── Choose build system ───────────────────────────────────────────────────
    let binaries = if use_cmake(version) {
        build_bitcoin_cmake(&src_dir, cores, env, log_tx, progress_tx).await?
    } else {
        build_bitcoin_autotools(&src_dir, cores, env, log_tx, progress_tx).await?
    };

    progress_tx.send(AppMessage::Progress(0.9)).ok();

    // ── Copy binaries to output dir ───────────────────────────────────────────
    let output_dir = build_dir
        .join("binaries")
        .join(format!("bitcoin-{version_clean}"));
    let copied = copy_binaries(&output_dir, &binaries, log_tx)?;

    if copied.is_empty() {
        log(
            log_tx,
            "⚠️  Warning: No binaries were copied. Checking what exists...\n",
        );
        for binary in &binaries {
            let mark = if binary.exists() { "✓" } else { "❌" };
            log(log_tx, &format!("  {mark} {}\n", binary.display()));
        }
    }

    let n = copied.len();
    let dir_str = output_dir.display().to_string();
    log(
        log_tx,
        &format!("\n{sep}\n✅ BITCOIN CORE {version} COMPILED SUCCESSFULLY!\n{sep}\n\n📍 Binaries location: {dir_str}\n   Found {n} binaries\n\n"),
    );

    Ok(output_dir)
}

/// Compile Electrs from source.  Returns the output binaries directory.
pub async fn compile_electrs(
    version: &str,
    build_dir: &Path,
    cores: usize,
    env: &HashMap<String, String>,
    log_tx: &Sender<AppMessage>,
    progress_tx: &Sender<AppMessage>,
) -> Result<PathBuf> {
    let sep = "=".repeat(60);
    log(log_tx, &format!("\n{sep}\nCOMPILING ELECTRS {version}\n{sep}\n"));

    // ── Verify Rust / Cargo ───────────────────────────────────────────────────
    log(log_tx, "\n🔍 Verifying Rust installation...\n");

    match probe(&["cargo", "--version"], env) {
        Some(v) => log(log_tx, &format!("✓ Cargo found: {v}\n")),
        None => {
            let msg = "❌ Cargo not found in PATH!\n\nElectrs requires Rust/Cargo to compile.\n\nPlease:\n1. Click 'Check & Install Dependencies' button\n2. Ensure Rust is installed\n3. Restart this application";
            log(log_tx, msg);
            log_tx
                .send(AppMessage::ShowDialog {
                    title: "Rust Not Found".into(),
                    message: msg.into(),
                    is_error: true,
                })
                .ok();
            return Err(anyhow::anyhow!("Cargo not found - cannot compile Electrs"));
        }
    }

    if let Some(v) = probe(&["rustc", "--version"], env) {
        log(log_tx, &format!("✓ Rustc found: {v}\n"));
    } else {
        log(
            log_tx,
            "⚠️  Warning: rustc check failed, but cargo found. Proceeding...\n",
        );
    }

    let version_clean = version.trim_start_matches('v');
    let src_dir = build_dir.join(format!("electrs-{version_clean}"));

    std::fs::create_dir_all(build_dir).context("Failed to create build directory")?;

    clone_or_update(&src_dir, build_dir, version, ELECTRS_REPO, log_tx, env).await?;

    log(log_tx, &format!("\n🔧 Building with Cargo ({cores} jobs)...\n"));

    let path_preview = env
        .get("PATH")
        .map(|p| &p[..p.len().min(150)])
        .unwrap_or("");
    log(
        log_tx,
        &format!("Environment details:\n  PATH: {path_preview}...\n"),
    );
    if let Some(lcp) = env.get("LIBCLANG_PATH") {
        log(log_tx, &format!("  LIBCLANG_PATH: {lcp}\n"));
    }

    progress_tx.send(AppMessage::Progress(0.3)).ok();

    run_command(
        &format!("cargo build --release --jobs {cores}"),
        Some(&src_dir),
        env,
        log_tx,
    )
    .await
    .context("cargo build --release failed")?;

    progress_tx.send(AppMessage::Progress(0.85)).ok();

    // ── Copy binary ───────────────────────────────────────────────────────────
    log(log_tx, "\n📋 Collecting binaries...\n");
    let binary = src_dir.join("target/release/electrs");
    if !binary.exists() {
        return Err(anyhow::anyhow!(
            "Electrs binary not found at expected location: {}",
            binary.display()
        ));
    }

    let output_dir = build_dir
        .join("binaries")
        .join(format!("electrs-{version_clean}"));
    copy_binaries(&output_dir, &[binary], log_tx)?;

    let out_str = output_dir.display().to_string();
    log(
        log_tx,
        &format!("\n{sep}\n✅ ELECTRS {version} COMPILED SUCCESSFULLY!\n{sep}\n\n📍 Binary location: {out_str}/electrs\n\n"),
    );

    Ok(output_dir)
}

// ─── CMake build (Bitcoin Core v25+) ─────────────────────────────────────────

async fn build_bitcoin_cmake(
    src_dir: &Path,
    cores: usize,
    env: &HashMap<String, String>,
    log_tx: &Sender<AppMessage>,
    progress_tx: &Sender<AppMessage>,
) -> Result<Vec<PathBuf>> {
    log(log_tx, "\n🔨 Building with CMake...\n");

    log(
        log_tx,
        "\n⚙️  Configuring (wallet support disabled for node-only build)...\n",
    );
    run_command(
        "cmake -B build -DENABLE_WALLET=OFF -DENABLE_IPC=OFF",
        Some(src_dir),
        env,
        log_tx,
    )
    .await
    .context("cmake configure failed")?;

    progress_tx.send(AppMessage::Progress(0.5)).ok();
    log(log_tx, &format!("\n🔧 Compiling with {cores} cores...\n"));

    run_command(
        &format!("cmake --build build -j{cores}"),
        Some(src_dir),
        env,
        log_tx,
    )
    .await
    .context("cmake build failed")?;

    let bin_dir = src_dir.join("build/bin");
    Ok(vec![
        bin_dir.join("bitcoind"),
        bin_dir.join("bitcoin-cli"),
        bin_dir.join("bitcoin-tx"),
        bin_dir.join("bitcoin-wallet"),
        bin_dir.join("bitcoin-util"),
    ])
}

// ─── Autotools build (Bitcoin Core < v25) ────────────────────────────────────

async fn build_bitcoin_autotools(
    src_dir: &Path,
    cores: usize,
    env: &HashMap<String, String>,
    log_tx: &Sender<AppMessage>,
    progress_tx: &Sender<AppMessage>,
) -> Result<Vec<PathBuf>> {
    log(log_tx, "\n🔨 Building with Autotools...\n");

    log(log_tx, "\n⚙️  Running autogen.sh...\n");
    run_command("./autogen.sh", Some(src_dir), env, log_tx)
        .await
        .context("autogen.sh failed")?;

    log(
        log_tx,
        "\n⚙️  Configuring (wallet support disabled for node-only build)...\n",
    );
    run_command(
        "./configure --disable-wallet --disable-gui",
        Some(src_dir),
        env,
        log_tx,
    )
    .await
    .context("./configure failed")?;

    progress_tx.send(AppMessage::Progress(0.5)).ok();
    log(log_tx, &format!("\n🔧 Compiling with {cores} cores...\n"));

    run_command(&format!("make -j{cores}"), Some(src_dir), env, log_tx)
        .await
        .context("make failed")?;

    let bin_dir = src_dir.join("bin");
    Ok(vec![
        bin_dir.join("bitcoind"),
        bin_dir.join("bitcoin-cli"),
        bin_dir.join("bitcoin-tx"),
        bin_dir.join("bitcoin-wallet"),
    ])
}

// ─── Binary copy ─────────────────────────────────────────────────────────────

fn copy_binaries(
    dest_dir: &Path,
    binary_files: &[PathBuf],
    log_tx: &Sender<AppMessage>,
) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dest_dir).context("Failed to create output directory")?;
    log(
        log_tx,
        &format!("Copying binaries to: {}\n", dest_dir.display()),
    );

    let mut copied = Vec::new();
    for binary in binary_files {
        if binary.exists() {
            let name = binary.file_name().unwrap_or_default();
            let dest = dest_dir.join(name);
            match std::fs::copy(binary, &dest) {
                Ok(_) => {
                    // Make the binary executable (Unix permissions 0o755).
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(
                            &dest,
                            std::fs::Permissions::from_mode(0o755),
                        );
                    }
                    log(
                        log_tx,
                        &format!(
                            "✓ Copied: {} → {}\n",
                            name.to_string_lossy(),
                            dest.display()
                        ),
                    );
                    copied.push(dest);
                }
                Err(e) => {
                    log(
                        log_tx,
                        &format!("⚠️  Failed to copy {}: {e}\n", name.to_string_lossy()),
                    );
                }
            }
        } else {
            log(
                log_tx,
                &format!("⚠️  Binary not found (skipping): {}\n", binary.display()),
            );
        }
    }

    if copied.is_empty() {
        log(log_tx, "❌ WARNING: No binaries were copied!\n");
    }

    Ok(copied)
}

// ─── Version helpers ──────────────────────────────────────────────────────────

/// Parse a version tag into `(major, minor)`.  Strips any leading `v`.
pub fn parse_version(tag: &str) -> (u32, u32) {
    let tag = tag.trim_start_matches('v');
    // The regex is compiled once per call; for a GUI app the overhead is fine.
    let re = Regex::new(r"^(\d+)\.(\d+)").expect("static regex is valid");
    re.captures(tag)
        .and_then(|c| {
            let major = c.get(1)?.as_str().parse().ok()?;
            let minor = c.get(2)?.as_str().parse().ok()?;
            Some((major, minor))
        })
        .unwrap_or((0, 0))
}

/// Bitcoin Core v25+ uses CMake; older versions use Autotools.
pub fn use_cmake(version: &str) -> bool {
    let (major, _) = parse_version(version);
    major >= 25
}

// ─── Clone / update helper ────────────────────────────────────────────────────

async fn clone_or_update(
    src_dir: &Path,
    build_dir: &Path,
    version: &str,
    repo_url: &str,
    log_tx: &Sender<AppMessage>,
    env: &HashMap<String, String>,
) -> Result<()> {
    if !src_dir.exists() {
        log(
            log_tx,
            &format!("\n📥 Cloning repository from {repo_url}...\n"),
        );
        let src_str = src_dir.display().to_string();
        run_command(
            &format!("git clone --depth 1 --branch {version} {repo_url} {src_str}"),
            Some(build_dir),
            env,
            log_tx,
        )
        .await
        .context("git clone failed")?;
        log(
            log_tx,
            &format!("✓ Source cloned to {}\n", src_dir.display()),
        );
    } else {
        log(
            log_tx,
            &format!(
                "✓ Source directory already exists: {}\n",
                src_dir.display()
            ),
        );
        log(log_tx, &format!("📥 Updating to {version}...\n"));
        run_command(
            &format!("git fetch --depth 1 origin tag {version}"),
            Some(src_dir),
            env,
            log_tx,
        )
        .await
        .context("git fetch failed")?;
        run_command(
            &format!("git checkout {version}"),
            Some(src_dir),
            env,
            log_tx,
        )
        .await
        .context("git checkout failed")?;
        log(log_tx, &format!("✓ Updated to {version}\n"));
    }
    Ok(())
}

// ─── Inline log helper ────────────────────────────────────────────────────────

fn log(tx: &Sender<AppMessage>, msg: &str) {
    tx.send(AppMessage::Log(msg.to_string())).ok();
}
