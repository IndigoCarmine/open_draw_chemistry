//! The 3D view, shown instead of the drawing: the viewer showing the document's 3D model
//! ([`super::viewport3d`]) in the central area ([`viewer_area`]), and in the right panel
//! ([`edit_panel`]) the tools for editing it by hand ([`crate::three_d::edit`]) and the
//! controls that generate, optimise and export it ([`crate::three_d`]).
//!
//! In the viewer: drag to orbit, Shift+drag or right/middle drag to pan, wheel to zoom,
//! double-click empty space to reframe. Click atoms to pick them (up to four: distance, angle,
//! dihedral), click a bond to pick its two ends, right-click an atom to fix it in place. With a
//! rotatable bond picked the wheel turns it instead of zooming.

use egui::{Color32, Pos2, RichText, Stroke, vec2};
use serde::{Deserialize, Serialize};

use super::Command;
use super::viewport3d::{Frame3d, Hit, Style3d, Viewport3d, exclusive_groups};
use crate::document::Document;
use crate::theme::{self, dark};
use crate::three_d::edit::{self, Measure, Target};
use crate::three_d::geom::{self, Clash, Goal};
use crate::three_d::{self, Constraint, Export3d, ForceField, JobState, Model3d};
use eframe::egui_wgpu::RenderState;

/// Degrees one wheel notch (or step button) turns a bond; Shift for fine, Ctrl for coarse.
const STEP: f64 = 5.0;
const STEP_FINE: f64 = 1.0;
const STEP_COARSE: f64 = 15.0;

/// Highlight colours in the viewer (linear-ish RGB, drawn translucent).
const PICK_COLOR: (f32, f32, f32) = (0.98, 0.45, 0.22);
const ANCHOR_COLOR: (f32, f32, f32) = (0.20, 0.75, 0.98);
const CLASH_COLOR: (f32, f32, f32) = (1.0, 0.12, 0.12);
const MOVING_COLOR: (f32, f32, f32) = (0.55, 0.62, 0.80);

const PICK_STROKE: Color32 = Color32::from_rgb(0xF5, 0x8A, 0x4F);
const ANCHOR_STROKE: Color32 = Color32::from_rgb(0x4C, 0xC2, 0xF5);
const CLASH_STROKE: Color32 = Color32::from_rgb(0xFF, 0x4D, 0x4D);
/// The same roles as text on the light theme's panel (4.5:1 on it).
const ANCHOR_ON_LIGHT: Color32 = Color32::from_rgb(0x1B, 0x6E, 0x9C);
const CLASH_ON_LIGHT: Color32 = Color32::from_rgb(0xB4, 0x23, 0x23);
/// On-screen labels are drawn on this so they read over any atom colour.
const LABEL_BACK: Color32 = Color32::from_rgba_premultiplied(0x14, 0x12, 0x10, 0xD8);

/// The 3D panel's state shared by every document: the viewer (one GPU target, reloaded when
/// the active model changes) and the display options. Only the style is remembered.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct View3d {
    pub style: Style3d,
    /// Show atom numbers on every atom, not only the picked ones.
    pub numbers: bool,
    #[serde(skip)]
    viewport: Option<Viewport3d>,
    /// Clashes of the geometry last shown, recomputed when the atoms move.
    #[serde(skip)]
    clashes: (Vec<[f64; 3]>, Vec<Clash>),
    /// The value box: the atoms it was filled for, and its text.
    #[serde(skip)]
    value: (Vec<usize>, String),
}

impl View3d {
    /// Free the viewer's GPU texture (at exit).
    pub fn free(&mut self, render_state: &RenderState) {
        if let Some(viewport) = &mut self.viewport {
            viewport.free(render_state);
        }
    }

    fn clashes_for(&mut self, model: &Model3d) -> &[Clash] {
        let coords = edit::coords(model);
        if self.clashes.0 != coords {
            let elements: Vec<&str> = model.atoms.iter().map(|a| a.element.as_str()).collect();
            self.clashes.1 = geom::clashes(&coords, &elements, &edit::graph(model));
            self.clashes.0 = coords;
        }
        &self.clashes.1
    }
}

