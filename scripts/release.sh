#!/usr/bin/env bash
# Cuts a release: tags origin/main's head with the next version and pushes the tag.
# The release workflow then waits for CI on that commit, builds, signs, notarizes and
# publishes. Pushes to main alone no longer release.
#   scripts/release.sh            next patch (v0.1.11 -> v0.1.12)
#   scripts/release.sh 0.2.0      an explicit version
#   scripts/release.sh --dry-run  print what would be tagged
set -euo pipefail
cd "$(dirname "$0")/.."
dry=0; want=""
for a in "$@"; do
  case "$a" in
    --dry-run) dry=1 ;;
    -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
    *) want="${a#v}" ;;
  esac
done
git fetch -q --tags origin main
sha="$(git rev-parse origin/main)"
if [ -n "$(git tag --points-at "$sha" -l 'v*')" ]; then
  echo "origin/main ($(git rev-parse --short "$sha")) is already released as $(git tag --points-at "$sha" -l 'v*' | tr '\n' ' ')"
  exit 0
fi
version="${want:-$(scripts/release-version.sh "$sha")}"
case "$version" in *[!0-9.]*|"") echo "not a version: $version" >&2; exit 2 ;; esac
git rev-parse -q --verify "refs/tags/v$version" >/dev/null && { echo "v$version already exists" >&2; exit 2; }
prev="$(git tag --sort=-v:refname -l 'v*' | head -n1)"
echo "v$version <- $(git log -1 --format='%h %s' "$sha")"
echo "changes since ${prev:-the start}:"
git log --no-merges --format='  %s' ${prev:+$prev..}"$sha" | grep -v -e '^  docs:' -e '^  chore:' -e '^  site:' | head -n 30
if command -v gh >/dev/null; then
  ci="$(gh run list -w ci -c "$sha" -L 1 --json status,conclusion -q '.[0] | "\(.status) \(.conclusion)"' 2>/dev/null || true)"
  echo "ci on this commit: ${ci:-not started}"
  case "$ci" in completed*) [ "$ci" = "completed success" ] || { echo "ci failed here; fix main first" >&2; exit 1; } ;; esac
fi
[ "$dry" = 1 ] && exit 0
git tag -a "v$version" -m "poo $version" "$sha"
git push -q origin "v$version"
echo "pushed v$version; the release workflow takes it from here (gh run watch)"
