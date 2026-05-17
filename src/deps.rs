// src/deps.rs
//
// Background task: check build dependencies for supported platforms.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use anyhow::Result;
use tokio::sync::oneshot;

use crate::env_setup::{
    first_existing_command, is_supported_platform, linux_distribution_id, platform_summary,
    supported_platforms_message,
};
use crate::messages::{log_msg, AppMessage, ConfirmRequest};
use crate::process::{probe, run_command};

const MACOS_BREW_PACKAGES: &[&str] = &[
    "automake",
    "libtool",
    "pkg-config",
    "boost",
    "cmake",
    "llvm",
    "libevent",
    "rocksdb",
    "git",
];

const LINUX_TOOLS: &[ToolRequirement] = &[
    ToolRequirement::one("git", &["git"]),
    ToolRequirement::one("cmake", &["cmake"]),
    ToolRequirement::one("make", &["make", "gmake"]),
    ToolRequirement::one("C compiler", &["cc", "gcc", "clang"]),
    ToolRequirement::one("C++ compiler", &["c++", "g++", "clang++"]),
    ToolRequirement::one("pkg-config", &["pkg-config"]),
    ToolRequirement::one("clang", &["clang"]),
];

const LINUX_PKG_CONFIG_MODULES: &[PkgConfigRequirement] = &[
    PkgConfigRequirement::new("libevent", "libevent"),
    PkgConfigRequirement::new("X11", "x11"),
    PkgConfigRequirement::new("XCB", "xcb"),
    PkgConfigRequirement::new("Xcursor", "xcursor"),
    PkgConfigRequirement::new("Xrandr", "xrandr"),
    PkgConfigRequirement::new("Xi", "xi"),
    PkgConfigRequirement::new("xkbcommon", "xkbcommon"),
    PkgConfigRequirement::new("Wayland client", "wayland-client"),
    PkgConfigRequirement::new("Wayland cursor", "wayland-cursor"),
    PkgConfigRequirement::new("Wayland EGL", "wayland-egl"),
    PkgConfigRequirement::new("libudev", "libudev"),
    PkgConfigRequirement::new("D-Bus", "dbus-1"),
];

const DEBIAN_PACKAGES: &[&str] = &[
    "build-essential",
    "pkg-config",
    "cmake",
    "git",
    "clang",
    "libclang-dev",
    "libevent-dev",
    "libx11-dev",
    "libxcb1-dev",
    "libxcursor-dev",
    "libxrandr-dev",
    "libxi-dev",
    "libxkbcommon-dev",
    "libwayland-dev",
    "libudev-dev",
    "libdbus-1-dev",
];

const FEDORA_PACKAGES: &[&str] = &[
    "gcc",
    "gcc-c++",
    "pkgconf-pkg-config",
    "cmake",
    "git",
    "clang",
    "clang-devel",
    "libevent-devel",
    "libX11-devel",
    "libxcb-devel",
    "libXcursor-devel",
    "libXrandr-devel",
    "libXi-devel",
    "libxkbcommon-devel",
    "wayland-devel",
    "systemd-devel",
    "dbus-devel",
];

const ARCH_PACKAGES: &[&str] = &[
    "base-devel",
    "pkgconf",
    "cmake",
    "git",
    "clang",
    "libevent",
    "libx11",
    "libxcb",
    "libxcursor",
    "libxrandr",
    "libxi",
    "libxkbcommon",
    "wayland",
    "systemd",
    "dbus",
];

#[derive(Clone, Copy)]
struct ToolRequirement {
    name: &'static str,
    commands: &'static [&'static str],
}

