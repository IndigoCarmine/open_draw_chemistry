//! The panel on the right. It follows the view: with the drawing it holds the analysis of the
//! drawing (or of the selection) — formula, masses, isotope pattern and elemental analysis —
//! and with the 3D model the 3D 編集 controls ([`super::view3d::edit_panel`]).
//!
//! The panel can be collapsed into a narrow strip on the right edge; the app keeps that state
//! as `show_properties` and toggles it when either function here returns `true`.

use egui::{Rect, UiBuilder, Vec2, vec2};

use crate::ui::ViewMode;
use crate::ui::icons::{self, Icon};

/// Width of the collapsed strip, including its frame's margins.
pub const COLLAPSED_WIDTH: f32 = 30.0;

/// Side of the collapse / expand buttons.
const BUTTON: f32 = 22.0;

/// The panel's name in `view`.
pub fn title(view: ViewMode) -> &'static str {
    match view {
        ViewMode::TwoD => "解析",
        ViewMode::ThreeD => "3D 編集",
        ViewMode::Assembly => "集合体",
    }
}

/// Draws the expanded panel with `contents`, which start with their own heading (the button
/// sits beside it). Returns `true` when its collapse button was clicked.
pub fn properties(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) -> bool {
    properties_unscrolled(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, contents);
    })
}

/// [`properties`] for contents that lay out the whole panel themselves, scrolling what they
/// need to (the 集合体 panel keeps its tabs and footer in place).
pub fn properties_unscrolled(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) -> bool {
    // The button sits in the top-right corner, beside the contents' own heading. It is added
    // after the content so that it is on top of it for clicks.
    let max = ui.max_rect();
    let button_rect = Rect::from_min_size(
        egui::pos2(max.right() - BUTTON, max.top() - 2.0),
        Vec2::splat(BUTTON),
    );
    contents(ui);
    let mut corner = ui.new_child(UiBuilder::new().max_rect(button_rect));
    collapse_button(&mut corner, Icon::ChevronsRight, "パネルを畳む")
}

/// Draws the collapsed strip of the panel named `title`. Returns `true` when it asks to
/// expand the panel again.
pub fn collapsed(ui: &mut egui::Ui, title: &str) -> bool {
    let tip = format!("{title}パネルを開く");
    ui.vertical_centered(|ui| {
        let expand = collapse_button(ui, Icon::ChevronsLeft, &tip);
        ui.add_space(6.0);
        // The title set vertically, one character per line; clicking it expands too.
        let label = ui
            .add(
                egui::Label::new(egui::RichText::new(vertical(title)).weak())
                    .sense(egui::Sense::click()),
            )
            .on_hover_text(&tip);
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

/// `text` with one character per line, for a label in the narrow strip. Spaces are dropped:
/// a blank line would read as two labels.
fn vertical(text: &str) -> String {
    let chars: Vec<String> = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(String::from)
        .collect();
    chars.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_puts_one_character_per_line() {
        assert_eq!(vertical("解析"), "解\n析");
        assert_eq!(vertical("3D 編集"), "3\nD\n編\n集");
        assert_eq!(vertical("A"), "A");
        assert_eq!(vertical(""), "");
    }

    #[test]
    fn the_panel_is_named_after_what_it_holds() {
        assert_eq!(title(ViewMode::TwoD), "解析");
        assert_eq!(title(ViewMode::ThreeD), "3D 編集");
        assert_eq!(title(ViewMode::Assembly), "集合体");
    }
}
