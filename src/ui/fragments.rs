//! 断片の管理: lists the fragments, and adds, renames and deletes the user's own.
//!
//! The window only draws and reports a [`FragmentAction`]; the app carries it out against the
//! [`crate::fragments::FragmentStore`] and pushes the new fragment list into every editor.

use std::sync::Arc;

use chembuider_rs::config::{FragmentDef, StyleConfig};
use chembuider_rs::widget::scene::{MoleculeScene, RenderTransform};
use chembuider_rs::widget::{DEFAULT_BOND_LENGTH, SCALE_FACTOR};
use egui::{Color32, Rect, Stroke, vec2};

use crate::fragments::{Entry, Source, check_name};

/// What the user asked the manager to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FragmentAction {
    /// Save the active document's selection under this name.
    AddFromSelection(String),
    Delete(String),
    Rename {
        from: String,
        to: String,
    },
    OpenFolder,
}

/// The window's state between frames.
#[derive(Default)]
pub struct FragmentManager {
    pub open: bool,
    filter: String,
    new_name: String,
    /// The user fragment whose row is being renamed, and the name typed so far.
    renaming: Option<(String, String)>,
    /// The user fragment whose deletion awaits confirmation.
    confirm_delete: Option<String>,
    /// The last action's outcome: `Ok` message or `Err` error.
    result: Option<Result<String, String>>,
}

/// What the window needs from the app this frame.
pub struct ManagerContext<'a> {
    pub entries: &'a [Entry],
    pub style: &'a StyleConfig,
    /// How many atoms the active document has selected (`None` without a document).
    pub selected: Option<usize>,
    pub dir: Option<&'a std::path::Path>,
}

const THUMB: egui::Vec2 = egui::vec2(140.0, 88.0);

impl FragmentManager {
    /// Report the outcome of the action the window last returned.
    pub fn set_result(&mut self, result: Result<String, String>) {
        if result.is_ok() {
            self.renaming = None;
            self.confirm_delete = None;
        }
        self.result = Some(result);
    }

    /// Clear the add field after a successful add.
    pub fn clear_new_name(&mut self) {
        self.new_name.clear();
    }

    pub fn show(&mut self, ctx: &egui::Context, cx: &ManagerContext<'_>) -> Option<FragmentAction> {
        let mut action = None;
        let mut open = self.open;
        egui::Window::new(t!("fragments.manager.title"))
            .open(&mut open)
            .default_size([620.0, 640.0])
            .resizable(true)
            .show(ctx, |ui| action = self.contents(ui, cx));
        self.open = open;
        action
    }

    fn contents(&mut self, ui: &mut egui::Ui, cx: &ManagerContext<'_>) -> Option<FragmentAction> {
        let mut action = None;
        ui.label(t!("fragments.manager.usage"));
        ui.add_space(4.0);

        // ── 追加 ──────────────────────────────────────────────────────────
        ui.group(|ui| {
            ui.strong(t!("fragments.manager.add_heading"));
            ui.horizontal(|ui| {
                ui.label(t!("fragments.manager.name"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_name)
                        .hint_text(t!("fragments.manager.name_hint"))
                        .desired_width(160.0),
                );
                let name = self.new_name.trim();
                let existing = cx.entries.iter().find(|e| e.def.name == name);
                let selected = cx.selected.unwrap_or(0);
                let can_add = selected > 0 && !name.is_empty();
                let label = if existing.is_some() {
                    t!("fragments.manager.overwrite")
                } else {
                    t!("fragments.manager.add")
                };
                if ui.add_enabled(can_add, egui::Button::new(label)).clicked() {
                    action = Some(FragmentAction::AddFromSelection(name.to_owned()));
                }
            });
            let name = self.new_name.trim();
            match cx.selected {
                None => {
                    ui.weak(t!("fragments.manager.no_document"));
                }
                Some(0) => {
                    ui.weak(t!("fragments.manager.select_atoms"));
                }
                Some(n) => {
                    ui.weak(tn!("fragments.manager.selected_atoms", n));
                }
            }
            if !name.is_empty() {
                if let Err(e) = check_name(name) {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                } else if let Some(e) = cx.entries.iter().find(|e| e.def.name == name) {
                    let note = match e.source {
                        Source::Builtin => t!("fragments.manager.replaces_builtin", name = name),
                        _ => t!("fragments.manager.overwrites_user", name = name),
                    };
                    ui.colored_label(ui.visuals().warn_fg_color, note);
                }
            }
        });

        match &self.result {
            Some(Ok(msg)) => {
                ui.label(msg);
            }
            Some(Err(msg)) => {
                ui.colored_label(ui.visuals().error_fg_color, msg);
            }
            None => {}
        }

        ui.horizontal(|ui| {
            ui.label(t!("fragments.manager.search"));
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text(t!("fragments.manager.search_hint"))
                    .desired_width(160.0),
            );
            if let Some(dir) = cx.dir
                && ui
                    .button(t!("fragments.manager.open_folder"))
                    .on_hover_text(dir.display().to_string())
                    .clicked()
            {
                action = Some(FragmentAction::OpenFolder);
            }
        });
        ui.separator();

