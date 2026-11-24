#!/bin/sh
set -eu

sources=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/patchbay-driver-test.XXXXXX")
trap 'rm -rf -- "$tmp"' EXIT
trap 'exit 1' HUP INT TERM

compile() {
    xcrun --sdk macosx clang -std=c11 -Wall -Wextra -Werror -O1 -g \
        -fsanitize=address,undefined -fno-sanitize-recover=all \
        -mmacosx-version-min=14.2 -framework CoreFoundation "$@"
}

compile -bundle "$sources/driver.c" "$sources/properties.c" -o "$tmp/Patchbay.bundle"
compile "$sources/driver_test.c" -o "$tmp/driver_test"
"$tmp/driver_test" "$tmp/Patchbay.bundle"
