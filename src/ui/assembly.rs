//! The assembly view (集合体): the built assembly in the central area ([`viewer_area`]) and, in
//! the right panel ([`panel`]), its monomers, the steps that copy them, how it is shown, and
//! exporting and saving it. The model is [`crate::assembly`]; this module only draws it and
//! turns clicks into edits.
//!
//! In the viewer: drag to orbit, Shift+drag or right/middle drag to pan, wheel to zoom,
//! double-click empty space to reframe, click an atom to select its molecule. A selected
//! molecule placed by hand turns with Alt+drag and moves with Shift+drag instead ([`place`]).
//! While a monomer is being oriented the viewer shows that monomer alone, and three clicks
//! pick its atoms.

mod place;

use eframe::egui_wgpu::RenderState;
use egui::{Color32, DragValue, Pos2, RichText, Stroke, vec2};
use serde::{Deserialize, Serialize};

use super::Command;
use super::viewport3d::{
    Frame3d, Hit, Scene, Style3d, Viewport3d, element_color, exclusive_groups,
};
use crate::assembly::model::{Array, Ring, Stack, helix_radius};
use crate::assembly::{AssemblyExport, AssemblyState, Built, Component, Step};
use crate::document::Document;
use crate::theme::{self, dark};
use crate::three_d;

/// The selected molecule's highlight in the viewer, and its colour in labels.
const SELECTED_COLOR: (f32, f32, f32) = (0.98, 0.45, 0.22);
const PICK_STROKE: Color32 = Color32::from_rgb(0xF5, 0x8A, 0x4F);
const LABEL_BACK: Color32 = Color32::from_rgba_premultiplied(0x14, 0x12, 0x10, 0xD8);

/// How the atoms are coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ColorMode {
    /// By molecule: a different hue for each copy.
    Molecule,
    /// By monomer: one hue per component.
    Component,
    /// By element. Last, as serde requires of the catch-all.
    #[default]
    #[serde(other)]
    Element,
}

impl ColorMode {
    pub const ALL: [ColorMode; 3] = [
        ColorMode::Element,
        ColorMode::Molecule,
        ColorMode::Component,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ColorMode::Element => "元素",
            ColorMode::Molecule => "分子ごと",
            ColorMode::Component => "単量体ごと",
        }
    }
}

/// Three atoms being picked to orient a monomer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OrientPick {
    component: usize,
    atoms: Vec<usize>,
}

/// What the three picked atoms become, in order.
const ORIENT_ROLES: [&str; 3] = ["原点", "+x 軸", "xy 平面"];

/// The assembly view's state shared by every document: the viewer and the display options
/// (remembered between runs), and the orientation being picked.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AssemblyView {
    pub style: Style3d,
    pub color: ColorMode,
    #[serde(skip)]
    viewport: Option<Viewport3d>,
    #[serde(skip)]
    orient: Option<OrientPick>,
    /// The last thing the panel did or could not do, and whether it is a problem.
    #[serde(skip)]
    pub report: Option<(String, bool)>,
    /// The component 配置 → 追加 places.
    #[serde(skip)]
    add_component: usize,
    /// Clashes between molecules, checked when the build changes.
    #[serde(skip)]
    clashes: place::ClashCheck,
}

impl AssemblyView {
    pub fn free(&mut self, render_state: &RenderState) {
        if let Some(viewport) = &mut self.viewport {
            viewport.free(render_state);
        }
    }

    /// Whether three atoms are being picked to orient a monomer.
    pub fn is_orienting(&self) -> bool {
        self.orient.is_some()
    }

    /// Start picking the orientation atoms of `component`.
    pub fn start_orienting(&mut self, component: usize) {
        self.orient = Some(OrientPick {
            component,
            atoms: Vec::new(),
        });
        self.report = None;
    }
}

/// A distinct, readable colour for item `i` (golden-angle hues).
pub fn palette_color(i: usize) -> [f32; 4] {
    let hue = (i as f32 * 0.618_034).fract();
    let c = Color32::from(egui::ecolor::Hsva::new(hue, 0.55, 0.95, 1.0));
    let [r, g, b, _] = c.to_array();
    [
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        1.0,
    ]
}

/// Each atom's colour in `mode`.
pub fn atom_colors(built: &Built, mode: ColorMode) -> Vec<[f32; 4]> {
    match mode {
        ColorMode::Element => built
            .atoms
            .iter()
            .map(|a| element_color(&a.element))
            .collect(),
        ColorMode::Molecule => built
            .molecule_of_atom
            .iter()
            .map(|&m| palette_color(m))
            .collect(),
        ColorMode::Component => built
            .molecule_of_atom
            .iter()
            .map(|&m| palette_color(built.component_of_molecule[m]))
            .collect(),
    }
}

