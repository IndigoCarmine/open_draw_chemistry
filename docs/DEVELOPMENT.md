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

No chemistry toolkit needs installing: 3D generation, molecular mechanics and reading 3D files come
from [molecule_optimizer_rs](https://github.com/IndigoCarmine/molecule_optimizer_rs), and SMILES
from [chembuider-rs](https://github.com/IndigoCarmine/chembuider-rs), so a Rust toolchain (plus
the platform's linker: MSVC Build Tools on Windows, Xcode command-line tools on macOS, and the
headers under [Linux build dependencies](#linux-build-dependencies)) is all a build needs:

```bash
cargo run -- path/to/file.cdx     # the file is optional
```

The Windows packages carry the Visual C++ runtime (`msvcp140.dll`, `vcruntime140.dll`,
`vcruntime140_1.dll`) app-local, so they run on a clean Windows without `vc_redist.x64.exe`.

## Cutting a release

```bash
git tag v0.1.0
git push origin v0.1.0
```

The workflow derives the version from the tag (`v0.1.0` → `0.1.0`) and feeds it to the installer, the `.deb` and the `.dmg`, so `Cargo.toml`'s `version` is not the source of truth for releases. Bump it anyway to keep the About dialog honest.

Running the workflow manually (**Actions → Release → Run workflow**) builds everything as `0.0.0-dev` and uploads the artifacts *without* publishing a release — useful for testing changes to the workflow.

## Signing updates

At startup the app looks at the latest GitHub Release (`src/update/`). It installs an update by
itself only if the asset's signature verifies against the public key built into it,
`resources/update-signing.pub`. Until that file holds a real key it holds a placeholder, and
the app sends users to the Release page instead of installing anything.

| Installed as | Yes in the update dialog |
| --- | --- |
| Windows installer | Downloads `…-setup.exe`, quits, runs it with `/SILENT` in the install's own mode (`/CURRENTUSER`, or `/ALLUSERS` with a UAC prompt), restarts |
| Linux AppImage | Downloads the new AppImage, renames it over the old file, restarts, and goes back to the old file if the new one does not start |
| macOS `.app` | Downloads the `.dmg`, copies the app out with `hdiutil` and `ditto`, swaps the bundle (`renamex_np` with `RENAME_SWAP`, two renames where the filesystem cannot), restarts, and goes back to the old bundle if the new one does not start |
| Windows portable zip, Linux `.deb`, anything else | Opens the Release page |

A failed download, verification or install also ends at the Release page. The Windows setup is
verified a second time just before it runs, since it waits in a directory the user can write to.

An AppImage or `.app` update stages the new version beside the app and keeps the previous one
there, as `.open-draw-chemistry-update-*` (`.open-draw-chemistry-update-old-*` for the previous
version). A later start of the app deletes what is over a day old. A previous version is only
deleted by the app it belongs to, once that is back at its path. If an install can neither finish
nor be undone, both versions stay on disk and a message box says where.

A download that finishes while a parameter scan or 3D job is running does not quit the app, since
that would end the job. The app waits with the "Restart now" window instead.

To set up signing, once, with OpenSSL 3:

```bash
bash scripts/gen-update-key.sh ~/open-draw-chemistry-update-signing.pem
gh secret set UPDATE_SIGNING_KEY --repo IndigoCarmine/open_draw_chemistry \
  < ~/open-draw-chemistry-update-signing.pem
git add resources/update-signing.pub   # and commit it
```

Keep the private key out of the repository and back it up: a copy of the app only trusts the key
it was built with, so **replacing the key leaves every installed copy going to the Release page**
for that one update to a build with the new key. Generating a key refuses to overwrite a private
key file, or a `resources/update-signing.pub` that already holds a real key.

When a tag is released, `scripts/sign-update-assets.sh` writes `<asset>.sig` beside each asset.
While `resources/update-signing.pub` holds the placeholder and the `UPDATE_SIGNING_KEY` secret is
missing, it warns and signs nothing. With a real key committed a missing secret stops the release,
as does a secret that does not match the key. A signature is the 64-byte ed25519
signature of this text, so it binds the file's name, and with it the version, as well as its
content, and can be checked with OpenSSL:

```text
open-draw-chemistry-update-v1
<asset name>
<sha256 of the asset, in lower-case hex>
```

A build made before the key is committed cannot update itself automatically, and neither can any
copy update to a release without `.sig` files: those users see the Release page.

`cargo test real_ -- --ignored` checks the real API and download path over the network.

### HTTPS

The app is **AGPL-3.0-or-later**, which takes code under Apache-2.0, MIT, BSD and the like. HTTPS
comes from the OS, so no TLS library is linked or bundled:

| Platform | HTTPS |
| --- | --- |
| Windows, macOS | `ureq` over `native-tls`: SChannel, Security.framework |
| Linux | The system's `curl`, run as a separate program (`src/update/curl.rs`) |

Without `curl` on Linux the update check fails silently, as it does offline, and the app never
offers an update. The `.deb` recommends the `curl` package.

`openssl` stays in `Cargo.lock`, and in `cargo tree --target all`, which ignores `cfg`:
`native-tls` declares it for Linux, where this app does not use `native-tls`. Nothing builds it.

## Signing and notarisation

The macOS bundle is **ad-hoc signed** (`codesign --sign -`). That is enough for the app to launch at all on Apple Silicon, but it is not notarised, so on first launch users must right-click the app and choose **Open**. Proper distribution needs a paid Apple Developer account, `codesign` with a Developer ID, and `xcrun notarytool submit`.

The Windows installer is unsigned, so SmartScreen will warn on download. Signing needs a code-signing certificate and `signtool`.

## Building installers locally

```bash
cargo build --release

# Windows (needs Inno Setup 6): the app folder (also the portable zip), then the installer
bash scripts/stage-windows.sh           # -> target\stage\windows\
iscc installer\windows.iss              # -> dist\

# Linux .deb (needs cargo-deb)
bash scripts/stage-linux.sh target/deb-root
cargo install cargo-deb && cargo deb --no-build

# macOS universal .app (needs cargo-bundle and both Rust targets)
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

