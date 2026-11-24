#!/bin/sh
set -eu

sources=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
target=$1
case "$2" in
    aarch64|arm64) arch=arm64 ;;
    x86_64) arch=x86_64 ;;
    *) printf 'Architecture HAL inconnue : %s\n' "$2" >&2; exit 1 ;;
esac

parent=$(dirname -- "$target")
mkdir -p "$parent"
staging=$(mktemp -d "$parent/.patchbay-driver.XXXXXX")
trap 'rm -rf "$staging"' EXIT
bundle="$staging/Patchbay.driver"
mkdir -p "$bundle/Contents/MacOS"
cp "$sources/Info.plist" "$bundle/Contents/Info.plist"
xcrun --sdk macosx clang -std=c11 -Wall -Wextra -Werror -O2 \
    -arch "$arch" -mmacosx-version-min=14.2 -bundle \
    -Wl,-exported_symbol,_Patchbay_Create -framework CoreFoundation \
    "$sources/driver.c" "$sources/properties.c" -o "$bundle/Contents/MacOS/Patchbay"
codesign --force --sign - "$bundle"
codesign --verify --strict "$bundle"

if [ -d "$target" ]; then mv "$target" "$staging/previous.driver"; fi
if ! mv "$bundle" "$target"; then
    if [ -d "$staging/previous.driver" ]; then mv "$staging/previous.driver" "$target"; fi
    exit 1
fi
