#!/usr/bin/env bash
# Lay out everything the Windows app needs at runtime in one folder: what the portable zip
# holds and what the installer (installer/windows.iss) installs.
#
#   scripts/stage-windows.sh [dest]        # default: target/stage/windows
#
# Run after `cargo build --release`. The folder gets
#
#   open_draw_chemistry.exe
#   msvcp140.dll, vcruntime140.dll,     the Visual C++ runtime, app-local (see below)
#   vcruntime140_1.dll
#
# The Visual C++ runtime: the app is built with /MD, so it loads VCRUNTIME140 (and, for any
# C++ code linked in, MSVCP140/VCRUNTIME140_1), which a clean Windows does not have. Microsoft
# allows these redistributable DLLs to be deployed app-local (copied beside the executable;
# see "Redistributing Visual C++ Files" in the Visual Studio docs and the redist list,
# VC\Redist\MSVC\<version>\...), and the executable's own directory comes first in the DLL
# search order, so the copies here are used even where an older runtime is installed. That
# keeps the portable zip working on a clean machine and lets a per-user install proceed without
# the elevation vc_redist.x64.exe would need. The universal CRT (api-ms-win-crt-*) is part of
# Windows 10 and later and is not copied.
#
# The runtime DLLs are taken from the newest Visual Studio installation (found with vswhere),
# which is the one that built the app; set VCRUNTIME_DIR to a Microsoft.VC14x.CRT folder to use
# another copy.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="${1:-$root/target/stage/windows}"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
exe="$target_dir/release/open_draw_chemistry.exe"

[ -f "$exe" ] || { echo "error: $exe not found; run cargo build --release first" >&2; exit 1; }

if [ -z "${VCRUNTIME_DIR:-}" ]; then
  vswhere="/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"
  [ -f "$vswhere" ] || { echo "error: vswhere not found; set VCRUNTIME_DIR" >&2; exit 1; }
  # The x64 (not onecore, not debug) redistributable folder of the newest installation.
  found="$("$vswhere" -latest -products '*' -find 'VC\Redist\MSVC\*\x64\Microsoft.VC14*.CRT\vcruntime140.dll' \
    | tr -d '\r' | sort | tail -n 1)"
  [ -n "$found" ] || { echo "error: no Visual C++ redistributable found; set VCRUNTIME_DIR" >&2; exit 1; }
  VCRUNTIME_DIR="$(dirname "$(cygpath -u "$found" 2> /dev/null || echo "$found")")"
fi

rm -rf "$dest"
mkdir -p "$dest"
cp "$exe" "$dest/"
for dll in msvcp140.dll vcruntime140.dll vcruntime140_1.dll; do
  cp "$VCRUNTIME_DIR/$dll" "$dest/"
done
echo "Staged the Windows app in $dest (Visual C++ runtime from $VCRUNTIME_DIR)"
