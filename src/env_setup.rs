// src/env_setup.rs
//
// Platform discovery and build environment construction.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// ─── Homebrew discovery ───────────────────────────────────────────────────────

/// Return the path to the `brew` binary, checking Apple Silicon first.
#[must_use]
pub fn find_brew() -> Option<String> {
    #[cfg(not(target_os = "macos"))]
    {
        None
    }

    #[cfg(target_os = "macos")]
    {
        const CANDIDATES: [&str; 1] = ["/opt/homebrew/bin/brew"];
        CANDIDATES
            .iter()
            .copied()
            .find(|p| Path::new(p).is_file())
            .map(str::to_owned)
    }
}

/// Derive the Homebrew prefix from the brew binary path.
#[must_use]
pub fn brew_prefix(brew: &str) -> String {
    let brew_path = Path::new(brew);
    brew_path
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| "/opt/homebrew".to_owned(), |p| p.display().to_string())
}

/// Human-readable OS and architecture string for diagnostics.
#[must_use]
pub fn platform_summary() -> String {
    format!("{} {}", operating_system_summary(), std::env::consts::ARCH)
}

/// Default output directory for compiled Bitcoin Core and Electrs binaries.
#[must_use]
pub fn default_build_dir() -> PathBuf {
    if cfg!(target_os = "linux") {
        if let Some(dir) = xdg_data_home() {
            return dir.join("BitForge").join("bitcoin_builds");
        }
    }

    home_dir().map_or_else(
        || std::env::temp_dir().join("bitcoin_builds"),
        |h| h.join("Downloads").join("bitcoin_builds"),
    )
}

/// Detect Linux distribution ID from `/etc/os-release`.
#[must_use]
#[cfg(target_os = "linux")]
pub fn linux_distribution_id() -> Option<String> {
    os_release_field("ID")
}

/// Detect Linux distribution ID from `/etc/os-release`.
#[must_use]
#[cfg(not(target_os = "linux"))]
pub const fn linux_distribution_id() -> Option<String> {
    None
}

#[must_use]
pub fn is_supported_platform() -> bool {
    matches!(
        (std::env::consts::OS, std::env::consts::ARCH),
        ("macos" | "linux", "aarch64") | ("linux", "x86_64")
    )
}

#[must_use]
pub const fn supported_platforms_message() -> &'static str {
    "Supported platforms are macOS Apple Silicon (aarch64-apple-darwin), Linux x86_64 (x86_64-unknown-linux-gnu), and Linux ARM64 (aarch64-unknown-linux-gnu)."
}

fn operating_system_summary() -> String {
    if cfg!(target_os = "macos") {
        return format!("macOS {}", macos_version());
    }

    if cfg!(target_os = "linux") {
        let pretty = os_release_field("PRETTY_NAME")
            .or_else(|| os_release_field("NAME"))
            .unwrap_or_else(|| "Linux".to_owned());
        return pretty;
    }

    std::env::consts::OS.to_owned()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn xdg_data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".local").join("share")))
}

#[cfg(target_os = "linux")]
fn os_release_field(name: &str) -> Option<String> {
    let content = std::fs::read_to_string("/etc/os-release").ok()?;
    content.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if key == name {
            Some(value.trim_matches('"').to_owned())
        } else {
            None
        }
    })
}

#[cfg(not(target_os = "linux"))]
const fn os_release_field(_name: &str) -> Option<String> {
    None
}

// ─── Tool discovery ───────────────────────────────────────────────────────────

/// Find a command in PATH.
#[must_use]
pub fn find_in_path(tool: &str, env: &HashMap<String, String>) -> Option<PathBuf> {
    let paths = env.get("PATH")?;
    paths
        .split(':')
        .filter(|p| !p.is_empty())
        .map(|dir| Path::new(dir).join(tool))
        .find(|candidate| candidate.is_file())
}

#[must_use]
pub fn command_exists(tool: &str, env: &HashMap<String, String>) -> bool {
    find_in_path(tool, env).is_some()
}

#[must_use]
pub fn first_existing_command<'a>(
    tools: impl IntoIterator<Item = &'a str>,
    env: &HashMap<String, String>,
) -> Option<&'a str> {
    tools.into_iter().find(|tool| command_exists(tool, env))
}

