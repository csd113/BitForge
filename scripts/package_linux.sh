#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

APP_NAME="${APP_NAME:-BitForge}"
BINARY_PATH="${BINARY_PATH:-}"
OUTPUT_DIR="${OUTPUT_DIR:-$REPO_ROOT/dist}"
ARCHIVE_NAME="${ARCHIVE_NAME:-}"

err() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

[[ -n "$BINARY_PATH" ]] || err "BINARY_PATH must be set"
[[ -f "$BINARY_PATH" ]] || err "Binary not found: $BINARY_PATH"
[[ -n "$ARCHIVE_NAME" ]] || err "ARCHIVE_NAME must be set"

STAGING_DIR="$OUTPUT_DIR/${APP_NAME}"
ARCHIVE_PATH="$OUTPUT_DIR/$ARCHIVE_NAME"

rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR" "$OUTPUT_DIR"

cp "$BINARY_PATH" "$STAGING_DIR/bitforge"
chmod 755 "$STAGING_DIR/bitforge"

cat > "$STAGING_DIR/README.txt" <<README
BitForge Linux package

Run:
  ./bitforge

Supported Linux release targets:
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu

See the project README for distro package dependencies.
README

rm -f "$ARCHIVE_PATH"
tar -C "$OUTPUT_DIR" -czf "$ARCHIVE_PATH" "$APP_NAME"
tar -tzf "$ARCHIVE_PATH" >/dev/null

printf '%s\n' "$ARCHIVE_PATH"
