# 開発ガイド

[English](DEVELOPMENT.md) · [README に戻る](../README_ja.md)

> **Rust 1.95 以上**が必要です（Wayland のドラッグ＆ドロップ用 `egui-winit` フォークによる）。

## Office（OLE）登録の詳細

登録は `HKCU\Software\Classes` の下だけに書き込みます。このアプリ自身の OLE クラス
（`OpenDrawChemistry.Document.1`。「挿入 ▸ オブジェクト」にも出ます）と、**ChemDraw が
インストールされていない場合に限り**、ChemDraw のクラスをこのアプリで開く `TreatAs` です。
ChemDraw がある PC では ChemDraw の設定は一切変更しません。後から ChemDraw をインストールした
場合は、もう一度 `--register-ole` を実行する（またはアンインストールする）と ChemDraw に戻ります。

編集はこのアプリのウィンドウで行います（Word の中でのインプレース編集には対応していません）。
問題の調査には、`LocalServer32` のコマンドに `--ole-log <ファイル>` を加えると、
コンテナからの呼び出しがログに記録されます。

## ソースからビルドする

このアプリには [OpenBabel](https://openbabel.org) が必須です（3D 生成・SMILES・3D ファイルの読み込み）。
[openbabel_rs](https://github.com/IndigoCarmine/openbabel_rs) を通してリンクします。最初に一度（openbabel_rs を更新したときも）
ビルド済みの OpenBabel を取得してください（`bash` と、`gh` または `curl` が必要です）。

```bash
bash scripts/fetch-openbabel.sh   # third_party/openbabel-prebuilt/ に展開（コミットしない）
cargo run -- path/to/file.cdx     # ファイルの指定は省略可
```

`.cargo/config.toml` が `OPENBABEL_SYS_PREBUILT_DIR` をこのディレクトリに向けます。
Windows では openbabel-sys のビルドスクリプトが OpenBabel の DLL とプラグインを実行ファイルの
隣にコピーし（Linux / macOS は rpath）、`cargo run` も `cargo test` もそのまま動きます。
バインディングの小さな C++ シムのために C++ コンパイラ（Windows では MSVC Build Tools）は必要です。

ビルド済みの OpenBabel があるのは `x86_64-pc-windows-msvc`、`x86_64-unknown-linux-gnu`、
`aarch64-apple-darwin`、`x86_64-apple-darwin` です。ホスト以外のターゲット向けにビルドするときは、
そのターゲットのビルド済み OpenBabel を別ディレクトリに取得し、ビルドをそこへ向けます。

```bash
bash scripts/fetch-openbabel.sh x86_64-apple-darwin   # -> third_party/openbabel-prebuilt-x86_64-apple-darwin/
OPENBABEL_SYS_PREBUILT_DIR=$PWD/third_party/openbabel-prebuilt-x86_64-apple-darwin \
  cargo build --release --target x86_64-apple-darwin
```

ビルド済みのないターゲットでは、CMake を入れて `OPENBABEL_SYS_PREBUILT_DIR` を空にし、OpenBabel を
ソースからビルドしてください（初回は 10〜20 分かかります）。OpenBabel なしのビルドはありません。

リリース版はすべて OpenBabel を同梱しています。

| パッケージ | ライブラリ | プラグイン | データ |
| --- | --- | --- | --- |
| Windows（インストーラ・zip） | `.exe` の隣の `openbabel-3.dll`・`inchi.dll` | `.exe` の隣の `*.obf` | `.exe` の隣の `data\` |
| Linux（`.deb`・AppImage） | `usr/lib/open-draw-chemistry/`（rpath `$ORIGIN/../lib/open-draw-chemistry`） | `usr/lib/open-draw-chemistry/openbabel/` | `usr/share/open-draw-chemistry/openbabel/` |
| macOS（`.app`・ユニバーサル） | `Contents/Frameworks/`（rpath `@executable_path/../Frameworks`） | `Contents/Frameworks/openbabel/` | `Contents/Resources/openbabel/` |

プラグインとデータの場所は、アプリが起動時に OpenBabel に伝えます（`src/three_d/runtime.rs`）。
Windows 版は Visual C++ ランタイム（`msvcp140.dll`・`vcruntime140.dll`・`vcruntime140_1.dll`）も
アプリと同じフォルダに同梱しているので、`vc_redist.x64.exe` を入れていないまっさらな Windows でも動きます。
`open_draw_chemistry --check-openbabel` はウィンドウを開かずに同梱の OpenBabel を試します。
リリースのワークフローは各パッケージでこれを実行します。

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
bash scripts/fetch-openbabel.sh
cargo build --release

# Windows（Inno Setup 6 が必要）：アプリのフォルダ（ポータブル zip の中身）を作ってからインストーラ
bash scripts/stage-windows.sh           # -> target\stage\windows\
iscc installer\windows.iss              # -> dist\

# Linux .deb（cargo-deb と patchelf が必要）
bash scripts/stage-linux.sh target/deb-root && mkdir -p target/deb-root/DEBIAN
cargo install cargo-deb && cargo deb --no-build

# macOS ユニバーサル .app（cargo-bundle と両アーキテクチャの OpenBabel が必要）
# .github/workflows/release.yml の macos ジョブ（scripts/bundle-macos.sh）を参照してください。
# さらに .dmg を作る場合は `brew install create-dmg` が必要です
```

## act でワークフローを試す

[`act`](https://github.com/nektos/act) を使うと Docker 上でワークフローをローカル実行できます。

```bash
act -W .github/workflows/ci.yml -j lint
act -W .github/workflows/release.yml -j linux
```

`act` で動かせるのは Linux ジョブだけです。`windows` と `macos` ジョブは実際のランナーが必要です。使用するランナーイメージは [`.actrc`](../.actrc) に固定しています。

## Linux でのビルドに必要なパッケージ

```bash
sudo apt-get install -y \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libwayland-dev libegl1-mesa-dev \
  libgtk-3-dev libssl-dev
```