/// The central area in the assembly view.
pub fn viewer_area(
    ui: &mut egui::Ui,
    title: &str,
    doc: Option<&mut Document>,
    view: &mut AssemblyView,
    render_state: Option<&RenderState>,
) {
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0.0, theme::DARK_SURFACE);
    *ui.visuals_mut() = theme::dark_visuals();
    ui.visuals_mut().override_text_color = Some(dark::TEXT);

    let Some(doc) = doc else { return };
    if ui.ctx().memory(|m| m.focused().is_none()) {
        shortcuts(ui, &mut doc.assembly, view);
    }
    // An orientation in progress for a monomer that has gone (undo) is dropped.
    if view
        .orient
        .as_ref()
        .is_some_and(|o| o.component >= doc.assembly.design.components.len())
    {
        view.orient = None;
    }
    header(ui, title, &doc.assembly, view);
    separator(ui);

    let message = if doc.assembly.is_empty() {
        Some("単量体がありません。\n3D 画面で 3D モデルを作り「3D」メニューの「分子集合体を作成」を選ぶか、\n右パネルの「今の 3D モデルを追加」「ファイルから追加」で単量体を加えてください".to_owned())
    } else if let Err(e) = doc.assembly.built() {
        Some(e.to_string())
    } else if render_state.is_none() {
        Some("3D 表示には wgpu レンダラーが必要です（この環境では使えません）".to_owned())
    } else {
        None
    };
    match (message, render_state) {
        (None, Some(render_state)) => viewer(ui, &mut doc.assembly, view, render_state),
        (message, _) => {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(message.unwrap_or_default())
                        .size(12.0)
                        .color(dark::MUTED),
                );
            });
        }
    }
}

fn header(ui: &mut egui::Ui, title: &str, state: &AssemblyState, view: &mut AssemblyView) {
    ui.horizontal(|ui| {
        ui.add_space(12.0);
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
            let mut cancel = false;
            if let Some(pick) = &view.orient {
                cancel = ui
                    .small_button("やめる")
                    .on_hover_text("Esc でも")
                    .clicked();
                let name = state
                    .design
                    .components
                    .get(pick.component)
                    .map_or("", |c| c.name.as_str());
                let next = pick.atoms.len().min(2);
                ui.label(
                    RichText::new(format!(
                        "「{name}」の向き合わせ：{}に置く原子をクリック（{}/3）",
                        ORIENT_ROLES[next],
                        pick.atoms.len() + 1
                    ))
                    .size(12.0)
                    .color(PICK_STROKE),
                );
            }
            if cancel {
                view.orient = None;
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(RichText::new(format!("集合体：{title}")).size(12.0))
                        .truncate(),
                );
            });
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

/// Undo, redo and Escape (stop orienting, clear the selection) from the keyboard.
fn shortcuts(ui: &egui::Ui, state: &mut AssemblyState, view: &mut AssemblyView) {
    use egui::{Key, KeyboardShortcut, Modifiers};
    let (redo, undo, escape) = ui.input_mut(|i| {
        let redo = i.consume_shortcut(&KeyboardShortcut::new(
            Modifiers::COMMAND | Modifiers::SHIFT,
            Key::Z,
        )) || i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Y));
        let undo = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z));
        let escape = i.consume_key(Modifiers::NONE, Key::Escape);
        (redo, undo, escape)
    });
    if undo {
        history(state, view, false);
    }
    if redo {
        history(state, view, true);
    }
    if escape && view.orient.take().is_none() {
        state.selected = None;
    }
}

fn history(state: &mut AssemblyState, view: &mut AssemblyView, redo: bool) {
    let done = if redo { state.redo() } else { state.undo() };
    view.report = Some(match (done, redo) {
        (true, false) => ("元に戻しました".to_owned(), false),
        (true, true) => ("やり直しました".to_owned(), false),
        (false, false) => ("元に戻す操作はありません".to_owned(), false),
        (false, true) => ("やり直す操作はありません".to_owned(), false),
    });
}

