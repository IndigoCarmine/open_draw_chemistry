# egui Template

[English README](README.md)

[egui](https://github.com/emilk/egui) / [eframe](https://github.com/emilk/egui/tree/master/crates/eframe) でクロスプラットフォームのデスクトップアプリを作るためのテンプレートです。

`v*` タグを push すると、GitHub Actions が **Windows / macOS / Linux 向けインストーラ**をビルドして Release に公開します。

| プラットフォーム | 成果物 | 形式 |
| --- | --- | --- |
| Windows | `egui-template-<version>-windows-x64-setup.exe` | Inno Setup インストーラ |
| Windows | `egui-template-<version>-windows-x64.exe` | ポータブル実行ファイル |
| macOS | `egui-template-<version>-macos-universal.dmg` | Applications へドラッグする形式のディスクイメージ（Intel + Apple Silicon ユニバーサル） |
| macOS | `egui-template-<version>-macos-universal` | ポータブル実行ファイル |
| Linux | `egui-template-<version>-linux-x86_64.AppImage` | 単体で動作する AppImage |
| Linux | `egui-template_<version>_amd64.deb` | Debian / Ubuntu パッケージ |
| Linux | `egui-template-<version>-linux-x86_64` | ポータブル実行ファイル |

## 含まれているもの

- **状態の永続化** — eframe の `persistence` 機能により、再起動しても画面の状態が復元されます。
- **全プラットフォーム分のアイコン** — `resources/` に `icon.png`（ウィンドウ + Linux）・`icon.ico`（Windows の `.exe`）・`icon.icns`（macOS バンドル）を用意しています。
- **宣言的な macOS バンドル** — `cargo bundle` が `[package.metadata.bundle]` から `Info.plist` を生成するため、plist を手で管理する必要がありません。
- **Wayland でのドラッグ&ドロップ** — `[patch.crates-io]` で `winit` / `egui-winit` のフォークを指定しています（本家は Wayland のネイティブ DnD に未対応）。
- **テスト可能なアプリロジック** — アプリ本体を `main.rs` ではなく `src/lib.rs` に置いているため、`cargo test` から参照できます。
- **CI** — 3 プラットフォームで `cargo fmt` / `cargo clippy -D warnings` / `cargo test` を実行します。

> **Rust 1.95 以上**が必要です。edition 2024 自体は 1.85 で足りますが、Wayland DnD 用の
> `egui-winit` フォークが要求バージョンを引き上げています。

## はじめかた

```bash
cargo run
```

あとは [`src/app.rs`](src/app.rs) を編集してください。中のデモ用ウィジェットは削除する前提で置いてあります。

## 自分のアプリにする

名前は複数のファイルから相互に参照しているため、次の順で変更してください。

1. **`Cargo.toml`** — `name` / `version` / `description` / `authors` / `license` / `repository` と `[package.metadata.deb]` セクション。
2. **`src/lib.rs`** — `APP_NAME`（ウィンドウタイトルと永続化キー）。
3. **`.github/workflows/release.yml`** — `BIN_NAME`（`Cargo.toml` の `name` と一致させる）と `APP_NAME`（成果物のファイル名に使用）。
4. **`installer/windows.iss`** — `MyAppName` / `MyAppPublisher` / `MyAppURL` / `MyAppExeName`、そして **`AppId` の GUID を新しく生成**してください。`AppId` は Windows がアップグレードを判別するための識別子です。一度リリースしたあとは絶対に変更しないでください。
5. **`Cargo.toml` の `[package.metadata.bundle]`** — `name` / `identifier`（自分が管理するドメインを使用）/ `category` / 各種説明文。あわせて `release.yml` の `macos` ジョブ内にハードコードされている `egui Template.app` のパスも、`bundle.name` と一致するように更新してください。
6. **`installer/linux/egui-template.desktop`** — ファイル名自体もリネームし、`Exec` と `Icon` を更新。
7. **`installer/linux/AppRun`** — `exec` 行のバイナリ名。
8. **`resources/icon.png`** / **`resources/icon.ico`** / **`resources/icon.icns`** — 自分のアイコンに差し替え。PNG は 1024×1024、`.ico` はマルチ解像度（16〜256 px）、`.icns` は最低でも 32/128/256/512/1024 px を含む状態を保ってください。
9. **`LICENSE`** — 著作権者名を記入。

## リリースする

```bash
git tag v0.1.0
git push origin v0.1.0
```

ワークフローはタグからバージョンを取り出し（`v0.1.0` → `0.1.0`）、インストーラ・`.deb`・`.dmg` に渡します。つまりリリースのバージョンは `Cargo.toml` の `version` ではなくタグが基準です。とはいえ About ダイアログの表示が合わなくなるので、`Cargo.toml` 側も上げておくことをおすすめします。

手動実行（**Actions → Release → Run workflow**）した場合は、すべて `0.0.0-dev` としてビルドし、Release を作らずに成果物だけをアップロードします。ワークフロー自体の動作確認に便利です。

## 署名と公証（notarization）について

macOS の `.app` は **アドホック署名**（`codesign --sign -`）です。これは Apple Silicon で「アプリが破損しています」と言われずに起動させるために必要な最低限の署名で、公証は通っていません。そのため利用者は初回のみ右クリック →「開く」で起動する必要があります。正式に配布するには有償の Apple Developer アカウント、Developer ID による `codesign`、`xcrun notarytool submit` が必要です。

Windows のインストーラも未署名なので、ダウンロード時に SmartScreen の警告が出ます。署名にはコードサイニング証明書と `signtool` が必要です。

## ローカルでインストーラをビルドする

```bash
cargo build --release

# Windows（Inno Setup 6 が必要）
iscc installer\windows.iss              # -> dist\

# Linux .deb（cargo-deb が必要）
cargo install cargo-deb && cargo deb

# macOS .app（cargo-bundle が必要）
cargo install cargo-bundle && cargo bundle --release
# さらに .dmg を作る場合は `brew install create-dmg` が必要です。
# 詳細は .github/workflows/release.yml の macos ジョブを参照してください
```

## act でワークフローを試す

[`act`](https://github.com/nektos/act) を使うと Docker 上でワークフローをローカル実行できます。

```bash
act -W .github/workflows/ci.yml -j lint
act -W .github/workflows/release.yml -j linux
```

`act` で動かせるのは Linux ジョブだけです。`windows` と `macos` ジョブは実際のランナーが必要です。使用するランナーイメージは [`.actrc`](.actrc) に固定しています。

## Linux でのビルドに必要なパッケージ

```bash
sudo apt-get install -y \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libwayland-dev libegl1-mesa-dev \
  libgtk-3-dev libssl-dev
```

## ライセンス

MIT — [LICENSE](LICENSE) を参照してください。
