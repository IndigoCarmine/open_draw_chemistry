//! The panel on the right. Per the design it holds the analysis of the drawing (or of the
//! selection): formula, masses, isotope pattern and elemental analysis.
//!
//! The panel can be collapsed into a narrow strip on the right edge; the app keeps that state
//! as `show_properties` and toggles it when either function here returns `true`.

use chembuider_rs::widget::analysis::AnalysisPanel;
use egui::{Rect, UiBuilder, Vec2, vec2};

use crate::document::Document;
use crate::ui::icons::{self, Icon};

/// Width of the collapsed strip, including its frame's margins.
pub const COLLAPSED_WIDTH: f32 = 30.0;

/// Side of the collapse / expand buttons.
const BUTTON: f32 = 22.0;

/// Draws the expanded panel. Returns `true` when its collapse button was clicked.
pub fn properties(ui: &mut egui::Ui, doc: &Document, analysis: &mut AnalysisPanel) -> bool {
    // The button sits in the top-right corner, beside the panel's own 「解析」 heading. It is
    // added after the content so that it is on top of it for clicks.
    let max = ui.max_rect();
    let button_rect = Rect::from_min_size(
        egui::pos2(max.right() - BUTTON, max.top() - 2.0),
        Vec2::splat(BUTTON),
    );
    egui::ScrollArea::vertical().show(ui, |ui| {
        // The panel reports a click on its copy button; the status bar is not told here.
        let _ = analysis.ui(ui, &doc.editor.molecule, &doc.editor.selected_atoms);
    });
    let mut corner = ui.new_child(UiBuilder::new().max_rect(button_rect));
    collapse_button(&mut corner, Icon::ChevronsRight, "パネルを畳む")
}

/// Draws the collapsed strip. Returns `true` when it asks to expand the panel again.
pub fn collapsed(ui: &mut egui::Ui) -> bool {
    ui.vertical_centered(|ui| {
        let expand = collapse_button(ui, Icon::ChevronsLeft, "解析パネルを開く");
        ui.add_space(6.0);
        // 「解析」 set vertically, one character per line; clicking it expands too.
        let label = ui
            .add(
                egui::Label::new(egui::RichText::new(vertical("解析")).weak())
                    .sense(egui::Sense::click()),
            )
            .on_hover_text("解析パネルを開く");
        expand || label.clicked()
    })
    .inner
}

fn collapse_button(ui: &mut egui::Ui, icon: Icon, tooltip: &str) -> bool {
    icons::icon_button(
        ui,
        icon,
        vec2(BUTTON, BUTTON),
        icons::Frame::Flat,
        false,
        true,
        tooltip,
    )
    .clicked()
}

/// `text` with one character per line, for a label in the narrow strip.
fn vertical(text: &str) -> String {
    let chars: Vec<String> = text.chars().map(String::from).collect();
    chars.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_puts_one_character_per_line() {
        assert_eq!(vertical("解析"), "解\n析");
        assert_eq!(vertical("A"), "A");
        assert_eq!(vertical(""), "");
    }
}