// ─── Build environment ────────────────────────────────────────────────────────

/// Build a complete process environment suitable for spawning compilation
/// children. Prepends package-manager, Cargo, LLVM, and system tool paths to
/// `PATH`, sets native library paths where needed, and inherits everything else
/// from the parent process.
#[must_use]
pub fn setup_build_environment(brew_pfx: Option<&str>) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();

    let home = env.get("HOME").map_or("", String::as_str).to_owned();

    let mut path_parts: Vec<String> = Vec::with_capacity(24);

    if let Some(pfx) = brew_pfx {
        push_path(&mut path_parts, Path::new(pfx).join("bin"));
    }

    #[cfg(target_os = "macos")]
    {
        push_path(&mut path_parts, "/opt/homebrew/bin");
    }

    if !home.is_empty() {
        let cargo_bin = Path::new(&home).join(".cargo").join("bin");
        if cargo_bin.is_dir() {
            push_path(&mut path_parts, cargo_bin);
        }
    }

    let llvm_candidates = build_llvm_candidates(brew_pfx);
    let llvm_prefix_found = llvm_candidates.iter().find(|candidate| {
        let bin = Path::new(candidate).join("bin");
        if bin.is_dir() {
            push_path(&mut path_parts, bin);
            true
        } else {
            false
        }
    });

    if let Some(existing) = env.get("PATH") {
        path_parts.extend(
            existing
                .split(':')
                .filter(|p| !p.is_empty())
                .map(ToOwned::to_owned),
        );
    }

    path_parts.extend(
        ["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"]
            .into_iter()
            .map(ToOwned::to_owned),
    );

    let mut seen: HashSet<String> = HashSet::with_capacity(path_parts.len());
    let deduped: Vec<String> = path_parts
        .into_iter()
        .filter(|p| !p.is_empty() && seen.insert(p.clone()))
        .collect();

    env.insert("PATH".to_owned(), deduped.join(":"));

    if let Some(pfx) = llvm_prefix_found {
        let lib = Path::new(pfx).join("lib").display().to_string();
        env.insert("LIBCLANG_PATH".to_owned(), lib.clone());
        if cfg!(target_os = "macos") {
            env.insert("DYLD_LIBRARY_PATH".to_owned(), lib);
        } else if cfg!(target_os = "linux") {
            env.insert("LD_LIBRARY_PATH".to_owned(), lib);
        }
    }

    env
}

fn push_path(path_parts: &mut Vec<String>, path: impl AsRef<Path>) {
    path_parts.push(path.as_ref().display().to_string());
}

// ─── LLVM prefix candidates ───────────────────────────────────────────────────

fn build_llvm_candidates(brew_pfx: Option<&str>) -> Vec<String> {
    let mut v = Vec::with_capacity(8);
    if let Some(pfx) = brew_pfx {
        v.push(
            Path::new(pfx)
                .join("opt")
                .join("llvm")
                .display()
                .to_string(),
        );
    }

    if cfg!(target_os = "macos") {
        v.push("/opt/homebrew/opt/llvm".to_owned());
    } else if cfg!(target_os = "linux") {
        v.extend(
            [
                "/usr/lib/llvm",
                "/usr/lib/llvm-18",
                "/usr/lib/llvm-17",
                "/usr/lib/llvm-16",
                "/usr/lib/llvm-15",
                "/usr/lib64/llvm",
            ]
            .into_iter()
            .map(ToOwned::to_owned),
        );
    }

    v
}

// ─── macOS version ────────────────────────────────────────────────────────────

/// Return the macOS product version string, e.g. `"14.4.1"`.
/// Falls back to `"unknown"` when `sw_vers` is unavailable.
#[must_use]
pub fn macos_version() -> String {
    #[cfg(not(target_os = "macos"))]
    {
        "not macOS".to_owned()
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map_or_else(|| "unknown".to_owned(), |s| s.trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{brew_prefix, is_supported_platform};

    #[test]
    fn brew_prefix_uses_parent_of_bin_directory() {
        assert_eq!(brew_prefix("/opt/homebrew/bin/brew"), "/opt/homebrew");
    }

    #[test]
    fn supported_platform_detection_is_callable() {
        let _ = is_supported_platform();
    }
}
