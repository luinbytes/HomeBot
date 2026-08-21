#!/bin/sh
set -eu

temporary=$(mktemp -d "${TMPDIR:-/tmp}/homebot-packaging-check.XXXXXX")
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

sh -n scripts/package-macos.sh
sh -n scripts/verify-macos-bundle.sh
sh -n scripts/render-arch-pkgbuild.sh
sh -n scripts/verify-arch-package.sh

source_sha=$(sha256sum Cargo.toml | cut -d' ' -f1)
scripts/render-arch-pkgbuild.sh 0.0.1 "$source_sha" 1 "$temporary/PKGBUILD"
bash -n "$temporary/PKGBUILD"
python3 scripts/release-manifest.py \
  --output "$temporary/manifest.json" --artifact Cargo.toml \
  --platform linux --architecture x86_64 --version 0.0.1 --signing package
python3 - "$temporary/manifest.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    manifest = json.load(source)
assert manifest["schema_version"] == 1
assert manifest["product"] == "HomeBot"
assert manifest["protocol_minimum"] == 1
assert manifest["protocol_maximum"] == 1
assert len(manifest["sha256"]) == 64
PY
python3 -c "import xml.etree.ElementTree as tree; tree.parse('packaging/arch/homebot.svg')"
