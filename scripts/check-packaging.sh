#!/bin/sh
set -eu

temporary=$(mktemp -d "${TMPDIR:-/tmp}/homebot-packaging-check.XXXXXX")
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

sh -n scripts/package-macos.sh
sh -n scripts/verify-macos-bundle.sh
sh -n scripts/notarize-macos-app.sh
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

mkdir -p "$temporary/bin" "$temporary/HomeBot.app"
: > "$temporary/notary.zip"
cat > "$temporary/bin/xcrun" <<'SH'
#!/bin/sh
if [ "$1" = notarytool ]; then
  printf '%s\n' '{"id":"fixture-notary-id","status":"Accepted","message":"must not persist"}'
fi
exit 0
SH
cat > "$temporary/bin/codesign" <<'SH'
#!/bin/sh
exit 0
SH
cp "$temporary/bin/codesign" "$temporary/bin/spctl"
chmod 755 "$temporary/bin/xcrun" "$temporary/bin/codesign" "$temporary/bin/spctl"
PATH="$temporary/bin:$PATH" scripts/notarize-macos-app.sh \
  "$temporary/HomeBot.app" "$temporary/notary.zip" homebot-ci "$temporary/notarization.json"
python3 - "$temporary/notarization.json" <<'PY'
import json
import sys
with open(sys.argv[1], encoding="utf-8") as source:
    evidence = json.load(source)
assert evidence == {"id": "fixture-notary-id", "status": "Accepted"}
PY
cat > "$temporary/bin/xcrun" <<'SH'
#!/bin/sh
printf '%s\n' '{"id":"fixture-rejected-id","status":"Invalid","message":"sensitive diagnostic"}'
SH
chmod 755 "$temporary/bin/xcrun"
if PATH="$temporary/bin:$PATH" scripts/notarize-macos-app.sh \
  "$temporary/HomeBot.app" "$temporary/notary.zip" homebot-ci "$temporary/rejected.json"; then
  echo "rejected notarisation unexpectedly succeeded" >&2
  exit 1
fi
test ! -e "$temporary/rejected.json"

cat > "$temporary/bin/apksigner" <<'SH'
#!/bin/sh
printf '%s\n' 'Verifies' 'Signer #1 certificate SHA-256 digest: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
SH
cat > "$temporary/bin/aapt" <<'SH'
#!/bin/sh
printf "%s\n" "package: name='dev.homebot.android' versionCode='1000000' versionName='1.0.0'"
SH
chmod 755 "$temporary/bin/apksigner" "$temporary/bin/aapt"
printf 'signed-apk-fixture' > "$temporary/signed.apk"
PATH="$temporary/bin:$PATH" HOMEBOT_VERSION=1.0.0 HOMEBOT_ANDROID_SIGNING=ci-ephemeral \
  scripts/package-android.sh "$temporary/signed.apk" "$temporary/android-dist"
(cd "$temporary/android-dist" && sha256sum -c HomeBot-1.0.0-android.SHA256SUMS)
python3 - "$temporary/android-dist/HomeBot-1.0.0-android.signature.json" <<'PY'
import json
import sys
with open(sys.argv[1], encoding="utf-8") as source:
    evidence = json.load(source)
assert evidence["signing"] == "ci-ephemeral"
assert len(evidence["certificate_sha256"]) == 64
PY
