#!/bin/sh
set -eu

app=${1:?usage: notarize-macos-app.sh APP NOTARY_ZIP KEYCHAIN_PROFILE EVIDENCE_JSON}
notary_zip=${2:?usage: notarize-macos-app.sh APP NOTARY_ZIP KEYCHAIN_PROFILE EVIDENCE_JSON}
profile=${3:?usage: notarize-macos-app.sh APP NOTARY_ZIP KEYCHAIN_PROFILE EVIDENCE_JSON}
evidence=${4:?usage: notarize-macos-app.sh APP NOTARY_ZIP KEYCHAIN_PROFILE EVIDENCE_JSON}

test -d "$app" || { echo "missing application bundle: $app" >&2; exit 2; }
test -f "$notary_zip" || { echo "missing notarisation ZIP: $notary_zip" >&2; exit 2; }
case "$profile" in *[!A-Za-z0-9._-]*|'') echo "invalid notary keychain profile name" >&2; exit 2;; esac

temporary=$(mktemp -d "${TMPDIR:-/tmp}/homebot-notary.XXXXXX")
trap 'rm -rf "$temporary"' EXIT HUP INT TERM
raw="$temporary/notary.json"
xcrun notarytool submit "$notary_zip" --keychain-profile "$profile" --wait --output-format json > "$raw"
python3 - "$raw" "$temporary/evidence.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    result = json.load(source)
if result.get("status") != "Accepted" or not result.get("id"):
    raise SystemExit("Apple notarisation was not accepted")
with open(sys.argv[2], "w", encoding="utf-8") as target:
    json.dump({"id": result["id"], "status": result["status"]}, target, indent=2, sort_keys=True)
    target.write("\n")
PY
xcrun stapler staple "$app"
xcrun stapler validate "$app"
codesign --verify --deep --strict --verbose=2 "$app"
spctl --assess --type execute --verbose=2 "$app"
mkdir -p "$(dirname "$evidence")"
mv "$temporary/evidence.json" "$evidence"
