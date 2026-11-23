//! Copy for Office: on Windows a copy is published as an OLE data object, so a plain paste in
//! Word or PowerPoint embeds the structure as a ChemDraw object (double-click edits it), while
//! ChemDraw and this app still read the CDX / our own JSON from the same clipboard.
//!
//! The editor widget (chembuider-rs) has its own copy, which writes the formats straight to the
//! Win32 clipboard. Word pastes that as a picture (it prefers the "PNG" among them to the
//! object), so the app routes Edit > Copy, the toolbar and Ctrl+C on the canvas through [`copy`],
//! which publishes the formats the way ChemDraw does (see `clipboard`). Other platforms keep the
//! widget's copy.

//!
//! Paste goes the other way through [`paste`]: besides what the widget reads (our own JSON, the
//! CDX), it takes an embedded ChemDraw object copied in Word or PowerPoint, which reaches the
//! clipboard only as "Embed Source" (see `paste`), and keeps a CDX's arrows and text.

#[cfg(windows)]
pub(crate) mod clipboard;
#[cfg(any(windows, test))]
mod insert;
#[cfg(windows)]
pub mod paste;

use chembuider_rs::ChemStructEditor;

/// The status line after a successful [`copy`].
pub fn copied_message() -> &'static str {
    if cfg!(windows) {
        t!("ole.clipboard.copied_for_office")
    } else {
        t!("ole.clipboard.copied")
    }
}
/// Copy the selection (or the whole drawing when nothing is selected) to the system clipboard.
/// Returns false when there is nothing to copy.
pub fn copy(editor: &ChemStructEditor, ctx: &egui::Context) -> bool {
    #[cfg(windows)]
    {
        let Some(text) = editor.copy_to_string() else {
            return false;
        };
        if clipboard::set(&editor.molecule, &editor.config.style, &text).is_ok() {
            return true;
        }
        // OLE unavailable (e.g. the thread is not an OLE apartment): the widget's own copy still
        // gives ChemDraw, pictures and paste-back.
    }
    editor.copy(ctx)
}

/// Copy the 3D view, drawn into `object` (the model as MOL2, its look and picture): on Windows
/// an OLE object Office embeds and edits again here (see `clipboard::set_3d`), elsewhere, or
/// when OLE is unavailable, the picture. The status line, or why nothing was copied.
pub fn copy_3d(
    object: &crate::ole_server::model3d::Object3d,
    ctx: &egui::Context,
) -> Result<&'static str, String> {
    #[cfg(windows)]
    match clipboard::set_3d(object) {
        Ok(()) => return Ok(t!("ole.clipboard.copied_3d_for_office")),
        // The picture is copied instead; say why the object was not.
        Err(e) => eprintln!("warn: the 3D view was copied as a picture only: {e}"),
    }
    let pixels = object
        .pixels()
        .ok_or_else(|| t!("three_d.error.not_shown").to_owned())?;
    ctx.copy_image(egui::ColorImage::from_rgba_unmultiplied(
        [pixels.width as usize, pixels.height as usize],
        &pixels.rgba,
    ));
    Ok(t!("ole.clipboard.copied_3d_image"))
}

/// Take this frame's copy shortcut (Ctrl+C, or egui's Copy/Cut event) in a view without a text
/// field focused, on every platform: the 3D view copies itself with it. Returns whether a copy
/// was asked for.
pub fn take_view_copy_shortcut(ctx: &egui::Context) -> bool {
    if ctx.text_edit_focused() {
        return false;
    }
    ctx.input_mut(|input| {
        let before = input.events.len();
        input
            .events
            .retain(|e| !matches!(e, egui::Event::Copy | egui::Event::Cut));
        let had_event = input.events.len() != before;
        had_event | input.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
    })
}