/// The viewer: the built assembly (or, while orienting, the one monomer) with picking.
fn viewer(
    ui: &mut egui::Ui,
    state: &mut AssemblyState,
    view: &mut AssemblyView,
    render_state: &RenderState,
) {
    let viewport = view.viewport.get_or_insert_with(Viewport3d::new);
    viewport.set_style(view.style);
    viewport.set_background(theme::DARK_SURFACE);

    if let Some(pick) = view.orient.clone() {
        let Some(component) = state.design.components.get(pick.component) else {
            return;
        };
        viewport.sync_scene(
            Some(Scene {
                // One source per monomer of each document, apart from the assembly's.
                source: (state.id() << 16) | (pick.component as u64 + 1),
                revision: state.revision(),
                atoms: &component.atoms,
                bonds: &component.bonds,
            }),
            0,
            || {
                component
                    .atoms
                    .iter()
                    .map(|a| element_color(&a.element))
                    .collect()
            },
        );
        viewport.set_highlights(
            vec![moleucle_3dview_rs::AtomGroup {
                atom_indices: pick.atoms.clone(),
                color: SELECTED_COLOR,
            }],
            Vec::new(),
        );
        let frame = match viewport.show(ui, render_state, false) {
            Ok(frame) => frame,
            Err(e) => return failed(ui, &e),
        };
        orient_overlay(ui, viewport, &frame, component, &pick.atoms);
        if let Some(Hit::Atom(i)) = frame.primary {
            pick_orient_atom(state, view, i);
        }
        return;
    }

    let Ok(built) = state.built() else { return };
    let color = view.color;
    viewport.sync_scene(
        Some(Scene {
            source: state.id() << 16,
            revision: built.revision,
            atoms: &built.atoms,
            bonds: &built.bonds,
        }),
        color as u64,
        || atom_colors(&built, color),
    );
    let selected: Vec<usize> = state
        .selected
        .filter(|&m| m < built.molecule_count())
        .map(|m| built.molecule_atoms(m).collect())
        .unwrap_or_default();
    let clashes = view.clashes.get(ui.ctx(), state.id(), &built).map(|c| c.0);
    let clashing = clashes
        .as_deref()
        .map(place::clash_atoms)
        .unwrap_or_default();
    viewport.set_highlights(
        exclusive_groups(&[
            (&selected, SELECTED_COLOR),
            (&clashing, place::CLASH_HIGHLIGHT),
        ]),
        Vec::new(),
    );
    // Alt or Shift with a hand-placed molecule selected: the drag moves the molecule, not
    // the camera.
    let movable = state
        .selected
        .and_then(|m| state.design.manual_index(m))
        .is_some();
    let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
    viewport.claim_drag(movable && (alt || shift));
    let frame = match viewport.show(ui, render_state, false) {
        Ok(frame) => frame,
        Err(e) => return failed(ui, &e),
    };
    place::overlay(ui, viewport, &frame, state, &built, clashes.as_deref());
    assembly_overlay(ui, &frame, state, &built);
    if let Some(delta) = frame.drag {
        place::drag_selected(state, viewport, &frame, delta, alt);
    }
    let molecule_of = |hit: Hit| match hit {
        Hit::Atom(a) => built.molecule_of_atom.get(a).copied(),
        Hit::Bond(b) => built
            .bonds
            .get(b)
            .and_then(|b| built.molecule_of_atom.get(b.begin).copied()),
        Hit::Nothing => None,
    };
    if let Some(hit) = frame.primary {
        state.selected = molecule_of(hit);
    }
}

fn failed(ui: &mut egui::Ui, e: &str) {
    ui.label(RichText::new(format!("3D 表示に失敗しました: {e}")).color(theme::ORANGE_ON_DARK));
}

/// Add atom `i` to the orientation being picked; the third one sets the orientation.
fn pick_orient_atom(state: &mut AssemblyState, view: &mut AssemblyView, i: usize) {
    let Some(pick) = &mut view.orient else { return };
    if let Some(at) = pick.atoms.iter().position(|&a| a == i) {
        // Clicking a picked atom again takes it (and those after it) back.
        pick.atoms.truncate(at);
        return;
    }
    pick.atoms.push(i);
    if pick.atoms.len() < 3 {
        return;
    }
    let (component, atoms) = (
        pick.component,
        [pick.atoms[0], pick.atoms[1], pick.atoms[2]],
    );
    view.orient = None;
    let Some(c) = state.design.components.get(component) else {
        return;
    };
    match crate::assembly::model::orientation(&c.atoms, atoms) {
        Ok(_) => {
            let name = c.name.clone();
            state.edit(|d| d.components[component].orient = Some(atoms));
            view.report = Some((
                format!(
                    "「{name}」の向きを原子 {}・{}・{} で合わせました",
                    atoms[0] + 1,
                    atoms[1] + 1,
                    atoms[2] + 1
                ),
                false,
            ));
        }
        Err(e) => view.report = Some((e.to_string(), true)),
    }
}

/// A label on a dark plate, so it reads over any atom colour.
fn tag(painter: &egui::Painter, pos: Pos2, text: String, color: Color32) {
    let galley = painter.layout_no_wrap(text, egui::FontId::proportional(11.0), color);
    let r = egui::Rect::from_min_size(pos, galley.size()).expand2(vec2(4.0, 1.0));
    painter.rect_filled(r, 3.0, LABEL_BACK);
    painter.galley(pos, galley, color);
}

