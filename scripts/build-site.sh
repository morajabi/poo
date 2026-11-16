#!/usr/bin/env bash
# Builds the landing page (poo.morajabi.com) into OUT (default _site): the page, the
# icon, a social preview image, and install.sh at /install so `curl -fsSL poo.morajabi.com/install | sh` works.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/_site}"
rm -rf "$OUT" && mkdir -p "$OUT/images"
cp "$ROOT/site/index.html" "$ROOT/site/CNAME" "$OUT/"
touch "$OUT/.nojekyll"
cp "$ROOT"/site/images/*.png "$OUT/images/"
cp -R "$ROOT/site/fonts" "$OUT/fonts"
cp "$ROOT/install.sh" "$OUT/install"
cp "$ROOT/install.sh" "$OUT/install.sh"
echo "site in $OUT"
