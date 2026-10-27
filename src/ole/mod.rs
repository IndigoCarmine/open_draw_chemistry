//! Copy for Office: on Windows a copy is published as an OLE data object, so a plain paste in
//! Word or PowerPoint embeds the structure as a ChemDraw object (double-click edits it), while
//! ChemDraw and this app still read the CDX / our own JSON from the same clipboard.
//!
//! The editor widget (chembuider-rs) has its own copy, which writes the formats straight to the
//! Win32 clipboard. Word pastes that as a picture (it prefers the "PNG" among them to the
//! object), so the app routes Edit > Copy, the toolbar and Ctrl+C on the canvas through [`copy`],
//! which publishes the formats the way ChemDraw does (see `clipboard`). Other platforms keep the
//! widget's copy.

#[cfg(windows)]
mod clipboard;

use chembuider_rs::ChemStructEditor;

/// The status line after a successful [`copy`].
#[cfg(windows)]
pub const COPIED_MESSAGE: &str = "コピーしました（Word・PowerPoint には ChemDraw オブジェクトとして、ChemDraw には構造として貼り付けられます）";
#[cfg(not(windows))]
pub const COPIED_MESSAGE: &str = "コピーしました";

/// Copy the selection (or the whole drawing when nothing is selected) to the system clipboard.
/// Returns false when there is nothing to copy.
pub fn copy(editor: &ChemStructEditor, ctx: &egui::Context) -> bool {
    #[cfg(windows)]
    {
        let Some(text) = editor.copy_to_string() else {
            return false;
        };
        if clipboard::set(&editor.molecule, &text).is_ok() {
            return true;
        }
        // OLE unavailable (e.g. the thread is not an OLE apartment): the widget's own copy still
        // gives ChemDraw, pictures and paste-back.
    }
    editor.copy(ctx)
}

/// Take this frame's copy shortcut (Ctrl+C, and egui's Copy/Cut events) away from the editor
/// widget, which would otherwise publish its own clipboard over [`copy`]'s. Call it before the
/// editor is shown; returns whether a copy was requested. Text fields keep their shortcut, and
/// on platforms other than Windows nothing is taken.
pub fn take_copy_shortcut(ctx: &egui::Context) -> bool {
    if !cfg!(windows) || ctx.egui_wants_keyboard_input() {
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

    #[test]
    fn other_input_is_left_alone() {
        frame_with(vec![egui::Event::Text("c".into())], |ctx| {
            assert!(!take_copy_shortcut(ctx));
            assert_eq!(ctx.input(|i| i.events.len()), 1);
        });
    }
}
