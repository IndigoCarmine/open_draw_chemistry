#!/usr/bin/env bash
# Download the prebuilt OpenBabel that openbabel-sys links against, so building this app
# needs neither CMake nor the 10–20 minute OpenBabel source build.
#
# The archive is unpacked into third_party/openbabel-prebuilt/, which .cargo/config.toml
# hands to openbabel-sys through OPENBABEL_SYS_PREBUILT_DIR. Run it once after cloning (and
# again after bumping openbabel_rs); CI runs it before every build.
#
#   scripts/fetch-openbabel.sh            # the host's target
#   scripts/fetch-openbabel.sh <triple>   # another target (cross-compiling)
#
# Prebuilts exist for x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu and
# aarch64-apple-darwin. On any other target this script says so and exits 0 without
# unpacking anything; build there with `--no-default-features` (no 3D generation) or install
# CMake and let openbabel-sys build OpenBabel from source (see README).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$root/third_party/openbabel-prebuilt"
repo="IndigoCarmine/openbabel_rs"

# The archive must come from the very openbabel-sys being built (its build script checks), so
# the tag is read from Cargo.lock rather than written down a second time here.
tag="$(sed -n 's/.*openbabel_rs[^?]*?tag=\([^#]*\)#.*/\1/p' "$root/Cargo.lock" | head -n 1)"
if [ -z "$tag" ]; then
  echo "error: openbabel_rs is not pinned to a tag in Cargo.lock" >&2
  exit 1
fi

triple="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
archive="openbabel-sys-$triple.tar.gz"

case "$triple" in
  x86_64-pc-windows-msvc | x86_64-unknown-linux-gnu | aarch64-apple-darwin) ;;
  *)
    echo "warning: openbabel_rs $tag has no prebuilt for $triple." >&2
    echo "         Build with --no-default-features, or install CMake to build OpenBabel from source" >&2
    echo "         (and unset OPENBABEL_SYS_PREBUILT_DIR)." >&2
    exit 0
    ;;
esac

stamp="$dest/.fetched"
if [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$tag $triple" ]; then
  echo "OpenBabel prebuilt $tag ($triple) is already in $dest"
  exit 0
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $archive from $repo $tag"
if command -v gh > /dev/null 2>&1 && gh auth status > /dev/null 2>&1; then
  gh release download "$tag" -R "$repo" -p "$archive" -D "$tmp"
else
  curl -fL --retry 3 -o "$tmp/$archive" \
    "https://github.com/$repo/releases/download/$tag/$archive"
fi

rm -rf "$dest"
mkdir -p "$dest"
# The archive's root is the install tree itself (include/, bin/ or lib/, and
# openbabel-sys-prebuilt.txt), so it is unpacked straight into $dest.
tar -xzf "$tmp/$archive" -C "$dest"
if [ ! -f "$dest/openbabel-sys-prebuilt.txt" ]; then
  echo "error: $archive does not have the expected layout (no openbabel-sys-prebuilt.txt)" >&2
  exit 1
fi
echo "$tag $triple" > "$stamp"
echo "OpenBabel prebuilt $tag ($triple) unpacked into $dest"
