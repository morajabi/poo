#!/bin/sh
# poo installer: curl -fsSL https://raw.githubusercontent.com/morajabi/poo/main/install.sh | sh
#   POO_INSTALL_DIR   where the CLI goes (default ~/.local/bin)
#   POO_APP=1         also install Poo.app (menu bar + window) into /Applications (or ~/Applications)
#   POO_VERSION       not supported yet; installs the latest release
set -eu

REPO="morajabi/poo"
MANIFEST="${POO_RELEASES_URL:-https://github.com/$REPO/releases/latest/download/latest.json}"
DIR="${POO_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
die() { printf 'poo install: %s\n' "$*" >&2; exit 1; }

os="$(uname -s)"
arch="$(uname -m)"
case "$arch" in arm64|aarch64) arch=aarch64 ;; x86_64|amd64) arch=x86_64 ;; *) die "unsupported CPU $arch" ;; esac
case "$os" in
  Darwin) key="cli-$arch-apple-darwin" ;;
  Linux) key="cli-$arch-unknown-linux-gnu" ;;
  *) die "unsupported OS $os" ;;
esac

command -v curl >/dev/null || die "curl is required"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "$MANIFEST" -o "$tmp/latest.json" || die "could not download the release manifest"
# Tiny JSON reads without jq: "key": "value" pairs inside a named object.
read_json() { # $1 object, $2 key
  awk -v obj="\"$1\"" -v key="\"$2\"" '
    index($0, obj) { inside = 1 }
    inside && index($0, key) { sub(/.*": *"/, ""); sub(/".*/, ""); print; exit }
    inside && /}/ { inside = 0 }
  ' "$tmp/latest.json"
}
version="$(awk -F'"' '/"version"/ { print $4; exit }' "$tmp/latest.json")"
url="$(read_json assets "$key")"
sum="$(read_json sha256 "$key")"
[ -n "$url" ] || die "no build for $key in release $version"

say "Installing poo $version ($key)"
curl -fsSL "$url" -o "$tmp/poo.tar.gz" || die "download failed"
if command -v shasum >/dev/null; then got="$(shasum -a 256 "$tmp/poo.tar.gz" | cut -d' ' -f1)"
elif command -v sha256sum >/dev/null; then got="$(sha256sum "$tmp/poo.tar.gz" | cut -d' ' -f1)"
else got="$(openssl dgst -sha256 "$tmp/poo.tar.gz" | awk '{print $NF}')"; fi
[ "$got" = "$sum" ] || die "checksum mismatch (expected $sum, got $got)"

tar -xzf "$tmp/poo.tar.gz" -C "$tmp"
bin="$(find "$tmp" -type f -name poo | head -n1)"
[ -n "$bin" ] || die "archive has no poo binary"
mkdir -p "$DIR"
install -m 0755 "$bin" "$DIR/poo"
say "Installed $DIR/poo"

if [ "${POO_APP:-0}" = "1" ] && [ "$os" = "Darwin" ]; then
  app_url="$(read_json assets app-macos)"
  app_sum="$(read_json sha256 app-macos)"
  if [ -n "$app_url" ]; then
    curl -fsSL "$app_url" -o "$tmp/Poo.app.zip" || die "could not download Poo.app"
    got="$(shasum -a 256 "$tmp/Poo.app.zip" | cut -d' ' -f1)"
    [ "$got" = "$app_sum" ] || die "app checksum mismatch"
    ditto -x -k "$tmp/Poo.app.zip" "$tmp/app"
    apps=/Applications
    [ -w "$apps" ] || { apps="$HOME/Applications"; mkdir -p "$apps"; }
    osascript -e 'tell application id "com.morajabi.poo" to quit' >/dev/null 2>&1 || true
    # New copy in place first, then swap, so a failure never leaves no app at all.
    ditto "$tmp/app/Poo.app" "$apps/.Poo.app.new" || die "could not write to $apps"
    rm -rf "$apps/Poo.app"
    mv "$apps/.Poo.app.new" "$apps/Poo.app"
    say "Installed $apps/Poo.app"
    open "$apps/Poo.app" || true
  fi
fi

case ":$PATH:" in *":$DIR:"*) ;; *) say "Add $DIR to your PATH, e.g.: echo 'export PATH=\"$DIR:\$PATH\"' >> ~/.zshrc" ;; esac
say ""
say "Next: poo setup          (choose your dev folders)"
say "      poo integration install all   (skill + MCP for Claude Code, Codex, Cursor...)"
say "Agents: poo --skill. People: poo --help."
say "poo updates itself in the background once a day (turn off: poo config set updates.auto false)."
