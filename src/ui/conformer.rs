//! The 配座探索 (conformational search) window: search the 3D model's conformers in the
//! background, list them lowest energy first, put one into the model with a click (one undoable
//! step), and save them all as an SD file.

use std::time::Duration;

use egui::{Color32, RichText, vec2};

use crate::conformer::{Method, Run, Settings};
use crate::document::Document;
use crate::scan::run::format_duration;
use crate::theme;
use crate::three_d::{ENERGY_UNIT, ForceField};

/// The window's state (not saved).
#[derive(Debug, Default)]
pub struct ConformerWindow {
    pub open: bool,
    /// The settings as being edited; the force field is the document's until changed.
    settings: Settings,
    force_field_chosen: bool,
    /// The conformer last put into the model (its rank).
    applied: Option<usize>,
    /// The last problem or confirmation, and whether it is a problem.
    message: Option<(String, bool)>,
}

/// Keep every document's search collecting its conformers, also while the window is closed;
/// keeps frames coming while any runs.
pub fn poll(ctx: &egui::Context, docs: &mut [Document]) {
    let mut running = false;
    for doc in docs {
        if let Some(run) = &mut doc.conformers {
            run.poll();
            running |= run.is_running();
        }
    }
    if running {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

/// Show the window (when open). Returns a message for the status bar when a conformer was put
/// into the model.
pub fn window(
    ctx: &egui::Context,
    win: &mut ConformerWindow,
    doc: Option<&mut Document>,
) -> Option<String> {
    if !win.open {
        return None;
    }
    let mut open = true;
    let mut status = None;
    egui::Window::new(t!("conformer.window.title"))
        .open(&mut open)
        .default_size(vec2(460.0, 560.0))
        .min_width(360.0)
        .resizable(true)
        .collapsible(true)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                status = body(ui, win, doc);
            });
        });
    win.open = open;
    status
}

fn muted(ui: &egui::Ui, text: impl Into<String>) -> RichText {
    RichText::new(text.into())
        .size(11.0)
        .color(theme::muted(ui.visuals().dark_mode))
}

fn warn_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        theme::ORANGE_ON_DARK
    } else {
        theme::ORANGE_TEXT
    }
}

fn body(
    ui: &mut egui::Ui,
    win: &mut ConformerWindow,
    doc: Option<&mut Document>,
) -> Option<String> {
    ui.spacing_mut().item_spacing.y = 6.0;
    let Some(doc) = doc else {
        ui.label(muted(ui, t!("conformer.window.no_document")));
        return None;
    };
    if !win.force_field_chosen {
        win.settings.force_field = doc.three_d.force_field;
    }
    setup(ui, win, doc);
    if let Some((text, problem)) = &win.message {
        let text = RichText::new(text);
        ui.label(if *problem {
            text.color(warn_color(ui))
        } else {
            text
        });
    }
    ui.separator();
    results(ui, win, doc)
}

