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

mod contacts;
mod pipeline;
mod place;
mod style;

use eframe::egui_wgpu::RenderState;
use egui::{Color32, DragValue, Pos2, RichText, Stroke, vec2};
use serde::{Deserialize, Serialize};
use style::Look;

use super::Command;
use super::viewport3d::{
    Frame3d, Hit, Scene, Style3d, Viewport3d, element_color, exclusive_groups,
};
use crate::assembly::model::{Array, Lattice, Ring, Stack, helix_radius};
use crate::assembly::symop::{self, SymOp};
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

/// How a crystal is shown: its unit cell's outline, and periodic images of the whole built
/// crystal (its supercell) around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CrystalDisplay {
    pub show_cell: bool,
    /// Copies drawn along a, b, c, the crystal itself included (1 = none).
    pub images: [u32; 3],
}

impl Default for CrystalDisplay {
    fn default() -> Self {
        Self {
            show_cell: true,
            images: [1, 1, 1],
        }
    }
}

/// The most periodic images along one axis (the renderer draws at most 729 in all).
const MAX_IMAGES: u32 = 9;

/// Three atoms being picked to orient a monomer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OrientPick {
    component: usize,
    atoms: Vec<usize>,
}

/// What the three picked atoms become, in order.
const ORIENT_ROLES: [&str; 3] = ["原点", "+x 軸", "xy 平面"];

/// The right panel's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PanelTab {
    /// 表示: how the assembly is shown, what it holds, exporting.
    Display,
    /// 構築: the monomers, the steps and the hand placement. Last, as serde requires of the
    /// catch-all.
    #[default]
    #[serde(other)]
    Build,
}

/// The assembly view's state shared by every document: the viewer and the display options
/// (remembered between runs), and the orientation being picked.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AssemblyView {
    pub style: Style3d,
    pub color: ColorMode,
    pub crystal: CrystalDisplay,
    /// Which hydrogen bonds and π-stacks are drawn.
    pub interactions: super::interactions::ShowInteractions,
    /// The panel's tab.
    pub tab: PanelTab,
    /// The step whose card shows its parameters (one at most).
    pub expanded: Option<usize>,
    /// The monomer whose name is being edited, and whether its field still has to take focus.
    #[serde(skip)]
    renaming: Option<(usize, bool)>,
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
    /// Hydrogen bonds and π-stacks, found when the build changes.
    #[serde(skip)]
    contacts: contacts::ContactCheck,
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
    _title: &str,
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
    orient_prompt(ui, &doc.assembly, view);

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

/// While orienting a monomer, the prompt for the next atom and a cancel button (nothing
/// otherwise: the viewer has no header bar).
fn orient_prompt(ui: &mut egui::Ui, state: &AssemblyState, view: &mut AssemblyView) {
    let Some(pick) = &view.orient else { return };
    let mut cancel = false;
    ui.horizontal(|ui| {
        ui.add_space(12.0);
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
        cancel = ui
            .small_button("やめる")
            .on_hover_text("Esc でも")
            .clicked();
    });
    if cancel {
        view.orient = None;
    }
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
        viewport.set_cell(None, None);
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
    // A crystal: the unit cell's outline and periodic images of the whole build (clicking an
    // image selects the molecule it is a copy of: the renderer reports primary indices).
    viewport.set_cell(
        built
            .crystal
            .as_ref()
            .filter(|_| view.crystal.show_cell)
            .map(|c| c.cell.vectors()),
        built.cell.map(|c| (c.vectors(), view.crystal.images)),
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
    let show = view.interactions;
    if let Some((found, _)) = view.contacts.get(ui.ctx(), state.id(), &built, show) {
        contacts::overlay(ui, viewport, &frame, &built, &found, show);
    }
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

/// A structural change to the design, applied once the panel has let go of it.
enum Change {
    AddModel,
    RemoveComponent(usize),
    SetBase(usize),
    ClearOrient(usize),
    AddStep(Step),
    RemoveStep(usize),
    /// Move the step at the first index to the second (the others close up).
    MoveStep(usize, usize),
    /// Insert a copy of the step right after it.
    DuplicateStep(usize),
    /// Switch the step on or off.
    ToggleStep(usize),
}

/// The right panel in the assembly view: the heading and the tabs, the tab's contents
/// (scrolling), and a footer with the counts and undo / redo.
pub fn panel(
    ui: &mut egui::Ui,
    doc: Option<&mut Document>,
    view: &mut AssemblyView,
    out: &mut Vec<Command>,
) {
    let ink = Ink::of(ui);
    let look = Look::of(ui);
    ui.spacing_mut().item_spacing.y = 5.0;
    ui.label(RichText::new("集合体").size(15.0).strong().color(look.ink));
    ui.add_space(4.0);
    let Some(doc) = doc else {
        ui.label(ink.muted("文書が開かれていません"));
        return;
    };
    let tabs = [(PanelTab::Build, "構築"), (PanelTab::Display, "表示")];
    if let Some(tab) = style::tabs(ui, &look, view.tab, &tabs) {
        view.tab = tab;
    }
    let model_name = model_name(doc);
    let has_model = doc.three_d.model().is_some();
    let mut changes = Vec::new();

    egui::Panel::bottom("assembly_footer")
        .frame(egui::Frame::NONE.inner_margin(egui::Margin {
            top: 6,
            ..egui::Margin::ZERO
        }))
        .resizable(false)
        .show(ui, |ui| footer(ui, &mut doc.assembly, view, &look, &ink));
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let state = &mut doc.assembly;
            match view.tab {
                PanelTab::Build => {
                    build_tab(ui, state, view, &look, &ink, has_model, out, &mut changes);
                }
                PanelTab::Display => display_tab(ui, state, view, &look, &ink, out),
            }
            ui.add_space(8.0);
        });

    for change in changes {
        apply(change, doc, &model_name, view);
    }
}

