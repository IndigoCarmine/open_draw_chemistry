#!/usr/bin/env bash
# Turn the .app cargo-bundle made into the universal app with its OpenBabel:
#
#   Contents/MacOS/open_draw_chemistry          x86_64 + arm64 (lipo), rpath
#                                               @executable_path/../Frameworks
#   Contents/Frameworks/libopenbabel.8.dylib    OpenBabel and its InChI library, universal,
#   Contents/Frameworks/libinchi.1.dylib        under their install names (@rpath/...)
#   Contents/Frameworks/openbabel/*.so          the format/force-field plugins, universal
#                                               (BABEL_LIBDIR; their own rpath @loader_path/..
#                                               finds the libraries above)
#   Contents/Resources/openbabel/               the data files (BABEL_DATADIR)
#
# The app points OpenBabel at the plugins and data itself (src/three_d/runtime.rs). Everything
# is signed ad hoc afterwards, since install_name_tool and lipo invalidate signatures and Apple
# Silicon refuses to load unsigned code.
#
#   scripts/bundle-macos.sh <app> <x86_64 binary> <arm64 binary> <x86_64 OpenBabel> <arm64 OpenBabel>
#
# The binaries are the two `cargo build --release --target …` outputs, each linked against the
# OpenBabel prebuilt for its architecture (the last two arguments, as unpacked by
# scripts/fetch-openbabel.sh).
set -euo pipefail

app="$1"
bin_x86="$2"
bin_arm="$3"
ob_x86="$4"
ob_arm="$5"
name="$(basename "$bin_arm")"
contents="$app/Contents"
frameworks="$contents/Frameworks"
mkdir -p "$frameworks/openbabel" "$contents/Resources/openbabel"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# 1. The executable: each slice loses the rpath into its build machine's OpenBabel and gains
#    the bundle's, then the two are joined.
for arch in x86_64 arm64; do
  if [ "$arch" = x86_64 ]; then src="$bin_x86"; else src="$bin_arm"; fi
  cp "$src" "$tmp/$arch"
  chmod u+w "$tmp/$arch"
  otool -l "$tmp/$arch" | awk '/cmd LC_RPATH/ { getline; getline; print $2 }' | while read -r rpath; do
    install_name_tool -delete_rpath "$rpath" "$tmp/$arch"
  done
  install_name_tool -add_rpath "@executable_path/../Frameworks" "$tmp/$arch"
done
lipo -create -output "$contents/MacOS/$name" "$tmp/x86_64" "$tmp/arm64"

# 2. The shared libraries, universal, named as they are loaded (their install id).
universal() { # <arm64 file> <x86_64 file> <output>
  if [ ! -f "$2" ]; then
    echo "error: $1 has no x86_64 counterpart ($2)" >&2
    exit 1
  fi
  lipo -create -output "$3" "$2" "$1"
}
for lib in "$ob_arm"/lib/lib*.dylib; do
  [ -L "$lib" ] && continue
  id="$(otool -D "$lib" | tail -n 1)"
  universal "$lib" "$ob_x86/lib/$(basename "$lib")" "$frameworks/$(basename "$id")"
done
# 3. The plugins, from OpenBabel's versioned directory (lib/openbabel/3.2.1).
for plugin in "$ob_arm"/lib/openbabel/*/*.so; do
  rel="${plugin#"$ob_arm"/}"
  universal "$plugin" "$ob_x86/$rel" "$frameworks/openbabel/$(basename "$plugin")"
done
# 4. The data (text, the same for both architectures).
cp "$ob_arm"/share/openbabel/*/* "$contents/Resources/openbabel/"

# 5. Sign the inner code first, then the app around it.
find "$frameworks" -type f \( -name '*.dylib' -o -name '*.so' \) -exec codesign --force --sign - {} +
codesign --force --sign - "$contents/MacOS/$name"
codesign --force --sign - "$app"
codesign --verify --deep --strict --verbose=2 "$app"

lipo -info "$contents/MacOS/$name"
otool -L "$contents/MacOS/$name" | grep -i -e openbabel -e inchi
otool -l "$contents/MacOS/$name" | grep -A2 LC_RPATH
echo "Bundled OpenBabel into $app"