/// Take this frame's copy shortcut (Ctrl+C, and egui's Copy/Cut events) away from the editor
/// widget, which would otherwise publish its own clipboard over [`copy`]'s. Call it before the
/// editor is shown; returns whether a copy was requested. Text fields keep their shortcut, and
/// on platforms other than Windows nothing is taken.
pub fn take_copy_shortcut(ctx: &egui::Context) -> bool {
    if !cfg!(windows) || ctx.text_edit_focused() {
        return false;
    }
    ctx.input_mut(|input| {
        let before = input.events.len();
        input
            .events
            .retain(|e| !matches!(e, egui::Event::Copy | egui::Event::Cut));
        let had_event = input.events.len() != before;
        // `|` (not `||`): the key press must be consumed even when the event was there too.
        had_event | input.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
    })
}

/// Paste from the system clipboard into `editor` (Edit > Paste, the toolbar and Ctrl+V on the
/// canvas). Returns whether anything was pasted, or the status line when the clipboard holds
/// an embedded object that cannot be pasted.
///
/// On Windows, in order: our own "ChemBuilder Molecule" (through the widget), the "ChemDraw
/// Interchange Format" CDX, an embedded object's `CONTENTS` ("Embed Source", which is all a
/// copy of a ChemDraw object in Word or PowerPoint carries), and last whatever else the
/// widget's paste reads (text in our format). A CDX keeps its arrows, text and tables.
pub fn paste(editor: &mut ChemStructEditor) -> Result<bool, String> {
    #[cfg(windows)]
    if let Ok(clipboard) = paste::Clipboard::open() {
        if clipboard
            .app_text()
            .is_some_and(|text| editor.paste_from_string(&text))
        {
            return Ok(true);
        }
        let mut error = None;
        // Paste a CDX, or say why it cannot be read.
        let mut paste_cdx = |cdx: Result<Vec<u8>, String>| -> Result<bool, String> {
            let bytes = cdx?;
            let loaded = crate::io::decode(crate::io::Format::Cdx, &bytes)
                .map_err(|_| t!("ole.paste.cdx_unreadable").to_owned())?;
            Ok(insert::paste_drawing(editor, &loaded.molecule))
        };
        // The embedded object is read only when the CDX did not paste.
        match clipboard.cdx().map(|cdx| paste_cdx(Ok(cdx))) {
            Some(Ok(true)) => return Ok(true),
            Some(Err(message)) => error = Some(message),
            _ => {}
        }
        match clipboard
            .embedded_cdx()
            .map(|cdx| paste_cdx(cdx.map_err(|e| e.to_string())))
        {
            Some(Ok(true)) => return Ok(true),
            Some(Err(message)) => error = error.or(Some(message)),
            _ => {}
        }
        return match (editor.paste_clipboard(), error) {
            (true, _) => Ok(true),
            (false, Some(message)) => Err(message),
            (false, None) => Ok(false),
        };
    }
    Ok(editor.paste_clipboard())
}

/// Ctrl+V on the canvas, taken from the editor widget so that [`paste`] handles it.
///
/// On Windows the widget reads Ctrl+V straight from the keyboard (`GetAsyncKeyState`, because
/// egui turns the shortcut into a text-only event), so consuming egui's input would not stop
/// it. Instead [`take`](Self::take) sees the same key press first and, for that one frame,
/// tells the widget the window is not focused — the one condition under which it ignores the
/// key — and [`release`](Self::release) restores it after the widget has been shown.
#[derive(Debug, Default)]
pub struct PasteShortcut {
    /// The V key was down last frame (the press is taken once, on its way down).
    #[cfg(windows)]
    v_down: bool,
    /// The focus was hidden this frame and must be restored.
    hidden: bool,
}