/// The 構築 tab: the monomers, the steps' pipeline and the box adding one, and the hand
/// placement.
#[allow(clippy::too_many_arguments)]
fn build_tab(
    ui: &mut egui::Ui,
    state: &mut AssemblyState,
    view: &mut AssemblyView,
    look: &Look,
    ink: &Ink,
    has_model: bool,
    out: &mut Vec<Command>,
    changes: &mut Vec<Change>,
) {
    style::caption(ui, look, "単量体");
    components_section(ui, state, view, look, has_model, out, changes);
    if !state.is_empty() {
        ui.add_space(8.0);
        pipeline::pipeline(ui, state, view, look, ink, changes);
        ui.add_space(8.0);
        pipeline::add_box(ui, look, state, changes);
    }
    style::caption(ui, look, "配置");
    style::card(ui, look, false, 0.0, |ui| {
        place::section(ui, state, view, ink)
    });
}

/// The 表示 tab: how the assembly is shown, what it holds, and writing it out.
fn display_tab(
    ui: &mut egui::Ui,
    state: &mut AssemblyState,
    view: &mut AssemblyView,
    look: &Look,
    ink: &Ink,
    out: &mut Vec<Command>,
) {
    let has_crystal = state.built().is_ok_and(|b| b.crystal.is_some());
    style::caption(ui, look, "表示");
    style::card(ui, look, false, 0.0, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(ink.muted("スタイル"));
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
        crystal_display(ui, &mut view.crystal, has_crystal, ink);
        if ui
            .add_enabled(view.viewport.is_some(), egui::Button::new("視点をリセット"))
            .on_hover_text("何もない所をダブルクリックでも")
            .clicked()
            && let Some(viewport) = &mut view.viewport
        {
            viewport.reset_view();
        }
    });

    style::caption(ui, look, "相互作用");
    style::card(ui, look, false, 0.0, |ui| {
        contacts::toggles(ui, &mut view.interactions);
    });

    style::caption(ui, look, "情報");
    style::card(ui, look, false, 0.0, |ui| {
        info_section(ui, state, ink);
        place::clash_info(ui, state, view, ink);
        contacts::info(ui, state, view, ink);
    });

    style::caption(ui, look, "書き出し");
    style::card(ui, look, false, 0.0, |ui| {
        let can_export = state.built().is_ok_and(|b| !b.is_empty());
        ui.horizontal_wrapped(|ui| {
            for export in AssemblyExport::ALL
                .into_iter()
                .chain(AssemblyExport::CRYSTAL)
            {
                let enabled = can_export && (!export.needs_crystal() || has_crystal);
                if ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(format!(".{}", export.extension())),
                    )
                    .on_hover_text(if export.needs_crystal() {
                        format!(
                            "{} に書き出す（格子定数・対称操作・非対称単位の分率座標）",
                            export.description()
                        )
                    } else {
                        format!("{} に書き出す", export.description())
                    })
                    .on_disabled_hover_text(if !can_export {
                        "書き出す集合体がありません"
                    } else {
                        "CIF には結晶の手順が必要です（手順の「結晶」を追加してください）"
                    })
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
    });
}

/// The footer under both tabs: the last report, the molecule, atom and bond counts, and undo /
/// redo.
fn footer(
    ui: &mut egui::Ui,
    state: &mut AssemblyState,
    view: &mut AssemblyView,
    look: &Look,
    ink: &Ink,
) {
    if let Some((text, problem)) = &view.report {
        let color = if *problem { ink.warn } else { ink.muted };
        ui.label(RichText::new(text).size(11.0).color(color));
    }
    let (molecules, atoms, bonds) = match state.built() {
        Ok(b) => (
            b.molecule_count().to_string(),
            b.atoms.len().to_string(),
            b.bonds.len().to_string(),
        ),
        Err(_) => (
            state.design.molecule_count().to_string(),
            state.design.atom_count().to_string(),
            "—".to_owned(),
        ),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (k, (n, unit)) in [(molecules, "分子"), (atoms, "原子"), (bonds, "結合")]
            .into_iter()
            .enumerate()
        {
            if k > 0 {
                ui.add_space(8.0);
            }
            ui.label(RichText::new(n).size(12.5).strong().color(look.ink));
            ui.label(look.muted(unit));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            use super::icons::Icon;
            let redo = style::tool(
                ui,
                look,
                Icon::Redo,
                "元に戻した編集をやり直す（Ctrl+Y）",
                state.can_redo(),
                false,
                false,
            );
            if redo.clicked() {
                history(state, view, true);
            }
            let undo = style::tool(
                ui,
                look,
                Icon::Undo,
                "集合体の編集を元に戻す（Ctrl+Z）",
                state.can_undo(),
                false,
                false,
            );
            if undo.clicked() {
                history(state, view, false);
            }
        });
    });
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

