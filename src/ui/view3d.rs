//! The 3D view beside (or instead of) the drawing. The 3D builder comes in a later phase
//! (OpenBabel for coordinates, moleucle_3dview_rs for rendering); until then the panel keeps
//! the design's place and controls, disabled.

use egui::{Color32, RichText, vec2};

use crate::theme::{self, dark};

/// Force fields the 3D builder will offer.
const FORCE_FIELDS: [&str; 4] = ["MMFF94", "MMFF94s", "UFF", "GAFF"];

pub fn view3d(ui: &mut egui::Ui, title: &str) {
    // Dark in both themes, as in the design: a 3D model reads best on a dark ground.
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0.0, theme::DARK_SURFACE);
    ui.visuals_mut().override_text_color = Some(dark::TEXT);

    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(RichText::new(format!("3D：{title}")).size(12.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(12.0);
            ui.label(
                RichText::new("ボール＆スティック")
                    .size(11.0)
                    .color(dark::MUTED),
            );
        });
    });
    separator(ui);

    // The footer wraps on a narrow panel, so its height is measured rather than assumed.
    let footer = 64.0;
    let body = vec2(
        ui.available_width(),
        (ui.available_height() - footer).max(120.0),
    );
    ui.allocate_ui(body, |ui| {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new(
                    "3D ビューは準備中です
2D の構造から 3D モデルを生成して表示します",
                )
                .size(13.0)
                .color(dark::MUTED),
            );
        });
    });

    separator(ui);
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 0))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(false, |ui| {
                    ui.label(RichText::new("力場").size(11.0).color(dark::MUTED));
                    let mut chosen = FORCE_FIELDS[0];
                    egui::ComboBox::from_id_salt("force_field")
                        .selected_text(chosen)
                        .show_ui(ui, |ui| {
                            for ff in FORCE_FIELDS {
                                ui.selectable_value(&mut chosen, ff, ff);
                            }
                        });
                    let _ = ui.button("2D から再生成");
                    let _ = ui.add(
                        egui::Button::new(RichText::new("構造最適化").color(Color32::WHITE))
                            .fill(theme::ORANGE_DEEP),
                    );
                });
                // Kept on one line: the wrapping row moves it down whole instead.
                ui.add(
                    egui::Label::new(
                        RichText::new("E = — kcal/mol")
                            .monospace()
                            .size(11.0)
                            .color(dark::MUTED),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
            });
        });
}

fn separator(ui: &mut egui::Ui) {
    let y = ui.cursor().top();
    let x = ui.max_rect().x_range();
    ui.painter()
        .hline(x, y, egui::Stroke::new(1.0, dark::BORDER));
    ui.add_space(6.0);
}
