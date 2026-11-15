#!/usr/bin/env bash
# Builds the poo CLI and Poo.app (menu bar + window) with the CLI bundled inside.
#   ARCHS="arm64 x86_64"  universal build (default: this machine's arch)
#   SIGN_IDENTITY="Developer ID Application: ..."  (default: ad-hoc)
#   OUT=dist
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${VERSION:-$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)}"
OUT="${OUT:-$ROOT/dist}"
ARCHS="${ARCHS:-$(uname -m)}"
SIGN_IDENTITY="${SIGN_IDENTITY:--}"

rust_target() {
  case "$1" in
    arm64|aarch64) echo aarch64-apple-darwin ;;
    x86_64) echo x86_64-apple-darwin ;;
    *) echo "unknown arch $1" >&2; exit 1 ;;
  esac
}

mkdir -p "$OUT"
echo "==> poo CLI $VERSION ($ARCHS)"
bins=()
for a in $ARCHS; do
  t="$(rust_target "$a")"
  rustup target add "$t" >/dev/null 2>&1 || true
  cargo build --release --target "$t" --manifest-path "$ROOT/Cargo.toml"
  bins+=("$ROOT/target/$t/release/poo")
done
lipo -create "${bins[@]}" -output "$OUT/poo"
chmod +x "$OUT/poo"

echo "==> Poo.app"
# The native build system on purpose: SwiftPM's default engine stamps the binary with the
# deployment target as its SDK version, and macOS then draws the app with legacy window
# chrome (old traffic lights, corner radius, toolbars). One build per arch, then lipo.
app_bins=()
for a in $ARCHS; do
  swift build -c release --build-system native --package-path "$ROOT/app" --arch "$a"
  app_bins+=("$(swift build -c release --build-system native --package-path "$ROOT/app" --arch "$a" --show-bin-path)/Poo")
done
BIN_DIR="$OUT/app-bin"
mkdir -p "$BIN_DIR"
lipo -create "${app_bins[@]}" -output "$BIN_DIR/Poo"
sdk="$(vtool -show-build "$BIN_DIR/Poo" | awk '/ sdk /{print $2; exit}')"
if [ "${sdk%%.*}" -lt 26 ]; then
  echo "error: Poo was linked against SDK $sdk; macOS would show legacy UI. Build with Xcode 26 or later." >&2
  exit 1
fi

APP="$OUT/Poo.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/bin"
cp "$BIN_DIR/Poo" "$APP/Contents/MacOS/Poo"
cp "$OUT/poo" "$APP/Contents/Resources/bin/poo"
"$ROOT/scripts/build-icon.sh" "$APP/Contents/Resources"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>com.morajabi.poo</string>
  <key>CFBundleName</key><string>Poo</string>
  <key>CFBundleDisplayName</key><string>Poo</string>
  <key>CFBundleExecutable</key><string>Poo</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleIconName</key><string>AppIcon</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>LSUIElement</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHumanReadableCopyright</key><string>MIT License</string>
</dict>
</plist>
PLIST

echo "==> signing ($SIGN_IDENTITY)"
ts="--timestamp"
[ "$SIGN_IDENTITY" = "-" ] && ts="--timestamp=none"
codesign --force --options runtime "$ts" --sign "$SIGN_IDENTITY" "$APP/Contents/Resources/bin/poo"
codesign --force --options runtime "$ts" --sign "$SIGN_IDENTITY" "$APP"

echo "==> archives"
( cd "$OUT" && rm -f Poo.app.zip && ditto -c -k --keepParent Poo.app Poo.app.zip )
echo "$APP"
