# Bitcoin Core & Electrs Compiler (Rust/macOS)

A complete, native macOS GUI application for compiling Bitcoin Core and Electrs
from source.  Written in idiomatic Rust as a full rewrite of the original Python
/tkinter application.

---

## Architecture Analysis (original Python app)

The Python application is a 1 072-line Tkinter GUI with:

| Component | Description |
|-----------|-------------|
| **GUI layer** | tkinter + ttk: header, Step 1–3 frames, progress bar, dark-terminal log, compile button, status bar |
| **Process management** | `subprocess.Popen` with `shell=True`, real-time stdout streaming via `for line in process.stdout` |
| **Threading** | Every blocking action runs in a `threading.Thread(daemon=True)` so the Tk event loop never stalls |
| **GitHub API** | `requests.get(…)` to `/repos/bitcoin/bitcoin/releases` and `/repos/romanz/electrs/releases`, RC tags filtered out |
| **Dependency checker** | `brew list <pkg>` probe loop → optional `brew install`; `rustc --version` / `cargo --version` probes |
| **Build logic** | Bitcoin: CMake for v25+, Autotools for older; Electrs: `cargo build --release` |
| **File management** | `git clone --depth 1 --branch <tag>` or `git fetch`+`git checkout`; `shutil.copy2` + `chmod 755` |
| **Environment** | Constructs a full `PATH` string (Homebrew + ~/.cargo/bin + LLVM) and sets LIBCLANG_PATH |
| **Dialogs** | `messagebox.showinfo/showerror/askyesno` from the tkinter main thread |

Control flow:
1. Detect brew → build PATH → launch GUI → start background thread to load versions.
2. User clicks **Check & Install Dependencies** → background thread checks brew packages + Rust toolchain, installs missing items, shows dialogs.
3. User selects target/version/cores/dir → clicks **Start Compilation** → background thread clones/updates source, builds with the correct tool, copies binaries, shows result dialog.

---

## Rust Framework Decision

**Choice: egui / eframe** (over Tauri or iced)

| Criterion | egui/eframe | Tauri | iced |
|-----------|-------------|-------|------|
| Build steps | Single `cargo build` | Needs Node.js/npm | Single `cargo build` |
| macOS renderer | Metal via `wgpu` | WebKit/WKWebView | Custom wgpu |
| Complexity | Low | High | Medium |
| `.app` bundling | `cargo-bundle` | Built-in | `cargo-bundle` |
| Async integration | Simple channels | Built-in | Complex |
| Visual match to Tkinter | Good | Exact | Good |

egui is immediate-mode: every frame the UI is rebuilt from state, matching
tkinter's event-driven model in a simpler way, with no separate "update state
→ signal widget" cycle.

---

## Project Structure

```
bitcoin-compiler/
├── Cargo.toml            # Dependencies and bundle metadata
└── src/
    ├── main.rs           # Entry point: runtime, channels, eframe launch
    ├── app.rs            # BitcoinCompilerApp: egui UI + state + spawners
    ├── messages.rs       # AppMessage enum, ConfirmRequest struct
    ├── env_setup.rs      # find_brew(), brew_prefix(), setup_build_environment()
    ├── github.rs         # fetch_bitcoin_versions(), fetch_electrs_versions()
    ├── process.rs        # run_command() async subprocess with streaming, probe()
    ├── compiler.rs       # compile_bitcoin(), compile_electrs(), copy_binaries()
    └── deps.rs           # check_dependencies_task()
```

### Concurrency design

```
  Main thread (egui)                Tokio thread pool
  ──────────────────                ─────────────────
  update() every frame
    drain msg_rx ──────────────── AppMessage::Log / Progress / Versions / Dialog / TaskDone
    drain confirm_rx ──────────── ConfirmRequest { title, message, response_tx }
    render modal if pending
    render UI
    │
    on button click ──────────── runtime.spawn(async move { … })
                                   │  log_tx.send(AppMessage::Log(…))
                                   │  progress_tx.send(AppMessage::Progress(…))
                                   │  confirm_tx.send(ConfirmRequest { response_tx }) ──► UI modal
                                   │  response_rx.await ◄── oneshot from UI
                                   │  …
                                   └─ done_tx.send(AppMessage::TaskDone)
```

