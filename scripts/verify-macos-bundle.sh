#!/bin/sh
set -eu

app=${1:?usage: verify-macos-bundle.sh APP TARGET [IDENTITY]}
target=${2:?usage: verify-macos-bundle.sh APP TARGET [IDENTITY]}
identity=${3:--}
test -d "$app/Contents"
test -x "$app/Contents/MacOS/HomeBot"
test -x "$app/Contents/Resources/bin/homebot-server"
plutil -lint "$app/Contents/Info.plist" >/dev/null
test "$(plutil -extract CFBundleIdentifier raw "$app/Contents/Info.plist")" = dev.homebot.desktop

case "$target" in
  x86_64-apple-darwin) machine='x86_64' ;;
  aarch64-apple-darwin) machine='arm64' ;;
  *) echo "unsupported target: $target" >&2; exit 2 ;;
esac
file "$app/Contents/MacOS/HomeBot" | grep -q "$machine"
file "$app/Contents/Resources/bin/homebot-server" | grep -q "$machine"
codesign --verify --deep --strict --verbose=2 "$app"

if [ "$identity" = - ]; then
  codesign -dv "$app" 2>&1 | grep -q 'Signature=adhoc'
else
  codesign -dv "$app" 2>&1 | grep -q 'Authority=Developer ID Application:'
fi

# The packaged desktop owns the same supervised, loopback-only server implementation and the
# standalone headless binary remains available for diagnostics/service packaging.
strings "$app/Contents/MacOS/HomeBot" | grep -q 'HomeBot server is unavailable'
strings "$app/Contents/Resources/bin/homebot-server" | grep -q 'HOMEBOT_ALLOW_REMOTE'