/// A monomer's molecular formula in Hill order (C, H, then the rest alphabetically; all
/// alphabetically without carbon), e.g. "C10H8".
fn formula(c: &Component) -> String {
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for a in &c.atoms {
        *counts.entry(a.element.as_str()).or_default() += 1;
    }
    let mut order: Vec<&str> = Vec::new();
    if counts.contains_key("C") {
        order.push("C");
        if counts.contains_key("H") {
            order.push("H");
        }
    }
    let rest: Vec<&str> = counts
        .keys()
        .copied()
        .filter(|e| !order.contains(e))
        .collect();
    order.extend(rest);
    order
        .iter()
        .map(|e| match counts[e] {
            1 => (*e).to_owned(),
            n => format!("{e}{n}"),
        })
        .collect()
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
            view.renaming = None;
        }
        Change::SetBase(i) => state.edit(|d| d.base = i),
        Change::ClearOrient(i) => state.edit(|d| d.components[i].orient = None),
        Change::AddStep(step) => {
            let at = state.edit(|d| {
                d.steps.push(step);
                d.steps.len() - 1
            });
            view.expanded = Some(at);
        }
        Change::RemoveStep(i) => {
            state.edit(|d| d.steps.remove(i));
            view.expanded = match view.expanded {
                Some(e) if e == i => None,
                Some(e) if e > i => Some(e - 1),
                e => e,
            };
        }
        Change::MoveStep(from, to) => {
            if from == to || from.max(to) >= state.design.steps.len() {
                return;
            }
            state.edit(|d| {
                let step = d.steps.remove(from);
                d.steps.insert(to, step);
            });
            view.expanded = view
                .expanded
                .map(|e| pipeline::index_after_move(e, from, to));
        }
        Change::DuplicateStep(i) => {
            if let Some(step) = state.design.steps.get(i).cloned() {
                state.edit(|d| d.steps.insert(i + 1, step));
                view.expanded = Some(i + 1);
            }
        }
        Change::ToggleStep(i) => {
            if i < state.design.steps.len() {
                state.edit(|d| {
                    let on = d.steps[i].enabled_mut();
                    *on = !*on;
                });
            }
        }
    }
}

