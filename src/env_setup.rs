// src/env_setup.rs
//
// Mirrors the Python helpers: find_brew(), BREW_PREFIX detection, and
// setup_build_environment() which builds the complete HashMap that is passed
// as the child process's environment for every compilation step.

use std::collections::HashMap;

// ─── Homebrew discovery ───────────────────────────────────────────────────────

/// Return the path to the `brew` executable, checking Apple Silicon first.
pub fn find_brew() -> Option<String> {
    let candidates = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"];
    for path in &candidates {
        if std::path::Path::new(path).is_file() {
            return Some(path.to_string());
        }
    }
    None
}

/// Derive the Homebrew prefix from the brew binary path.
pub fn brew_prefix(brew: &str) -> String {
    if brew.contains("/opt/homebrew") {
        "/opt/homebrew".to_string()
    } else {
        "/usr/local".to_string()
    }
}

// ─── Build environment ────────────────────────────────────────────────────────

/// Build a complete process environment `HashMap` suitable for spawning
/// compilation child processes.  The logic is a faithful port of the Python
/// `setup_build_environment()` function.
///
/// Strategy:
///   1. Start with the parent process's current environment.
///   2. Prepend Homebrew, Cargo, and LLVM directories to `PATH`.
///   3. Set `LIBCLANG_PATH` / `DYLD_LIBRARY_PATH` for the LLVM that ships
///      with Homebrew (required to build Electrs's RocksDB bindings).
///   4. Remove duplicate PATH components while preserving order.
pub fn setup_build_environment(brew_pfx: Option<&str>) -> HashMap<String, String> {
    // Start with the inherited environment so that things like HOME, USER,
    // TMPDIR, SSH_AUTH_SOCK, etc. are all available to child processes.
    let mut env: HashMap<String, String> = std::env::vars().collect();

    let home = env
        .get("HOME")
        .cloned()
        .unwrap_or_else(|| "/Users/user".to_string());

    // ── Build ordered PATH components ────────────────────────────────────────
    let mut path_parts: Vec<String> = Vec::new();

    if let Some(pfx) = brew_pfx {
        path_parts.push(format!("{pfx}/bin"));
    }
    // Always include both Homebrew locations so the binary works on both
    // Apple Silicon and Intel Macs even when brew_pfx is already set.
    path_parts.push("/opt/homebrew/bin".to_string());
    path_parts.push("/usr/local/bin".to_string());

    // Rust / Cargo binaries
    let cargo_bin = format!("{home}/.cargo/bin");
    if std::path::Path::new(&cargo_bin).is_dir() {
        path_parts.push(cargo_bin);
    }

    // LLVM — needed for Electrs's librocksdb-sys / bindgen
    let llvm_candidates = build_llvm_candidates(brew_pfx);
    let mut llvm_prefix_found: Option<String> = None;

    for candidate in &llvm_candidates {
        let bin = format!("{candidate}/bin");
        if std::path::Path::new(&bin).is_dir() {
            path_parts.push(bin);
            llvm_prefix_found = Some(candidate.clone());
            break;
        }
    }

    // Append the existing PATH so system utilities remain accessible
    if let Some(existing) = env.get("PATH") {
        path_parts.push(existing.clone());
    }
    path_parts.push("/usr/bin".to_string());
    path_parts.push("/bin".to_string());
    path_parts.push("/usr/sbin".to_string());
    path_parts.push("/sbin".to_string());

    // Deduplicate while preserving first-occurrence order
    let mut seen = std::collections::HashSet::new();
    let deduped: Vec<String> = path_parts
        .into_iter()
        .filter(|p| !p.is_empty() && seen.insert(p.clone()))
        .collect();

    env.insert("PATH".to_string(), deduped.join(":"));

    // ── LLVM library paths (needed by Electrs / RocksDB bindgen) ─────────────
    if let Some(llvm_pfx) = llvm_prefix_found {
        let lib = format!("{llvm_pfx}/lib");
        env.insert("LIBCLANG_PATH".to_string(), lib.clone());
        env.insert("DYLD_LIBRARY_PATH".to_string(), lib);
    }

    env
}

// ─── Helper: collect LLVM prefix candidates ───────────────────────────────────

fn build_llvm_candidates(brew_pfx: Option<&str>) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(pfx) = brew_pfx {
        v.push(format!("{pfx}/opt/llvm"));
    }
    v.push("/opt/homebrew/opt/llvm".to_string());
    v.push("/usr/local/opt/llvm".to_string());
    v
}

// ─── macOS version helper ─────────────────────────────────────────────────────

/// Return the macOS product version string, e.g. "14.4.1".
/// Falls back to "unknown" if `sw_vers` is unavailable.
pub fn macos_version() -> String {
    std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