fn orient_overlay(
    ui: &egui::Ui,
    viewport: &Viewport3d,
    frame: &Frame3d,
    component: &Component,
    picked: &[usize],
) {
    let painter = ui.painter_at(frame.rect);
    let at = |i: usize| {
        component
            .atoms
            .get(i)
            .and_then(|a| viewport.project(frame.rect, a.pos))
    };
    let points: Vec<Pos2> = picked.iter().filter_map(|&i| at(i)).collect();
    if points.len() >= 2 {
        painter.add(egui::Shape::dashed_line(
            &points,
            Stroke::new(1.5, PICK_STROKE),
            5.0,
            3.0,
        ));
    }
    for (n, &i) in picked.iter().enumerate() {
        if let Some(p) = at(i) {
            tag(
                &painter,
                p + vec2(7.0, -16.0),
                format!(
                    "{} {}{}",
                    ORIENT_ROLES[n],
                    component.atoms[i].element,
                    i + 1
                ),
                PICK_STROKE,
            );
        }
    }
    if let Some(Hit::Atom(i)) = frame.hovered
        && let Some(pointer) = ui.ctx().pointer_hover_pos()
        && let Some(atom) = component.atoms.get(i)
    {
        tag(
            &painter,
            pointer + vec2(14.0, 10.0),
            format!("{}{}", atom.element, i + 1),
            dark::TEXT,
        );
    }
}

/// The molecule under the pointer, named.
fn assembly_overlay(ui: &egui::Ui, frame: &Frame3d, state: &AssemblyState, built: &Built) {
    let painter = ui.painter_at(frame.rect);
    if let Some(Hit::Atom(a)) = frame.hovered
        && let Some(pointer) = ui.ctx().pointer_hover_pos()
        && let Some(&m) = built.molecule_of_atom.get(a)
    {
        let name = state
            .design
            .components
            .get(built.component_of_molecule[m])
            .map_or("", |c| c.name.as_str());
        let k = a - built.molecule_start[m];
        let how = if state.design.manual_index(m).is_some() {
            "手動"
        } else {
            "手順"
        };
        tag(
            &painter,
            pointer + vec2(14.0, 10.0),
            format!(
                "分子 {}（{name}・{how}）· {}{}",
                m + 1,
                built.atoms[a].element,
                k + 1
            ),
            dark::TEXT,
        );
    }
}

/// Text colours for the panel on the light and the dark theme.
struct Ink {
    muted: Color32,
    warn: Color32,
}

impl Ink {
    fn of(ui: &egui::Ui) -> Self {
        if ui.visuals().dark_mode {
            Self {
                muted: dark::MUTED,
                warn: theme::ORANGE_ON_DARK,
            }
        } else {
            Self {
                muted: theme::light::MUTED,
                warn: theme::ORANGE_TEXT,
            }
        }
    }

