#!/usr/bin/env bash
# Lay out the Linux app with its OpenBabel under <root>/usr, the tree both the AppImage
# (root = AppDir) and the .deb (root = target/deb-root, see [package.metadata.deb] in
# Cargo.toml) are made from:
#
#   usr/bin/open_draw_chemistry                         rpath $ORIGIN/../lib/open-draw-chemistry
#   usr/lib/open-draw-chemistry/libopenbabel.so.8       OpenBabel and its InChI library, under
#   usr/lib/open-draw-chemistry/libinchi.so.1           their sonames
#   usr/lib/open-draw-chemistry/openbabel/*.so          the format/force-field plugins
#                                                       (BABEL_LIBDIR; their own rpath $ORIGIN/..
#                                                       finds the libraries above)
#   usr/share/open-draw-chemistry/openbabel/            the data files (BABEL_DATADIR)
#
# The app points OpenBabel at the plugins and data itself (src/three_d/runtime.rs), so nothing
# needs setting in the environment. Its other libraries (glibc, libstdc++, zlib, and libxml2
# for the XML-based format plugins) come from the system.
#
#   scripts/stage-linux.sh <root>
#
# Run after `cargo build --release` and scripts/fetch-openbabel.sh. Needs patchelf.
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="${1:?usage: stage-linux.sh <root>}"
target_dir="${CARGO_TARGET_DIR:-$root_dir/target}"
exe="$target_dir/release/open_draw_chemistry"
ob="${OPENBABEL_SYS_PREBUILT_DIR:-$root_dir/third_party/openbabel-prebuilt}"

[ -f "$exe" ] || { echo "error: $exe not found; run cargo build --release first" >&2; exit 1; }
[ -d "$ob/lib/openbabel" ] || { echo "error: no OpenBabel in $ob; run scripts/fetch-openbabel.sh" >&2; exit 1; }

usr="$dest/usr"
libdir="$usr/lib/open-draw-chemistry"
mkdir -p "$usr/bin" "$libdir/openbabel" "$usr/share/open-draw-chemistry/openbabel"

install -m 755 "$exe" "$usr/bin/open_draw_chemistry"
# The build linked against the prebuilt's absolute lib directory; point the binary at the
# copy beside it instead.
patchelf --set-rpath '$ORIGIN/../lib/open-draw-chemistry' "$usr/bin/open_draw_chemistry"

# Each real shared library (not its symlinks) under the soname the loader asks for.
for lib in "$ob"/lib/lib*.so.*; do
  [ -L "$lib" ] && continue
  soname="$(patchelf --print-soname "$lib")"
  install -m 644 "$lib" "$libdir/$soname"
done
# OpenBabel installs its plugins and data under a versioned directory (lib/openbabel/3.2.1);
# the bundle flattens it.
install -m 644 "$ob"/lib/openbabel/*/*.so "$libdir/openbabel/"
install -m 644 "$ob"/share/openbabel/*/* "$usr/share/open-draw-chemistry/openbabel/"

# Every library the binary and the plugins need must now resolve, without the build tree.
missing="$(ldd "$usr/bin/open_draw_chemistry" "$libdir"/openbabel/*.so | grep 'not found' | sort -u || true)"
if [ -n "$missing" ]; then
  echo "error: unresolved libraries in the staged app:" >&2
  echo "$missing" >&2
  exit 1
fi
if patchelf --print-rpath "$usr/bin/open_draw_chemistry" | grep -qF "$ob"; then
  echo "error: the binary still points into the build tree" >&2
  exit 1
fi
echo "Staged the Linux app under $usr"
