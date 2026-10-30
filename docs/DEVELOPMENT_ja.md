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

## 更新の署名

アプリは起動時に GitHub の最新の Release を確認します（`src/update/`）。自動でインストールするのは、
アセットの署名がアプリに組み込まれた公開鍵 `resources/update-signing.pub` で検証できたときだけです。
このファイルに本物の鍵が入るまでは中身がプレースホルダーで、アプリは何もインストールせず Release ページを開きます。

| インストール形式 | 更新ダイアログで「はい」を選ぶと |
| --- | --- |
| Windows インストーラ | `…-setup.exe` をダウンロードしてアプリを終了し、インストール時と同じモード（`/CURRENTUSER`、または UAC の確認が出る `/ALLUSERS`）で `/SILENT` 実行してから再起動 |
| Linux AppImage | 新しい AppImage をダウンロードし、古いファイルに rename で置き換えて再起動 |
| macOS `.app` | `.dmg` をダウンロードし、`hdiutil` と `ditto` でアプリを取り出し、rename でバンドルを入れ替えて再起動 |
| Windows ポータブル zip、Linux `.deb`、その他 | Release ページを開く |

ダウンロード・検証・インストールに失敗した場合も Release ページを開きます。

署名の準備は最初に一度だけ行います（OpenSSL 3 が必要）:

```bash
bash scripts/gen-update-key.sh ~/open-draw-chemistry-update-signing.pem
gh secret set UPDATE_SIGNING_KEY --repo IndigoCarmine/open_draw_chemistry \
  < ~/open-draw-chemistry-update-signing.pem
git add resources/update-signing.pub   # コミットする
```

秘密鍵はリポジトリに入れず、バックアップを取ってください。アプリはビルド時に組み込まれた鍵しか信用しないので、
**鍵を替えると、インストール済みのアプリは新しい鍵のビルドへ更新するその一度だけ Release ページに進みます**。
鍵の生成スクリプトは既存のファイルを上書きしません。

タグをリリースすると、`scripts/sign-update-assets.sh` が各アセットの横に `<asset>.sig` を書き出します。
シークレット `UPDATE_SIGNING_KEY` がなければ警告を出して署名せず、`resources/update-signing.pub` と一致しなければリリースを止めます。
署名は次のテキストに対する 64 バイトの ed25519 署名です。ファイルの内容だけでなく、名前（つまりバージョン）にも結び付いています。OpenSSL でも検証できます:

```text
open-draw-chemistry-update-v1
<アセット名>
<アセットの sha256（小文字の 16 進）>
```

鍵をコミットする前のビルドと、`.sig` のない Release への更新は自動ではできず、Release ページを開きます。

`cargo test real_github -- --ignored` は実際の API とダウンロードの経路をネットワーク越しに確認します。

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