    fn muted(&self, text: impl Into<String>) -> RichText {
        RichText::new(text).size(11.0).color(self.muted)
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    let heading = ui.visuals().widgets.noninteractive.fg_stroke.color;
    ui.label(RichText::new(title).size(12.0).color(heading));
    ui.separator();
}

/// A structural change to the design, applied once the panel has let go of it.
enum Change {
    AddModel,
    RemoveComponent(usize),
    SetBase(usize),
    ClearOrient(usize),
    AddStep(Step),
    RemoveStep(usize),
    MoveStep(usize, usize),
}

/// The right panel in the assembly view.
pub fn panel(
    ui: &mut egui::Ui,
    doc: Option<&mut Document>,
    view: &mut AssemblyView,
    out: &mut Vec<Command>,
) {
    let ink = Ink::of(ui);
    ui.spacing_mut().item_spacing.y = 5.0;
    let heading = ui.visuals().widgets.noninteractive.fg_stroke.color;
    ui.label(RichText::new("集合体").size(15.0).color(heading));
    ui.add_space(2.0);
    let Some(doc) = doc else {
        ui.label(ink.muted("文書が開かれていません"));
        return;
    };
    let model_name = model_name(doc);
    let has_model = doc.three_d.model().is_some();
    let state = &mut doc.assembly;
    let mut changes = Vec::new();

    ui.horizontal(|ui| {
        if ui
            .add_enabled(state.can_undo(), egui::Button::new("↶ 元に戻す").small())
            .on_hover_text("集合体の編集を元に戻す（Ctrl+Z）")
            .clicked()
        {
            history(state, view, false);
        }
        if ui
            .add_enabled(state.can_redo(), egui::Button::new("↷ やり直し").small())
            .on_hover_text("元に戻した編集をやり直す（Ctrl+Y）")
            .clicked()
        {
            history(state, view, true);
        }
    });

    section(ui, "単量体");
    components_section(ui, state, view, &ink, &mut changes);
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(has_model, egui::Button::new("今の 3D モデルを追加"))
            .on_hover_text("この文書の 3D モデルを単量体として加えます")
            .on_disabled_hover_text("3D モデルがありません。先に「2D から 3D を生成」してください")
            .clicked()
        {
            changes.push(Change::AddModel);
        }
        if ui
            .add_enabled(three_d::AVAILABLE, egui::Button::new("ファイルから追加…"))
            .on_hover_text("MOL2・PDB・XYZ・MOL・SDF の 3D 構造を単量体として加えます")
            .on_disabled_hover_text(
                "3D 形式の読み込みには OpenBabel が必要です（この版にはありません）",
            )
            .clicked()
        {
            out.push(Command::AssemblyAddFile);
        }
    });

    section(ui, "手順");
    steps_section(ui, state, &ink, &mut changes);

    section(ui, "配置");
    place::section(ui, state, view, &ink);

    section(ui, "表示");
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("assembly_style")
            .selected_text(view.style.label())
            .show_ui(ui, |ui| {
                for style in Style3d::ALL {
                    ui.selectable_value(&mut view.style, style, style.label());
                }
            });
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(ink.muted("色"));
        for mode in ColorMode::ALL {
            ui.selectable_value(&mut view.color, mode, mode.label());
        }
    });

    section(ui, "情報");
    info_section(ui, state, &ink);
    place::clash_info(ui, state, view, &ink);

    section(ui, "書き出し");
    let can_export = state.built().is_ok_and(|b| !b.is_empty());
    ui.horizontal_wrapped(|ui| {
        for export in AssemblyExport::ALL {
            if ui
                .add_enabled(
                    can_export,
                    egui::Button::new(format!(".{}", export.extension())),
                )
                .on_hover_text(format!("{} に書き出す", export.description()))
                .on_disabled_hover_text("書き出す集合体がありません")
                .clicked()
            {
                out.push(Command::AssemblyExport(export));
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!state.is_empty(), egui::Button::new("集合体を保存…"))
            .on_hover_text(
                "単量体と手順を .odasm ファイルに保存します（開くと同じ集合体になります）",
            )
            .clicked()
        {
            out.push(Command::AssemblySave);
        }
        if ui
            .button("集合体を開く…")
            .on_hover_text(
                ".odasm ファイルを開いて、この文書の集合体と置き換えます（元に戻せます）",
            )
            .clicked()
        {
            out.push(Command::AssemblyOpen);
        }
    });

    if let Some((text, problem)) = &view.report {
        let color = if *problem { ink.warn } else { ink.muted };
        ui.add_space(4.0);
        ui.label(RichText::new(text).size(11.0).color(color));
    }

    for change in changes {
        apply(change, doc, &model_name, view);
    }
}

