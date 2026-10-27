//! The 3D view beside (or instead of) the drawing: the controls that generate and optimise the
//! document's 3D model (see [`crate::three_d`]), and a summary of the model. The viewer that
//! renders it (moleucle_3dview_rs) comes later; until then the body describes the model in text.

use egui::{Color32, RichText, vec2};

use super::Command;
use crate::document::Document;
use crate::theme::{self, dark};
use crate::three_d::{self, ForceField, JobState};

pub fn view3d(ui: &mut egui::Ui, title: &str, doc: Option<&Document>, out: &mut Vec<Command>) {
    // Dark in both themes, as in the design: a 3D model reads best on a dark ground.
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0.0, theme::DARK_SURFACE);
    ui.visuals_mut().override_text_color = Some(dark::TEXT);

    let job = doc.map(|d| &d.three_d);
    let model = job.and_then(|j| j.model());
    let running = job.is_some_and(|j| j.is_running());
    let stale = doc.is_some_and(|d| d.three_d.is_stale(&d.editor.molecule));

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
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() / 2.0 - 60.0).max(0.0));
                summary(ui, job.map(|j| &j.state), stale);
            });
        });
    });

    separator(ui);
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 0))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(three_d::AVAILABLE && doc.is_some() && !running, |ui| {
                    ui.label(RichText::new("力場").size(11.0).color(dark::MUTED));
                    let current = job.map(|j| j.force_field).unwrap_or_default();
                    let mut chosen = current;
                    egui::ComboBox::from_id_salt("force_field")
                        .selected_text(chosen.id())
                        .show_ui(ui, |ui| {
                            for ff in ForceField::ALL {
                                ui.selectable_value(&mut chosen, ff, ff.id());
                            }
                        });
                    if chosen != current {
                        out.push(Command::SetForceField(chosen));
                    }
                    if ui.button("2D から再生成").clicked() {
                        out.push(Command::Generate3d);
                    }
                    let optimize = ui.add_enabled(
                        model.is_some(),
                        egui::Button::new(RichText::new("構造最適化").color(Color32::WHITE))
                            .fill(theme::ORANGE_DEEP),
                    );
                    if optimize.clicked() {
                        out.push(Command::Optimize3d);
                    }
                });
                // Kept on one line: the wrapping row moves it down whole instead.
                let energy = match model {
                    Some(m) => format!("E = {:.1} {}", m.energy, m.energy_unit),
                    None => "E = — kcal/mol".to_owned(),
                };
                ui.add(
                    egui::Label::new(
                        RichText::new(energy)
                            .monospace()
                            .size(11.0)
                            .color(dark::MUTED),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
                if let Some(m) = model {
                    ui.label(
                        RichText::new(format!("{} 原子", m.atoms.len()))
                            .size(11.0)
                            .color(dark::MUTED),
                    );
                }
            });
        });
}

/// The panel body: what there is to show of the model, or why there is none.
fn summary(ui: &mut egui::Ui, state: Option<&JobState>, stale: bool) {
    let muted = |text: &str| RichText::new(text).size(13.0).color(dark::MUTED);
    if !three_d::AVAILABLE {
        ui.label(muted(
            "この版では 3D 生成を利用できません
OpenBabel を有効にしてビルドすると使えます",
        ));
        return;
    }
    match state {
        None | Some(JobState::Idle) => {
            ui.label(muted(
                "「2D から再生成」で 2D の構造から 3D モデルを生成します",
            ));
        }
        Some(JobState::Running(task)) => {
            ui.spinner();
            ui.label(muted(task.running_label()));
        }
        Some(JobState::Failed(message)) => {
            ui.label(
                RichText::new(message)
                    .size(13.0)
                    .color(theme::ORANGE_ON_DARK),
            );
            ui.label(muted("構造を確認して「2D から再生成」でやり直してください"));
        }
        Some(JobState::Done(model)) => {
            let row = |ui: &mut egui::Ui, key: &str, value: String| {
                ui.label(RichText::new(key).size(12.0).color(dark::MUTED));
                ui.label(RichText::new(value).size(13.0));
                ui.end_row();
            };
            egui::Grid::new("model_summary")
                .num_columns(2)
                .spacing(vec2(16.0, 6.0))
                .show(ui, |ui| {
                    row(ui, "分子式", model.formula.clone());
                    row(
                        ui,
                        "原子数",
                        format!("{}（うち H {}）", model.atoms.len(), model.hydrogen_count()),
                    );
                    row(
                        ui,
                        "エネルギー",
                        format!("{:.1} {}", model.energy, model.energy_unit),
                    );
                    row(ui, "力場", model.force_field.id().to_owned());
                });
            ui.add_space(10.0);
            if stale {
                ui.label(
                    RichText::new("2D の構造が変更されています。「2D から再生成」で更新できます")
                        .size(12.0)
                        .color(theme::ORANGE_ON_DARK),
                );
            }
            ui.label(muted("3D ビューアは準備中です"));
        }
    }
}

fn separator(ui: &mut egui::Ui) {
    let y = ui.cursor().top();
    let x = ui.max_rect().x_range();
    ui.painter()
        .hline(x, y, egui::Stroke::new(1.0, dark::BORDER));
    ui.add_space(6.0);
}
