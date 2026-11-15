#!/usr/bin/env python3
"""Writes latest.json, the manifest the installer, `poo update` and the app read.

usage: release-manifest.py VERSION BASE_URL ARTIFACT_DIR
"""
import hashlib
import json
import pathlib
import sys

version, base, folder = sys.argv[1], sys.argv[2].rstrip("/"), pathlib.Path(sys.argv[3])
assets, sums = {}, {}
for f in sorted(folder.iterdir()):
    if f.name.startswith("poo-") and f.name.endswith(".tar.gz"):
        key = "cli-" + f.name[len("poo-"):-len(".tar.gz")]
    elif f.name == "Poo.app.zip":
        key = "app-macos"
    else:
        continue
    assets[key] = f"{base}/{f.name}"
    sums[key] = hashlib.sha256(f.read_bytes()).hexdigest()

print(json.dumps({"version": version, "notes": f"poo {version}", "assets": assets, "sha256": sums}, indent=2))