/// The name a monomer made from the document's 3D model gets: the file's name, or the
/// model's formula for a document never saved.
pub fn model_name(doc: &Document) -> String {
    doc.path
        .as_deref()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .or_else(|| doc.three_d.model().map(|m| m.formula.clone()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "monomer".to_owned())
}

fn apply(change: Change, doc: &mut Document, model_name: &str, view: &mut AssemblyView) {
    let state = &mut doc.assembly;
    match change {
        Change::AddModel => {
            if let Some(model) = doc.three_d.model() {
                let component = Component::from_model(model_name, model);
                view.report = Some((
                    format!(
                        "単量体「{}」（{} 原子）を追加しました",
                        component.name,
                        component.atoms.len()
                    ),
                    false,
                ));
                state.edit(|d| d.components.push(component));
            }
        }
        Change::RemoveComponent(i) => {
            state.edit(|d| {
                d.components.remove(i);
                d.manual.retain(|m| m.component != i);
                for m in &mut d.manual {
                    if m.component > i {
                        m.component -= 1;
                    }
                }
                if d.base > i || d.base >= d.components.len() {
                    d.base = d.base.saturating_sub(1);
                }
            });
            view.orient = None;
        }
        Change::SetBase(i) => state.edit(|d| d.base = i),
        Change::ClearOrient(i) => state.edit(|d| d.components[i].orient = None),
        Change::AddStep(step) => state.edit(|d| d.steps.push(step)),
        Change::RemoveStep(i) => {
            state.edit(|d| d.steps.remove(i));
        }
        Change::MoveStep(from, to) => state.edit(|d| d.steps.swap(from, to)),
    }
}

fn components_section(
    ui: &mut egui::Ui,
    state: &mut AssemblyState,
    view: &mut AssemblyView,
    ink: &Ink,
    changes: &mut Vec<Change>,
) {
    if state.design.components.is_empty() {
        ui.label(
            ink.muted("単量体がありません。3D モデルを追加するか、ファイルから読み込んでください"),
        );
        return;
    }
    let several = state.design.components.len() > 1;
    let base = state.design.base;
    let mut renamed = false;
    for (i, c) in state.design.components.iter_mut().enumerate() {
        ui.push_id(("component", i), |ui| {
            ui.horizontal(|ui| {
                if several
                    && ui
                        .radio(base == i, "")
                        .on_hover_text("手順で複製する単量体にする")
                        .clicked()
                {
                    changes.push(Change::SetBase(i));
                }
                renamed |= ui
                    .add(egui::TextEdit::singleline(&mut c.name).desired_width(110.0))
                    .on_hover_text(
                        "単量体の名前。先頭の英数字 3 文字が PDB・MOL2 の残基名になります",
                    )
                    .changed();
                ui.label(ink.muted(format!("{} 原子", c.atoms.len())));
                if ui
                    .small_button("✕")
                    .on_hover_text("この単量体を削除")
                    .clicked()
                {
                    changes.push(Change::RemoveComponent(i));
                }
            });
            ui.horizontal_wrapped(|ui| {
                match c.orient {
                    Some([a, b, o]) => {
                        ui.label(ink.muted(format!(
                            "向き：原子 {}・{}・{}",
                            a + 1,
                            b + 1,
                            o + 1
                        )));
                        if ui
                            .small_button("解除")
                            .on_hover_text("向き合わせをやめ、読み込んだままの座標を使う")
                            .clicked()
                        {
                            changes.push(Change::ClearOrient(i));
                        }
                    }
                    None => {
                        ui.label(ink.muted("向き：そのまま"));
                    }
                }
                let picking = view.orient.as_ref().is_some_and(|o| o.component == i);
                if ui
                    .add_enabled(!picking, egui::Button::new("3 原子で向き合わせ").small())
                    .on_hover_text(
                        "ビューアで原子を 3 つ順にクリックします。1 つ目を原点に、2 つ目を +x 軸上に、3 つ目を xy 平面（y < 0 側）に置きます。\n環状の手順では 1 つ目の原子が円の上に乗り、分子は +x（外向き）に伸びます",
                    )
                    .clicked()
                {
                    view.start_orienting(i);
                }
            });
        });
    }
    if renamed {
        state.touch();
    }
}

fn steps_section(
    ui: &mut egui::Ui,
    state: &mut AssemblyState,
    ink: &Ink,
    changes: &mut Vec<Change>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(ink.muted("追加"));
        if ui
            .button("環状")
            .on_hover_text("分子を z 軸まわりの円に並べます（ロゼット）")
            .clicked()
        {
            changes.push(Change::AddStep(Step::new_ring()));
        }
        if ui
            .button("積層・らせん")
            .on_hover_text("それまでの全体を z 方向に積み重ねます。ねじれとずれでらせんになります")
            .clicked()
        {
            changes.push(Change::AddStep(Step::new_stack()));
        }
        if ui
            .button("配列")
            .on_hover_text("それまでの全体を一定の間隔と回転で並べます")
            .clicked()
        {
            changes.push(Change::AddStep(Step::new_array()));
        }
    });
    if state.design.steps.is_empty() {
        ui.label(ink.muted(
            "手順がありません。手順は上から順に適用され、それぞれがそれまでの全体を複製します",
        ));
        return;
    }
    let count = state.design.steps.len();
    let mut changed = false;
    for (i, step) in state.design.steps.iter_mut().enumerate() {
        ui.push_id(("step", i), |ui| {
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        changed |= ui
                            .checkbox(step.enabled_mut(), "")
                            .on_hover_text("この手順を使う（外すと飛ばします）")
                            .changed();
                        ui.label(RichText::new(format!("{}. {}", i + 1, step.label())).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button("✕")
                                .on_hover_text("この手順を削除")
                                .clicked()
                            {
                                changes.push(Change::RemoveStep(i));
                            }
                            if ui
                                .add_enabled(i + 1 < count, egui::Button::new("↓").small())
                                .on_hover_text("後ろへ")
                                .clicked()
                            {
                                changes.push(Change::MoveStep(i, i + 1));
                            }
                            if ui
                                .add_enabled(i > 0, egui::Button::new("↑").small())
                                .on_hover_text("前へ")
                                .clicked()
                            {
                                changes.push(Change::MoveStep(i, i - 1));
                            }
                        });
                    });
                    ui.add_enabled_ui(step.enabled(), |ui| {
                        changed |= step_params(ui, step, ink);
                    });
                });
        });
    }
    if changed {
        state.touch();
    }
}