/// The 3D view in the window's central area: the viewer and a line naming what it shows, or
/// why there is nothing to show. The controls are in the right panel ([`edit_panel`]).
pub fn viewer_area(
    ui: &mut egui::Ui,
    title: &str,
    doc: Option<&mut Document>,
    view: &mut View3d,
    render_state: Option<&RenderState>,
) {
    // Dark in both themes, as in the design: a 3D model reads best on a dark ground.
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0.0, theme::DARK_SURFACE);
    // Dark widgets too, or the light theme's white buttons carry light text.
    *ui.visuals_mut() = theme::dark_visuals();
    ui.visuals_mut().override_text_color = Some(dark::TEXT);

    let running = doc.as_deref().is_some_and(|d| d.three_d.is_running());
    header(ui, title, view, running);
    separator(ui);

    let has_model = doc
        .as_deref()
        .is_some_and(|d| d.three_d.shown_model().is_some());
    match (doc, render_state) {
        (Some(doc), Some(render_state)) if has_model => {
            // The 3D edits' undo, redo and Escape, unless a field in the panel is being typed in.
            if ui.ctx().memory(|m| m.focused().is_none()) {
                shortcuts(ui, doc);
            }
            viewer(ui, doc, view, render_state);
        }
        (doc, _) => {
            let doc = doc.as_deref();
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space((ui.available_height() / 2.0 - 60.0).max(0.0));
                    let state = doc.map(|d| &d.three_d.state);
                    let stale = doc.is_some_and(|d| d.three_d.is_stale(&d.editor.molecule));
                    summary(ui, state, stale, render_state.is_some());
                });
            });
        }
    }
}

/// The line over the viewer: what it shows, the work running, and resetting the view.
fn header(ui: &mut egui::Ui, title: &str, view: &mut View3d, running: bool) {
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        // Laid out right to left so the controls keep their room and the title is truncated.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(12.0);
            if ui
                .small_button("⟲")
                .on_hover_text("表示をリセット（何もない所をダブルクリックでも）")
                .clicked()
                && let Some(viewport) = &mut view.viewport
            {
                viewport.reset_view();
            }
            if running {
                ui.spinner();
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(RichText::new(format!("3D：{title}")).size(12.0)).truncate(),
                );
            });
        });
    });
}

/// The right panel's contents in the 3D view: how the model is shown, generating and
/// optimising it, editing it by hand, and exporting it. Drawn in the app's own theme.
pub fn edit_panel(
    ui: &mut egui::Ui,
    doc: Option<&mut Document>,
    view: &mut View3d,
    out: &mut Vec<Command>,
) {
    let ink = Ink::of(ui);
    let mut doc = doc;
    ui.spacing_mut().item_spacing.y = 5.0;
    // Headings in the text colour, as the analysis panel's (`strong` would be the accent).
    let heading = ui.visuals().widgets.noninteractive.fg_stroke.color;
    ui.label(RichText::new("3D 編集").size(15.0).color(heading));
    ui.add_space(2.0);

    section(ui, "表示");
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("style3d")
            .selected_text(view.style.label())
            .show_ui(ui, |ui| {
                for style in Style3d::ALL {
                    ui.selectable_value(&mut view.style, style, style.label());
                }
            });
        ui.toggle_value(&mut view.numbers, "原子番号")
            .on_hover_text("すべての原子に番号を表示");
    });

    section(ui, "モデル");
    model_section(ui, doc.as_deref(), &ink, out);

    if let Some(doc) = doc.as_deref_mut()
        && doc.three_d.shown_model().is_some()
    {
        section(ui, "編集");
        tools(ui, doc, view, &ink);
    }

    section(ui, "エクスポート");
    let has_model = doc.as_deref().is_some_and(|d| d.three_d.model().is_some());
    ui.horizontal_wrapped(|ui| {
        for export in Export3d::ALL {
            if ui
                .add_enabled(
                    has_model,
                    egui::Button::new(format!(".{}", export.extension())),
                )
                .on_hover_text(format!("{} に書き出す", export.description()))
                .on_disabled_hover_text("書き出す 3D モデルがありません")
                .clicked()
            {
                out.push(Command::Export3d(export));
            }
        }
    });
}

/// A section heading in the panel.
fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    let heading = ui.visuals().widgets.noninteractive.fg_stroke.color;
    ui.label(RichText::new(title).size(12.0).color(heading));
    ui.separator();
}

