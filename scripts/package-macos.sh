#!/bin/sh
set -eu

if [ "$(uname -s)" != "Darwin" ]; then
  echo "package-macos.sh must run on macOS" >&2
  exit 2
fi

target=${HOMEBOT_TARGET:?set HOMEBOT_TARGET to x86_64-apple-darwin or aarch64-apple-darwin}
case "$target" in
  x86_64-apple-darwin|aarch64-apple-darwin) ;;
  *) echo "unsupported macOS target: $target" >&2; exit 2 ;;
esac

version=${HOMEBOT_VERSION:-0.0.1}
build_version=${HOMEBOT_BUILD_VERSION:-1}
profile=${HOMEBOT_PROFILE:-release}
output_dir=${HOMEBOT_OUTPUT_DIR:-dist}
target_dir=${CARGO_TARGET_DIR:-target}
source_date_epoch=${SOURCE_DATE_EPOCH:-1704067200}
identity=${HOMEBOT_SIGN_IDENTITY:--}
require_release_signing=${HOMEBOT_REQUIRE_RELEASE_SIGNING:-0}

case "$version" in *[!0-9A-Za-z.+-]*) echo "invalid version" >&2; exit 2;; esac
case "$build_version" in ''|*[!0-9]*) echo "build version must be numeric" >&2; exit 2;; esac
if [ "$require_release_signing" = 1 ] && [ "$identity" = - ]; then
  echo "a Developer ID Application identity is required for release packaging" >&2
  exit 3
fi

desktop="$target_dir/$target/$profile/homebot-desktop"
server="$target_dir/$target/$profile/homebot-server"
test -x "$desktop" || { echo "missing desktop binary: $desktop" >&2; exit 2; }
test -x "$server" || { echo "missing server binary: $server" >&2; exit 2; }

stage=$(mktemp -d "${TMPDIR:-/tmp}/homebot-macos.XXXXXX")
trap 'rm -rf "$stage"' EXIT HUP INT TERM
app="$stage/HomeBot.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/bin" "$output_dir"
cp "$desktop" "$app/Contents/MacOS/HomeBot"
cp "$server" "$app/Contents/Resources/bin/homebot-server"
chmod 755 "$app/Contents/MacOS/HomeBot" "$app/Contents/Resources/bin/homebot-server"

sed -e "s/@VERSION@/$version/g" -e "s/@BUILD_VERSION@/$build_version/g" \
  packaging/macos/Info.plist.in > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"

codesign --force --timestamp=none --options runtime --entitlements packaging/macos/HomeBot.entitlements \
  --sign "$identity" "$app/Contents/Resources/bin/homebot-server"
codesign --force --timestamp=none --options runtime --entitlements packaging/macos/HomeBot.entitlements \
  --sign "$identity" "$app/Contents/MacOS/HomeBot"
codesign --force --timestamp=none --options runtime --entitlements packaging/macos/HomeBot.entitlements \
  --sign "$identity" "$app"

scripts/verify-macos-bundle.sh "$app" "$target" "$identity"

# Normalise filesystem timestamps before archiving. `ditto` is also emitted for Apple's
# notarisation service; the tarball is the reproducible release payload.
epoch_stamp=$(date -u -r "$source_date_epoch" +%Y%m%d%H%M.%S)
find "$app" -exec touch -h -t "$epoch_stamp" {} +
case "$target" in
  x86_64-*) arch=x86_64 ;;
  aarch64-*) arch=arm64 ;;
esac
basename="HomeBot-$version-macos-$arch"
archive="$output_dir/$basename.tar.gz"
notary_zip="$output_dir/$basename-notarization.zip"
(cd "$stage" && find HomeBot.app -print | LC_ALL=C sort | tar --no-recursion --format ustar --uid 0 --gid 0 --uname root --gname wheel -cf - -T -) | gzip -n > "$archive"
ditto -c -k --keepParent "$app" "$notary_zip"

signing=adhoc
if [ "$identity" != - ]; then signing=developer-id; fi
python3 scripts/release-manifest.py \
  --output "$output_dir/$basename.manifest.json" \
  --artifact "$archive" --platform macos --architecture "$arch" \
  --version "$version" --signing "$signing"
(cd "$output_dir" && shasum -a 256 "$basename.tar.gz" "$basename-notarization.zip" "$basename.manifest.json" > "$basename.SHA256SUMS")

printf '%s\n' "$archive" "$notary_zip" "$output_dir/$basename.manifest.json" "$output_dir/$basename.SHA256SUMS"
