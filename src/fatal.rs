//! The last-resort error screen: a native message box for a failure that leaves no window to
//! show it in. A release build has no console on Windows, so without it a start that fails (no
//! usable graphics device, say) or a panic just looks like the app never opening.
//!
//! Nothing here goes through egui or wgpu, which are what may have failed.

use open_draw_chemistry::{APP_NAME, i18n, t};

/// Debug builds keep the console and are run by scripts that wait for the process to exit, so
/// only release builds stop on a dialog.
const DIALOG: bool = cfg!(not(debug_assertions));

/// Report that the app could not start: on stderr (as before) and, in a release build, in a
/// message box saying what is missing and what to do about it.
pub fn startup_failed(error: &eframe::Error) {
    eprintln!("Error: {error:?}");
    if !DIALOG {
        return;
    }
    let mut text = t!("fatal.startup", app = APP_NAME);
    // wgpu tries Vulkan, Direct3D 12 (hardware, then the software adapter) and OpenGL, and
    // loads their libraries at run time: this is every one of them missing or unusable.
    if matches!(error, eframe::Error::Wgpu(_)) {
        text.push_str("\n\n");
        text.push_str(t!("fatal.graphics"));
    }
    text.push_str("\n\n");
    text.push_str(&t!("fatal.details", error = error));
    message_box(&text);
}

/// Show a panic on the main thread, which ends the app, in a message box (Windows release
/// builds; elsewhere the default hook's message on stderr is all there is).
///
/// The text is embedded rather than looked up in a catalog: a panic inside the hook aborts, and
/// the panic being reported may have come from loading the catalogs.
pub fn install_panic_hook() {
    if !DIALOG || !cfg!(windows) {
        return;
    }
    let main_thread = std::thread::current().id();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        // A worker's panic is its job's failure, which the app reports itself.
        if std::thread::current().id() != main_thread {
            return;
        }
        let intro = match i18n::language() {
            i18n::Lang::Ja => "内部エラーが発生したため終了します。", // i18n-allow: must not use the catalog
            _ => "An internal error occurred and the app has to close.",
        };
        let text = format!("{APP_NAME}\n\n{intro}\n\n{info}");
        // From a thread of its own: a message box on this one would dispatch the app's window
        // messages into code that is in the middle of unwinding.
        let _ = std::thread::spawn(move || message_box(&text)).join();
    }));
}

#[cfg(windows)]
fn message_box(text: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW,
    };
    use windows::core::HSTRING;

    // SAFETY: both strings are valid, NUL-terminated UTF-16 for the duration of the call.
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            &HSTRING::from(APP_NAME),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

/// Best effort: rfd's dialog needs a desktop portal or zenity on Linux, and shows nothing
/// without them.
#[cfg(not(windows))]
fn message_box(text: &str) {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(APP_NAME)
        .set_description(text)
        .show();
}
