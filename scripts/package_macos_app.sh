#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

APP_NAME="${APP_NAME:-BitForge}"
EXECUTABLE_NAME="${EXECUTABLE_NAME:-}"
BUNDLE_ID="${BUNDLE_ID:-com.bitforge.app}"
VERSION="${VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -n1)}"
MINIMUM_MACOS="${MINIMUM_MACOS:-12.0}"
ICON_PATH="${ICON_PATH:-$REPO_ROOT/app-icon.icns}"
BINARY_PATH="${BINARY_PATH:-}"
OUTPUT_DIR="${OUTPUT_DIR:-$REPO_ROOT/dist}"
ZIP_NAME="${ZIP_NAME:-}"
CODESIGN_IDENTITY="${CODESIGN_IDENTITY:--}"

err() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

[[ -n "$VERSION" ]] || err "Could not determine version from Cargo.toml"
if [[ -z "$BINARY_PATH" ]]; then
    (cd "$REPO_ROOT" && cargo build)
    BINARY_PATH="$REPO_ROOT/target/debug/bitforge"
fi
[[ -f "$BINARY_PATH" ]] || err "Binary not found: $BINARY_PATH"
[[ -f "$ICON_PATH" ]] || err "Icon not found: $ICON_PATH"
if [[ -z "$EXECUTABLE_NAME" ]]; then
    EXECUTABLE_NAME="$APP_NAME"
fi

APP_DIR="$OUTPUT_DIR/${APP_NAME}.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"
EXECUTABLE_PATH="$MACOS_DIR/$EXECUTABLE_NAME"
PLIST_PATH="$CONTENTS_DIR/Info.plist"

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

cp -X "$BINARY_PATH" "$EXECUTABLE_PATH"
chmod 755 "$EXECUTABLE_PATH"
cp -X "$ICON_PATH" "$RESOURCES_DIR/app-icon.icns"
printf 'APPL????' > "$CONTENTS_DIR/PkgInfo"

cat > "$PLIST_PATH" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
    "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>

    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>

    <key>CFBundleExecutable</key>
    <string>${EXECUTABLE_NAME}</string>

    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>

    <key>CFBundleVersion</key>
    <string>${VERSION}</string>

    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>

    <key>CFBundlePackageType</key>
    <string>APPL</string>

    <key>CFBundleIconFile</key>
    <string>app-icon</string>

    <key>LSMinimumSystemVersion</key>
    <string>${MINIMUM_MACOS}</string>
</dict>
</plist>
PLIST

plutil -lint "$PLIST_PATH"
test -x "$EXECUTABLE_PATH"
test -f "$RESOURCES_DIR/app-icon.icns"

icon_file="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' "$PLIST_PATH")"
[[ "$icon_file" == "app-icon" ]] || err "CFBundleIconFile should be app-icon, found: $icon_file"

xattr -cr "$APP_DIR" 2>/dev/null || true
codesign --force --sign "$CODESIGN_IDENTITY" --deep "$APP_DIR"
codesign --verify --deep --strict --verbose=2 "$APP_DIR"

if [[ -n "$ZIP_NAME" ]]; then
    mkdir -p "$OUTPUT_DIR"
    ZIP_PATH="$OUTPUT_DIR/$ZIP_NAME"
    rm -f "$ZIP_PATH"
    ditto -c -k --keepParent "$APP_DIR" "$ZIP_PATH"
    printf '%s\n' "$ZIP_PATH"
fi

printf '%s\n' "$APP_DIR"