/// Text colours for the panel, readable on the light and the dark theme alike.
struct Ink {
    muted: Color32,
    warn: Color32,
    pick: Color32,
    anchor: Color32,
    clash: Color32,
}

impl Ink {
    fn of(ui: &egui::Ui) -> Self {
        if ui.visuals().dark_mode {
            Self {
                muted: dark::MUTED,
                warn: theme::ORANGE_ON_DARK,
                pick: PICK_STROKE,
                anchor: ANCHOR_STROKE,
                clash: CLASH_STROKE,
            }
        } else {
            Self {
                muted: theme::light::MUTED,
                warn: theme::ORANGE_TEXT,
                pick: theme::ORANGE_TEXT,
                anchor: ANCHOR_ON_LIGHT,
                clash: CLASH_ON_LIGHT,
            }
        }
    }

    fn muted(&self, text: &str) -> RichText {
        RichText::new(text).size(11.0).color(self.muted)
    }
}

/// The viewer, with picking, the wheel, and the labels drawn over it.
fn viewer(ui: &mut egui::Ui, doc: &mut Document, view: &mut View3d, render_state: &RenderState) {
    let job = &mut doc.three_d;
    let editable = !job.is_running();
    let Some(model) = job.shown_model() else {
        return;
    };
    let viewport = view.viewport.get_or_insert_with(Viewport3d::new);
    viewport.set_style(view.style);
    viewport.set_background(theme::DARK_SURFACE);
    viewport.sync(Some(model));

    let picked = job.edit.picked.clone();
    let anchor = job.edit.anchor;
    let rotor = editable
        .then(|| edit::picked_rotor(model, &picked))
        .flatten();
    let moving: Vec<usize> = rotor
        .and_then(|(b, c)| geom::moving_side(&edit::graph(model), b, c, anchor).ok())
        .map(|(_, side)| side)
        .unwrap_or_default();
    let model = model.clone();
    let clashes = view.clashes_for(&model).to_vec();
    let clash_atoms: Vec<usize> = clashes.iter().flat_map(|c| [c.a, c.b]).collect();
    let anchor_list: Vec<usize> = anchor.into_iter().collect();
    let viewport = view.viewport.get_or_insert_with(Viewport3d::new);
    viewport.set_highlights(
        exclusive_groups(&[
            (&anchor_list, ANCHOR_COLOR),
            (&picked, PICK_COLOR),
            (&clash_atoms, CLASH_COLOR),
            (&moving, MOVING_COLOR),
        ]),
        rotor.map(|(b, c)| vec![(b, c)]).unwrap_or_default(),
    );

    let frame = match viewport.show(ui, render_state, rotor.is_some()) {
        Ok(frame) => frame,
        Err(e) => {
            ui.label(
                RichText::new(format!("3D 表示に失敗しました: {e}")).color(theme::ORANGE_ON_DARK),
            );
            return;
        }
    };
    overlay(
        ui,
        viewport,
        &frame,
        &model,
        &picked,
        anchor,
        &clashes,
        view.numbers,
    );
    if editable {
        react(ui, doc, &frame, rotor);
    }
}

/// Act on this frame's clicks and wheel.
fn react(ui: &egui::Ui, doc: &mut Document, frame: &Frame3d, rotor: Option<(usize, usize)>) {
    let Some((model, edit3d)) = doc.three_d.model_and_edit() else {
        return;
    };
    match frame.primary {
        Some(Hit::Atom(i)) => {
            edit3d.toggle_pick(i);
            edit3d.report = None;
        }
        Some(Hit::Bond(b)) => {
            if let Some(bond) = model.bonds.get(b) {
                edit3d.pick_bond(bond.begin, bond.end);
                edit3d.report = None;
            }
        }
        Some(Hit::Nothing) => edit3d.clear_picks(),
        None => {}
    }
    match frame.secondary {
        Some(Hit::Atom(i)) => edit3d.toggle_anchor(i),
        Some(Hit::Nothing) => edit3d.anchor = None,
        _ => {}
    }
    if rotor.is_some() && frame.wheel_notches.abs() > f32::EPSILON {
        let modifiers = ui.input(|i| i.modifiers);
        let step = if modifiers.shift {
            STEP_FINE
        } else if modifiers.command {
            STEP_COARSE
        } else {
            STEP
        };
        edit::rotate_picked(model, edit3d, step * f64::from(frame.wheel_notches));
    }
}

