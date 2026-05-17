# Changelog

## Unreleased

### Added

- Added first-class support for Linux x86_64 and Linux ARM64/aarch64 alongside macOS Apple Silicon.
- Added Linux native dependency checks for compiler/linker tools, `pkg-config`, `libevent`, `libclang`, X11/Wayland, `libudev`, and D-Bus development packages.
- Added distro-aware Linux package guidance for Debian/Ubuntu, Fedora, and Arch.
- Added release artifacts for `BitForge-macos-arm64.zip`, `BitForge-linux-x64.tar.gz`, and `BitForge-linux-arm64.tar.gz`.
- Added Linux release packaging for executable-preserving `.tar.gz` archives.

### Changed

- Limited supported macOS builds to Apple Silicon (`aarch64-apple-darwin`) and removed local Intel/universal build paths.
- Made startup, status, build directory defaults, and environment setup platform-aware for macOS and Linux.
- Updated documentation to describe supported platforms, required native dependencies, and release artifacts.

### Fixed

- Removed Homebrew as a startup requirement on Linux while preserving macOS Apple Silicon Homebrew behavior.
- Added a fail-closed startup check for unsupported OS/architecture combinations.

### CI / Release

- Reworked CI to validate macOS Apple Silicon, Linux x86_64, and Linux ARM64 builds.
- Reworked release automation to publish only the three supported artifacts and verify binary architecture, including Linux ARM64.
- Kept Windows, macOS Intel, and universal macOS artifacts out of CI and release outputs.

## v0.1.2 - 2026-05-15

- Renamed the app and build outputs to BitForge across the project.
- Bumped the application and bundle version to 0.1.2.

## v0.1.1 - 2026-04-11

- Fixed dependency checks so the app only reports success when every required Homebrew package is actually installed.
- Hardened binary copying so executable permission failures now stop the build instead of being silently ignored.
- Removed startup panics from runtime/client initialization and replaced them with user-facing startup errors.
- Added a macOS-only GitHub Actions workflow that runs `cargo fmt --all --check` and strict `cargo clippy`.

## v0.1.0 - 2026-04-11

- Initial public release of BitForge.