/// A labelled number with a tooltip; returns whether it changed.
fn number(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f64,
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
    suffix: &str,
    tip: &str,
) -> bool {
    ui.label(label).on_hover_text(tip);
    let changed = ui
        .add(
            DragValue::new(value)
                .speed(speed)
                .range(range)
                .max_decimals(3)
                .suffix(suffix),
        )
        .on_hover_text(tip)
        .changed();
    ui.end_row();
    changed
}

fn count(ui: &mut egui::Ui, label: &str, value: &mut u32, tip: &str) -> bool {
    ui.label(label).on_hover_text(tip);
    let changed = ui
        .add(DragValue::new(value).speed(0.1).range(1..=10_000))
        .on_hover_text(tip)
        .changed();
    ui.end_row();
    changed
}

fn vector(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut [f64; 3],
    speed: f64,
    suffix: &str,
    tip: &str,
) -> bool {
    ui.label(label).on_hover_text(tip);
    let mut changed = false;
    ui.horizontal(|ui| {
        for (axis, v) in ["x", "y", "z"].iter().zip(value.iter_mut()) {
            changed |= ui
                .add(
                    DragValue::new(v)
                        .speed(speed)
                        .max_decimals(3)
                        .prefix(format!("{axis} "))
                        .suffix(suffix),
                )
                .on_hover_text(tip)
                .changed();
        }
    });
    ui.end_row();
    changed
}

/// The step's parameters. Returns whether any changed.
fn step_params(ui: &mut egui::Ui, step: &mut Step, ink: &Ink) -> bool {
    let mut changed = false;
    egui::Grid::new("params")
        .num_columns(2)
        .spacing(vec2(8.0, 4.0))
        .show(ui, |ui| match step {
            Step::Ring(Ring {
                n,
                radius,
                tilt_deg,
                ..
            }) => {
                changed |= count(
                    ui,
                    "個数",
                    n,
                    "環に並べる分子の数。i 番目は z 軸まわりに 360°/個数 × i 回した位置に置かれます",
                );
                changed |= number(
                    ui,
                    "半径",
                    radius,
                    0.05,
                    0.0..=1000.0,
                    " Å",
                    "分子の原点（向き合わせの 1 つ目の原子）が乗る円の半径。小さすぎると重なり、大きすぎると水素結合が届きません",
                );
                changed |= number(
                    ui,
                    "傾き",
                    tilt_deg,
                    0.5,
                    -180.0..=180.0,
                    "°",
                    "円に置く前に分子を y 軸まわりに傾ける角度。0 なら分子は環の面（xy 平面）に寝たまま",
                );
            }
            Step::Stack(Stack {
                n,
                rise,
                twist_deg,
                slip,
                ..
            }) => {
                changed |= count(ui, "層数", n, "積み重ねる層（ディスク）の数");
                changed |= number(
                    ui,
                    "層間距離",
                    rise,
                    0.02,
                    -1000.0..=1000.0,
                    " Å",
                    "層と層の z 方向の間隔。π スタックならおよそ 3.4〜3.5 Å",
                );
                changed |= number(
                    ui,
                    "ねじれ",
                    twist_deg,
                    0.5,
                    -360.0..=360.0,
                    "°",
                    "1 層ごとに z 軸まわりに回す角度。正なら +z から見て反時計回り。\n周期的につなげるなら 層数 × ねじれ が 360°/環の個数 の倍数になるようにします",
                );
                changed |= number(
                    ui,
                    "ずれ",
                    slip,
                    0.02,
                    -100.0..=100.0,
                    " Å",
                    "隣り合う層の横方向のずれ。0 でなければ、層はらせん軸（z 軸）のまわりを\n半径 (ずれ/2) / sin(ねじれ/2) の円に沿って進みます。ねじれが 0 なら x 方向にまっすぐずれます",
                );
                let radius = helix_radius(*slip, *twist_deg);
                ui.label(ink.muted("らせん半径"));
                ui.label(ink.muted(match radius {
                    Some(r) => format!("{:.3} Å", r.abs()),
                    None if *slip != 0.0 => "—（ねじれ 0：まっすぐずれる）".to_owned(),
                    None => "—（ずれなし）".to_owned(),
                }));
                ui.end_row();
            }
            Step::Array(Array {
                n,
                offset,
                euler_deg,
                ..
            }) => {
                changed |= count(ui, "個数", n, "並べる数（1 つ目はそのままの位置）");
                changed |= vector(
                    ui,
                    "間隔",
                    offset,
                    0.05,
                    " Å",
                    "i 番目を i × 間隔 だけ動かします",
                );
                changed |= vector(
                    ui,
                    "回転",
                    euler_deg,
                    0.5,
                    "°",
                    "i 番目を原点まわりに i × 角度 回してから動かします（x → y → z 軸の順）",
                );
            }
        });
    changed
}