/// The settings, and starting or cancelling the search.
fn setup(ui: &mut egui::Ui, win: &mut ConformerWindow, doc: &mut Document) {
    let running = doc.conformers.as_ref().is_some_and(Run::is_running);
    let atoms = doc.three_d.model().map(|m| m.atoms.len());
    ui.label(muted(ui, t!("conformer.window.intro")));

    ui.add_enabled_ui(!running, |ui| {
        egui::Grid::new("conformer_settings")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                let s = &mut win.settings;
                ui.label(t!("conformer.window.method"));
                egui::ComboBox::from_id_salt("conformer_method")
                    .selected_text(s.method.label())
                    .show_ui(ui, |ui| {
                        for method in Method::ALL {
                            ui.selectable_value(&mut s.method, method, method.label())
                                .on_hover_text(method.tip());
                        }
                    })
                    .response
                    .on_hover_text(s.method.tip());
                ui.end_row();

                ui.label(t!("conformer.window.starts"));
                ui.add(egui::DragValue::new(&mut s.starts).range(1..=usize::MAX))
                    .on_hover_text(t!("conformer.window.starts_tip"));
                ui.end_row();

                ui.label(t!("conformer.window.rmsd"));
                ui.add(
                    egui::DragValue::new(&mut s.rmsd_threshold)
                        .range(0.01..=5.0)
                        .speed(0.01)
                        .max_decimals(2)
                        .suffix(" Å"),
                )
                .on_hover_text(t!("conformer.window.rmsd_tip"));
                ui.end_row();

                ui.label(t!("conformer.window.energy_window"));
                ui.add(
                    egui::DragValue::new(&mut s.energy_window)
                        .range(0.0..=1000.0)
                        .speed(0.1)
                        .max_decimals(1)
                        .suffix(format!(" {ENERGY_UNIT}")),
                )
                .on_hover_text(t!("conformer.window.energy_window_tip"));
                ui.end_row();

                ui.label(t!("conformer.window.force_field"));
                let before = s.force_field;
                egui::ComboBox::from_id_salt("conformer_force_field")
                    .selected_text(s.force_field.id())
                    .show_ui(ui, |ui| {
                        for ff in ForceField::ALL {
                            ui.selectable_value(&mut s.force_field, ff, ff.id());
                        }
                    });
                if s.force_field != before {
                    win.force_field_chosen = true;
                }
                ui.end_row();

                ui.label(t!("conformer.window.seed"));
                ui.add(egui::DragValue::new(&mut win.settings.seed).range(0..=u32::MAX as u64))
                    .on_hover_text(t!("conformer.window.seed_tip"));
                ui.end_row();
            });
    });

    let refused = match atoms {
        None => Some(t!("conformer.window.no_model").to_owned()),
        Some(_) => None,
    };
    if let Some(why) = &refused {
        ui.label(RichText::new(why).color(warn_color(ui)));
    } else if let Some(n) = atoms {
        ui.label(muted(ui, t!("conformer.window.summary", atoms = n)));
    }

    ui.horizontal(|ui| {
        if running {
            if ui.button(t!("common.cancel")).clicked()
                && let Some(run) = &mut doc.conformers
            {
                run.cancel();
            }
        } else {
            let ready = refused.is_none() && !doc.three_d.is_running();
            if ui
                .add_enabled(ready, egui::Button::new(t!("conformer.window.start")))
                .on_disabled_hover_text(t!("conformer.window.start_disabled_tip"))
                .clicked()
            {
                start(win, doc);
            }
        }
    });
}

fn start(win: &mut ConformerWindow, doc: &mut Document) {
    let Some(model) = doc.three_d.model() else {
        return;
    };
    match Run::start(model, win.settings) {
        Ok(run) => {
            doc.conformers = Some(run);
            win.applied = None;
            win.message = None;
        }
        Err(e) => win.message = Some((e, true)),
    }
}

/// The progress, the conformers found, and saving them.
fn results(ui: &mut egui::Ui, win: &mut ConformerWindow, doc: &mut Document) -> Option<String> {
    let Some(run) = &doc.conformers else {
        ui.label(muted(ui, t!("conformer.window.not_run")));
        return None;
    };
    let progress = run.progress();
    let found = run.conformers.len();
    if run.is_running() {
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            let mut text = t!(
                "conformer.window.progress",
                done = progress.done,
                total = progress.total,
                found = found,
                elapsed = format_duration(progress.elapsed)
            );
            if let Some(left) = progress.left {
                text.push_str(&t!(
                    "conformer.window.time_left",
                    left = format_duration(left)
                ));
            }
            ui.label(text);
        });
        ui.add(egui::ProgressBar::new(
            progress.done as f32 / progress.total.max(1) as f32,
        ));
    } else if let Some(e) = run.error() {
        ui.label(RichText::new(t!("conformer.window.failed", error = e)).color(warn_color(ui)));
    } else {
        let elapsed = format_duration(progress.elapsed);
        ui.label(if run.was_cancelled() {
            t!(
                "conformer.window.cancelled",
                found = found,
                done = progress.done,
                total = progress.total,
                elapsed = elapsed
            )
        } else {
            t!(
                "conformer.window.done",
                found = found,
                total = progress.total,
                elapsed = elapsed
            )
        });
    }
    if progress.failed > 0 {
        ui.label(muted(
            ui,
            t!("conformer.window.failed_starts", failed = progress.failed),
        ));
    }
    let fits = doc.three_d.model().is_some_and(|m| run.fits(m));
    if found > 0 && !fits {
        ui.label(RichText::new(t!("conformer.apply.model_changed")).color(warn_color(ui)));
    }
    if found == 0 {
        return None;
    }

    ui.horizontal(|ui| {
        ui.label(muted(ui, t!("conformer.window.click_to_apply")));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(
                    !run.is_running(),
                    egui::Button::new(t!("conformer.window.save_sdf")),
                )
                .on_hover_text(t!("conformer.window.save_sdf_tip"))
                .clicked()
            {
                win.message = Some(save_sdf(run));
            }
        });
    });

    let lowest = run.lowest().unwrap_or(0.0);
    let mut clicked = None;
    egui::Grid::new("conformer_list")
        .num_columns(4)
        .striped(true)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            ui.label(RichText::new("#").strong());
            ui.label(
                RichText::new(t!("conformer.window.relative_energy", unit = ENERGY_UNIT)).strong(),
            );
            ui.label(RichText::new(t!("conformer.window.energy", unit = ENERGY_UNIT)).strong());
            ui.label(RichText::new(t!("conformer.window.converged")).strong());
            ui.end_row();
            for (rank, c) in run.conformers.iter().enumerate() {
                let selected = win.applied == Some(rank);
                if ui
                    .add_enabled(
                        fits,
                        egui::Button::selectable(selected, format!("{}", rank + 1)),
                    )
                    .on_hover_text(t!("conformer.window.apply_tip"))
                    .clicked()
                {
                    clicked = Some(rank);
                }
                ui.label(format!("{:.2}", c.energy - lowest));
                ui.label(format!("{:.3}", c.energy));
                if c.converged {
                    ui.label("✔");
                } else {
                    ui.label(RichText::new("—").color(warn_color(ui)))
                        .on_hover_text(t!("conformer.window.not_converged_tip"));
                }
                ui.end_row();
            }
        });

    let rank = clicked?;
    let message = apply(doc, rank);
    win.applied = Some(rank);
    Some(message)
}

