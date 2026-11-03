#!/usr/bin/env bash
# Turn the single-architecture .app cargo-bundle made into the universal app:
#
#   Contents/MacOS/open_draw_chemistry          x86_64 + arm64 (lipo)
#
# The app is then signed ad hoc, since lipo invalidates the signature and Apple Silicon refuses
# to load unsigned code.
#
#   scripts/bundle-macos.sh <app> <x86_64 binary> <arm64 binary>
#
# The binaries are the two `cargo build --release --target …` outputs.
set -euo pipefail

app="$1"
bin_x86="$2"
bin_arm="$3"
name="$(basename "$bin_arm")"
contents="$app/Contents"

lipo -create -output "$contents/MacOS/$name" "$bin_x86" "$bin_arm"

codesign --force --sign - "$contents/MacOS/$name"
codesign --force --sign - "$app"
codesign --verify --deep --strict --verbose=2 "$app"

lipo -info "$contents/MacOS/$name"
echo "Bundled the universal binary into $app"
