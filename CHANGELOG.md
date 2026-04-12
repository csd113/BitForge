# Changelog

## v0.1.1 - 2026-04-11

- Fixed dependency checks so the app only reports success when every required Homebrew package is actually installed.
- Hardened binary copying so executable permission failures now stop the build instead of being silently ignored.
- Removed startup panics from runtime/client initialization and replaced them with user-facing startup errors.
- Added a macOS-only GitHub Actions workflow that runs `cargo fmt --all --check` and strict `cargo clippy`.

## v0.1.0 - 2026-04-11

- Initial public release of BitForge.