/// The monomers, one card each with its actions in a 「…」 menu; with none, how to add one.
fn components_section(
    ui: &mut egui::Ui,
    state: &mut AssemblyState,
    view: &mut AssemblyView,
    look: &Look,
    has_model: bool,
    out: &mut Vec<Command>,
    changes: &mut Vec<Change>,
) {
    if state.design.components.is_empty() {
        let inner = egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(look.muted(
                    "単量体がありません。3D モデルを追加するか、ファイルから読み込んでください",
                ));
                ui.horizontal_wrapped(|ui| {
                    add_buttons(ui, has_model, out, changes);
                });
            });
        style::dashed_box(ui, inner.response.rect, look.line);
        return;
    }
    let several = state.design.components.len() > 1;
    let base = state.design.base;
    let mut renamed = false;
    let mut stop_renaming = false;
    for (i, c) in state.design.components.iter_mut().enumerate() {
        ui.push_id(("component", i), |ui| {
            style::card(ui, look, false, 0.0, |ui| {
                ui.horizontal(|ui| {
                    ui.set_min_height(24.0);
                    match &mut view.renaming {
                        Some((k, focus)) if *k == i => {
                            let field = ui
                                .add(
                                    egui::TextEdit::singleline(&mut c.name)
                                        .font(egui::TextStyle::Monospace)
                                        .desired_width(130.0),
                                )
                                .on_hover_text(
                                    "単量体の名前。先頭の英数字 3 文字が PDB・MOL2 の残基名になります（Enter で確定）",
                                );
                            if std::mem::take(focus) {
                                field.request_focus();
                            } else if field.lost_focus() {
                                stop_renaming = true;
                            }
                            renamed |= field.changed();
                        }
                        _ => {
                            let name = ui
                                .add(
                                    egui::Label::new(
                                        RichText::new(&c.name)
                                            .monospace()
                                            .size(13.0)
                                            .color(look.ink),
                                    )
                                    .truncate()
                                    .sense(egui::Sense::click()),
                                )
                                .on_hover_text(format!(
                                    "{} 原子（ダブルクリックで名前を変更）",
                                    c.atoms.len()
                                ));
                            if name.double_clicked() {
                                view.renaming = Some((i, true));
                            }
                        }
                    }
                    if several && base == i {
                        ui.label(RichText::new("基準").size(10.5).color(look.accent))
                            .on_hover_text("手順で複製する単量体");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let more = style::tool(
                            ui,
                            look,
                            super::icons::Icon::More,
                            "単量体の操作",
                            true,
                            false,
                            false,
                        );
                        egui::Popup::menu(&more).show(|ui| {
                            add_buttons(ui, has_model, out, changes);
                            ui.separator();
                            let picking = view.orient.as_ref().is_some_and(|o| o.component == i);
                            if ui
                                .add_enabled(!picking, egui::Button::new("3 原子で向き合わせ"))
                                .on_hover_text(
                                    "ビューアで原子を 3 つ順にクリックします。1 つ目を原点に、2 つ目を +x 軸上に、3 つ目を xy 平面（y < 0 側）に置きます。\n環状の手順では 1 つ目の原子が円の上に乗り、分子は +x（外向き）に伸びます",
                                )
                                .clicked()
                            {
                                view.start_orienting(i);
                            }
                            if ui
                                .add_enabled(c.orient.is_some(), egui::Button::new("向き合わせを解除"))
                                .on_hover_text("向き合わせをやめ、読み込んだままの座標を使う")
                                .clicked()
                            {
                                changes.push(Change::ClearOrient(i));
                            }
                            if ui.button("名前を変更").clicked() {
                                view.renaming = Some((i, true));
                            }
                            if ui
                                .add_enabled(several && base != i, egui::Button::new("基準の単量体にする"))
                                .on_hover_text("手順で複製する単量体にする")
                                .clicked()
                            {
                                changes.push(Change::SetBase(i));
                            }
                            ui.separator();
                            if ui.button("削除").on_hover_text("この単量体を削除").clicked() {
                                changes.push(Change::RemoveComponent(i));
                            }
                        });
                        let formula = formula(c);
                        if formula != c.name {
                            ui.label(look.mono(formula, look.muted));
                        }
                    });
                });
                let picking = view.orient.as_ref().is_some_and(|o| o.component == i);
                if picking {
                    ui.label(RichText::new("向き合わせ中：ビューアで原子をクリック").size(11.0).color(look.accent));
                } else if let Some([a, b, o]) = c.orient {
                    ui.label(look.muted(format!("向き：原子 {}・{}・{}", a + 1, b + 1, o + 1)));
                }
            });
        });
    }
    if renamed {
        state.touch();
    }
    if stop_renaming {
        view.renaming = None;
    }
}

/// 今の 3D モデルを追加 and ファイルから追加….
fn add_buttons(
    ui: &mut egui::Ui,
    has_model: bool,
    out: &mut Vec<Command>,
    changes: &mut Vec<Change>,
) {
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
}

/// Width of the parameters' label column.
const LABEL_WIDTH: f32 = 56.0;
const FIELD_HEIGHT: f32 = 22.0;

