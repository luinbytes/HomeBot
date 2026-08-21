#!/bin/sh
set -eu

version=$(cat VERSION)
printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || {
  echo "VERSION must contain a semantic x.y.z version" >&2
  exit 1
}

python3 - "$version" <<'PY'
import json
import pathlib
import subprocess
import sys

version = sys.argv[1]
metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--format-version", "1", "--no-deps"], text=True
))
homebot = [package for package in metadata["packages"] if package["name"].startswith("homebot-")]
assert homebot, "no HomeBot workspace packages found"
wrong = {package["name"]: package["version"] for package in homebot if package["version"] != version}
assert not wrong, f"HomeBot Cargo versions differ from VERSION={version}: {wrong}"

gradle = pathlib.Path("android/app/build.gradle.kts").read_text(encoding="utf-8")
assert 'resolve("VERSION")' in gradle
assert "1_000_000" in gradle and "1_000" in gradle
client = pathlib.Path(
    "android/app/src/main/java/dev/homebot/android/connection/HomeBotClient.kt"
).read_text(encoding="utf-8")
assert "BuildConfig.VERSION_NAME" in client

for script in ("scripts/package-macos.sh", "scripts/package-android.sh"):
    text = pathlib.Path(script).read_text(encoding="utf-8")
    assert "cat VERSION" in text, f"{script} is not bound to VERSION"
PY

printf 'release version consistency: %s\n' "$version"
