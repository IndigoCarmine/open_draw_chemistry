//! The panel on the right. Per the design it holds the analysis of the drawing (or of the
//! selection): formula, masses, isotope pattern and elemental analysis.

use chembuider_rs::widget::analysis::AnalysisPanel;

use crate::document::Document;

pub fn properties(ui: &mut egui::Ui, doc: &Document, analysis: &mut AnalysisPanel) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        // The panel reports a click on its copy button; the status bar is not told here.
        let _ = analysis.ui(ui, &doc.editor.molecule, &doc.editor.selected_atoms);
    });
}
