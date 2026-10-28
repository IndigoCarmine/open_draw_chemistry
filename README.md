# Open DrawChemistry

[日本語版 README](README_ja.md)

A desktop application for drawing chemical structures and reaction schemes, in the spirit of
ChemDraw, ChemSketch and XDrawChem. Built with [egui](https://github.com/emilk/egui) on top of
[chembuider-rs](https://github.com/IndigoCarmine/chembuider-rs) (the structure editor widget)
and [cdx_file_rs](https://github.com/IndigoCarmine/cdx_file_rs) (ChemDraw file codecs).

## Features (phase 1)

- Draw and edit structures: select / bond / eraser tools, bond styles (wedge, hash, bold,
  wavy, dative, …), element picker, ChemDraw-style bond menu on right-click, undo/redo, 2D clean-up.
- **Open and save ChemDraw files (`.cdx` / `.cdxml`).** Objects the editor cannot show yet
  (arrows, captions, shapes) are kept when the file is saved.
- Several documents in tabs, recent files, drag-and-drop to open, unsaved-change prompts.
- Analysis panel: formula, exact mass, isotope pattern (m/z), elemental analysis.
- Export MOL2 / SMILES (and PNG on Windows). On Windows, Copy puts a ChemDraw-compatible
  structure plus an EMF picture on the clipboard for Office.
- **2D → 3D** (OpenBabel): generate a 3D model of the drawing (hydrogens added, wedges
  respected, abbreviations such as Ph / OMe expanded), relax it with MMFF94 / MMFF94s / UFF /
  GAFF, and export it as MOL / SDF / XYZ / PDB. The 3D viewer itself is still to come; for now
  the panel shows the model's formula, atom count and energy.

Planned: free-form reaction schemes (arrows, text, brackets, shapes), the 3D viewer,
ibisOffice embedding, and in-place (inside the document window) OLE editing.

> **Rust 1.95+** is required (raised by the `egui-winit` fork used for Wayland drag-and-drop).

## Editing ChemDraw objects in Word / PowerPoint (Windows)

On a PC **without ChemDraw**, the ChemDraw structures embedded in Word and PowerPoint documents
can be edited here: double-click one and it opens in a tab marked 「Word の埋め込み — …」;
保存 (Ctrl+S) or closing the tab writes the structure (and its picture) back into the document.
The objects stay ChemDraw objects, so ChemDraw opens them again wherever it is installed.

The installer sets this up for the current user. By hand:

```sh
open_draw_chemistry.exe --register-ole     # register (per user, no admin rights needed)
open_draw_chemistry.exe --unregister-ole   # remove exactly what was registered
```

Registration writes only under `HKCU\Software\Classes`: this app's own OLE class
(`OpenDrawChemistry.Document.1`, also offered by Insert ▸ Object) and — **only when ChemDraw
is not installed** — a `TreatAs` that sends ChemDraw's class to this app. Where ChemDraw is
installed nothing of ChemDraw's is changed. If ChemDraw is installed later, run
`--register-ole` again (or uninstall) to hand its objects back to it.

Objects are edited in this app's window (in-place editing inside Word is not supported).
For diagnosis, the server logs what the container asks for when its `LocalServer32` command has
`--ole-log <file>` added.

## Getting started

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

`act` only runs the Linux jobs — `windows` and `macos` need real runners. The runner image is pinned in [`.actrc`](.actrc).

## Linux build dependencies

Building on Linux needs the usual windowing/GL headers:

```bash
sudo apt-get install -y \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libwayland-dev libegl1-mesa-dev \
  libgtk-3-dev libssl-dev
```

## License

GPL-2.0-or-later — see [LICENSE](LICENSE). (GPL because 3D generation links OpenBabel.)