/// Labels over the viewer: picked atoms numbered in order with the measurement between them,
/// the fixed atom, clashes, and the atom under the pointer.
#[allow(clippy::too_many_arguments)]
fn overlay(
    ui: &egui::Ui,
    viewport: &Viewport3d,
    frame: &Frame3d,
    model: &Model3d,
    picked: &[usize],
    anchor: Option<usize>,
    clashes: &[Clash],
    numbers: bool,
) {
    let painter = ui.painter_at(frame.rect);
    let at = |i: usize| {
        model
            .atoms
            .get(i)
            .and_then(|a| viewport.project(frame.rect, a.pos))
    };
    let font = egui::FontId::proportional(11.0);
    let tag = |pos: Pos2, text: String, color: Color32| {
        let galley = painter.layout_no_wrap(text, font.clone(), color);
        let r = egui::Rect::from_min_size(pos, galley.size()).expand2(vec2(4.0, 1.0));
        painter.rect_filled(r, 3.0, LABEL_BACK);
        painter.galley(pos, galley, color);
    };

    if numbers {
        for i in 0..model.atoms.len() {
            if let Some(p) = at(i) {
                let galley = painter.layout_no_wrap(
                    (i + 1).to_string(),
                    egui::FontId::proportional(9.5),
                    dark::MUTED,
                );
                painter.galley(p - galley.size() / 2.0, galley, dark::MUTED);
            }
        }
    }

    // Clashes: a dashed red line between the pair, the worst few with their distance.
    for (n, clash) in clashes.iter().enumerate().take(40) {
        if let (Some(p), Some(q)) = (at(clash.a), at(clash.b)) {
            painter.add(egui::Shape::dashed_line(
                &[p, q],
                Stroke::new(1.5, CLASH_STROKE),
                4.0,
                3.0,
            ));
            if n < 5 {
                tag(
                    p.lerp(q, 0.5) + vec2(4.0, -8.0),
                    format!("{:.2} Å", clash.distance),
                    CLASH_STROKE,
                );
            }
        }
    }

    // The picked atoms in order, joined, with what they measure.
    let points: Vec<Pos2> = picked.iter().filter_map(|&i| at(i)).collect();
    if points.len() == picked.len() {
        if points.len() >= 2 {
            painter.add(egui::Shape::dashed_line(
                &points,
                Stroke::new(1.5, PICK_STROKE),
                5.0,
                3.0,
            ));
        }
        for (n, (&i, &p)) in picked.iter().zip(&points).enumerate() {
            tag(
                p + vec2(7.0, -16.0),
                format!("{} {}", n + 1, edit::atom_label(model, i)),
                PICK_STROKE,
            );
        }
        if let Some(measure) = Measure::of(picked, &edit::coords(model)) {
            let centre = points
                .iter()
                .fold(Pos2::ZERO, |acc, p| acc + p.to_vec2() / points.len() as f32);
            tag(centre + vec2(6.0, 6.0), measure.to_string(), Color32::WHITE);
        }
    }
    if let Some(p) = anchor.and_then(at) {
        tag(p + vec2(7.0, 4.0), "固定".to_owned(), ANCHOR_STROKE);
    }

    // The atom under the pointer, with its distance from the last one picked.
    if let Some(Hit::Atom(i)) = frame.hovered
        && let Some(pointer) = ui.ctx().pointer_hover_pos()
    {
        let mut text = edit::atom_label(model, i);
        if let Some(&last) = picked.last()
            && last != i
        {
            text.push_str(&format!("  {:.3} Å", model.distance(last, i)));
        }
        tag(pointer + vec2(14.0, 10.0), text, dark::TEXT);
    }
}

/// Undo/redo and clearing the selection from the keyboard while the pointer is over the panel.
fn shortcuts(ui: &egui::Ui, doc: &mut Document) {
    use egui::{Key, KeyboardShortcut, Modifiers};
    if doc.three_d.is_running() {
        return;
    }
    let (redo, undo, escape) = ui.input_mut(|i| {
        let redo = i.consume_shortcut(&KeyboardShortcut::new(
            Modifiers::COMMAND | Modifiers::SHIFT,
            Key::Z,
        )) || i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Y));
        let undo = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z));
        let escape = i.consume_key(Modifiers::NONE, Key::Escape);
        (redo, undo, escape)
    });
    if let Some((model, edit3d)) = doc.three_d.model_and_edit() {
        if undo {
            history(model, edit3d, false);
        }
        if redo {
            history(model, edit3d, true);
        }
        if escape {
            edit3d.clear_picks();
        }
    }
}

