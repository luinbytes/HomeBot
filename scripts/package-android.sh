#!/bin/sh
set -eu

input=${1:?usage: package-android.sh SIGNED_APK OUTPUT_DIR}
output_dir=${2:?usage: package-android.sh SIGNED_APK OUTPUT_DIR}
version=${HOMEBOT_VERSION:?set HOMEBOT_VERSION}
signing=${HOMEBOT_ANDROID_SIGNING:?set HOMEBOT_ANDROID_SIGNING to android-release or ci-ephemeral}
case "$version" in ''|*[!0-9A-Za-z.+-]*) echo "invalid version" >&2; exit 2;; esac
case "$signing" in android-release|ci-ephemeral) ;; *) echo "invalid Android signing classification" >&2; exit 2;; esac
test -f "$input" || { echo "missing signed APK: $input" >&2; exit 2; }

find_build_tool() {
  name=$1
  command -v "$name" 2>/dev/null || {
    root=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}
    test -n "$root" || return 1
    find "$root/build-tools" -mindepth 2 -maxdepth 2 -type f -name "$name" -print | LC_ALL=C sort -V | tail -1
  }
}

apksigner=$(find_build_tool apksigner) || { echo "apksigner was not found" >&2; exit 2; }
aapt=$(find_build_tool aapt) || { echo "aapt was not found" >&2; exit 2; }
temporary=$(mktemp -d "${TMPDIR:-/tmp}/homebot-android-package.XXXXXX")
trap 'rm -rf "$temporary"' EXIT HUP INT TERM
verification="$temporary/apksigner.txt"
"$apksigner" verify --verbose --print-certs "$input" > "$verification"
grep -q '^Verifies$' "$verification"
certificate_sha256=$(
  awk 'tolower($0) ~ /certificate sha-256 digest:/ {
    sub(/^.*[Dd][Ii][Gg][Ee][Ss][Tt]:[[:space:]]*/, "")
    print
    exit
  }' "$verification" | tr -d ':[:space:]'
)
case "$certificate_sha256" in
  ''|*[!0-9a-fA-F]*) echo "APK certificate digest is unavailable" >&2; exit 2;;
esac
test "${#certificate_sha256}" -eq 64 || { echo "APK certificate digest has invalid length" >&2; exit 2; }

badging=$("$aapt" dump badging "$input" | sed -n '1p')
printf '%s\n' "$badging" | grep -Fq "name='dev.homebot.android'"
printf '%s\n' "$badging" | grep -Fq "versionName='$version'"

mkdir -p "$output_dir"
basename="HomeBot-$version-android"
artifact="$output_dir/$basename.apk"
cp "$input" "$artifact"
python3 scripts/release-manifest.py \
  --output "$output_dir/$basename.manifest.json" \
  --artifact "$artifact" --platform android --architecture universal \
  --version "$version" --signing "$signing"
python3 - "$output_dir/$basename.signature.json" "$certificate_sha256" "$signing" <<'PY'
import json
import sys
with open(sys.argv[1], "w", encoding="utf-8") as target:
    json.dump({"certificate_sha256": sys.argv[2].lower(), "signing": sys.argv[3]}, target, indent=2, sort_keys=True)
    target.write("\n")
PY
(cd "$output_dir" && sha256sum "$basename.apk" "$basename.manifest.json" "$basename.signature.json" > "$basename.SHA256SUMS")
printf '%s\n' "$artifact" "$output_dir/$basename.manifest.json" "$output_dir/$basename.signature.json" "$output_dir/$basename.SHA256SUMS"
