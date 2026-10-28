// Release builds get no console window on Windows; debug builds keep it so
// `println!`/panics stay visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::ffi::OsString;
use std::path::PathBuf;

use open_draw_chemistry::{APP_NAME, OpenDrawApp};

/// The window/taskbar icon, embedded at compile time — the source tree is not
/// present on an end user's machine, so it cannot be loaded from disk.
const ICON_BYTES: &[u8] = include_bytes!("../resources/icon.png");

fn load_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory_with_format(ICON_BYTES, image::ImageFormat::Png).ok()?;
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();

    Some(egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

fn native_options() -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([640.0, 420.0]);

    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }

    eframe::NativeOptions {
        viewport,
        // The 3D viewer renders through wgpu; glow (the default) has no render state for it.
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    }
}

fn main() -> eframe::Result<()> {
    // First, while the process is single-threaded: where the installed OpenBabel is.
    open_draw_chemistry::three_d::runtime::init();

    let args: Vec<OsString> = std::env::args_os().skip(1).collect();

    // `--check-openbabel`: prove that this installation finds its OpenBabel (the release
    // workflow runs it on every package), then exit without opening a window.
    if args.first().is_some_and(|a| a == "--check-openbabel") {
        let result = open_draw_chemistry::three_d::runtime::self_check(true);
        match &result {
            Ok(report) => print!("{report}"),
            Err(report) => eprintln!("{report}"),
        }
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }

    // Registration writes translated verbs to the registry: use the OS language for them (the
    // app sets the user's saved choice itself once its window opens).
    #[cfg(windows)]
    open_draw_chemistry::i18n::set_language(open_draw_chemistry::i18n::Lang::system());
    #[cfg(windows)]
    if let Some(result) = ole::run(&args) {
        return result;
    }

    // Files to open, as passed by the desktop (`%F`) or a drop onto the executable.
    let files: Vec<PathBuf> = args.into_iter().map(PathBuf::from).collect();

    eframe::run_native(
        APP_NAME,
        native_options(),
        Box::new(|cc| Ok(Box::new(OpenDrawApp::new(cc, files)))),
    )
}

/// The OLE server's command lines (Windows):
///
/// - `--register-ole [args…]` / `--unregister-ole`: register the server for the current user
///   (any further arguments are appended to the server's command line, for testing);
/// - `-Embedding` (from COM): serve embedded objects. `--ole-log <file>` logs what the containers
///   ask for; `--ole-own-class` stores new objects under this app's class instead of ChemDraw's
///   and `--ole-test-script <file>` applies a scripted edit (see `ole_server::script`), both
///   for testing.
#[cfg(windows)]
mod ole {
    use std::ffi::OsString;

    use open_draw_chemistry::ole_server::{self, com, register, script::Script};
    use open_draw_chemistry::{APP_NAME, OpenDrawApp};

    pub fn run(args: &[OsString]) -> Option<eframe::Result<()>> {
        let first = args.first().map(|a| a.to_string_lossy().into_owned());
        match first.as_deref() {
            Some("--register-ole") => {
                let extra: Vec<String> = args[1..]
                    .iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect();
                match register::register_current_exe(&extra.join(" ")) {
                    Ok(r) => {
                        println!("OLE サーバーを登録しました: {}", r.created.join(", ")); // i18n-allow: CLI output
                        if r.treat_as {
                            println!("ChemDraw のオブジェクトをこのアプリで開くよう設定しました"); // i18n-allow: CLI output
                        } else {
                            println!(
                                "ChemDraw がインストールされているため、ChemDraw の設定は変更していません" // i18n-allow: CLI output
                            );
                        }
                        if r.removed_stale_treat_as {
                            println!("以前の ChemDraw の代理設定を取り消しました"); // i18n-allow: CLI output
                        }
                    }
                    Err(e) => {
                        eprintln!("OLE サーバーを登録できませんでした: {e}"); // i18n-allow: CLI output
                        std::process::exit(1);
                    }
                }
                Some(Ok(()))
            }
            Some("--unregister-ole") => {
                if let Err(e) = register::unregister_current_user() {
                    eprintln!("OLE サーバーの登録を解除できませんでした: {e}"); // i18n-allow: CLI output
                    std::process::exit(1);
                }
                println!("OLE サーバーの登録を解除しました"); // i18n-allow: CLI output
                Some(Ok(()))
            }
            _ if ole_server::is_embedding_launch(args) => Some(serve(args)),
            _ => None,
        }
    }

    /// The value after `flag` in `args`.
    fn value<'a>(args: &'a [OsString], flag: &str) -> Option<&'a OsString> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
    }

    fn serve(args: &[OsString]) -> eframe::Result<()> {
        if let Some(path) = value(args, "--ole-log") {
            ole_server::open_log(std::path::Path::new(path));
        }
        ole_server::log(&format!("started: {args:?}"));
        let own_class = args.iter().any(|a| a == "--ole-own-class");
        let script = value(args, "--ole-test-script")
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| Script::parse(&text));
        let script = match script {
            Some(Ok(s)) => Some(s),
            Some(Err(e)) => {
                ole_server::log(&format!("--ole-test-script: {e}"));
                None
            }
            None => None,
        };
        let (server, mut bridge) = match com::Server::start(own_class) {
            Ok(started) => started,
            Err(e) => {
                ole_server::log(&format!("OLE サーバーを開始できませんでした: {e}")); // i18n-allow: log output
                return Ok(());
            }
        };
        bridge.script = script;
        // The window opens when a container shows an object; a container that only needed the
        // object running (or nothing at all) never sees one.
        if !server.wait_until_shown() {
            ole_server::log("no object to show; exiting");
            server.shutdown();
            return Ok(());
        }
        ole_server::log("showing the window");
        let result = eframe::run_native(
            APP_NAME,
            super::native_options(),
            Box::new(move |cc| {
                let ctx = cc.egui_ctx.clone();
                Ok(Box::new(
                    OpenDrawApp::new(cc, Vec::new()).with_ole(bridge, &ctx),
                ))
            }),
        );
        ole_server::log(&format!("window closed: {result:?}"));
        server.shutdown();
        ole_server::log("server stopped");
        result
    }
}