/// One parameter row: the label, then `field` filling the rest of the width.
fn field_row(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    field: impl FnOnce(&mut egui::Ui) -> bool,
) -> bool {
    ui.horizontal(|ui| {
        let muted = theme::muted(ui.visuals().dark_mode);
        ui.allocate_ui_with_layout(
            vec2(LABEL_WIDTH, FIELD_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(LABEL_WIDTH);
                ui.label(RichText::new(label).size(12.0).color(muted))
                    .on_hover_text(tip);
            },
        );
        field(ui)
    })
    .inner
}

/// The width of each of `n` fields sharing the rest of the row.
fn share(ui: &egui::Ui, n: usize) -> f32 {
    let gaps = ui.spacing().item_spacing.x * (n.saturating_sub(1)) as f32;
    ((ui.available_width() - gaps) / n as f32).max(30.0)
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
    field_row(ui, label, tip, |ui| {
        ui.add_sized(
            vec2(share(ui, 1), FIELD_HEIGHT),
            DragValue::new(value)
                .speed(speed)
                .range(range)
                .max_decimals(3)
                .suffix(suffix),
        )
        .on_hover_text(tip)
        .changed()
    })
}

fn count(ui: &mut egui::Ui, label: &str, value: &mut u32, tip: &str) -> bool {
    field_row(ui, label, tip, |ui| {
        ui.add_sized(
            vec2(share(ui, 1), FIELD_HEIGHT),
            DragValue::new(value).speed(0.1).range(1..=10_000),
        )
        .on_hover_text(tip)
        .changed()
    })
}

fn vector(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut [f64; 3],
    speed: f64,
    suffix: &str,
    tip: &str,
) -> bool {
    field_row(ui, label, tip, |ui| {
        let width = share(ui, 3);
        let mut changed = false;
        for (axis, v) in ["x", "y", "z"].iter().zip(value.iter_mut()) {
            changed |= ui
                .add_sized(
                    vec2(width, FIELD_HEIGHT),
                    DragValue::new(v)
                        .speed(speed)
                        .max_decimals(2)
                        .prefix(format!("{axis} "))
                        .suffix(suffix),
                )
                .on_hover_text(tip)
                .changed();
        }
        changed
    })
}

/// The step's parameters. Returns whether any changed.
fn step_params(ui: &mut egui::Ui, step: &mut Step, ink: &Ink) -> bool {
    if let Step::Lattice(lattice) = step {
        return lattice_params(ui, lattice, ink);
    }
    let mut changed = false;
    ui.spacing_mut().item_spacing.y = 4.0;
    match step {
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
            field_row(
                ui,
                "らせん半径",
                "ずれとねじれから決まります",
                |ui| {
                    ui.label(ink.muted(match radius {
                        Some(r) => format!("{:.3} Å", r.abs()),
                        None if *slip != 0.0 => "—（ねじれ 0：まっすぐずれる）".to_owned(),
                        None => "—（ずれなし）".to_owned(),
                    }));
                    false
                },
            );
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
        Step::Lattice(_) => unreachable!("edited by lattice_params"),
    }
    changed
}

/// The operations text being edited, and the operations it was last made from or turned
/// into: when the step's operations differ from those (undo, a preset, an opened file) the
/// text is made anew; otherwise it is kept, even while it does not parse.
#[derive(Debug, Clone)]
struct OpsText {
    text: String,
    synced: Vec<SymOp>,
}

/// The text to edit for the step's operations `ops`: the `previous` text while the step still
/// has the operations it was synced with, otherwise `ops` written out afresh.
fn ops_buffer(previous: Option<OpsText>, ops: &[SymOp]) -> OpsText {
    previous
        .filter(|b| b.synced == ops)
        .unwrap_or_else(|| OpsText {
            text: symop::format_list(ops),
            synced: ops.to_vec(),
        })
}

