#!/usr/bin/env bash
# Sparkle helpers for releases. The private key is the base64 seed that
# `generate_keys -x` exports, passed in SPARKLE_PRIVATE_KEY (never as an argument).
#   sparkle.sh pubkey                       print the base64 EdDSA public key
#   sparkle.sh appcast VERSION URL ZIP      print an appcast.xml with one signed item
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
: "${SPARKLE_PRIVATE_KEY:?SPARKLE_PRIVATE_KEY is not set}"

case "${1:-}" in
  pubkey)
    # CryptoKit's Ed25519 raw private key is the same 32-byte seed Sparkle stores.
    printf '%s' "$SPARKLE_PRIVATE_KEY" | swift "$ROOT/scripts/sparkle-pubkey.swift"
    ;;
  appcast)
    version="$2"; url="$3"; zip="$4"
    tool="${SIGN_UPDATE:-$ROOT/app/.build/artifacts/sparkle/Sparkle/bin/sign_update}"
    [ -x "$tool" ] || { echo "sign_update not found at $tool (run swift build in app/ first)" >&2; exit 1; }
    # Prints: sparkle:edSignature="..." length="..."
    attrs="$(printf '%s' "$SPARKLE_PRIVATE_KEY" | "$tool" --ed-key-file - "$zip")"
    case "$attrs" in *edSignature=*length=*) ;; *) echo "sign_update gave no signature" >&2; exit 1 ;; esac
    date="$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')"
    repo="${GITHUB_REPOSITORY:-morajabi/poo}"
    cat <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Poo</title>
    <link>https://github.com/$repo</link>
    <item>
      <title>Poo $version</title>
      <pubDate>$date</pubDate>
      <sparkle:version>$version</sparkle:version>
      <sparkle:shortVersionString>$version</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>14.0</sparkle:minimumSystemVersion>
      <sparkle:fullReleaseNotesLink>https://github.com/$repo/releases/tag/v$version</sparkle:fullReleaseNotesLink>
      <enclosure url="$url" type="application/octet-stream" $attrs />
    </item>
  </channel>
</rss>
XML
    ;;
  *) echo "usage: sparkle.sh pubkey | appcast VERSION URL ZIP" >&2; exit 2 ;;
esac