- `std::sync::mpsc` (synchronous) for **background → UI** messages — polled with `try_recv()` in `update()`.
- `tokio::sync::oneshot` for **UI → background** confirmation replies — awaited cooperatively.
- **No `unwrap()`** in production code paths; all errors propagate via `anyhow::Result`.

---

## Prerequisites

```bash
# macOS 12+ / Apple Silicon or Intel

# 1. Homebrew
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

# 2. Rust toolchain (stable)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# 3. (Optional) cargo-bundle for .app packaging
cargo install cargo-bundle
```

---

## Build

```bash
cd bitcoin-compiler

# Debug build (faster compile, larger binary)
cargo build

# Release build (optimised, stripped — recommended)
cargo build --release
```

The compiled binary is at:
```
target/release/bitcoin-compiler
```

---

## Run

```bash
# Direct
./target/release/bitcoin-compiler

# Or via cargo
cargo run --release
```

On first launch the app will:
1. Set a wide `PATH` covering Homebrew and `~/.cargo/bin`.
2. Fetch Bitcoin Core and Electrs release tags from GitHub in the background.
3. Display the log terminal and wait for user interaction.

---

## Bundle as a macOS .app

Using [cargo-bundle](https://github.com/burtonageo/cargo-bundle):

```bash
# Install cargo-bundle (once)
cargo install cargo-bundle

# Create the .app bundle
cargo bundle --release
```

The bundle will be created at:
```
target/release/bundle/osx/Bitcoin Compiler.app
```

You can drag it to `/Applications` like any other macOS application.

---

## Code Signing (optional)

To distribute outside the App Store or for Gatekeeper notarization:

```bash
# 1. Sign the bundle with your Developer ID
codesign --deep --force --verify --verbose \
  --sign "Developer ID Application: Your Name (TEAMID)" \
  "target/release/bundle/osx/Bitcoin Compiler.app"

# 2. Notarize (requires Apple ID + app-specific password)
xcrun notarytool submit \
  "target/release/bundle/osx/Bitcoin Compiler.app" \
  --apple-id your@apple.id \
  --team-id YOURTEAMID \
  --password "@keychain:AC_PASSWORD" \
  --wait

# 3. Staple the notarization ticket
xcrun stapler staple "target/release/bundle/osx/Bitcoin Compiler.app"
```

For local/personal use, right-click → Open to bypass Gatekeeper once.

---

## Cross-compilation (Intel ↔ Apple Silicon)

```bash
# Add the other target
rustup target add x86_64-apple-darwin    # Intel
rustup target add aarch64-apple-darwin   # Apple Silicon

# Build a universal binary
cargo build --release --target x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin

lipo -create \
  target/x86_64-apple-darwin/release/bitcoin-compiler \
  target/aarch64-apple-darwin/release/bitcoin-compiler \
  -output target/release/bitcoin-compiler-universal
```

---

## Rust Optimisations over Python

| Concern | Python | Rust |
|---------|--------|------|
| Memory | Unbounded tkinter Text widget | Log buffer trimmed at 4 000 lines |
| CPU (idle) | Tkinter event loop polls | `request_repaint_after(250ms)` when idle |
| CPU (busy) | Busy-loop in thread | `request_repaint_after(50ms)` drives terminal scroll |
| Thread safety | Global `log_text` widget + `after()` | Typed channels, no shared mutable state |
| Error handling | Bare `except: pass` | `anyhow::Result` propagated everywhere |
| Process streaming | `for line in process.stdout` (blocking) | tokio async BufReader — non-blocking |
| Binary | PyInstaller ≈ 50 MB | Stripped release ≈ 8–12 MB |

---

## Notes

- The app patches the process `PATH` at startup (mirrors the Python `os.environ["PATH"] = …` at module load time) so child processes find `brew`, `cmake`, `cargo`, etc.
- Berkeley DB is intentionally excluded — both here and in the Python original — because it is only needed for legacy wallet support, not for running a `bitcoind` node.
- The `--disable-wallet` / `-DENABLE_WALLET=OFF` flags are always passed to keep builds reproducible and dependency-light.
