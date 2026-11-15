#!/usr/bin/env bash
# Picks the version for a release of COMMIT (default HEAD) and prints it.
# major.minor come from Cargo.toml; the patch is one past the highest v<major>.<minor>.*
# tag (or Cargo.toml's patch when there is none yet). A commit that already carries a
# release tag keeps it, so re-running a release is idempotent. Needs the tags fetched.
set -euo pipefail
cd "$(dirname "$0")/.."
commit="${1:-HEAD}"
base="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
mm="${base%.*}"
patch0="${base##*.}"

existing="$(git tag --points-at "$commit" -l "v$mm.*" | sort -V | tail -n1)"
if [ -n "$existing" ]; then
  echo "${existing#v}"
  exit 0
fi
last="$(git tag -l "v$mm.*" | sed "s/^v$mm\.//" | grep -E '^[0-9]+$' | sort -n | tail -n1 || true)"
if [ -z "$last" ]; then
  next="$patch0"
else
  next=$((last + 1))
  [ "$next" -lt "$patch0" ] && next="$patch0"
fi
echo "$mm.$next"
