# egui Template

[日本語版 README](README_ja.md)

A starting point for cross-platform desktop apps built with [egui](https://github.com/emilk/egui) / [eframe](https://github.com/emilk/egui/tree/master/crates/eframe).

Push a `v*` tag and GitHub Actions builds and publishes **installers for Windows, macOS and Linux**.

| Platform | Artifact | Format |
| --- | --- | --- |
| Windows | `egui-template-<version>-windows-x64-setup.exe` | Inno Setup installer |
| Windows | `egui-template-<version>-windows-x64.exe` | Portable binary |
| macOS | `egui-template-<version>-macos-universal.dmg` | Drag-to-Applications disk image (universal: Intel + Apple Silicon) |
| macOS | `egui-template-<version>-macos-universal` | Portable binary |
| Linux | `egui-template-<version>-linux-x86_64.AppImage` | Self-contained AppImage |
| Linux | `egui-template_<version>_amd64.deb` | Debian/Ubuntu package |
| Linux | `egui-template-<version>-linux-x86_64` | Portable binary |

## What's in the box

- **Persistent state** — window contents survive restarts via eframe's `persistence` feature.
- **Icons for every platform** — `resources/` holds `icon.png` (window + Linux), `icon.ico` (Windows `.exe`) and `icon.icns` (macOS bundle).
- **Declarative macOS bundling** — `cargo bundle` generates `Info.plist` from `[package.metadata.bundle]`, so there is no plist to hand-maintain.
- **Wayland drag-and-drop** — `[patch.crates-io]` pins `winit`/`egui-winit` forks that implement native DnD on Wayland (upstream does not).
- **Unit-testable app logic** — the app lives in `src/lib.rs`, not `main.rs`, so `cargo test` can reach it.
- **CI** — `cargo fmt`, `cargo clippy -D warnings` and `cargo test` on all three platforms.

> **Rust 1.95+** is required. Edition 2024 alone needs only 1.85, but the `egui-winit`
> fork used for Wayland DnD raises the floor.

## Getting started

```bash
cargo run
```

Then edit [`src/app.rs`](src/app.rs) — the demo widgets there are meant to be deleted.

## Making it yours

Rename in this order; the names are cross-referenced between files:

1. **`Cargo.toml`** — `name`, `version`, `description`, `authors`, `license`, `repository`, and the `[package.metadata.deb]` block.
2. **`src/lib.rs`** — `APP_NAME` (window title and persistence key).
3. **`.github/workflows/release.yml`** — `BIN_NAME` (must match `Cargo.toml`'s `name`) and `APP_NAME` (used in artifact filenames).
4. **`installer/windows.iss`** — `MyAppName`, `MyAppPublisher`, `MyAppURL`, `MyAppExeName`, and **generate a fresh `AppId` GUID**. The `AppId` is how Windows recognises an upgrade; once you have shipped a release, never change it.
5. **`Cargo.toml` `[package.metadata.bundle]`** — `name`, `identifier` (use a domain you control), `category`, descriptions. Also update the hard-coded `egui Template.app` paths in the `macos` job of `release.yml`, which must match `bundle.name`.
6. **`installer/linux/egui-template.desktop`** — rename the file too, and update `Exec`/`Icon`.
7. **`installer/linux/AppRun`** — the binary name in the `exec` line.
8. **`resources/icon.png`**, **`resources/icon.ico`** and **`resources/icon.icns`** — replace with your own art. Keep the PNG at 1024×1024, the `.ico` multi-resolution (16–256 px), and the `.icns` containing at least the 32/128/256/512/1024 px variants.
9. **`LICENSE`** — fill in the copyright holder.

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

MIT — see [LICENSE](LICENSE).
