<div align="center">

# ⚙️ BitForge

**A native macOS GUI for compiling Bitcoin Core and Electrs from source**

Built with Rust · egui · Metal-accelerated · Apple Silicon native

[![Rust](https://img.shields.io/badge/rust-1.80%2B-orange?logo=rust)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-macOS%2012%2B-blue?logo=apple)](https://www.apple.com/macos/)
[![Architecture](https://img.shields.io/badge/arch-arm64%20%7C%20x86__64-lightgrey)](#build)
[![License](https://img.shields.io/badge/license-MIT-green)](#license)

</div>

---

## What is BitForge?

BitForge is a native macOS desktop application that compiles **Bitcoin Core** (`bitcoind`) and the **Electrs** block indexer directly from source — no terminal required.

- Dependency checker with one-click Homebrew install
- Version selector pulling live tags from the GitHub Releases API
- Real-time streaming build log with a terminal-style dark panel
- Animated progress bar across clone → configure → compile → copy stages
- Configurable build directory and CPU core count
- Single-binary distribution — no runtime, no WebView, no Electron

Binaries produced by BitForge drop straight into **BitEngine** for node management.

---

## Screenshots

> _Main window showing dependency check, version selection, and live build log_

```
┌─────────────────────────────────────────────────────────────────────┐
│         Bitcoin Core & Electrs Compiler                              │
├─────────────────────────────────────────────────────────────────────┤
│  Step 1: [Check & Install Dependencies]                              │
├─────────────────────────────────────────────────────────────────────┤
│  Step 2: Select What to Compile                                      │
│  Target:  [ Bitcoin ▾ ]    CPU Cores: [7]  (max: 8)                 │
│  Build Directory: /Users/you/Downloads/bitcoin_builds   [Browse…]   │
├─────────────────────────────────────────────────────────────────────┤
│  Step 3: Select Versions                                             │
│  Bitcoin Version:  [ v27.1 ▾ ]  [Refresh]                           │
│  Electrs Version:  [ v0.10.5 ▾ ] [Refresh]                          │
├─────────────────────────────────────────────────────────────────────┤
│  Progress: ████████████████████░░░░░░░░░  68%                        │
├─────────────────────────────────────────────────────────────────────┤
│  Build Log                                                           │
│ ┌─────────────────────────────────────────────────────────────────┐ │
│ │ ============================================================    │ │
│ │ COMPILING BITCOIN CORE v27.1                                    │ │
│ │ ============================================================    │ │
│ │ $ git clone --depth 1 --branch 'v27.1' ...                     │ │
│ │ ✓ Source cloned to ~/Downloads/bitcoin_builds/bitcoin-27.1     │ │
│ │ $ cmake -B build -DENABLE_WALLET=OFF -DENABLE_IPC=OFF          │ │
│ │ -- Configuring done                                             │ │
│ │ -- Build files have been written to: build/                     │ │
│ │ $ cmake --build build -j7                                       │ │
│ │ [  3%] Building CXX object src/...                             │ │
│ └─────────────────────────────────────────────────────────────────┘ │
├─────────────────────────────────────────────────────────────────────┤
│                    [ 🚀  Start Compilation ]                         │
├─────────────────────────────────────────────────────────────────────┤
│  System: macOS 14.5  |  Homebrew: /opt/homebrew  |  CPUs: 8         │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Features

### Dependency checker
Scans for all required Homebrew packages (`cmake`, `llvm`, `boost`, `rocksdb`, and more). Missing packages are listed with a Yes/No confirmation dialog before anything is installed. The Rust toolchain is verified separately and supports standard manual installs such as `rustup`; BitForge falls back to Homebrew only if Rust is absent.

### Live version selection
Pulls the latest stable release tags directly from the GitHub Releases API on startup. Pre-releases and release candidates (`rc`) are filtered out automatically. Hit **Refresh** at any time to re-fetch.

### Build targets

| Target | Build system | Notes |
|---|---|---|
| Bitcoin Core v25+ | CMake | Wallet disabled (node-only) |
| Bitcoin Core < v25 | Autotools | Wallet + GUI disabled |
| Electrs (any) | Cargo | Requires Rust toolchain |
| Both | Sequential | Bitcoin first, then Electrs |

### Real-time streaming log
Every line of stdout and stderr from every child process (git, cmake, make, cargo) is streamed to the terminal panel as it arrives. stdout and stderr are drained concurrently to prevent OS pipe-buffer deadlocks. The log is capped at 4 000 lines with automatic trimming — no unbounded memory growth.

### Output binaries
Compiled binaries are copied into a versioned subdirectory inside the build folder:

```
~/Downloads/bitcoin_builds/
└── binaries/
    ├── bitcoin-27.1/
    │   ├── bitcoind
    │   ├── bitcoin-cli
    │   ├── bitcoin-tx
    │   ├── bitcoin-wallet
    │   └── bitcoin-util
    └── electrs-0.10.5/
        └── electrs
```

All binaries are set `chmod 755` automatically. This layout is recognised by **BitEngine**'s binary updater.

### Graceful task cancellation
All long-running child processes are spawned with `kill_on_drop(true)` — if the application exits mid-build, no orphan processes are left behind.

---

## Build

### Prerequisites

```bash
# Install Rust (skip if already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Apple Silicon target (already present on arm64 Macs — add to be sure)
rustup target add aarch64-apple-darwin

# Intel Mac target
rustup target add x86_64-apple-darwin

# Required Homebrew packages
brew install cmake llvm boost miniupnpc zeromq sqlite libevent rocksdb \
             automake libtool pkg-config python git rust
```

> **Requires:** Rust 1.80+, macOS 12 Monterey or later, Xcode Command Line Tools (`xcode-select --install`)

### Development build

```bash
cargo build
./target/debug/bitcoin-compiler
```

### Release build (optimised, stripped)

```bash
# Apple Silicon
cargo build --release --target aarch64-apple-darwin

# Intel
cargo build --release --target x86_64-apple-darwin
```

### Bundle as a `.app`

```bash
./build.sh
# Output: ./dist/BitForge.app

open dist/BitForge.app
```

The script compiles for the current architecture, assembles the `.app` directory structure, writes `Info.plist`, copies the binary, and applies an ad-hoc codesign so Gatekeeper does not block local execution.

#### Universal binary (arm64 + x86_64)

```bash
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin

lipo -create \
  target/aarch64-apple-darwin/release/bitcoin-compiler \
  target/x86_64-apple-darwin/release/bitcoin-compiler \
  -output dist/BitForge.app/Contents/MacOS/BitForge

codesign --force --deep --sign "-" dist/BitForge.app
```

---

## Distribution & codesigning

For distribution outside the App Store you need a **Developer ID Application** certificate from Apple:

```bash
# Sign
codesign --force --deep \
  --sign "Developer ID Application: Your Name (TEAMID)" \
  --options runtime \
  dist/BitForge.app

# Notarise (requires app-specific password from appleid.apple.com)
xcrun notarytool submit dist/BitForge.app \
  --apple-id you@example.com \
  --team-id TEAMID \
  --password APP_SPECIFIC_PASSWORD \
  --wait

# Staple the ticket so the app passes Gatekeeper offline
xcrun stapler staple dist/BitForge.app
```

---

## Architecture

```
src/
├── main.rs        Entry point
│                  · Widens PATH for child processes (Homebrew, Cargo, LLVM)
│                  · Creates tokio multi-thread runtime (scaled to CPU count)
│                  · Creates std::sync::mpsc channels (AppMessage, ConfirmRequest)
│                  · Launches eframe (Metal/wgpu) on the main thread
│
├── app.rs         egui application state and render loop
│                  · BitcoinCompilerApp struct (all UI state)
│                  · drain_messages(): processes channel inbox each frame
│                  · render_modal(): Alert and Yes/No Confirm overlays
│                  · Repaint at 50 ms while busy, 250 ms when idle
│
├── messages.rs    Channel message types
│                  · AppMessage: Log | Progress | VersionsLoaded | ShowDialog | TaskDone
│                  · ConfirmRequest: title + message + oneshot reply channel
│                  · log_msg(): shared log helper used by all modules
│
├── compiler.rs    Bitcoin Core and Electrs compilation logic
│                  · compile_bitcoin(): clone/update → cmake or autotools → copy
│                  · compile_electrs(): clone/update → cargo build → copy
│                  · parse_version(): LazyLock<Regex> (compiled once)
│                  · validate_version_tag(): shell-injection guard
│                  · shell_quote(): safe POSIX quoting for git args
│
├── deps.rs        Dependency checking and installation
│                  · check_dependencies_task(): async, tokio::process throughout
│                  · check_rust_installation(): probe → brew install → re-probe
│                  · ask_confirm(): oneshot bridge for UI Yes/No dialogs
│
├── github.rs      GitHub Releases API client
│                  · LazyLock<reqwest::Client>: single shared connection pool
│                  · Filters prerelease flag AND "rc" in tag name
│                  · fetch_bitcoin_versions() / fetch_electrs_versions()
│
├── env_setup.rs   Build environment construction
│                  · find_brew(): Apple Silicon then Intel path check
│                  · setup_build_environment(): PATH dedup with HashSet<&str>
│                  · LIBCLANG_PATH / DYLD_LIBRARY_PATH for RocksDB bindgen
│
└── process.rs     Child process management
                   · run_command(): sh -c, concurrent stdout+stderr drain
                   · probe(): async tokio::process, no thread blocking
                   · kill_on_drop(true): no zombie processes on cancellation
```

### Threading model

```
Main thread (egui / eframe event loop)
   ├─ update() called each frame
   │    ├─ drain_messages()  → try_recv() on std::sync::mpsc (non-blocking)
   │    └─ render_modal()    → modal overlay if pending
   └─ request_repaint_after(50 ms | 250 ms)

tokio multi-thread runtime (worker threads = min(CPU count, 8))
   ├─ github::fetch_*()          → reqwest HTTP, shared client pool
   ├─ deps::check_dependencies() → tokio::process brew list / brew install
   └─ compiler::compile_*()      → tokio::process git / cmake / make / cargo
        ├─ stdout reader task  ─┐
        └─ stderr reader task  ─┴→ Sender<AppMessage::Log> → UI channel
```

The egui render loop never blocks. All process I/O runs on the tokio runtime. Communication is exclusively through `std::sync::mpsc` (background → UI) and `tokio::sync::oneshot` (UI → background for Yes/No confirmations).

---

## Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `eframe` / `egui` | 0.28 | GUI framework (Metal via wgpu, immediate-mode) |
| `tokio` | 1 | Async runtime (rt-multi-thread, process, io-util, sync, time) |
| `reqwest` | 0.12 | HTTP client for GitHub API (rustls, no OpenSSL) |
| `serde` | 1 | JSON deserialisation of GitHub API responses |
| `anyhow` | 1 | Ergonomic error propagation throughout |
| `regex` | 1 | Version tag parsing (LazyLock, compiled once) |
| `rfd` | 0.14 | Native macOS folder picker (NSOpenPanel) |

---

## Related projects

- [BitEngine](https://github.com/csd113/BitEngine) — launch, monitor, and shut down the nodes that BitForge builds
- [Bitcoin Core](https://github.com/bitcoin/bitcoin)
- [Electrs](https://github.com/romanz/electrs)

---

## License

MIT — see [LICENSE](LICENSE).
