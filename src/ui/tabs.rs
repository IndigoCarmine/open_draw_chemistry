//! The strip of open documents above the canvas: the active tab is a white card, as in the design.

use egui::{RichText, vec2};

use super::Command;
use crate::document::Document;

pub fn tab_strip(ui: &mut egui::Ui, docs: &[Document], active: usize, out: &mut Vec<Command>) {
    egui::ScrollArea::horizontal().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let visuals = ui.visuals().clone();
            for (i, doc) in docs.iter().enumerate() {
                let title = if doc.is_dirty() {
                    format!("{} ●", doc.title())
                } else {
                    doc.title()
                };
                let is_active = i == active;
                let text = if is_active {
                    RichText::new(title).color(visuals.widgets.noninteractive.fg_stroke.color)
                } else {
                    RichText::new(title).color(crate::theme::muted(visuals.dark_mode))
                };
                let mut button = egui::Button::new(text.size(12.5)).min_size(vec2(0.0, 30.0));
                button = if is_active {
                    button
                        .fill(visuals.extreme_bg_color)
                        .stroke(visuals.widgets.noninteractive.bg_stroke)
                } else {
                    button.frame(false)
                };
                let tab = ui.add(button);
                if tab.clicked() {
                    out.push(Command::SelectTab(i));
                }
                if tab.middle_clicked() {
                    out.push(Command::CloseTab(i));
                }
                if let Some(path) = &doc.path {
                    tab.on_hover_text(path.display().to_string());
                }
                let close = ui.add(egui::Button::new(RichText::new("×").size(13.0)).frame(false));
                if close.on_hover_text("閉じる").clicked() {
                    out.push(Command::CloseTab(i));
                }
                ui.add_space(6.0);
            }
            if ui
                .add(egui::Button::new(RichText::new("＋").size(14.0)).frame(false))
                .on_hover_text("新しい文書")
                .clicked()
            {
                out.push(Command::New);
            }
        });
    });
}