impl PasteShortcut {
    /// Call before the editor is shown; returns whether Ctrl+V was pressed on the canvas this
    /// frame. Text fields keep their own paste. On platforms other than Windows nothing is
    /// taken (the widget pastes egui's text event itself).
    pub fn take(&mut self, ctx: &egui::Context) -> bool {
        #[cfg(windows)]
        {
            use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL};
            let down = |key: i32| (unsafe { GetAsyncKeyState(key) } as u16) & 0x8000 != 0;
            let v = down(i32::from(b'V'));
            let pressed = v && !self.v_down && down(i32::from(VK_CONTROL.0));
            self.v_down = v;
            self.take_press(ctx, pressed)
        }
        #[cfg(not(windows))]
        {
            self.take_press(ctx, false)
        }
    }

    /// The part of [`take`](Self::take) after the keyboard is read (testable without one).
    fn take_press(&mut self, ctx: &egui::Context, pressed: bool) -> bool {
        if !pressed || ctx.text_edit_focused() || !ctx.input(|i| i.focused) {
            return false;
        }
        ctx.input_mut(|i| i.focused = false);
        self.hidden = true;
        true
    }

    /// Call after the editor is shown: gives the window its focus back.
    pub fn release(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.hidden) {
            ctx.input_mut(|i| i.focused = true);
        }
    }
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn the_views_copy_shortcut_is_taken_on_every_platform() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![egui::Event::Copy],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            assert!(take_view_copy_shortcut(ui.ctx()));
            assert!(!take_view_copy_shortcut(ui.ctx()), "taken once");
        });
        output.textures_delta.clear();
    }

    #[test]
    fn a_press_hides_the_focus_from_the_widget_for_one_frame() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            focused: true,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx();
            let mut shortcut = PasteShortcut::default();
            assert!(!shortcut.take_press(ctx, false));
            assert!(ctx.input(|i| i.focused));
            assert!(shortcut.take_press(ctx, true));
            assert!(!ctx.input(|i| i.focused), "the widget must not paste too");
            shortcut.release(ctx);
            assert!(ctx.input(|i| i.focused));
            // Without the focus (another window has it) nothing is pasted.
            ctx.input_mut(|i| i.focused = false);
            assert!(!shortcut.take_press(ctx, true));
            shortcut.release(ctx);
            assert!(!ctx.input(|i| i.focused));
        });
        output.textures_delta.clear();
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn frame_with(events: Vec<egui::Event>, mut run: impl FnMut(&egui::Context)) {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| run(ui.ctx()));
        output.textures_delta.clear();
    }

    #[test]
    fn copy_and_cut_events_are_taken_from_the_editor() {
        for event in [egui::Event::Copy, egui::Event::Cut] {
            frame_with(vec![event], |ctx| {
                assert!(take_copy_shortcut(ctx));
                // Nothing left for the widget to copy over ours.
                assert!(!ctx.input(|i| {
                    i.events
                        .iter()
                        .any(|e| matches!(e, egui::Event::Copy | egui::Event::Cut))
                }));
                assert!(!take_copy_shortcut(ctx));
            });
        }
    }

    /// Pastes the clipboard into an empty editor with [`paste`], as Edit > Paste does (e.g.
    /// after copying an object in Word), and prints what arrived.
    ///   cargo test --lib ole::tests::paste_live_clipboard_into_editor -- --ignored --nocapture
    #[test]
    #[ignore = "reads the system clipboard"]
    fn paste_live_clipboard_into_editor() {
        let mut editor = ChemStructEditor::default();
        match paste(&mut editor) {
            Ok(pasted) => println!(
                "pasted={pasted} atoms={} bonds={} objects={} undo={}",
                editor.molecule.atoms.len(),
                editor.molecule.bonds.len(),
                editor.molecule.objects.len(),
                editor.undo_stack.len()
            ),
            Err(message) => println!("error={message}"),
        }
    }

    #[test]
    fn other_input_is_left_alone() {
        frame_with(vec![egui::Event::Text("c".into())], |ctx| {
            assert!(!take_copy_shortcut(ctx));
            assert_eq!(ctx.input(|i| i.events.len()), 1);
        });
    }
}
