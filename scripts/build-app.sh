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
arch_flags=()
for a in $ARCHS; do arch_flags+=(--arch "$a"); done
swift build -c release --package-path "$ROOT/app" "${arch_flags[@]}"
BIN_DIR="$(swift build -c release --package-path "$ROOT/app" "${arch_flags[@]}" --show-bin-path)"

APP="$OUT/Poo.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/bin"
cp "$BIN_DIR/Poo" "$APP/Contents/MacOS/Poo"
cp "$OUT/poo" "$APP/Contents/Resources/bin/poo"
if [ -f "$ROOT/app/Resources/AppIcon.icns" ]; then
  cp "$ROOT/app/Resources/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
fi
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
  <key>LSMinimumSystemVersion</key><string>14.0</string>
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