fn history(model: &mut Model3d, edit3d: &mut three_d::Edit3d, redo: bool) {
    let label = if redo {
        edit3d.redo(model).map(|l| format!("やり直しました：{l}"))
    } else {
        edit3d.undo(model).map(|l| format!("元に戻しました：{l}"))
    };
    edit3d.report = Some((
        label.unwrap_or_else(|| "3D の編集履歴はありません".to_owned()),
        false,
    ));
}

/// The tools for editing the model by hand, in the right panel.
fn tools(ui: &mut egui::Ui, doc: &mut Document, view: &mut View3d, ink: &Ink) {
    let Some((model, edit3d)) = doc.three_d.model_and_edit() else {
        ui.label(ink.muted("最適化中は編集できません"));
        return;
    };
    history_row(ui, model, edit3d);
    selection_row(ui, model, edit3d, ink);
    value_row(ui, model, edit3d, &mut view.value, ink);
    if let Some((text, problem)) = &edit3d.report {
        let color = if *problem { ink.warn } else { ink.muted };
        ui.label(RichText::new(text).size(11.0).color(color));
    }
    constraints_section(ui, model, edit3d, ink);
    clash_row(ui, model, view, ink);
}

fn history_row(ui: &mut egui::Ui, model: &mut Model3d, edit3d: &mut three_d::Edit3d) {
    ui.horizontal(|ui| {
        let undo_tip = edit3d
            .undo_label()
            .map_or("元に戻す操作はありません".to_owned(), |l| {
                format!("元に戻す：{l}（Ctrl+Z）")
            });
        if ui
            .add_enabled(edit3d.can_undo(), egui::Button::new("↶ 元に戻す").small())
            .on_hover_text(undo_tip)
            .on_disabled_hover_text("元に戻す操作はありません")
            .clicked()
        {
            history(model, edit3d, false);
        }
        let redo_tip = edit3d
            .redo_label()
            .map_or("やり直す操作はありません".to_owned(), |l| {
                format!("やり直す：{l}（Ctrl+Y）")
            });
        if ui
            .add_enabled(edit3d.can_redo(), egui::Button::new("↷ やり直し").small())
            .on_hover_text(redo_tip)
            .on_disabled_hover_text("やり直す操作はありません")
            .clicked()
        {
            history(model, edit3d, true);
        }
    });
}

fn selection_row(ui: &mut egui::Ui, model: &Model3d, edit3d: &mut three_d::Edit3d, ink: &Ink) {
    if edit3d.picked.is_empty() && edit3d.anchor.is_none() {
        ui.label(ink.muted(
            "原子をクリックで選択（4 つまで）・結合をクリックで両端を選択・右クリックで原子を固定",
        ));
        return;
    }
    ui.horizontal_wrapped(|ui| {
        if !edit3d.picked.is_empty() {
            let names: Vec<String> = edit3d
                .picked
                .iter()
                .map(|&i| edit::atom_label(model, i))
                .collect();
            ui.label(RichText::new("選択").size(11.0).color(ink.muted));
            ui.label(RichText::new(names.join(" – ")).size(12.0).color(ink.pick));
            if ui.small_button("解除").clicked() {
                edit3d.clear_picks();
                edit3d.report = None;
            }
        }
        if let Some(anchor) = edit3d.anchor {
            ui.label(RichText::new("固定").size(11.0).color(ink.muted));
            ui.label(
                RichText::new(edit::atom_label(model, anchor))
                    .size(12.0)
                    .color(ink.anchor),
            )
            .on_hover_text("編集ではこの原子を含まない側が動きます");
            if ui.small_button("解除").clicked() {
                edit3d.anchor = None;
            }
        }
    });
}

