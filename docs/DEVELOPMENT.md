# Development guide

[日本語](DEVELOPMENT_ja.md) · [Back to README](../README.md)

> **Rust 1.95+** is required (raised by the `egui-winit` fork used for Wayland drag-and-drop).

## Office (OLE) registration details

Registration writes only under `HKCU\Software\Classes`: this app's own OLE class
(`OpenDrawChemistry.Document.1`, also offered by Insert ▸ Object) and — **only when ChemDraw
is not installed** — a `TreatAs` that sends ChemDraw's class to this app. Where ChemDraw is
installed nothing of ChemDraw's is changed. If ChemDraw is installed later, run
`--register-ole` again (or uninstall) to hand its objects back to it.

Objects are edited in this app's window (in-place editing inside Word is not supported).
For diagnosis, the server logs what the container asks for when its `LocalServer32` command has
`--ole-log <file>` added.

## Building from source

The app requires [OpenBabel](https://openbabel.org) (3D generation, SMILES, reading 3D files),
linked through [openbabel_rs](https://github.com/IndigoCarmine/openbabel_rs). Fetch its prebuilt
OpenBabel once (and again after bumping openbabel_rs); the script needs `bash` and `gh` or
`curl`:

```bash
bash scripts/fetch-openbabel.sh   # unpacks into third_party/openbabel-prebuilt/ (not committed)
cargo run -- path/to/file.cdx     # the file is optional
```

`.cargo/config.toml` points `OPENBABEL_SYS_PREBUILT_DIR` at that directory, and the
openbabel-sys build script copies OpenBabel's DLLs and plugins next to the binaries on Windows
(Linux and macOS use an rpath), so `cargo run` and `cargo test` need nothing else. A C++
compiler is still required for the small binding shim (MSVC Build Tools on Windows).

Prebuilts exist for `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`,
`aarch64-apple-darwin` and `x86_64-apple-darwin`. To build for another target than the host,
fetch its prebuilt into a directory of its own and point the build at it:

```bash
bash scripts/fetch-openbabel.sh x86_64-apple-darwin   # -> third_party/openbabel-prebuilt-x86_64-apple-darwin/
OPENBABEL_SYS_PREBUILT_DIR=$PWD/third_party/openbabel-prebuilt-x86_64-apple-darwin \
  cargo build --release --target x86_64-apple-darwin
```

On a target without a prebuilt, install CMake and set `OPENBABEL_SYS_PREBUILT_DIR` to an empty
value to build OpenBabel from source (10–20 minutes the first time). There is no build without
OpenBabel.

Every release ships OpenBabel with the app:

| Package | Libraries | Plugins | Data |
| --- | --- | --- | --- |
| Windows (installer, zip) | `openbabel-3.dll`, `inchi.dll` beside the `.exe` | `*.obf` beside the `.exe` | `data\` beside the `.exe` |
| Linux (`.deb`, AppImage) | `usr/lib/open-draw-chemistry/` (rpath `$ORIGIN/../lib/open-draw-chemistry`) | `usr/lib/open-draw-chemistry/openbabel/` | `usr/share/open-draw-chemistry/openbabel/` |
| macOS (`.app`, universal) | `Contents/Frameworks/` (rpath `@executable_path/../Frameworks`) | `Contents/Frameworks/openbabel/` | `Contents/Resources/openbabel/` |

The app points OpenBabel at its plugins and data at startup (`src/three_d/runtime.rs`).
The Windows packages also carry the Visual C++ runtime (`msvcp140.dll`, `vcruntime140.dll`,
`vcruntime140_1.dll`) app-local, so they run on a clean Windows without `vc_redist.x64.exe`.
`open_draw_chemistry --check-openbabel` exercises the installed OpenBabel without opening a
window; the release workflow runs it on every package.

## Cutting a release

```bash
git tag v0.1.0
git push origin v0.1.0
```

The workflow derives the version from the tag (`v0.1.0` → `0.1.0`) and feeds it to the installer, the `.deb` and the `.dmg`, so `Cargo.toml`'s `version` is not the source of truth for releases. Bump it anyway to keep the About dialog honest.

Running the workflow manually (**Actions → Release → Run workflow**) builds everything as `0.0.0-dev` and uploads the artifacts *without* publishing a release — useful for testing changes to the workflow.

## Signing and notarisation

The macOS bundle is **ad-hoc signed** (`codesign --sign -`). That is enough for the app to launch at all on Apple Silicon, but it is not notarised, so on first launch users must right-click the app and choose **Open**. Proper distribution needs a paid Apple Developer account, `codesign` with a Developer ID, and `xcrun notarytool submit`.

The Windows installer is unsigned, so SmartScreen will warn on download. Signing needs a code-signing certificate and `signtool`.

## Building installers locally

```bash
bash scripts/fetch-openbabel.sh
cargo build --release

# Windows (needs Inno Setup 6): the app folder (also the portable zip), then the installer
bash scripts/stage-windows.sh           # -> target\stage\windows\
iscc installer\windows.iss              # -> dist\

# Linux .deb (needs cargo-deb and patchelf)
bash scripts/stage-linux.sh target/deb-root && mkdir -p target/deb-root/DEBIAN
cargo install cargo-deb && cargo deb --no-build

# macOS universal .app (needs cargo-bundle and both architectures' OpenBabel)
# see the `macos` job in .github/workflows/release.yml (scripts/bundle-macos.sh),
# and for a .dmg on top of it `brew install create-dmg`
```

## Testing workflows with act

[`act`](https://github.com/nektos/act) runs the workflows in Docker locally:

```bash
act -W .github/workflows/ci.yml -j lint
act -W .github/workflows/release.yml -j linux
```

`act` only runs the Linux jobs — `windows` and `macos` need real runners. The runner image is pinned in [`.actrc`](../.actrc).

## Linux build dependencies

Building on Linux needs the usual windowing/GL headers:

```bash
sudo apt-get install -y \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libwayland-dev libegl1-mesa-dev \
  libgtk-3-dev libssl-dev
```

