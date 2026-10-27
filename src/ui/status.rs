//! The status bar along the bottom of the window.

use chembuider_rs::reaction::MolecularFormula;

use super::palette::tool_name;
use crate::document::Document;

pub fn status_bar(ui: &mut egui::Ui, doc: Option<&Document>, message: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        let muted = crate::theme::muted(ui.visuals().dark_mode);
        let strong = ui.visuals().widgets.noninteractive.fg_stroke.color;
        let pair = |ui: &mut egui::Ui, key: &str, value: &str| {
            let mut job = egui::text::LayoutJob::default();
            let font = egui::FontId::proportional(12.0);
            job.append(
                key,
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color: muted,
                    ..Default::default()
                },
            );
            job.append(
                value,
                0.0,
                egui::TextFormat {
                    font_id: font,
                    color: strong,
                    ..Default::default()
                },
            );
            ui.label(job);
        };
        let small = |text: String| egui::RichText::new(text).size(12.0).color(muted);

        if let Some(doc) = doc {
            let editor = &doc.editor;
            pair(ui, "ツール：", tool_name(editor));
            pair(ui, "元素：", &editor.current_element);
            ui.label(small(format!(
                "原子 {} · 結合 {}",
                editor.molecule.atoms.len(),
                editor.molecule.bonds.len()
            )));
            let formula = MolecularFormula::of(&editor.molecule);
            if !formula.is_empty() {
                ui.label(small(formula.plain()));
            }
        }
        ui.label(small(message.to_owned()));
        if let Some(doc) = doc {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mono = |text: String| {
                    egui::RichText::new(text)
                        .monospace()
                        .size(11.5)
                        .color(muted)
                };
                // The widget's default zoom (1.0) is its natural size.
                ui.label(mono(format!("{:.0}%", doc.editor.zoom * 100.0)));
                ui.label(mono(doc.format.extension().to_ascii_uppercase()));
            });
        }
    });
}
