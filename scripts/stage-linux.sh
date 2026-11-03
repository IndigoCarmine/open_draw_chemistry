#!/usr/bin/env bash
# Lay out the Linux app under <root>/usr, the tree both the AppImage (root = AppDir) and the
# .deb (root = target/deb-root, see [package.metadata.deb] in Cargo.toml) are made from:
#
#   usr/bin/open_draw_chemistry
#
# Its libraries (glibc and the like) come from the system. The release workflow adds the
# .desktop file and the icon to the AppImage itself; the .deb takes them straight from the
# repository.
#
#   scripts/stage-linux.sh <root>
#
# Run after `cargo build --release`.
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="${1:?usage: stage-linux.sh <root>}"
target_dir="${CARGO_TARGET_DIR:-$root_dir/target}"
exe="$target_dir/release/open_draw_chemistry"

[ -f "$exe" ] || { echo "error: $exe not found; run cargo build --release first" >&2; exit 1; }

usr="$dest/usr"
mkdir -p "$usr/bin"
install -m 755 "$exe" "$usr/bin/open_draw_chemistry"

# Every library the binary needs must resolve on this system.
missing="$(ldd "$usr/bin/open_draw_chemistry" | grep 'not found' | sort -u || true)"
if [ -n "$missing" ]; then
  echo "error: unresolved libraries in the staged app:" >&2
  echo "$missing" >&2
  exit 1
fi
echo "Staged the Linux app under $usr"