/// Put conformer `rank` into the model, as one undo step.
fn apply(doc: &mut Document, rank: usize) -> String {
    let Some(run) = &doc.conformers else {
        return String::new();
    };
    let Some(conformer) = run.conformers.get(rank) else {
        return String::new();
    };
    let lowest = run.lowest().unwrap_or(0.0);
    if !doc.three_d.model().is_some_and(|m| run.fits(m)) {
        return t!("conformer.apply.model_changed").into();
    }
    let Some((model, edit3d)) = doc.three_d.model_and_edit() else {
        return t!("conformer.apply.no_model").into();
    };
    edit3d.apply(
        model,
        conformer.coords.clone(),
        t!("conformer.apply.undo_label"),
    );
    t!(
        "conformer.apply.done",
        rank = rank + 1,
        energy = format!("{:.2}", conformer.energy - lowest),
        unit = ENERGY_UNIT
    )
}

fn save_sdf(run: &Run) -> (String, bool) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("SD File", &["sdf"])
        .set_file_name("conformers.sdf")
        .save_file()
    else {
        return (t!("conformer.export.cancelled").into(), false);
    };
    match std::fs::write(&path, run.to_sdf()) {
        Ok(()) => (t!("conformer.export.done", path = path.display()), false),
        Err(e) => (t!("conformer.export.failed", error = e), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::three_d::Job;
    use chembuider_rs::Config;

    /// The window draws before, during and after a search, and clicking a conformer puts it
    /// into the model as an undoable step.
    #[test]
    fn the_window_draws_and_a_conformer_is_applied() {
        let mut doc = Document::new(&Config::default(), 1);
        let model = crate::conformer::tests::butane(ForceField::Uff);
        doc.three_d = Job::with_model(model.clone(), &doc.editor.molecule.clone());
        let mut win = ConformerWindow {
            open: true,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let draw = |doc: &mut Document, win: &mut ConformerWindow| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                window(&ui.ctx().clone(), win, Some(doc));
            });
            output.textures_delta.clear();
        };
        draw(&mut doc, &mut win);

        win.settings.starts = 10;
        start(&mut win, &mut doc);
        assert!(doc.conformers.as_ref().is_some_and(Run::is_running));
        draw(&mut doc, &mut win);
        doc.conformers.as_mut().unwrap().wait();
        draw(&mut doc, &mut win);

        let run = doc.conformers.as_ref().unwrap();
        let last = run.conformers.len() - 1;
        let wanted = run.conformers[last].coords.clone();
        let message = apply(&mut doc, last);
        assert!(!message.is_empty());
        let applied: Vec<_> = doc
            .three_d
            .model()
            .unwrap()
            .atoms
            .iter()
            .map(|a| a.pos)
            .collect();
        assert_eq!(applied, wanted);
        assert!(doc.three_d.edit.can_undo());
        draw(&mut doc, &mut win);
    }
}
