#!/usr/bin/env bash
# Builds the poo CLI and Poo.app (menu bar + window) with the CLI bundled inside.
#   ARCHS="arm64 x86_64"  universal build (default: this machine's arch)
#   SIGN_IDENTITY="Developer ID Application: ..."  (default: one from the keychain, else ad-hoc)
#   SPARKLE_PUBLIC_KEY=<base64 EdDSA public key>  turns on Sparkle updates (default: off)
#   SPARKLE_FEED_URL  (default: the latest GitHub release's appcast.xml)
#   OUT=dist
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${VERSION:-$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)}"
OUT="${OUT:-$ROOT/dist}"
ARCHS="${ARCHS:-$(uname -m)}"
# Default: a Developer ID (or Apple Development) identity from the keychain when there is
# one, so the app keeps one code identity across rebuilds and macOS keeps its privacy grants
# (Downloads, Desktop, Full Disk Access). Ad-hoc signing changes identity on every build.
if [ -z "${SIGN_IDENTITY:-}" ]; then
  SIGN_IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -n1)"
  [ -n "$SIGN_IDENTITY" ] || SIGN_IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' | head -n1)"
  [ -n "$SIGN_IDENTITY" ] || SIGN_IDENTITY="-"
fi
SPARKLE_PUBLIC_KEY="${SPARKLE_PUBLIC_KEY:-}"
SPARKLE_FEED_URL="${SPARKLE_FEED_URL:-https://github.com/morajabi/poo/releases/latest/download/appcast.xml}"

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
SWIFT_BIN=""
for a in $ARCHS; do
  swift build -c release --build-system native --package-path "$ROOT/app" --arch "$a"
  dir="$(swift build -c release --build-system native --package-path "$ROOT/app" --arch "$a" --show-bin-path)"
  app_bins+=("$dir/Poo")
  SWIFT_BIN="${SWIFT_BIN:-$dir}"
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
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/bin" "$APP/Contents/Frameworks"
cp "$BIN_DIR/Poo" "$APP/Contents/MacOS/Poo"
# Sparkle ships as a binary framework; keep its symlinks (ditto) and point the app at it.
# The framework is already universal, so any arch's copy will do.
ditto "$SWIFT_BIN/Sparkle.framework" "$APP/Contents/Frameworks/Sparkle.framework"
install_name_tool -add_rpath "@executable_path/../Frameworks" "$APP/Contents/MacOS/Poo"
cp "$OUT/poo" "$APP/Contents/Resources/bin/poo"
"$ROOT/scripts/build-icon.sh" "$APP/Contents/Resources"
SPARKLE_KEYS=""
if [ -n "$SPARKLE_PUBLIC_KEY" ]; then
  SPARKLE_KEYS="  <key>SUFeedURL</key><string>$SPARKLE_FEED_URL</string>
  <key>SUPublicEDKey</key><string>$SPARKLE_PUBLIC_KEY</string>
  <key>SUEnableAutomaticChecks</key><true/>
  <key>SUAutomaticallyUpdate</key><true/>
  <key>SUScheduledCheckInterval</key><integer>86400</integer>"
else
  echo "==> no SPARKLE_PUBLIC_KEY: this build will not update itself"
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
  <key>CFBundleIconName</key><string>AppIcon</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>LSUIElement</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHumanReadableCopyright</key><string>MIT License</string>
$SPARKLE_KEYS
</dict>
</plist>
PLIST

echo "==> signing ($SIGN_IDENTITY)"
# Developer ID: hardened runtime + timestamp, as notarization requires. Ad-hoc: neither,
# because library validation under the hardened runtime refuses to load an ad-hoc
# signed Sparkle.framework (ad-hoc code has no Team ID to match).
if [ "$SIGN_IDENTITY" = "-" ]; then
  flags=(--timestamp=none)
else
  flags=(--options runtime --timestamp)
fi
sign() { codesign --force "${flags[@]}" --sign "$SIGN_IDENTITY" "$@"; }
# Inside out: Sparkle's helpers, the framework, the bundled CLI, then the app.
SPK="$APP/Contents/Frameworks/Sparkle.framework/Versions/B"
sign "$SPK/XPCServices/Installer.xpc"
sign --preserve-metadata=entitlements "$SPK/XPCServices/Downloader.xpc"
sign "$SPK/Autoupdate"
sign "$SPK/Updater.app"
sign "$APP/Contents/Frameworks/Sparkle.framework"
sign "$APP/Contents/Resources/bin/poo"
sign "$APP"
codesign --verify --deep --strict "$APP"

echo "==> archives"
( cd "$OUT" && rm -f Poo.app.zip && ditto -c -k --keepParent Poo.app Poo.app.zip )
echo "$APP"
