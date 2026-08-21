#!/bin/sh
set -eu

version=${1:?usage: render-arch-pkgbuild.sh VERSION SOURCE_SHA256 [PKGREL] [OUTPUT]}
source_sha=${2:?usage: render-arch-pkgbuild.sh VERSION SOURCE_SHA256 [PKGREL] [OUTPUT]}
pkgrel=${3:-1}
output=${4:-packaging/arch/PKGBUILD}
case "$version" in ''|*[!0-9A-Za-z.+]*) echo "invalid package version" >&2; exit 2;; esac
case "$source_sha" in *[!0-9a-f]*|'') echo "source SHA-256 must be lowercase hexadecimal" >&2; exit 2;; esac
test "${#source_sha}" -eq 64 || { echo "source SHA-256 must contain 64 characters" >&2; exit 2; }
case "$pkgrel" in ''|*[!0-9]*) echo "pkgrel must be numeric" >&2; exit 2;; esac

sed -e "s/@VERSION@/$version/g" -e "s/@SOURCE_SHA256@/$source_sha/g" -e "s/@PKGREL@/$pkgrel/g" \
  packaging/arch/PKGBUILD.in > "$output"
