#!/usr/bin/env bash
# Compiles app/Icon/AppIcon.icon (Icon Composer, Liquid Glass) into Assets.car for
# macOS 26+ and AppIcon.icns for macOS 14-15.
#   scripts/build-icon.sh [out-dir]   (default: dist/icon)
#   ICON=app/Icon/concepts/Platter.icon scripts/build-icon.sh   try another concept
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ICON="${ICON:-$ROOT/app/Icon/AppIcon.icon}"
OUT="${1:-$ROOT/dist/icon}"
mkdir -p "$OUT"

# actool takes the icon name from the bundle name, so stage it as AppIcon.icon.
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp -R "$ICON" "$STAGE/AppIcon.icon"

xcrun actool "$STAGE/AppIcon.icon" \
  --compile "$OUT" \
  --app-icon AppIcon \
  --platform macosx \
  --minimum-deployment-target 14.0 \
  --output-partial-info-plist "$STAGE/partial.plist" \
  --output-format human-readable-text >/dev/null
test -f "$OUT/Assets.car" && test -f "$OUT/AppIcon.icns"
