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

Planned: free-form reaction schemes (arrows, text, brackets, shapes), 3D model building
(OpenBabel), ibisOffice embedding, and a COM OLE server for Word/PowerPoint.

> **Rust 1.95+** is required (raised by the `egui-winit` fork used for Wayland drag-and-drop).

## Getting started

```bash
cargo run -- path/to/file.cdx   # the file is optional
```

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
cargo build --release

# Windows (needs Inno Setup 6)
iscc installer\windows.iss              # -> dist\

# Linux .deb (needs cargo-deb)
cargo install cargo-deb && cargo deb

# macOS .app (needs cargo-bundle)
cargo install cargo-bundle && cargo bundle --release
# ...and a .dmg on top of it (needs `brew install create-dmg`);
# see the `macos` job in .github/workflows/release.yml
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

GPL-2.0-or-later — see [LICENSE](LICENSE). (GPL because the planned 3D builder links OpenBabel.)
