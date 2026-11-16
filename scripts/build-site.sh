#!/usr/bin/env bash
# Builds the landing page (poo.morajabi.com) into OUT (default _site): the page, the
# screenshots it shows, and install.sh at /install so `curl -fsSL poo.morajabi.com/install | sh` works.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/_site}"
rm -rf "$OUT" && mkdir -p "$OUT/images"
cp "$ROOT/site/index.html" "$ROOT/site/CNAME" "$OUT/"
touch "$OUT/.nojekyll"
for f in icon.png menubar-expanded.png overview.png cli-overview.png; do
  src="$ROOT/site/images/$f"; [ -f "$src" ] || src="$ROOT/docs/images/$f"
  cp "$src" "$OUT/images/$f"
done
cp "$ROOT/docs/images/menubar-expanded.png" "$OUT/images/og.png"
cp "$ROOT/install.sh" "$OUT/install"
cp "$ROOT/install.sh" "$OUT/install.sh"
echo "site in $OUT"