/// The crystal step's editor: space group, cell constants, repeats and the operations as
/// text. Returns whether the step changed.
fn lattice_params(ui: &mut egui::Ui, l: &mut Lattice, ink: &Ink) -> bool {
    let mut changed = false;
    let preset = symop::matching_preset(&l.symops);
    ui.horizontal_wrapped(|ui| {
        ui.label("空間群")
            .on_hover_text("選ぶと下の対称操作がその空間群の一般位置で置き換わります");
        egui::ComboBox::from_id_salt("space_group")
            .selected_text(preset.map_or("（任意の操作）", |p| p.name))
            .show_ui(ui, |ui| {
                for p in symop::PRESETS {
                    if ui
                        .selectable_label(preset == Some(p), format!("{}  {}", p.name, p.hm))
                        .on_hover_text(p.system)
                        .clicked()
                    {
                        l.symops = p.symops();
                        changed = true;
                    }
                }
            });
        if let Some(p) = preset {
            ui.label(ink.muted(p.system));
        }
    });
    ui.spacing_mut().item_spacing.y = 4.0;
    {
        let c = &mut l.cell;
        for (name, value) in [("a", &mut c.a), ("b", &mut c.b), ("c", &mut c.c)] {
            changed |= number(
                ui,
                name,
                value,
                0.02,
                0.1..=1000.0,
                " Å",
                "単位胞の稜の長さ。a は x 軸に沿い、b は xy 平面にあります",
            );
        }
        for (name, value, tip) in [
            ("α", &mut c.alpha, "b と c のなす角"),
            ("β", &mut c.beta, "a と c のなす角"),
            ("γ", &mut c.gamma, "a と b のなす角"),
        ] {
            changed |= number(ui, name, value, 0.1, 1.0..=179.0, "°", tip);
        }
        let tip = "単位胞を a・b・c 方向にいくつ並べるか";
        changed |= field_row(ui, "繰り返し", tip, |ui| {
            let width = share(ui, 3);
            let mut changed = false;
            for (axis, n) in ["a", "b", "c"].iter().zip(l.reps.iter_mut()) {
                changed |= ui
                    .add_sized(
                        vec2(width, FIELD_HEIGHT),
                        DragValue::new(n)
                            .speed(0.05)
                            .range(1..=100)
                            .prefix(format!("{axis} ")),
                    )
                    .on_hover_text(tip)
                    .changed();
            }
            changed
        });
    }

    ui.label("対称操作").on_hover_text(
        "1 行に 1 つ、x,y,z 形式で（例: -x,y+1/2,-z+1/2）。CIF の _space_group_symop の行をそのまま貼り付けても読めます。x,y,z を必ず含めてください",
    );
    let id = ui.make_persistent_id("symops");
    let mut buffer = ops_buffer(ui.data_mut(|d| d.get_temp::<OpsText>(id)), &l.symops);
    let edited = ui
        .add(
            egui::TextEdit::multiline(&mut buffer.text)
                .font(egui::TextStyle::Monospace)
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        )
        .changed();
    let parsed = symop::parse_list(&buffer.text);
    if edited
        && let Ok(ops) = &parsed
        && *ops != l.symops
    {
        l.symops = ops.clone();
        buffer.synced = ops.clone();
        changed = true;
    }
    let problem = match &parsed {
        Err(e) => Some(e.clone()),
        Ok(_) => l.check().err(),
    };
    ui.data_mut(|d| d.insert_temp(id, buffer));
    if let Some(problem) = problem {
        ui.label(RichText::new(problem).size(11.0).color(ink.warn));
    } else {
        ui.label(ink.muted(format!(
            "{} 操作 × {} 胞（特殊位置で重なる分子は 1 つにまとめます）",
            l.symops.len(),
            l.cell_count()
        )));
    }
    changed
}

/// The crystal's display options in the 表示 section.
fn crystal_display(ui: &mut egui::Ui, display: &mut CrystalDisplay, has_crystal: bool, ink: &Ink) {
    ui.add_enabled_ui(has_crystal, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut display.show_cell, "単位胞を表示")
                .on_hover_text("原点の単位胞を枠で示します")
                .on_disabled_hover_text("結晶の手順があるときに使えます");
        });
        ui.horizontal_wrapped(|ui| {
            let tip = "組み上げた結晶全体（繰り返し後の箱）を a・b・c 方向にいくつ並べて見せるか。\n1 なら並べません。表示だけで、書き出しには含まれません。周期像をクリックすると元の分子を選択します";
            ui.label(ink.muted("周期像")).on_hover_text(tip);
            for (axis, n) in ["a", "b", "c"].iter().zip(display.images.iter_mut()) {
                ui.add(
                    DragValue::new(n)
                        .speed(0.05)
                        .range(1..=MAX_IMAGES)
                        .prefix(format!("{axis} ")),
                )
                .on_hover_text(tip)
                .on_disabled_hover_text("結晶の手順があるときに使えます");
            }
        });
    });
}

