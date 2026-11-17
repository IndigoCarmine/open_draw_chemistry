//! The panel on the right. It follows the view: with the drawing it holds the analysis of the
//! selection — formula, masses, isotope pattern and elemental analysis ([`analysis`]; nothing
//! is analysed while nothing is selected) — and with the 3D model the 3D 編集 controls
//! ([`super::view3d::edit_panel`]).
//!
//! The panel can be collapsed into a narrow strip on the right edge; the app keeps that state
//! as `show_properties` and toggles it when either function here returns `true`.

use std::collections::HashSet;

use chembuider_rs::widget::analysis::AnalysisPanel;
use chembuider_rs::{Config, Molecule};
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
        ViewMode::TwoD => t!("properties.title.analysis"),
        ViewMode::ThreeD => t!("properties.title.edit_3d"),
        ViewMode::Assembly => t!("properties.title.assembly"),
    }
}

/// The atoms the 解析 panel analyses: the selection, or `None` while nothing is selected. The
/// whole drawing is never analysed: on a big file that is both noise and work on every frame.
pub fn analysed_selection(selection: &HashSet<u32>) -> Option<&HashSet<u32>> {
    (!selection.is_empty()).then_some(selection)
}

/// Draws the 解析 panel's contents for the structures of `mol` with a selected atom: the
/// library's analysis and the SMILES row. With nothing selected only the heading and a hint
/// are drawn, and nothing is computed.
pub fn analysis(
    ui: &mut egui::Ui,
    panel: &mut AnalysisPanel,
    mol: &Molecule,
    selection: &HashSet<u32>,
    config: &Config,
) {
    let Some(selection) = analysed_selection(selection) else {
        ui.heading(title(ViewMode::TwoD));
        ui.weak(t!("properties.select_hint"));
        return;
    };
    // The panel reports a click on its copy button; the status bar is not told here.
    let _ = panel.ui(ui, mol, selection);
    crate::ui::smiles_row::show(ui, mol, selection, config);
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
    collapse_button(&mut corner, Icon::ChevronsRight, t!("properties.collapse"))
}

/// Draws the collapsed strip of the panel named `title`. Returns `true` when it asks to
/// expand the panel again.
pub fn collapsed(ui: &mut egui::Ui, title: &str) -> bool {
    let tip = t!("properties.expand", title = title);
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

    /// Every text laid out by one headless frame of the 解析 panel.
    fn analysis_texts(mol: &Molecule, selection: &HashSet<u32>) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut panel = AnalysisPanel::default();
        let config = Config::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            analysis(ui, &mut panel, mol, selection, &config);
        });
        // The font atlas is not uploaded anywhere.
        output.textures_delta.clear();
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        texts
    }

    fn collect_texts(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_texts(s, out)),
            _ => {}
        }
    }

    /// Ethane, and the id of one of its carbons.
    fn ethane() -> (Molecule, u32) {
        let mut mol = Molecule::default();
        let a = mol.add_atom("C".into(), [0.0, 0.0], 0);
        let b = mol.add_atom("C".into(), [1.5, 0.0], 0);
        mol.add_bond(a, b, chembuider_rs::BondOrder::Single);
        (mol, a)
    }

    #[test]
    fn nothing_is_analysed_without_a_selection() {
        assert_eq!(analysed_selection(&HashSet::new()), None);
        let selection: HashSet<u32> = [3, 7].into();
        assert_eq!(analysed_selection(&selection), Some(&selection));
    }

    #[test]
    fn the_analysis_panel_only_shows_a_hint_until_something_is_selected() {
        let (mol, a) = ethane();
        let hint = t!("properties.select_hint");
        assert_eq!(hint, "Select a structure to analyse it");

        // Nothing selected: the heading and the hint, and no value of the drawing.
        let texts = analysis_texts(&mol, &HashSet::new());
        assert_eq!(texts, ["Analysis", hint]);

        // One atom selected: the analysis of its structure, and no hint.
        let texts = analysis_texts(&mol, &[a].into());
        assert!(!texts.iter().any(|t| t == hint), "{texts:?}");
        assert!(texts.iter().any(|t| t == "SMILES"), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("30.0")), "{texts:?}");
    }

    #[test]
    fn the_hint_is_translated() {
        let _lang = crate::i18n::ScopedLanguage::new(crate::i18n::Lang::Ja);
        assert_eq!(t!("properties.select_hint"), "構造を選択すると解析します");
    }

    #[test]
    fn the_panel_is_named_after_what_it_holds() {
        assert_eq!(title(ViewMode::TwoD), "Analysis");
        assert_eq!(title(ViewMode::ThreeD), "3D editing");
        assert_eq!(title(ViewMode::Assembly), "Assembly");
        let _lang = crate::i18n::ScopedLanguage::new(crate::i18n::Lang::Ja);
        assert_eq!(title(ViewMode::TwoD), "解析");
        assert_eq!(title(ViewMode::ThreeD), "3D 編集");
        assert_eq!(title(ViewMode::Assembly), "集合体");
    }
}
