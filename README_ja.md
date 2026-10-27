# Open DrawChemistry

[English README](README.md)

ChemDraw / ChemSketch / XDrawChem のような、化学構造式と反応式を描くデスクトップアプリです。
[egui](https://github.com/emilk/egui) で作られており、構造式エディタには
[chembuider-rs](https://github.com/IndigoCarmine/chembuider-rs)、ChemDraw ファイルの読み書きには
[cdx_file_rs](https://github.com/IndigoCarmine/cdx_file_rs) を使っています。

## 機能（第 1 段階）

- 構造式の作図・編集：選択／結合／消しゴム、結合の種類（くさび・破線くさび・太線・波線・配位など）、
  元素の選択、右クリックの結合メニュー（ChemDraw 風）、元に戻す／やり直し、2D クリーンアップ。
- **ChemDraw ファイル（`.cdx` / `.cdxml`）の読み込みと保存。** まだ表示できないオブジェクト
  （矢印・テキスト・図形など）は保存時にそのまま残ります。
- タブによる複数文書、最近使ったファイル、ドラッグ＆ドロップで開く、未保存の確認。
- 解析パネル：分子式、精密質量、同位体パターン（m/z）、元素分析。
- MOL2 / SMILES（Windows では PNG も）へのエクスポート。Windows ではコピーすると
  ChemDraw 互換の構造と EMF の図がクリップボードに入り、Office に貼り付けられます。

今後の予定：自由配置の反応式（矢印・テキスト・ブラケット・図形）、3D モデル化（OpenBabel）、
ibisOffice への埋め込み、Word / PowerPoint 用の COM OLE サーバー。

> **Rust 1.95 以上**が必要です（Wayland のドラッグ＆ドロップ用 `egui-winit` フォークによる）。

## はじめかた

```bash
cargo run -- path/to/file.cdx   # ファイルの指定は省略可
```

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

GPL-2.0-or-later — [LICENSE](LICENSE) を参照してください（予定している 3D 化で OpenBabel をリンクするため）。