fn info_section(ui: &mut egui::Ui, state: &mut AssemblyState, ink: &Ink) {
    match state.built() {
        Ok(built) => {
            egui::Grid::new("assembly_info")
                .num_columns(2)
                .spacing(vec2(16.0, 3.0))
                .show(ui, |ui| {
                    // The counts are in the footer; a crystal adds its cell here.
                    if let Some(crystal) = &built.crystal {
                        ui.label(ink.muted("単位胞"))
                            .on_hover_text("単位胞の体積");
                        ui.label(format!("{:.1} Å³", crystal.cell.volume()));
                        ui.end_row();
                        ui.label(ink.muted("Z")).on_hover_text(
                            "単位胞 1 つに入る分子の数（非対称単位 × 対称操作から、特殊位置で重なるものを除いた数）",
                        );
                        ui.label(if crystal.asymmetric.len() == 1 {
                            crystal.z.to_string()
                        } else {
                            format!(
                                "{}（非対称単位 {} 分子）",
                                crystal.z,
                                crystal.asymmetric.len()
                            )
                        });
                        ui.end_row();
                        ui.label(ink.muted("密度"))
                            .on_hover_text("単位胞の中身の質量 ÷ 単位胞の体積");
                        ui.label(match built.density() {
                            Some(d) => format!("{d:.3} g/cm³"),
                            None => "—（原子量のない元素があります）".to_owned(),
                        });
                        ui.end_row();
                    }
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

    /// The crystal editor and the crystal's display options draw, a bad cell included, and
    /// the operations text follows the step when it changes from outside (undo).
    #[test]
    fn the_crystal_editor_draws_headless_and_follows_undo() {
        let ctx = egui::Context::default();
        let mut doc = Document::new(&chembuider_rs::Config::default(), 1);
        doc.assembly.edit(|d| {
            d.components.push(water());
            d.steps.push(Step::new_lattice());
        });
        let mut view = AssemblyView::default();
        let mut commands = Vec::new();
        let mut frame = |doc: &mut Document, view: &mut AssemblyView| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                panel(ui, Some(doc), view, &mut commands);
                viewer_area(ui, "test", Some(doc), view, None);
            });
            output.textures_delta.clear();
        };
        frame(&mut doc, &mut view);
        let p21c = symop::PRESETS.iter().find(|p| p.name == "P2₁/c").unwrap();
        doc.assembly.edit(|d| {
            if let Step::Lattice(l) = &mut d.steps[0] {
                l.symops = p21c.symops();
                // Each angle in its field's range, but α ≥ β + γ: no such cell.
                l.cell.alpha = 150.0;
                l.cell.beta = 40.0;
                l.cell.gamma = 100.0;
            }
        });
        frame(&mut doc, &mut view);
        assert!(doc.assembly.built().is_err(), "an impossible cell");
        doc.assembly.undo();
        frame(&mut doc, &mut view);
        assert!(doc.assembly.built().unwrap().crystal.is_some());
        assert!(commands.is_empty());

        // Text that does not parse is kept while the step is unchanged; a step changed from
        // outside replaces it.
        let p1 = symop::PRESETS[0].symops();
        let typing = OpsText {
            text: "x,y,z\n-x,".to_owned(),
            synced: p1.clone(),
        };
        assert_eq!(ops_buffer(Some(typing.clone()), &p1).text, typing.text);
        let undone = ops_buffer(Some(typing), &p21c.symops());
        assert_eq!(undone.text, symop::format_list(&p21c.symops()));
        assert_eq!(ops_buffer(None, &p1).text, "x,y,z");
    }

    fn array(n: u32, z: f64) -> Step {
        Step::Array(Array {
            enabled: true,
            n,
            offset: [0.0, 0.0, z],
            euler_deg: [0.0; 3],
        })
    }

    fn doc_with(steps: Vec<Step>) -> Document {
        let mut doc = Document::new(&chembuider_rs::Config::default(), 1);
        doc.assembly.edit(|d| {
            d.components.push(water());
            d.steps = steps;
        });
        doc
    }

    #[test]
    fn the_pipeline_shows_each_steps_count_and_summary() {
        let steps = vec![array(7, 3.2), Step::new_ring()];
        let doc = doc_with(steps.clone());
        let counts = doc.assembly.design.running_counts();
        assert_eq!(counts, vec![7, 42]);
        assert_eq!(
            pipeline::count_text(&steps[0], 1, counts[0]),
            "× 7 → 7 分子"
        );
        assert_eq!(
            pipeline::count_text(&steps[1], counts[0], counts[1]),
            "× 6 → 42 分子"
        );
        assert_eq!(pipeline::summary(&steps[0]), "7個 · z 3.20Å");
        assert_eq!(pipeline::summary(&steps[1]), "6個 · r 5.00Å");
        assert_eq!(pipeline::summary(&Step::new_lattice()), "P1 · 1×1×1");
        let mut off = steps[1].clone();
        *off.enabled_mut() = false;
        assert_eq!(pipeline::count_text(&off, 7, 7), "飛ばす → 7 分子");
        assert_eq!(formula(&water()), "H2O");
    }

    #[test]
    fn a_dropped_card_moves_its_step_as_one_undo_step() {
        // Dropping in the gap a card is next to does not move it.
        assert_eq!(pipeline::drop_target(1, 1), None);
        assert_eq!(pipeline::drop_target(1, 2), None);
        assert_eq!(pipeline::drop_target(0, 3), Some(2));
        assert_eq!(pipeline::drop_target(2, 0), Some(0));
        let cards: Vec<egui::Rect> = (0..3)
            .map(|k| egui::Rect::from_min_size(egui::pos2(0.0, 50.0 * k as f32), vec2(100.0, 40.0)))
            .collect();
        assert_eq!(pipeline::gap_at(&cards, 5.0), 0);
        assert_eq!(pipeline::gap_at(&cards, 30.0), 1);
        assert_eq!(pipeline::gap_at(&cards, 500.0), 3);

        let mut doc = doc_with(vec![array(2, 3.0), Step::new_ring(), Step::new_stack()]);
        let mut view = AssemblyView {
            expanded: Some(0),
            ..Default::default()
        };
        let before = doc.assembly.design.steps.clone();
        apply(Change::MoveStep(0, 2), &mut doc, "m", &mut view);
        let labels: Vec<_> = doc.assembly.design.steps.iter().map(Step::label).collect();
        assert_eq!(labels, ["環状", "積層・らせん", "配列"]);
        assert_eq!(view.expanded, Some(2), "the open card moves with its step");
        assert!(doc.assembly.undo());
        assert_eq!(doc.assembly.design.steps, before, "one undo step");
        assert_eq!(pipeline::index_after_move(1, 0, 2), 0);
        assert_eq!(pipeline::index_after_move(1, 2, 0), 2);
        assert_eq!(pipeline::index_after_move(0, 2, 1), 0);
    }

    #[test]
    fn duplicate_delete_and_toggle_are_undoable() {
        let mut doc = doc_with(vec![array(2, 3.0), Step::new_ring()]);
        let mut view = AssemblyView::default();
        let start = doc.assembly.design.steps.clone();

        apply(Change::DuplicateStep(0), &mut doc, "m", &mut view);
        assert_eq!(doc.assembly.design.steps.len(), 3);
        assert_eq!(doc.assembly.design.steps[1], start[0]);
        assert_eq!(view.expanded, Some(1));
        assert_eq!(doc.assembly.design.running_counts(), vec![2, 4, 24]);

        apply(Change::ToggleStep(1), &mut doc, "m", &mut view);
        assert!(!doc.assembly.design.steps[1].enabled());
        assert_eq!(doc.assembly.design.running_counts(), vec![2, 2, 12]);

        apply(Change::RemoveStep(1), &mut doc, "m", &mut view);
        assert_eq!(doc.assembly.design.steps, start);
        assert_eq!(view.expanded, None, "the open card was deleted");

        // Undo walks back through delete, toggle and duplicate one at a time.
        assert!(doc.assembly.undo());
        assert!(!doc.assembly.design.steps[1].enabled());
        assert!(doc.assembly.undo());
        assert!(doc.assembly.design.steps[1].enabled());
        assert_eq!(doc.assembly.design.steps.len(), 3);
        assert!(doc.assembly.undo());
        assert_eq!(doc.assembly.design.steps, start);
    }

    /// Both tabs, with every card open in turn (the crystal's included), draw headless.
    #[test]
    fn both_tabs_draw_headless_with_each_card_open() {
        let ctx = egui::Context::default();
        let mut doc = doc_with(vec![
            array(7, 3.2),
            Step::new_ring(),
            Step::new_stack(),
            Step::new_lattice(),
        ]);
        doc.assembly.edit(|d| d.components.push(water()));
        let mut view = AssemblyView::default();
        let mut commands = Vec::new();
        for (tab, expanded) in [
            (PanelTab::Build, None),
            (PanelTab::Build, Some(0)),
            (PanelTab::Build, Some(1)),
            (PanelTab::Build, Some(2)),
            (PanelTab::Build, Some(3)),
            (PanelTab::Display, None),
        ] {
            view.tab = tab;
            view.expanded = expanded;
            view.renaming = Some((1, true));
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                panel(ui, Some(&mut doc), &mut view, &mut commands);
            });
            output.textures_delta.clear();
            assert_eq!(view.expanded, expanded);
        }
        assert!(commands.is_empty());
    }
}