        let needle = self.filter.trim().to_lowercase();
        let rows: Vec<&Entry> = cx
            .entries
            .iter()
            .filter(|e| needle.is_empty() || e.def.name.to_lowercase().contains(&needle))
            .collect();
        let row_height = THUMB.y + ui.spacing().item_spacing.y;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, row_height, rows.len(), |ui, range| {
                for entry in &rows[range] {
                    if let Some(a) = self.row(ui, entry, cx.style) {
                        action = Some(a);
                    }
                }
            });
        if rows.is_empty() {
            ui.weak(t!("fragments.manager.no_match"));
        }
        action
    }

    fn row(
        &mut self,
        ui: &mut egui::Ui,
        entry: &Entry,
        style: &StyleConfig,
    ) -> Option<FragmentAction> {
        let mut action = None;
        let name = &entry.def.name;
        ui.horizontal(|ui| {
            ui.set_height(THUMB.y);
            let (rect, _) = ui.allocate_exact_size(THUMB, egui::Sense::hover());
            thumbnail(ui, rect, &entry.def, style);

            ui.vertical(|ui| {
                let mut cancel = false;
                if let Some((old, new)) = self.renaming.as_mut().filter(|(old, _)| old == name) {
                    ui.horizontal(|ui| {
                        let edit = ui.add(egui::TextEdit::singleline(new).desired_width(120.0));
                        let enter =
                            edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button(t!("fragments.manager.rename_apply")).clicked() || enter {
                            action = Some(FragmentAction::Rename {
                                from: old.clone(),
                                to: new.trim().to_owned(),
                            });
                        }
                        cancel = ui.button(t!("common.cancel")).clicked();
                    });
                } else {
                    ui.strong(name);
                }
                if cancel {
                    self.renaming = None;
                }
                ui.horizontal(|ui| {
                    ui.weak(tn!("fragments.manager.atom_count", entry.def.atoms.len()));
                    ui.weak("·");
                    ui.weak(entry.source.label());
                });
            });

            if !entry.source.is_user() {
                return;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.confirm_delete.as_deref() == Some(name.as_str()) {
                    if ui.button(t!("common.cancel")).clicked() {
                        self.confirm_delete = None;
                    }
                    let delete = egui::Button::new(
                        egui::RichText::new(t!("fragments.manager.confirm_delete"))
                            .color(ui.visuals().error_fg_color),
                    );
                    if ui.add(delete).clicked() {
                        action = Some(FragmentAction::Delete(name.clone()));
                    }
                    ui.label(t!("fragments.manager.delete_question"));
                } else {
                    if ui.button(t!("fragments.manager.delete")).clicked() {
                        self.confirm_delete = Some(name.clone());
                        self.renaming = None;
                    }
                    if ui.button(t!("fragments.manager.rename")).clicked() {
                        self.renaming = Some((name.clone(), name.clone()));
                        self.confirm_delete = None;
                    }
                }
            });
        });
        action
    }
}

fn thumbnail_transform(
    rect: Rect,
    min: [f32; 2],
    max: [f32; 2],
    style: &StyleConfig,
) -> RenderTransform {
    let inner = rect.shrink(9.0);
    let span = vec2((max[0] - min[0]).max(1e-3), (max[1] - min[1]).max(1e-3));
    // At most the canvas's own scale at 40 % zoom: a lone atom is not blown up.
    let coord_scale = (inner.width() / span.x)
        .min(inner.height() / span.y)
        .min(SCALE_FACTOR * 0.4);
    // Scene labels use label_size * visual_scale; bonds use molecule units * coord_scale.
    // Keep the readability floor only while a label fits within a standard bond.
    let label_scale_limit = DEFAULT_BOND_LENGTH * coord_scale / style.label_size;
    let readability_floor = 0.55_f32.min(label_scale_limit);
    let visual_scale = (coord_scale / SCALE_FACTOR)
        .max(readability_floor)
        .min(label_scale_limit);
    let mid = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
    RenderTransform {
        coord_scale,
        offset: rect.center().to_vec2() - vec2(mid[0], mid[1]) * coord_scale,
        visual_scale,
    }
}