fn info_section(ui: &mut egui::Ui, state: &mut AssemblyState, ink: &Ink) {
    match state.built() {
        Ok(built) => {
            egui::Grid::new("assembly_info")
                .num_columns(2)
                .spacing(vec2(16.0, 3.0))
                .show(ui, |ui| {
                    ui.label(ink.muted("分子数"));
                    ui.label(built.molecule_count().to_string());
                    ui.end_row();
                    ui.label(ink.muted("原子数"));
                    ui.label(built.atoms.len().to_string());
                    ui.end_row();
                    ui.label(ink.muted("結合数"));
                    ui.label(built.bonds.len().to_string());
                    ui.end_row();
                });
            match state.selected {
                Some(m) if m < built.molecule_count() => {
                    let name = state
                        .design
                        .components
                        .get(built.component_of_molecule[m])
                        .map_or("", |c| c.name.as_str());
                    ui.label(
                        RichText::new(format!(
                            "選択：分子 {}（{name}、原子 {}〜{}）",
                            m + 1,
                            built.molecule_start[m] + 1,
                            built.molecule_start[m + 1]
                        ))
                        .size(12.0),
                    );
                }
                _ => {
                    ui.label(ink.muted("原子をクリックすると、その分子を選択します"));
                }
            }
        }
        Err(e) => {
            ui.label(RichText::new(e.to_string()).size(12.0).color(ink.warn));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::Design;
    use crate::three_d::{Atom3d, Bond3d};

    fn water() -> Component {
        Component {
            name: "water".into(),
            atoms: vec![
                Atom3d {
                    element: "O".into(),
                    pos: [0.0; 3],
                },
                Atom3d {
                    element: "H".into(),
                    pos: [0.96, 0.0, 0.0],
                },
                Atom3d {
                    element: "H".into(),
                    pos: [-0.24, 0.93, 0.0],
                },
            ],
            bonds: vec![
                Bond3d {
                    begin: 0,
                    end: 1,
                    order: 1,
                },
                Bond3d {
                    begin: 0,
                    end: 2,
                    order: 1,
                },
            ],
            orient: None,
        }
    }

    #[test]
    fn colours_follow_the_mode() {
        let mut state = AssemblyState::with_design(Design {
            components: vec![water()],
            steps: vec![Step::new_ring()],
            ..Default::default()
        });
        let built = state.built().unwrap();
        let by_element = atom_colors(&built, ColorMode::Element);
        assert_eq!(by_element[0], element_color("O"));
        let by_molecule = atom_colors(&built, ColorMode::Molecule);
        assert_eq!(by_molecule[0], by_molecule[2], "one molecule, one colour");
        assert_ne!(by_molecule[0], by_molecule[3], "the next molecule differs");
        let by_component = atom_colors(&built, ColorMode::Component);
        assert!(by_component.iter().all(|c| *c == by_component[0]));
    }

    #[test]
    fn three_picks_orient_the_monomer() {
        let mut state = AssemblyState::with_design(Design {
            components: vec![water()],
            ..Default::default()
        });
        let mut view = AssemblyView::default();
        view.start_orienting(0);
        pick_orient_atom(&mut state, &mut view, 1);
        pick_orient_atom(&mut state, &mut view, 0);
        // Clicking a picked atom takes it back.
        pick_orient_atom(&mut state, &mut view, 0);
        assert_eq!(view.orient.as_ref().unwrap().atoms, vec![1]);
        pick_orient_atom(&mut state, &mut view, 0);
        pick_orient_atom(&mut state, &mut view, 2);
        assert!(!view.is_orienting());
        assert_eq!(state.design.components[0].orient, Some([1, 0, 2]));
        assert!(state.can_undo());
        assert!(view.report.as_ref().is_some_and(|(_, problem)| !problem));
    }

    /// One headless frame of the panel and the viewer area, with and without a monomer.
    #[test]
    fn the_panel_and_viewer_draw_headless() {
        let ctx = egui::Context::default();
        let mut doc = Document::new(&chembuider_rs::Config::default(), 1);
        let mut view = AssemblyView::default();
        let mut commands = Vec::new();
        for round in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                panel(ui, Some(&mut doc), &mut view, &mut commands);
                viewer_area(ui, "test", Some(&mut doc), &mut view, None);
            });
            output.textures_delta.clear();
            if round == 0 {
                doc.assembly.edit(|d| {
                    d.components.push(water());
                    d.steps.push(Step::new_stack());
                });
            }
        }
        assert!(commands.is_empty());
    }
}