/// The measurement over the picked atoms, the box to set it, and the bond-turning buttons.
fn value_row(
    ui: &mut egui::Ui,
    model: &mut Model3d,
    edit3d: &mut three_d::Edit3d,
    value: &mut (Vec<usize>, String),
    ink: &Ink,
) {
    let muted = |text: &str| ink.muted(text);
    let graph = edit::graph(model);
    let target = Target::of(&edit3d.picked, &graph);
    let measure = Measure::of(&edit3d.picked, &edit::coords(model));
    let Some(measure) = measure else {
        return;
    };
    // The box is refilled with the current value whenever the selection changes, and not
    // otherwise, so it never fights the user's typing.
    if value.0 != edit3d.picked {
        *value = (edit3d.picked.clone(), format_value(measure));
    }
    let (name, unit) = match target {
        Target::BondLength(_) => ("結合長", "Å"),
        Target::Distance(_) => ("距離", "Å"),
        Target::Angle(_) => ("結合角", "°"),
        Target::Dihedral(_) => ("二面角", "°"),
        Target::Unbonded => match measure {
            Measure::Distance(_) => ("距離", "Å"),
            Measure::Angle(_) => ("角度", "°"),
            Measure::Dihedral(_) => ("二面角", "°"),
        },
        Target::None => return,
    };
    let mut apply: Option<f64> = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(name).size(11.0).color(ink.muted));
        ui.label(RichText::new(measure.to_string()).size(13.0).strong());
        if target == Target::Unbonded {
            ui.label(muted("（結合でつながった並びではないため測定のみ）"));
            return;
        }
        ui.label(RichText::new("→").color(ink.muted));
        let field = ui.add(
            egui::TextEdit::singleline(&mut value.1)
                .desired_width(56.0)
                .align(egui::Align2::RIGHT_CENTER),
        );
        ui.label(RichText::new(unit).size(11.0));
        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.button("設定").clicked() || entered {
            match parse_value(&value.1) {
                Some(v) => apply = Some(v),
                None => {
                    edit3d.report = Some(("数値を入力してください".to_owned(), true));
                }
            }
        }
    });
    let presets: &[(&str, f64)] = match target {
        Target::Angle(_) => &[
            ("sp³ 109.5°", 109.5),
            ("sp² 120°", 120.0),
            ("直線 180°", 180.0),
        ],
        Target::Dihedral(_) => &[
            ("シス 0°", 0.0),
            ("ゴーシュ 60°", 60.0),
            ("ゴーシュ −60°", -60.0),
            ("アンチ 180°", 180.0),
        ],
        _ => &[],
    };
    if !presets.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for (label, v) in presets {
                if ui.small_button(*label).clicked() {
                    value.1 = format!("{v}");
                    apply = Some(*v);
                }
            }
        });
    }
    if let Target::Distance([a, b]) = target {
        ui.horizontal_wrapped(|ui| {
            ui.label(muted("間の単結合を回して"));
            if ui.small_button("できるだけ遠く").clicked() {
                edit::solve(model, edit3d, a, b, Goal::Farthest);
            }
            if ui.small_button("できるだけ近く").clicked() {
                edit::solve(model, edit3d, a, b, Goal::Closest);
            }
        });
    }
    if edit::picked_rotor(model, &edit3d.picked).is_some() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("回転").size(11.0).color(ink.muted));
            for step in [-STEP_COARSE, -STEP, STEP, STEP_COARSE] {
                if ui.small_button(format!("{step:+}°")).clicked() {
                    edit::rotate_picked(model, edit3d, step);
                }
            }
            ui.label(muted("ホイールでも回転（Shift 1°・Ctrl 15°）"));
        });
    } else if let [b, c] = edit3d.picked[..]
        && graph.bonded(b, c)
        && let Err(reason) = geom::is_rotatable(&graph, b, c, edit::bond_order(model, b, c))
    {
        ui.label(muted(&format!("回転：{reason}")));
    }
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(
            &mut edit3d.lock_on_set,
            RichText::new("設定した値を最適化でも保つ").size(11.0),
        )
        .on_hover_text("設定するたびに、その値の制約を追加します");
        if ui
            .small_button("現在値で制約")
            .on_hover_text("選んだ原子の今の値を、最適化で保つ制約にします（1 原子なら固定）")
            .clicked()
        {
            let picked = edit3d.picked.clone();
            edit3d.constrain(&picked, measure.value());
        }
    });
    if let Some(v) = apply {
        let changed = edit::set_value(model, edit3d, v);
        if changed {
            value.1 = format_value_like(measure, v);
        }
    }
}

