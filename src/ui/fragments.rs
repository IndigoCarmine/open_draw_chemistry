//! 断片の管理: lists the fragments, and adds, renames and deletes the user's own.
//!
//! The window only draws and reports a [`FragmentAction`]; the app carries it out against the
//! [`crate::fragments::FragmentStore`] and pushes the new fragment list into every editor.

use std::sync::Arc;

use chembuider_rs::config::{FragmentDef, StyleConfig};
use chembuider_rs::widget::SCALE_FACTOR;
use chembuider_rs::widget::scene::{MoleculeScene, RenderTransform};
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

const THUMB: egui::Vec2 = egui::vec2(72.0, 44.0);

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
        egui::Window::new("断片の管理")
            .open(&mut open)
            .default_size([460.0, 560.0])
            .resizable(true)
            .show(ctx, |ui| action = self.contents(ui, cx));
        self.open = open;
        action
    }

    fn contents(&mut self, ui: &mut egui::Ui, cx: &ManagerContext<'_>) -> Option<FragmentAction> {
        let mut action = None;
        ui.label(
            "使い方: 原子を選んでラベルに断片名を入力し、x キーで展開します。\
             ユーザー断片は組み込みの同名の断片より優先されます。",
        );
        ui.add_space(4.0);

        // ── 追加 ──────────────────────────────────────────────────────────
        ui.group(|ui| {
            ui.strong("選択範囲から追加");
            ui.horizontal(|ui| {
                ui.label("名前");
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_name)
                        .hint_text("例: CO2tBu")
                        .desired_width(160.0),
                );
                let name = self.new_name.trim();
                let existing = cx.entries.iter().find(|e| e.def.name == name);
                let selected = cx.selected.unwrap_or(0);
                let can_add = selected > 0 && !name.is_empty();
                let label = if existing.is_some() {
                    "上書き保存"
                } else {
                    "追加"
                };
                if ui.add_enabled(can_add, egui::Button::new(label)).clicked() {
                    action = Some(FragmentAction::AddFromSelection(name.to_owned()));
                }
            });
            let name = self.new_name.trim();
            match cx.selected {
                None => {
                    ui.weak("文書が開かれていません");
                }
                Some(0) => {
                    ui.weak("選択ツールで断片にする原子を選択してください（結合点はホットスポットの原子、なければ最初に描いた原子）");
                }
                Some(n) => {
                    ui.weak(format!(
                        "選択中の原子 {n} 個（結合点はホットスポットの原子、なければ最初に描いた原子）"
                    ));
                }
            }
            if !name.is_empty() {
                if let Err(e) = check_name(name) {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                } else if let Some(e) = cx.entries.iter().find(|e| e.def.name == name) {
                    let note = match e.source {
                        Source::Builtin => format!("組み込みの「{name}」をこの断片で置き換えます"),
                        _ => format!("ユーザー断片「{name}」を上書きします"),
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
            ui.label("検索");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("断片名")
                    .desired_width(160.0),
            );
            if let Some(dir) = cx.dir
                && ui
                    .button("フォルダーを開く")
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
            ui.weak("一致する断片はありません");
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
                        if ui.button("変更").clicked() || enter {
                            action = Some(FragmentAction::Rename {
                                from: old.clone(),
                                to: new.trim().to_owned(),
                            });
                        }
                        cancel = ui.button("キャンセル").clicked();
                    });
                } else {
                    ui.strong(name);
                }
                if cancel {
                    self.renaming = None;
                }
                ui.horizontal(|ui| {
                    ui.weak(format!("{} 原子", entry.def.atoms.len()));
                    ui.weak("·");
                    ui.weak(entry.source.label());
                });
            });

            if !entry.source.is_user() {
                return;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.confirm_delete.as_deref() == Some(name.as_str()) {
                    if ui.button("キャンセル").clicked() {
                        self.confirm_delete = None;
                    }
                    let delete = egui::Button::new(
                        egui::RichText::new("削除する").color(ui.visuals().error_fg_color),
                    );
                    if ui.add(delete).clicked() {
                        action = Some(FragmentAction::Delete(name.clone()));
                    }
                    ui.label("削除しますか？");
                } else {
                    if ui.button("削除").clicked() {
                        self.confirm_delete = Some(name.clone());
                        self.renaming = None;
                    }
                    if ui.button("名前変更").clicked() {
                        self.renaming = Some((name.clone(), name.clone()));
                        self.confirm_delete = None;
                    }
                }
            });
        });
        action
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
    let inner = rect.shrink(9.0);
    let span = vec2((max[0] - min[0]).max(1e-3), (max[1] - min[1]).max(1e-3));
    // At most the canvas's own scale at 40 % zoom: a lone atom is not blown up.
    let coord_scale = (inner.width() / span.x)
        .min(inner.height() / span.y)
        .min(SCALE_FACTOR * 0.4);
    let mid = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
    let xf = RenderTransform {
        coord_scale,
        offset: rect.center().to_vec2() - vec2(mid[0], mid[1]) * coord_scale,
        // Labels stay readable even when the structure is shrunk a lot.
        visual_scale: (coord_scale / SCALE_FACTOR).max(0.55),
    };
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