/// A small drawing of `def` in `rect`, laid out by chembuider-rs's scene builder (the same
/// geometry as the canvas), with the attach atom ringed in orange.
fn thumbnail(ui: &egui::Ui, rect: Rect, def: &FragmentDef, style: &StyleConfig) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::WHITE);
    painter.rect_stroke(
        rect,
        4.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
    let mol = def.to_fragment().to_molecule();
    let Some((min, max)) = mol
        .atoms
        .iter()
        .fold(None, |acc: Option<([f32; 2], [f32; 2])>, a| {
            let (mut lo, mut hi) = acc.unwrap_or((a.pos, a.pos));
            for k in 0..2 {
                lo[k] = lo[k].min(a.pos[k]);
                hi[k] = hi[k].max(a.pos[k]);
            }
            Some((lo, hi))
        })
    else {
        return;
    };
    let xf = thumbnail_transform(rect, min, max, style);
    let scene = MoleculeScene::build(&mol, style, xf, |job| -> Arc<egui::Galley> {
        painter.layout_job(job)
    });
    for poly in scene.ring_fills {
        painter.add(egui::Shape::convex_polygon(
            poly.points,
            poly.color,
            Stroke::NONE,
        ));
    }
    for bond in &scene.bonds {
        for s in &bond.strokes {
            painter.line_segment([s.a, s.b], Stroke::new(s.width, Color32::BLACK));
        }
        for fill in &bond.fills {
            painter.add(egui::Shape::convex_polygon(
                fill.clone(),
                Color32::BLACK,
                Stroke::NONE,
            ));
        }
    }
    for label in scene.labels.values() {
        painter.rect_filled(label.rect.expand(scene.label_bg_pad), 0.0, Color32::WHITE);
        label.galley.paint(&painter, label.rect.min);
    }
    if let Some(attach) = mol.atoms.get(def.attach_idx) {
        painter.circle_stroke(
            xf.apply(attach.pos),
            5.0,
            Stroke::new(1.5, Color32::from_rgb(255, 140, 0)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fragments::FragmentStore;

    fn builtin_bounds(name: &str) -> ([f32; 2], [f32; 2]) {
        let dir = std::env::temp_dir().join(format!("odc-thumb-{}", std::process::id()));
        let entries = FragmentStore::at(dir).entries();
        let def = &entries
            .iter()
            .find(|entry| entry.def.name == name)
            .unwrap_or_else(|| panic!("missing built-in {name}"))
            .def;
        let mol = def.to_fragment().to_molecule();
        let mut min = mol.atoms[0].pos;
        let mut max = min;
        for atom in &mol.atoms {
            for k in 0..2 {
                min[k] = min[k].min(atom.pos[k]);
                max[k] = max[k].max(atom.pos[k]);
            }
        }
        (min, max)
    }

    #[test]
    fn thumbnail_labels_do_not_exceed_bond_length() {
        for name in ["Ph", "Boc", "CO2Me", "TMS"] {
            let (min, max) = builtin_bounds(name);
            for size in [THUMB, vec2(24.0, 24.0)] {
                for label_size in [StyleConfig::default().label_size, 200.0] {
                    let style = StyleConfig {
                        label_size,
                        ..StyleConfig::default()
                    };
                    let xf = thumbnail_transform(
                        Rect::from_min_size(egui::Pos2::ZERO, size),
                        min,
                        max,
                        &style,
                    );
                    let label_height = style.label_size * xf.visual_scale;
                    let bond_length = DEFAULT_BOND_LENGTH * xf.coord_scale;
                    assert!(
                        label_height <= bond_length + 1e-5,
                        "{name}: label {label_height} exceeds bond {bond_length}"
                    );
                }
            }
        }
    }

    #[test]
    fn single_atom_thumbnail_does_not_enlarge_labels() {
        let (min, max) = builtin_bounds("Me");
        assert_eq!(min, max);
        let old_inner = Rect::from_min_size(egui::Pos2::ZERO, vec2(72.0, 44.0)).shrink(9.0);
        let old_coord_scale = (old_inner.width() / (max[0] - min[0]).max(1e-3))
            .min(old_inner.height() / (max[1] - min[1]).max(1e-3))
            .min(SCALE_FACTOR * 0.4);
        let old_visual_scale = (old_coord_scale / SCALE_FACTOR).max(0.55);
        let xf = thumbnail_transform(
            Rect::from_min_size(egui::Pos2::ZERO, THUMB),
            min,
            max,
            &StyleConfig::default(),
        );
        assert!(xf.visual_scale <= old_visual_scale);
    }

    /// The window, with every built-in thumbnail, draws headlessly without panicking.
    #[test]
    fn the_window_draws_every_builtin_fragment() {
        let dir = std::env::temp_dir().join(format!("odc-ui-frag-{}", std::process::id()));
        let entries = FragmentStore::at(&dir).entries();
        let style = StyleConfig::default();
        let cx = ManagerContext {
            entries: &entries,
            style: &style,
            selected: Some(0),
            dir: Some(&dir),
        };
        let mut manager = FragmentManager {
            open: true,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 4000.0))),
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw, |ui| {
                assert_eq!(manager.show(ui.ctx(), &cx), None);
            });
            output.textures_delta.clear();
        }
        assert!(manager.open);
        assert!(!dir.exists(), "drawing writes nothing");
    }
}