fn constraints_section(
    ui: &mut egui::Ui,
    model: &Model3d,
    edit3d: &mut three_d::Edit3d,
    ink: &Ink,
) {
    if let [atom] = edit3d.picked[..]
        && ui
            .small_button(format!(
                "{} を最適化で動かさない",
                edit::atom_label(model, atom)
            ))
            .clicked()
    {
        edit3d.constrain(&[atom], 0.0);
    }
    if edit3d.constraints.is_empty() {
        return;
    }
    let coords = edit::coords(model);
    egui::CollapsingHeader::new(
        RichText::new(format!("最適化の制約（{}）", edit3d.constraints.len())).size(12.0),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.checkbox(
            &mut edit3d.use_constraints,
            RichText::new("最適化で制約を使う").size(11.0),
        );
        let mut remove = None;
        for (n, c) in edit3d.constraints.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui
                    .small_button("×")
                    .on_hover_text("この制約を削除")
                    .clicked()
                {
                    remove = Some(n);
                }
                let atoms: Vec<String> = c
                    .atoms()
                    .iter()
                    .map(|&i| edit::atom_label(model, i))
                    .collect();
                ui.label(
                    RichText::new(format!("{} {}", c.kind_label(), atoms.join("–"))).size(11.0),
                );
                if let (Some(target), Some(now)) = (c.target(), c.measure(&coords)) {
                    let off = match c {
                        Constraint::Torsion(..) => geom::wrap_degrees(now - target).abs(),
                        _ => (now - target).abs(),
                    };
                    let tolerance = if matches!(c, Constraint::Distance(..)) {
                        0.02
                    } else {
                        1.0
                    };
                    let color = if off > tolerance { ink.warn } else { ink.muted };
                    ui.label(
                        RichText::new(format!(
                            "目標 {target:.2}{u}・現在 {now:.2}{u}",
                            u = c.unit()
                        ))
                        .size(11.0)
                        .color(color),
                    );
                }
            });
        }
        if let Some(n) = remove {
            edit3d.constraints.remove(n);
        }
        if let Some(warning) = three_d::edit::conflict(&edit3d.constraints) {
            ui.label(RichText::new(warning).size(11.0).color(ink.warn));
        }
    });
}

fn clash_row(ui: &mut egui::Ui, model: &Model3d, view: &mut View3d, ink: &Ink) {
    let clashes = view.clashes_for(model);
    match clashes.first() {
        None => {
            ui.label(ink.muted("原子の衝突はありません"));
        }
        Some(worst) => {
            ui.label(
                RichText::new(format!(
                    "原子の衝突 {} 件（最も近い {}···{} {:.2} Å）— 構造最適化で解消できます",
                    clashes.len(),
                    edit::atom_label(model, worst.a),
                    edit::atom_label(model, worst.b),
                    worst.distance
                ))
                .size(11.0)
                .color(ink.clash),
            );
        }
    }
}

fn format_value(measure: Measure) -> String {
    match measure {
        Measure::Distance(v) => format!("{v:.3}"),
        Measure::Angle(v) | Measure::Dihedral(v) => format!("{v:.1}"),
    }
}

fn format_value_like(measure: Measure, v: f64) -> String {
    match measure {
        Measure::Distance(_) => format!("{v:.3}"),
        Measure::Angle(_) | Measure::Dihedral(_) => format!("{v:.1}"),
    }
}