impl ToolRequirement {
    const fn one(name: &'static str, commands: &'static [&'static str]) -> Self {
        Self { name, commands }
    }
}

#[derive(Clone, Copy)]
struct PkgConfigRequirement {
    name: &'static str,
    module: &'static str,
}

impl PkgConfigRequirement {
    const fn new(name: &'static str, module: &'static str) -> Self {
        Self { name, module }
    }
}

struct MissingDependency {
    name: String,
}

/// Background task: check and optionally install all dependencies.
///
/// Returns `true` when everything, including the Rust toolchain, is ready.
pub async fn check_dependencies_task(
    brew: Option<String>,
    env: HashMap<String, String>,
    log_tx: Sender<AppMessage>,
    confirm_tx: Sender<ConfirmRequest>,
) -> Result<bool> {
    log_msg(&log_tx, "\n=== Checking System Dependencies ===\n");
    log_msg(&log_tx, &format!("Platform: {}\n", platform_summary()));

    if !is_supported_platform() {
        let msg = format!(
            "{}\n\nCurrent platform: {}",
            supported_platforms_message(),
            platform_summary()
        );
        log_msg(&log_tx, &format!("❌ {msg}\n"));
        log_tx
            .send(AppMessage::ShowDialog {
                title: "Unsupported Platform".into(),
                message: msg,
                is_error: true,
            })
            .ok();
        return Ok(false);
    }

    let native_ok = if cfg!(target_os = "macos") {
        if let Some(brew) = brew.as_deref() {
            check_macos_dependencies(brew, &env, &log_tx, &confirm_tx).await?
        } else {
            show_missing_homebrew(&log_tx);
            false
        }
    } else if cfg!(target_os = "linux") {
        check_linux_dependencies(&env, &log_tx).await
    } else {
        false
    };

    if !native_ok {
        return Ok(false);
    }

    let rust_ok = check_rust_installation(brew.as_deref(), &env, &log_tx).await;

    log_msg(&log_tx, "\n=== Dependency Check Complete ===\n");

    if rust_ok {
        log_msg(&log_tx, "\n✓ Rust toolchain is ready!\n");
        log_tx
            .send(AppMessage::ShowDialog {
                title: "Dependency Check".into(),
                message: "All dependencies are installed and ready.\n\nYou can now proceed with compilation.".into(),
                is_error: false,
            })
            .ok();
    } else {
        log_msg(
            &log_tx,
            "\n⚠️  Rust toolchain needs attention (see messages above)\n",
        );
        log_tx
            .send(AppMessage::ShowDialog {
                title: "Dependency Check".into(),
                message: "Some dependencies need attention.\n\nCheck the log for details. You may need to restart the app after installing Rust.".into(),
                is_error: false,
            })
            .ok();
    }

    Ok(rust_ok)
}

async fn check_macos_dependencies(
    brew: &str,
    env: &HashMap<String, String>,
    log_tx: &Sender<AppMessage>,
    confirm_tx: &Sender<ConfirmRequest>,
) -> Result<bool> {
    log_msg(log_tx, &format!("✓ Homebrew found at: {brew}\n"));
    log_msg(log_tx, "\nChecking Homebrew packages...\n");

    let mut missing: Vec<&str> = Vec::new();
    for &pkg in MACOS_BREW_PACKAGES {
        let ok = tokio::process::Command::new(brew)
            .args(["list", pkg])
            .env_clear()
            .envs(env)
            .output()
            .await
            .is_ok_and(|o| o.status.success());

        if ok {
            log_msg(log_tx, &format!("  ✓ {pkg}\n"));
        } else {
            log_msg(log_tx, &format!("  ❌ {pkg} - not installed\n"));
            missing.push(pkg);
        }
    }

    if missing.is_empty() {
        log_msg(log_tx, "\n✓ All Homebrew packages are installed!\n");
        return Ok(true);
    }

    log_msg(
        log_tx,
        &format!("\n⚠️  Missing Homebrew packages: {}\n", missing.join(", ")),
    );

    let message = missing_packages_message("Homebrew packages", &missing);
    let should_install = ask_confirm(confirm_tx, "Install Missing Dependencies", &message).await;

    if !should_install {
        log_msg(
            log_tx,
            "\n⚠️  Dependencies not installed. Compilation may fail.\n",
        );
        return Ok(false);
    }

    let mut still_missing = Vec::new();
    for pkg in &missing {
        log_msg(log_tx, &format!("\n📦 Installing {pkg}...\n"));
        let cmd = format!("{brew:?} install {pkg}");
        match run_command(&cmd, None, env, log_tx).await {
            Ok(()) => {
                let installed = tokio::process::Command::new(brew)
                    .args(["list", pkg])
                    .env_clear()
                    .envs(env)
                    .output()
                    .await
                    .is_ok_and(|o| o.status.success());

                if installed {
                    log_msg(log_tx, &format!("✓ {pkg} installed successfully\n"));
                } else {
                    log_msg(
                        log_tx,
                        &format!("❌ {pkg} install finished but the package is still missing\n"),
                    );
                    still_missing.push(*pkg);
                }
            }
            Err(e) => {
                log_msg(log_tx, &format!("❌ Failed to install {pkg}: {e}\n"));
                still_missing.push(*pkg);
            }
        }
    }

    if still_missing.is_empty() {
        log_msg(log_tx, "\n✓ All Homebrew packages are installed!\n");
        Ok(true)
    } else {
        show_missing_native_dependencies(
            log_tx,
            "Dependency Check",
            &format!(
                "Some required Homebrew packages are still missing:\n\n{}\n\nInstall them and run the check again.",
                still_missing.join(", ")
            ),
        );
        Ok(false)
    }
}

async fn check_linux_dependencies(
    env: &HashMap<String, String>,
    log_tx: &Sender<AppMessage>,
) -> bool {
    log_msg(log_tx, "\nChecking Linux build dependencies...\n");
    log_msg(
        log_tx,
        "Release targets: x86_64-unknown-linux-gnu and aarch64-unknown-linux-gnu\n",
    );

    let mut missing = Vec::new();

    for requirement in LINUX_TOOLS {
        if let Some(tool) = first_existing_command(requirement.commands.iter().copied(), env) {
            log_msg(log_tx, &format!("  ✓ {}: {tool}\n", requirement.name));
        } else {
            log_msg(
                log_tx,
                &format!(
                    "  ❌ {} - missing (tried: {})\n",
                    requirement.name,
                    requirement.commands.join(", ")
                ),
            );
            missing.push(MissingDependency {
                name: requirement.name.to_owned(),
            });
        }
    }

    let pkg_config_available = first_existing_command(["pkg-config"], env).is_some();
    if pkg_config_available {
        log_msg(log_tx, "\nChecking pkg-config modules...\n");
        for requirement in LINUX_PKG_CONFIG_MODULES {
            let ok = tokio::process::Command::new("pkg-config")
                .args(["--exists", requirement.module])
                .env_clear()
                .envs(env)
                .output()
                .await
                .is_ok_and(|o| o.status.success());

            if ok {
                log_msg(
                    log_tx,
                    &format!("  ✓ {} ({})\n", requirement.name, requirement.module),
                );
            } else {
                log_msg(
                    log_tx,
                    &format!(
                        "  ❌ {} ({}) - missing\n",
                        requirement.name, requirement.module
                    ),
                );
                missing.push(MissingDependency {
                    name: requirement.name.to_owned(),
                });
            }
        }
    } else {
        log_msg(
            log_tx,
            "\nSkipping pkg-config module checks because pkg-config is missing.\n",
        );
    }

    if has_libclang(env) {
        log_msg(log_tx, "  ✓ libclang found\n");
    } else {
        log_msg(log_tx, "  ❌ libclang - missing\n");
        missing.push(MissingDependency {
            name: "libclang".to_owned(),
        });
    }

    if missing.is_empty() {
        log_msg(log_tx, "\n✓ All Linux native dependencies are installed!\n");
        return true;
    }

    let install = linux_install_guidance();
    let missing_names = missing
        .iter()
        .map(|dep| dep.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    log_msg(
        log_tx,
        &format!("\n❌ Missing Linux dependencies: {missing_names}\n\n{install}\n"),
    );
    show_missing_native_dependencies(
        log_tx,
        "Linux Dependencies Missing",
        &format!(
            "Missing Linux dependencies:\n\n{missing_names}\n\nInstall the matching distro packages and run the check again.\n\n{install}"
        ),
    );
    false
}

fn show_missing_homebrew(log_tx: &Sender<AppMessage>) {
    let message =
        "Homebrew was not found at /opt/homebrew/bin/brew.\n\nBitForge supports macOS Apple Silicon only; install Homebrew from https://brew.sh and restart BitForge.";
    log_msg(log_tx, &format!("❌ {message}\n"));
    log_tx
        .send(AppMessage::ShowDialog {
            title: "Homebrew Not Found".into(),
            message: message.into(),
            is_error: true,
        })
        .ok();
}

fn show_missing_native_dependencies(log_tx: &Sender<AppMessage>, title: &str, message: &str) {
    log_tx
        .send(AppMessage::ShowDialog {
            title: title.into(),
            message: message.into(),
            is_error: true,
        })
        .ok();
}

fn missing_packages_message(group: &str, missing: &[&str]) -> String {
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

    format!(
        "Found {count} missing {group}:\n\n{preview}{extra}\n\nInstall all missing packages now?"
    )
}

fn linux_install_guidance() -> String {
    let distro = linux_distribution_id().unwrap_or_default();
    let (manager, packages) = match distro.as_str() {
        "debian" | "ubuntu" | "linuxmint" | "pop" => {
            ("Debian/Ubuntu", format!("sudo apt-get update && sudo apt-get install -y {}", DEBIAN_PACKAGES.join(" ")))
        }
        "fedora" | "rhel" | "centos" | "rocky" | "almalinux" => (
            "Fedora/RHEL",
            format!("sudo dnf install -y {}", FEDORA_PACKAGES.join(" ")),
        ),
        "arch" | "manjaro" => (
            "Arch",
            format!("sudo pacman -S --needed {}", ARCH_PACKAGES.join(" ")),
        ),
        _ => (
            "Unknown distro",
            format!(
                "Debian/Ubuntu: sudo apt-get install -y {}\nFedora: sudo dnf install -y {}\nArch: sudo pacman -S --needed {}",
                DEBIAN_PACKAGES.join(" "),
                FEDORA_PACKAGES.join(" "),
                ARCH_PACKAGES.join(" ")
            ),
        ),
    };

    format!("Package guidance ({manager}):\n{packages}")
}

fn has_libclang(env: &HashMap<String, String>) -> bool {
    if env
        .get("LIBCLANG_PATH")
        .is_some_and(|path| Path::new(path).is_dir())
    {
        return true;
    }

    [
        "/usr/lib/llvm/lib/libclang.so",
        "/usr/lib/llvm-18/lib/libclang.so",
        "/usr/lib/llvm-17/lib/libclang.so",
        "/usr/lib/llvm-16/lib/libclang.so",
        "/usr/lib/llvm-15/lib/libclang.so",
        "/usr/lib64/libclang.so",
        "/usr/lib/libclang.so",
    ]
    .into_iter()
    .map(PathBuf::from)
    .any(|path| path.exists())
}

// ─── Rust toolchain check ─────────────────────────────────────────────────────

async fn check_rust_installation(
    brew: Option<&str>,
    env: &HashMap<String, String>,
    log_tx: &Sender<AppMessage>,
) -> bool {
    log_msg(log_tx, "\n=== Checking Rust Toolchain ===\n");

    let rustc_ok = probe_rust_tool("rustc", env).await.map_or_else(
        || {
            log_msg(
                log_tx,
                "❌ rustc not found in PATH or standard Cargo locations\n",
            );
            false
        },
        |v| {
            log_msg(log_tx, &format!("✓ rustc found: {v}\n"));
            true
        },
    );

    let cargo_ok = probe_rust_tool("cargo", env).await.map_or_else(
        || {
            log_msg(
                log_tx,
                "❌ cargo not found in PATH or standard Cargo locations\n",
            );
            false
        },
        |v| {
            log_msg(log_tx, &format!("✓ cargo found: {v}\n"));
            true
        },
    );

    if rustc_ok && cargo_ok {
        return true;
    }

    log_msg(log_tx, "\n❌ Rust toolchain not found or incomplete!\n");

    let Some(brew) = brew else {
        log_msg(
            log_tx,
            "Install Rust manually from https://rustup.rs, then restart BitForge.\n",
        );
        return false;
    };

    log_msg(log_tx, "Installing Rust via Homebrew...\n");

    let brew_knows_rust = tokio::process::Command::new(brew)
        .args(["info", "rust"])
        .env_clear()
        .envs(env)
        .output()
        .await
        .is_ok_and(|o| o.status.success());

    if !brew_knows_rust {
        log_msg(log_tx, "❌ Rust formula not found in Homebrew\n");
        return false;
    }

    log_msg(log_tx, "📦 Installing rust from Homebrew...\n");
    let brew_cmd = format!("{brew:?} install rust");
    if let Err(e) = run_command(&brew_cmd, None, env, log_tx).await {
        log_msg(log_tx, &format!("❌ Failed to install Rust: {e}\n"));
        return false;
    }

    log_msg(log_tx, "\nVerifying Rust installation...\n");
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    if let (Some(r), Some(c)) = (
        probe_rust_tool("rustc", env).await,
        probe_rust_tool("cargo", env).await,
    ) {
        log_msg(log_tx, &format!("✓ rustc installed: {r}\n"));
        log_msg(log_tx, &format!("✓ cargo installed: {c}\n"));
        true
    } else {
        log_msg(
            log_tx,
            "⚠️  Rust installed but binaries not yet in PATH. Restart the app.\n",
        );
        false
    }
}

async fn probe_rust_tool(tool: &str, env: &HashMap<String, String>) -> Option<String> {
    if let Some(version) = probe(&[tool, "--version"], env).await {
        return Some(version);
    }

    for candidate in rust_tool_candidates(tool, env)
        .into_iter()
        .filter(|candidate| candidate.is_file())
    {
        let Ok(output) = tokio::process::Command::new(&candidate)
            .arg("--version")
            .env_clear()
            .envs(env)
            .output()
            .await
        else {
            continue;
        };

        if output.status.success() {
            return String::from_utf8(output.stdout)
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty());
        }
    }

    None
}

fn rust_tool_candidates(tool: &str, env: &HashMap<String, String>) -> Vec<PathBuf> {
    let mut candidates = Vec::with_capacity(2);

    if let Some(cargo_home) = env.get("CARGO_HOME") {
        push_tool_candidate(
            &mut candidates,
            Path::new(cargo_home).join("bin").join(tool),
        );
    }

    if let Some(home) = env.get("HOME") {
        push_tool_candidate(
            &mut candidates,
            Path::new(home).join(".cargo").join("bin").join(tool),
        );
    }

    candidates
}

fn push_tool_candidate(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

// ─── Confirmation helper ──────────────────────────────────────────────────────

async fn ask_confirm(tx: &Sender<ConfirmRequest>, title: &str, message: &str) -> bool {
    let (response_tx, response_rx) = oneshot::channel::<bool>();
    tx.send(ConfirmRequest {
        title: title.to_owned(),
        message: message.to_owned(),
        response_tx,
    })
    .ok();
    response_rx.await.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{linux_install_guidance, rust_tool_candidates};
    use std::collections::HashMap;

    #[test]
    fn rust_tool_candidates_include_cargo_home_and_home() {
        let mut env = HashMap::new();
        env.insert("CARGO_HOME".to_owned(), "/tmp/cargo-home".to_owned());
        env.insert("HOME".to_owned(), "/tmp/home".to_owned());

        let candidates = rust_tool_candidates("cargo", &env);
        let candidate_strings: Vec<String> = candidates
            .into_iter()
            .map(|path| path.display().to_string())
            .collect();

        assert_eq!(
            candidate_strings,
            vec![
                "/tmp/cargo-home/bin/cargo".to_owned(),
                "/tmp/home/.cargo/bin/cargo".to_owned(),
            ]
        );
    }

    #[test]
    fn linux_install_guidance_mentions_major_distros() {
        let guidance = linux_install_guidance();
        assert!(guidance.contains("Package guidance"));
    }
}
