#!/usr/bin/env bash
# Writes VERSION into Cargo.toml's [package] version for a release build. CI only;
# the stamped file is never committed (the tag records the version instead).
set -euo pipefail
cd "$(dirname "$0")/.."
v="${1:?usage: stamp-version.sh VERSION}"
case "$v" in *[!0-9.]*|"") echo "bad version $v" >&2; exit 1 ;; esac
perl -0pi -e "s/^version = \"[^\"]*\"/version = \"$v\"/m" Cargo.toml
grep -m1 '^version' Cargo.toml
