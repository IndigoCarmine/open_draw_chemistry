// No console window on Windows, in any build: a console program started without one (by COM
// for the OLE server's `-Embedding`, or from Explorer) gets a new, black console window. Output
// and panics still reach the terminal the app is run from (see `console`).
#![windows_subsystem = "windows"]

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use open_draw_chemistry::ole_server::{self, script::Script};
use open_draw_chemistry::{APP_NAME, OpenDrawApp, acauda};

mod fatal;

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

/// A failed start gets an error screen of its own (see `fatal`): the window that would have
/// shown it never opened.
fn main() -> ExitCode {
    #[cfg(windows)]
    console::attach_to_parent();
    fatal::install_panic_hook();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            fatal::startup_failed(&error);
            ExitCode::FAILURE
        }
    }
}

fn run() -> eframe::Result<()> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();

    // Registration writes translated verbs to the registry: use the OS language for them (the
    // app sets the user's saved choice itself once its window opens).
    #[cfg(windows)]
    open_draw_chemistry::i18n::set_language(open_draw_chemistry::i18n::Lang::system());
    #[cfg(windows)]
    if let Some(result) = ole::run(&args) {
        return result;
    }

    // Started by AcaudaOffice for an embedded object (no COM: on every platform).
    match acauda::start(&args) {
        Ok(acauda::Launch::NotStarted) => {}
        Ok(acauda::Launch::Done) => return Ok(()),
        Ok(acauda::Launch::Serve(server, mut bridge)) => {
            if let Some(path) = value(&args, "--ole-log") {
                ole_server::open_log(std::path::Path::new(path));
            }
            bridge.script = test_script(&args);
            let result = eframe::run_native(
                APP_NAME,
                native_options(),
                Box::new(move |cc| {
                    let ctx = cc.egui_ctx.clone();
                    Ok(Box::new(
                        OpenDrawApp::new(cc, Vec::new()).with_ole(bridge, &ctx),
                    ))
                }),
            );
            server.shutdown();
            return result;
        }
        Err(e) => {
            eprintln!("AcaudaOffice: {e}");
            std::process::exit(1);
        }
    }

    // Files to open, as passed by the desktop (`%F`) or a drop onto the executable.
    let files: Vec<PathBuf> = args.into_iter().map(PathBuf::from).collect();

    eframe::run_native(
        APP_NAME,
        native_options(),
        Box::new(|cc| Ok(Box::new(OpenDrawApp::new(cc, files).with_update_check()))),
    )
}

/// The terminal a GUI-subsystem program is run from, for its output (`--register-ole`'s report,
/// `cargo run`'s log and panics).
#[cfg(windows)]
mod console {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
        STD_OUTPUT_HANDLE, SetStdHandle,
    };

    /// Write stdout/stderr to the parent's console when it has one and they go nowhere else.
    /// Handles the parent gave (a pipe or file: a test, a redirect) are kept; started by COM or
    /// Explorer there is no console to attach to and nothing happens.
    pub fn attach_to_parent() {
        let given = |id: STD_HANDLE| {
            // SAFETY: GetStdHandle only reads this process's standard handle table.
            unsafe { GetStdHandle(id) }
                .ok()
                .filter(|h| !h.is_invalid() && !h.0.is_null())
        };
        let handles: [(STD_HANDLE, Option<HANDLE>); 2] = [
            (STD_OUTPUT_HANDLE, given(STD_OUTPUT_HANDLE)),
            (STD_ERROR_HANDLE, given(STD_ERROR_HANDLE)),
        ];
        if handles.iter().all(|(_, h)| h.is_some()) {
            return;
        }
        // SAFETY: attaching to the parent's console has no preconditions; it fails harmlessly
        // when the parent has none.
        if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_err() {
            return;
        }
        for (id, handle) in handles {
            if let Some(handle) = handle {
                // SAFETY: `handle` is the process's own standard handle from before the attach.
                let _ = unsafe { SetStdHandle(id, handle) };
            }
        }
    }
}

/// The value after `flag` in `args`.
fn value<'a>(args: &'a [OsString], flag: &str) -> Option<&'a OsString> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
}

/// The scripted edit named by `--ole-test-script` (see `ole_server::script`).
fn test_script(args: &[OsString]) -> Option<Script> {
    let text = std::fs::read_to_string(value(args, "--ole-test-script")?).ok()?;
    match Script::parse(&text) {
        Ok(script) => Some(script),
        Err(e) => {
            ole_server::log(&format!("--ole-test-script: {e}"));
            None
        }
    }
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

    use open_draw_chemistry::ole_server::{self, com, register};
    use open_draw_chemistry::{APP_NAME, OpenDrawApp};

    use super::{test_script, value};

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

    fn serve(args: &[OsString]) -> eframe::Result<()> {
        if let Some(path) = value(args, "--ole-log") {
            ole_server::open_log(std::path::Path::new(path));
        }
        ole_server::log(&format!("started: {args:?}"));
        let own_class = args.iter().any(|a| a == "--ole-own-class");
        let (server, mut bridge) = match com::Server::start(own_class) {
            Ok(started) => started,
            Err(e) => {
                ole_server::log(&format!("OLE サーバーを開始できませんでした: {e}")); // i18n-allow: log output
                return Ok(());
            }
        };
        bridge.script = test_script(args);
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