/// A typed number, accepting full-width digits and minus signs as Japanese input gives them.
fn parse_value(text: &str) -> Option<f64> {
    let normal: String = text
        .trim()
        .chars()
        .map(|c| match c {
            '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c),
            '．' => '.',
            '－' | '−' | 'ー' => '-',
            '＋' => '+',
            _ => c,
        })
        .filter(|c| !matches!(c, '°' | 'Å' | ' '))
        .collect();
    normal.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Generating and optimising the model, and what it is: force field, energy, formula, and
/// whether the drawing has changed since.
fn model_section(ui: &mut egui::Ui, doc: Option<&Document>, ink: &Ink, out: &mut Vec<Command>) {
    let job = doc.map(|d| &d.three_d);
    let model = job.and_then(|j| j.shown_model());
    let running = job.is_some_and(|j| j.is_running());
    ui.add_enabled_ui(three_d::AVAILABLE && doc.is_some() && !running, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("力場").size(11.0).color(ink.muted));
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
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("2D から再生成")
                .on_hover_text("2D の構造から 3D モデルを作り直します")
                .clicked()
            {
                out.push(Command::Generate3d);
            }
            let constraints = job.map_or(0, |j| {
                j.model()
                    .map_or(0, |m| j.edit.active_constraints(m.atoms.len()).len())
            });
            let tip = if constraints > 0 {
                format!("制約 {constraints} 件を保って最適化します")
            } else {
                "力場で構造を最適化します".to_owned()
            };
            let optimize = ui
                .add_enabled(
                    job.is_some_and(|j| j.model().is_some()),
                    egui::Button::new(RichText::new("構造最適化").color(Color32::WHITE))
                        .fill(theme::ORANGE_DEEP),
                )
                .on_hover_text(tip);
            if optimize.clicked() {
                out.push(Command::Optimize3d);
            }
        });
    });
    match job.map(|j| &j.state) {
        Some(JobState::Running(task)) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(ink.muted(task.running_label()));
            });
        }
        Some(JobState::Failed(message)) => {
            ui.label(RichText::new(message).size(11.0).color(ink.warn));
        }
        _ => {}
    }
    if !three_d::AVAILABLE {
        ui.label(ink.muted(
            "この版では 3D 生成を利用できません（OpenBabel を有効にしてビルドすると使えます）",
        ));
    }
    let Some(m) = model else {
        if !running && three_d::AVAILABLE {
            ui.label(ink.muted(
                "2D 画面で分子を右クリックして「3D化」、または「2D から再生成」で 3D モデルを作ります",
            ));
        }
        return;
    };
    let pending = job.is_some_and(|j| j.energy_pending());
    let energy = if pending {
        format!("E = {:.1} {}（再計算中）", m.energy, m.energy_unit)
    } else {
        format!("E = {:.1} {}", m.energy, m.energy_unit)
    };
    ui.label(RichText::new(energy).monospace().size(12.0));
    let mut about = format!(
        "{}・{} 原子（うち H {}）",
        m.formula,
        m.atoms.len(),
        m.hydrogen_count()
    );
    if job.is_some_and(|j| j.scope().is_some()) {
        about += "・描画の一部の分子";
    }
    ui.label(ink.muted(&about));
    if doc.is_some_and(|d| d.three_d.is_stale(&d.editor.molecule)) {
        ui.label(
            RichText::new("2D が変更されています")
                .size(11.0)
                .color(ink.warn),
        )
        .on_hover_text("「2D から再生成」で 3D モデルを描き直した構造に合わせます");
    }
}

/// The panel body when there is no model to show: why, and what to do.
fn summary(ui: &mut egui::Ui, state: Option<&JobState>, stale: bool, can_render: bool) {
    if !three_d::AVAILABLE {
        ui.label(muted(
            "この版では 3D 生成を利用できません\nOpenBabel を有効にしてビルドすると使えます",
        ));
        return;
    }
    match state {
        None | Some(JobState::Idle) => {
            ui.label(muted(
                "2D 画面で分子を右クリックして「3D化」、または右パネルの「2D から再生成」で\n2D の構造から 3D モデルを生成します",
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
            if !can_render {
                ui.label(muted(
                    "3D 表示には wgpu レンダラーが必要です（この環境では使えません）",
                ));
            }
        }
    }
}

fn muted(text: &str) -> RichText {
    RichText::new(text).size(11.0).color(dark::MUTED)
}

fn separator(ui: &mut egui::Ui) {
    let y = ui.cursor().top();
    let x = ui.max_rect().x_range();
    ui.painter()
        .hline(x, y, egui::Stroke::new(1.0, dark::BORDER));
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_values_accept_full_width_input() {
        assert_eq!(parse_value(" 1.54 "), Some(1.54));
        assert_eq!(parse_value("－６０"), Some(-60.0));
        assert_eq!(parse_value("１０９．５°"), Some(109.5));
        assert_eq!(parse_value("abc"), None);
        assert_eq!(parse_value("NaN"), None);
    }
}
