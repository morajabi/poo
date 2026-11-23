#!/usr/bin/env bash
# Renders packaging/homebrew/*.rb for a published release into a tap layout:
#   scripts/homebrew.sh 0.1.16 [out-dir]   ->  out-dir/Formula/poo.rb, out-dir/Casks/poo-app.rb
# The release workflow runs it into a checkout of morajabi/homebrew-tap and pushes.
set -euo pipefail
VERSION="$1"
mkdir -p "${2:-dist/homebrew}"
OUT="$(cd "${2:-dist/homebrew}" && pwd)"
cd "$(dirname "$0")/.."
mkdir -p "$OUT/Formula" "$OUT/Casks"
manifest="$(curl -fsSL "https://github.com/morajabi/poo/releases/download/v$VERSION/latest.json")"
sha() {
  python3 -c '
import json, sys
s = json.loads(sys.argv[1])["sha256"].get(sys.argv[2], "")
if len(s) != 64: sys.exit("no sha256 for " + sys.argv[2] + " in latest.json")
print(s)' "$manifest" "$1"
}
header="# Generated from packaging/homebrew in morajabi/poo for v$VERSION. Do not edit here."
render() { # template out sed-args...
  local tpl="$1" out="$2"; shift 2
  { echo "$header"; grep -v '^#' "$tpl" | sed "$@"; } > "$out"
  if grep -qE 'VERSION|SHA_' "$out"; then echo "unfilled placeholder in $out" >&2; exit 1; fi
}
# Plain assignments, so a missing checksum stops the script under set -e.
a_mac="$(sha cli-aarch64-apple-darwin)"; x_mac="$(sha cli-x86_64-apple-darwin)"
a_linux="$(sha cli-aarch64-unknown-linux-gnu)"; x_linux="$(sha cli-x86_64-unknown-linux-gnu)"
app="$(sha app-macos)"
render packaging/homebrew/poo.rb "$OUT/Formula/poo.rb" \
  -e "s/VERSION/$VERSION/g" \
  -e "s/SHA_AARCH64_DARWIN/$a_mac/" -e "s/SHA_X86_64_DARWIN/$x_mac/" \
  -e "s/SHA_AARCH64_LINUX/$a_linux/" -e "s/SHA_X86_64_LINUX/$x_linux/"
render packaging/homebrew/poo-app.rb "$OUT/Casks/poo-app.rb" -e "s/VERSION/$VERSION/g" -e "s/SHA_APP/$app/"
echo "$OUT/Formula/poo.rb $OUT/Casks/poo-app.rb"
