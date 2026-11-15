#!/usr/bin/env bash
# Renders packaging/homebrew/*.rb for a published release: scripts/homebrew.sh 0.1.0 > out dir
set -euo pipefail
VERSION="$1"
OUT="${2:-dist/homebrew}"
mkdir -p "$OUT"
manifest="$(curl -fsSL "https://github.com/morajabi/poo/releases/download/v$VERSION/latest.json")"
sha() { python3 -c "import json,sys; print(json.loads(sys.argv[1])['sha256'].get(sys.argv[2], ''))" "$manifest" "$1"; }
sed -e "s/VERSION/$VERSION/g" \
    -e "s/SHA_AARCH64_DARWIN/$(sha cli-aarch64-apple-darwin)/" \
    -e "s/SHA_X86_64_DARWIN/$(sha cli-x86_64-apple-darwin)/" \
    -e "s/SHA_AARCH64_LINUX/$(sha cli-aarch64-unknown-linux-gnu)/" \
    -e "s/SHA_X86_64_LINUX/$(sha cli-x86_64-unknown-linux-gnu)/" \
    packaging/homebrew/poo.rb > "$OUT/poo.rb"
sed -e "s/VERSION/$VERSION/g" -e "s/SHA_APP/$(sha app-macos)/" packaging/homebrew/poo-app.rb > "$OUT/poo-app.rb"
echo "$OUT/poo.rb $OUT/poo-app.rb"
