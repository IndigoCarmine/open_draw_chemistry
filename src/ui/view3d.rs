//! The 3D view, shown instead of the drawing: the viewer showing the document's 3D model
//! ([`super::viewport3d`]) in the central area ([`viewer_area`]), and in the right panel
//! ([`edit_panel`]) the tools for editing it by hand ([`crate::three_d::edit`]) and the
//! controls that generate, optimise and export it ([`crate::three_d`]).
//!
//! In the viewer: drag to orbit, Shift+drag or right/middle drag to pan, wheel to zoom,
//! double-click empty space to reframe. Click atoms to pick them (up to four: distance, angle,
//! dihedral), click a bond to pick its two ends, right-click an atom to fix it in place. With a
//! rotatable bond picked the wheel turns it instead of zooming. Ctrl+click an atom (or a bond,
//! for both ends) or Ctrl+drag a rectangle to select atoms for deletion; Delete removes them, or
//! the picked atoms when none are selected. In a model of several molecules, Shift+drag one to
//! move it against the others and Alt+drag it to turn it, as in the assembly view.

use egui::{Color32, Pos2, RichText, Stroke, vec2};
use serde::{Deserialize, Serialize};

use super::Command;
use super::interactions::{self as contacts, ShowInteractions};
use super::viewport3d::{CameraState, Frame3d, Hit, Style3d, Viewport3d, exclusive_groups};
use crate::assembly::place::{self, ScreenBasis};
use crate::document::Document;
use crate::theme::{self, dark};
use crate::three_d::edit::{self, Measure, MoleculeDrag, Motion, Target};
use crate::three_d::geom::{self, Clash, Goal};
use crate::three_d::interactions::{self, Interactions};
use crate::three_d::{self, Constraint, Export3d, ForceField, JobState, Model3d};
use eframe::egui_wgpu::RenderState;

/// Degrees one wheel notch (or step button) turns a bond; Shift for fine, Ctrl for coarse.
const STEP: f64 = 5.0;
const STEP_FINE: f64 = 1.0;
const STEP_COARSE: f64 = 15.0;

/// Highlight colours in the viewer (linear-ish RGB, drawn translucent).
const PICK_COLOR: (f32, f32, f32) = (0.98, 0.45, 0.22);
const ANCHOR_COLOR: (f32, f32, f32) = (0.20, 0.75, 0.98);
pub(super) const CLASH_COLOR: (f32, f32, f32) = (1.0, 0.12, 0.12);
const MOVING_COLOR: (f32, f32, f32) = (0.55, 0.62, 0.80);
/// The atoms selected for deletion.
const SELECT_COLOR: (f32, f32, f32) = (0.92, 0.35, 0.85);

const SELECT_STROKE: Color32 = Color32::from_rgb(0xE8, 0x6A, 0xD8);
const SELECT_ON_LIGHT: Color32 = Color32::from_rgb(0x9A, 0x23, 0x8C);
const PICK_STROKE: Color32 = Color32::from_rgb(0xF5, 0x8A, 0x4F);
const ANCHOR_STROKE: Color32 = Color32::from_rgb(0x4C, 0xC2, 0xF5);
pub(super) const CLASH_STROKE: Color32 = Color32::from_rgb(0xFF, 0x4D, 0x4D);
/// The same roles as text on the light theme's panel (4.5:1 on it).
const ANCHOR_ON_LIGHT: Color32 = Color32::from_rgb(0x1B, 0x6E, 0x9C);
pub(super) const CLASH_ON_LIGHT: Color32 = Color32::from_rgb(0xB4, 0x23, 0x23);
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
    /// Which hydrogen bonds and π-stacks are drawn.
    pub interactions: ShowInteractions,
    #[serde(skip)]
    viewport: Option<Viewport3d>,
    /// Clashes of the geometry last shown, recomputed when the atoms move.
    #[serde(skip)]
    clashes: (Vec<[f64; 3]>, Vec<Clash>),
    /// Hydrogen bonds and π-stacks of the geometry (and bond count) last shown, the same way.
    #[serde(skip)]
    contacts: ((Vec<[f64; 3]>, usize), Interactions),
    /// The value box: the atoms it was filled for, and its text.
    #[serde(skip)]
    value: (Vec<usize>, String),
    /// The selection rectangle being dragged with Ctrl held: where it started and where the
    /// pointer is.
    #[serde(skip)]
    band: Option<(Pos2, Pos2)>,
    /// What was under the pointer when the primary button went down, or is now while it is
    /// up: where a drag starts.
    #[serde(skip)]
    hover: Option<Hit>,
    /// The molecule being dragged with Shift or Alt held.
    #[serde(skip)]
    grab: Option<Grab>,
}

/// A molecule taken hold of in the viewer.
struct Grab {
    /// The atom it was taken by: the drag is scaled at its depth, so it stays under the pointer.
    handle: usize,
    drag: MoleculeDrag,
    /// It holds the fixed atom, so it stays where it is.
    held: bool,
}

/// The molecule a primary drag starting on `hover` would take hold of, with the atom it is
/// taken by: one of several molecules of `model`, with Shift or Alt held and Ctrl (which
/// selects) not. `None` leaves the drag to the camera or the selection rectangle, as it always
/// is for a model of one molecule.
fn grab_reach(
    model: &Model3d,
    hover: Option<Hit>,
    modifiers: egui::Modifiers,
) -> Option<(usize, Vec<usize>)> {
    if modifiers.command || !(modifiers.shift || modifiers.alt) {
        return None;
    }
    let handle = match hover? {
        Hit::Atom(atom) => atom,
        Hit::Bond(bond) => model.bonds.get(bond)?.begin,
        Hit::Nothing => return None,
    };
    Some((handle, edit::separate_molecule(model, handle)?))
}

/// What a drag of `delta` points does to the molecule held, seen through `basis`: it follows
/// the pointer in the plane of the screen, or with `turn` (Alt) turns like a trackball.
fn drag_motion(basis: &ScreenBasis, delta: egui::Vec2, turn: bool) -> Option<Motion> {
    let delta = [f64::from(delta.x), f64::from(delta.y)];
    if delta == [0.0, 0.0] {
        return None;
    }
    Some(if turn {
        Motion::Turn(place::drag_rotation(basis, delta))
    } else {
        Motion::Move(place::drag_translation(basis, delta))
    })
}

impl View3d {
    /// Free the viewer's GPU texture (at exit).
    pub fn free(&mut self, render_state: &RenderState) {
        if let Some(viewport) = &mut self.viewport {
            viewport.free(render_state);
        }
    }

    /// The view as last drawn, as a PNG with a transparent background (see
    /// [`Viewport3d::render_png`]).
    pub fn render_png(&mut self, render_state: &RenderState) -> Result<Vec<u8>, String> {
        match &mut self.viewport {
            Some(viewport) => viewport.render_png(render_state),
            None => Err(t!("three_d.error.not_shown").to_owned()),
        }
    }

    /// Where the camera is and the size the view was last shown at (points); `None` before
    /// the viewer has shown a model.
    pub fn camera(&self) -> Option<(CameraState, egui::Vec2)> {
        let viewport = self.viewport.as_ref()?;
        Some((viewport.camera_state(), viewport.shown_points()?))
    }

    /// When the viewer next loads a model, put the camera at `state` instead of framing the
    /// model (see [`Viewport3d::prefer_camera`]).
    pub fn prefer_camera(&mut self, state: Option<CameraState>) {
        self.viewport
            .get_or_insert_with(Viewport3d::new)
            .prefer_camera(state);
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

    /// The model's hydrogen bonds and π-stacks (those inside it included), recomputed when its
    /// atoms move or its bonds change. A single model is small: this runs on the UI thread.
    fn contacts_for(&mut self, model: &Model3d) -> &Interactions {
        let key = (edit::coords(model), model.bonds.len());
        if self.contacts.0 != key {
            self.contacts.1 = interactions::of_model(model);
            self.contacts.0 = key;
        }
        &self.contacts.1
    }
}

/// The 3D view in the window's central area: the viewer and a line naming what it shows, or
/// why there is nothing to show. The controls are in the right panel ([`edit_panel`]).
pub fn viewer_area(
    ui: &mut egui::Ui,
    _title: &str,
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
    if running {
        running_overlay(ui);
    }

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

/// While generation or optimisation runs: a small spinner in the viewer's top-left corner,
/// over the model, taking clicks only in its own area.
fn running_overlay(ui: &egui::Ui) {
    let pos = ui.max_rect().left_top() + egui::vec2(10.0, 10.0);
    egui::Area::new(ui.scope_id().with("running3d"))
        .fixed_pos(pos)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(theme::DARK_SURFACE.gamma_multiply(0.8))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new(t!("three_d.view.working"))
                                .size(11.0)
                                .color(dark::TEXT),
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
    ui.label(
        RichText::new(t!("three_d.view.title"))
            .size(15.0)
            .color(heading),
    );
    ui.add_space(2.0);

    section(ui, t!("three_d.view.section_display"));
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("style3d")
            .selected_text(view.style.label())
            .show_ui(ui, |ui| {
                for style in Style3d::ALL {
                    ui.selectable_value(&mut view.style, style, style.label());
                }
            });
        ui.toggle_value(&mut view.numbers, t!("three_d.view.atom_numbers"))
            .on_hover_text(t!("three_d.view.atom_numbers_tip"));
    });
    contacts::toggles(ui, &mut view.interactions);
    if ui
        .add_enabled(
            view.viewport.is_some(),
            egui::Button::new(t!("three_d.view.reset_view")),
        )
        .on_hover_text(t!("three_d.view.reset_view_tip"))
        .clicked()
        && let Some(viewport) = &mut view.viewport
    {
        viewport.reset_view();
    }

    section(ui, t!("three_d.view.section_model"));
    model_section(ui, doc.as_deref(), &ink, out);

    if let Some(doc) = doc.as_deref_mut()
        && doc.three_d.shown_model().is_some()
    {
        section(ui, t!("three_d.view.section_edit"));
        tools(ui, doc, view, &ink);
    }

    section(ui, t!("three_d.view.section_export"));
    let has_model = doc.as_deref().is_some_and(|d| d.three_d.model().is_some());
    ui.horizontal_wrapped(|ui| {
        for export in Export3d::ALL {
            if ui
                .add_enabled(
                    has_model,
                    egui::Button::new(format!(".{}", export.extension())),
                )
                .on_hover_text(t!("three_d.view.export_tip", format = export.description()))
                .on_disabled_hover_text(t!("three_d.view.export_no_model"))
                .clicked()
            {
                out.push(Command::Export3d(export));
            }
        }
        let has_view = doc
            .as_deref()
            .is_some_and(|d| d.three_d.shown_model().is_some());
        if ui
            .add_enabled(has_view, egui::Button::new(".png"))
            .on_hover_text(t!("three_d.view.export_image_tip"))
            .on_disabled_hover_text(t!("three_d.view.export_no_model"))
            .clicked()
        {
            out.push(Command::Export3dImage);
        }
        // An object Office embeds and edits again here (on Windows; a picture elsewhere).
        if ui
            .add_enabled(has_view, egui::Button::new(t!("three_d.view.copy_object")))
            .on_hover_text(if cfg!(windows) {
                t!("three_d.view.copy_object_tip")
            } else {
                t!("three_d.view.copy_image_tip")
            })
            .on_disabled_hover_text(t!("three_d.view.export_no_model"))
            .clicked()
        {
            out.push(Command::Copy3d);
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
    select: Color32,
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
                select: SELECT_STROKE,
                anchor: ANCHOR_STROKE,
                clash: CLASH_STROKE,
            }
        } else {
            Self {
                muted: theme::light::MUTED,
                warn: theme::ORANGE_TEXT,
                pick: theme::ORANGE_TEXT,
                select: SELECT_ON_LIGHT,
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
    // With Ctrl held a drag draws the selection rectangle instead of turning the view; with
    // Shift or Alt, one that starts on a molecule among several moves that molecule.
    if !editable {
        view.band = None;
        view.grab = None;
    }
    let modifiers = ui.input(|i| i.modifiers);
    let reach = (editable && view.band.is_none() && view.grab.is_none())
        .then(|| grab_reach(model, view.hover, modifiers))
        .flatten();
    viewport.claim_drag(
        editable
            && (view.band.is_some() || view.grab.is_some() || modifiers.command || reach.is_some()),
    );

    let picked = job.edit.picked.clone();
    let selected: Vec<usize> = job.edit.selected.iter().copied().collect();
    let anchor = job.edit.anchor;
    // A molecule holding the fixed atom is not moved.
    let held = matches!((&reach, anchor), (Some((_, atoms)), Some(a)) if atoms.contains(&a));
    // The molecule in hand, or the one a drag from here would take: shown as what moves.
    let carried: Vec<usize> = match (&view.grab, &reach) {
        (Some(grab), _) if !grab.held => grab.drag.atoms().to_vec(),
        (None, Some((_, atoms))) if !held => atoms.clone(),
        _ => Vec::new(),
    };
    let cursor = match (&view.grab, &reach) {
        (Some(grab), _) if !grab.held => Some(egui::CursorIcon::Grabbing),
        (None, Some(_)) if !held => Some(egui::CursorIcon::Grab),
        (None, None) => None,
        _ => Some(egui::CursorIcon::NotAllowed),
    };
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
            (&selected, SELECT_COLOR),
            (&clash_atoms, CLASH_COLOR),
            (&moving, MOVING_COLOR),
            (&carried, MOVING_COLOR),
        ]),
        rotor.map(|(b, c)| vec![(b, c)]).unwrap_or_default(),
    );

    let frame = match viewport.show(ui, render_state, rotor.is_some()) {
        Ok(frame) => frame,
        Err(e) => {
            ui.label(
                RichText::new(t!("three_d.view.render_failed", error = e))
                    .color(theme::ORANGE_ON_DARK),
            );
            return;
        }
    };
    let show = view.interactions;
    if show.any() {
        view.contacts_for(&model);
        if let Some(viewport) = &view.viewport {
            contacts::draw(
                &ui.painter_at(frame.rect),
                |p| viewport.project(frame.rect, p),
                |a| model.atoms.get(a).map(|a| a.pos),
                &view.contacts.1,
                show,
            );
        }
    }
    let viewport = view.viewport.get_or_insert_with(Viewport3d::new);
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
    let (down, pressed) = ui.input(|i| (i.pointer.primary_down(), i.pointer.primary_pressed()));
    if !down || pressed {
        view.hover = frame.hovered;
    }
    if let Some(cursor) = cursor {
        ui.ctx().set_cursor_icon(cursor);
    }
    // A drag carries the molecule it took hold of; otherwise the rectangle follows it, and
    // letting go selects the atoms inside it.
    let mut banded = Vec::new();
    let mut motion = None;
    let mut released = None;
    if let Some(delta) = frame.drag {
        if view.grab.is_none()
            && let Some((handle, atoms)) = reach
        {
            view.grab = Some(Grab {
                handle,
                drag: MoleculeDrag::new(atoms),
                held,
            });
        }
        if let Some(grab) = &view.grab {
            motion = model.atoms.get(grab.handle).and_then(|atom| {
                let basis = viewport.screen_basis(frame.rect, atom.pos);
                drag_motion(&basis, delta, modifiers.alt)
            });
        } else {
            let (origin, pointer) =
                ui.input(|i| (i.pointer.press_origin(), i.pointer.latest_pos()));
            view.band = match view.band {
                Some((start, end)) => Some((start, pointer.unwrap_or(end + delta))),
                None => origin.map(|start| (start, pointer.unwrap_or(start + delta))),
            };
        }
    } else {
        released = view.grab.take();
        if let Some((start, end)) = view.band.take() {
            let area = egui::Rect::from_two_pos(start, end);
            banded = atoms_inside(&model, area, |p| viewport.project(frame.rect, p));
        }
    }
    if let Some((start, end)) = view.band {
        ui.painter_at(frame.rect).rect(
            egui::Rect::from_two_pos(start, end),
            0.0,
            SELECT_STROKE.gamma_multiply(0.15),
            Stroke::new(1.0, SELECT_STROKE),
            egui::StrokeKind::Inside,
        );
    }
    if editable {
        carry(doc, view.grab.as_mut(), motion, released);
        react(ui, doc, &frame, rotor, banded);
    }
}

/// Move the molecule in hand by this frame's `motion` (or say why it stays), and close the
/// drag that was `released`.
fn carry(
    doc: &mut Document,
    grab: Option<&mut Grab>,
    motion: Option<Motion>,
    released: Option<Grab>,
) {
    let Some((model, edit3d)) = doc.three_d.model_and_edit() else {
        return;
    };
    if let (Some(grab), Some(motion)) = (grab, motion) {
        if grab.held {
            edit3d.report = Some((t!("three_d.view.molecule_fixed").to_owned(), true));
        } else {
            edit::drag_molecule(model, edit3d, &mut grab.drag, motion);
        }
    }
    if let Some(grab) = released {
        edit::finish_molecule_drag(model, edit3d, &grab.drag);
    }
}

/// The atoms of `model` drawn inside `area` of the screen, in front of the camera.
fn atoms_inside(
    model: &Model3d,
    area: egui::Rect,
    project: impl Fn([f64; 3]) -> Option<Pos2>,
) -> Vec<usize> {
    model
        .atoms
        .iter()
        .enumerate()
        .filter(|(_, atom)| project(atom.pos).is_some_and(|p| area.contains(p)))
        .map(|(i, _)| i)
        .collect()
}

/// Act on this frame's clicks and wheel, and on the atoms a selection rectangle took in.
fn react(
    ui: &egui::Ui,
    doc: &mut Document,
    frame: &Frame3d,
    rotor: Option<(usize, usize)>,
    banded: Vec<usize>,
) {
    let Some((model, edit3d)) = doc.three_d.model_and_edit() else {
        return;
    };
    edit3d.selected.extend(banded);
    // With Ctrl a click selects for deletion instead of picking for a measurement.
    let select = ui.input(|i| i.modifiers.command);
    match frame.primary {
        Some(Hit::Atom(i)) if select => edit3d.toggle_selected(i),
        Some(Hit::Bond(b)) if select => {
            if let Some(bond) = model.bonds.get(b) {
                edit3d.toggle_selected_bond(bond.begin, bond.end);
            }
        }
        Some(Hit::Nothing) if select => {}
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
        Some(Hit::Nothing) => {
            edit3d.clear_picks();
            edit3d.selected.clear();
        }
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
        tag(
            p + vec2(7.0, 4.0),
            t!("three_d.view.fixed").to_owned(),
            ANCHOR_STROKE,
        );
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

/// Undo/redo, deleting the selected atoms and clearing the selection from the keyboard while
/// the pointer is over the panel.
fn shortcuts(ui: &egui::Ui, doc: &mut Document) {
    use egui::{Key, KeyboardShortcut, Modifiers};
    if doc.three_d.is_running() {
        return;
    }
    let (redo, undo, escape, delete) = ui.input_mut(|i| {
        let redo = i.consume_shortcut(&KeyboardShortcut::new(
            Modifiers::COMMAND | Modifiers::SHIFT,
            Key::Z,
        )) || i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Y));
        let undo = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z));
        let escape = i.consume_key(Modifiers::NONE, Key::Escape);
        let delete = i.consume_key(Modifiers::NONE, Key::Delete)
            || i.consume_key(Modifiers::NONE, Key::Backspace);
        (redo, undo, escape, delete)
    });
    if let Some((model, edit3d)) = doc.three_d.model_and_edit() {
        if undo {
            history(model, edit3d, false);
        }
        if redo {
            history(model, edit3d, true);
        }
        if delete {
            let targets = edit3d.delete_targets();
            edit::delete_atoms(model, edit3d, &targets);
        }
        if escape {
            edit3d.clear_picks();
            edit3d.selected.clear();
        }
    }
}

fn history(model: &mut Model3d, edit3d: &mut three_d::Edit3d, redo: bool) {
    let label = if redo {
        edit3d
            .redo(model)
            .map(|l| t!("three_d.view.redone", label = l))
    } else {
        edit3d
            .undo(model)
            .map(|l| t!("three_d.view.undone", label = l))
    };
    edit3d.report = Some((
        label.unwrap_or_else(|| t!("three_d.view.no_history").to_owned()),
        false,
    ));
}

/// The tools for editing the model by hand, in the right panel.
fn tools(ui: &mut egui::Ui, doc: &mut Document, view: &mut View3d, ink: &Ink) {
    let Some((model, edit3d)) = doc.three_d.model_and_edit() else {
        ui.label(ink.muted(t!("three_d.view.busy_optimizing")));
        return;
    };
    history_row(ui, model, edit3d);
    selection_row(ui, model, edit3d, ink);
    if edit::separate_molecule(model, 0).is_some() {
        ui.label(ink.muted(t!("three_d.view.move_hint")));
    }
    delete_row(ui, model, edit3d, ink);
    value_row(ui, model, edit3d, &mut view.value, ink);
    if let Some((text, problem)) = &edit3d.report {
        let color = if *problem { ink.warn } else { ink.muted };
        ui.label(RichText::new(text).size(11.0).color(color));
    }
    constraints_section(ui, model, edit3d, ink);
    clash_row(ui, model, view, ink);
    contacts_rows(ui, model, edit3d, view);
}

/// The hydrogen bonds and π-stacks shown, counted and the nearest listed; clicking a hydrogen
/// bond picks D, H and A (so its angle is measured).
fn contacts_rows(
    ui: &mut egui::Ui,
    model: &Model3d,
    edit3d: &mut three_d::Edit3d,
    view: &mut View3d,
) {
    let show = view.interactions;
    if !show.any() {
        return;
    }
    let found = view.contacts_for(model);
    let clicked = contacts::list(ui, found, show, "", t!("three_d.view.hbond_hint"), |a| {
        edit::atom_label(model, a)
    });
    if let Some(contacts::Clicked::HBond(k)) = clicked
        && let Some(hb) = found.hbonds.get(k)
    {
        edit3d.picked = vec![hb.donor, hb.hydrogen, hb.acceptor];
        edit3d.report = None;
    }
}

fn history_row(ui: &mut egui::Ui, model: &mut Model3d, edit3d: &mut three_d::Edit3d) {
    ui.horizontal(|ui| {
        let undo_tip = edit3d
            .undo_label()
            .map_or(t!("three_d.view.undo_none").to_owned(), |l| {
                t!("three_d.view.undo_tip", label = l)
            });
        if ui
            .add_enabled(
                edit3d.can_undo(),
                egui::Button::new(t!("three_d.view.undo")).small(),
            )
            .on_hover_text(undo_tip)
            .on_disabled_hover_text(t!("three_d.view.undo_none"))
            .clicked()
        {
            history(model, edit3d, false);
        }
        let redo_tip = edit3d
            .redo_label()
            .map_or(t!("three_d.view.redo_none").to_owned(), |l| {
                t!("three_d.view.redo_tip", label = l)
            });
        if ui
            .add_enabled(
                edit3d.can_redo(),
                egui::Button::new(t!("three_d.view.redo")).small(),
            )
            .on_hover_text(redo_tip)
            .on_disabled_hover_text(t!("three_d.view.redo_none"))
            .clicked()
        {
            history(model, edit3d, true);
        }
    });
}

fn selection_row(ui: &mut egui::Ui, model: &Model3d, edit3d: &mut three_d::Edit3d, ink: &Ink) {
    if edit3d.picked.is_empty() && edit3d.anchor.is_none() {
        ui.label(ink.muted(t!("three_d.view.pick_hint")));
        return;
    }
    ui.horizontal_wrapped(|ui| {
        if !edit3d.picked.is_empty() {
            let names: Vec<String> = edit3d
                .picked
                .iter()
                .map(|&i| edit::atom_label(model, i))
                .collect();
            ui.label(
                RichText::new(t!("three_d.view.selection"))
                    .size(11.0)
                    .color(ink.muted),
            );
            ui.label(RichText::new(names.join(" – ")).size(12.0).color(ink.pick));
            if ui.small_button(t!("three_d.view.clear")).clicked() {
                edit3d.clear_picks();
                edit3d.report = None;
            }
        }
        if let Some(anchor) = edit3d.anchor {
            ui.label(
                RichText::new(t!("three_d.view.fixed"))
                    .size(11.0)
                    .color(ink.muted),
            );
            ui.label(
                RichText::new(edit::atom_label(model, anchor))
                    .size(12.0)
                    .color(ink.anchor),
            )
            .on_hover_text(t!("three_d.view.anchor_tip"));
            if ui.small_button(t!("three_d.view.clear")).clicked() {
                edit3d.anchor = None;
            }
        }
    });
}

/// The atoms Delete would remove — those selected for it, else the picked ones — with the
/// buttons that widen them to their whole molecules and delete them.
fn delete_row(ui: &mut egui::Ui, model: &mut Model3d, edit3d: &mut three_d::Edit3d, ink: &Ink) {
    let targets = edit3d.delete_targets();
    if targets.is_empty() {
        return;
    }
    let mut delete = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(tn!("three_d.view.marked", targets.len()))
                .size(11.0)
                .color(ink.select),
        );
        if ui
            .small_button(t!("three_d.view.select_molecule"))
            .on_hover_text(t!("three_d.view.select_molecule_tip"))
            .clicked()
        {
            edit3d.selected = edit::molecules_of(model, targets.iter().copied());
        }
        if !edit3d.selected.is_empty() && ui.small_button(t!("three_d.view.clear")).clicked() {
            edit3d.selected.clear();
        }
        delete = ui
            .small_button(t!("three_d.view.delete"))
            .on_hover_text(t!("three_d.view.delete_tip"))
            .clicked();
    });
    if delete {
        edit::delete_atoms(model, edit3d, &targets);
    }
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
        Target::BondLength(_) => (t!("three_d.view.bond_length"), "Å"),
        Target::Distance(_) => (t!("three_d.view.distance"), "Å"),
        Target::Angle(_) => (t!("three_d.view.bond_angle"), "°"),
        Target::Dihedral(_) => (t!("three_d.view.dihedral"), "°"),
        Target::Unbonded => match measure {
            Measure::Distance(_) => (t!("three_d.view.distance"), "Å"),
            Measure::Angle(_) => (t!("three_d.view.angle"), "°"),
            Measure::Dihedral(_) => (t!("three_d.view.dihedral"), "°"),
        },
        Target::None => return,
    };
    let mut apply: Option<f64> = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(name).size(11.0).color(ink.muted));
        ui.label(RichText::new(measure.to_string()).size(13.0).strong());
        if target == Target::Unbonded {
            ui.label(muted(t!("three_d.view.measure_only")));
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
        if ui.button(t!("three_d.view.set")).clicked() || entered {
            match parse_value(&value.1) {
                Some(v) => apply = Some(v),
                None => {
                    edit3d.report = Some((t!("three_d.view.enter_number").to_owned(), true));
                }
            }
        }
    });
    let presets: Vec<(&str, f64)> = match target {
        Target::Angle(_) => vec![
            ("sp³ 109.5°", 109.5),
            ("sp² 120°", 120.0),
            (t!("three_d.view.preset_linear"), 180.0),
        ],
        Target::Dihedral(_) => vec![
            (t!("three_d.view.preset_cis"), 0.0),
            (t!("three_d.view.preset_gauche_plus"), 60.0),
            (t!("three_d.view.preset_gauche_minus"), -60.0),
            (t!("three_d.view.preset_anti"), 180.0),
        ],
        _ => Vec::new(),
    };
    if !presets.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for &(label, v) in &presets {
                if ui.small_button(label).clicked() {
                    value.1 = format!("{v}");
                    apply = Some(v);
                }
            }
        });
    }
    if let Target::Distance([a, b]) = target {
        ui.horizontal_wrapped(|ui| {
            ui.label(muted(t!("three_d.view.turn_between")));
            if ui.small_button(t!("three_d.view.farthest")).clicked() {
                edit::solve(model, edit3d, a, b, Goal::Farthest);
            }
            if ui.small_button(t!("three_d.view.closest")).clicked() {
                edit::solve(model, edit3d, a, b, Goal::Closest);
            }
        });
    }
    if edit::picked_rotor(model, &edit3d.picked).is_some() {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(t!("three_d.view.rotate"))
                    .size(11.0)
                    .color(ink.muted),
            );
            for step in [-STEP_COARSE, -STEP, STEP, STEP_COARSE] {
                if ui.small_button(format!("{step:+}°")).clicked() {
                    edit::rotate_picked(model, edit3d, step);
                }
            }
            ui.label(muted(t!("three_d.view.wheel_hint")));
        });
    } else if let [b, c] = edit3d.picked[..]
        && graph.bonded(b, c)
        && let Err(reason) = geom::is_rotatable(&graph, b, c, edit::bond_order(model, b, c))
    {
        ui.label(muted(&t!("three_d.view.cannot_rotate", reason = reason)));
    }
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(
            &mut edit3d.lock_on_set,
            RichText::new(t!("three_d.view.lock_on_set")).size(11.0),
        )
        .on_hover_text(t!("three_d.view.lock_on_set_tip"));
        if ui
            .small_button(t!("three_d.view.constrain_current"))
            .on_hover_text(t!("three_d.view.constrain_current_tip"))
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
            .small_button(t!(
                "three_d.view.fix_atom",
                atom = edit::atom_label(model, atom)
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
        RichText::new(t!(
            "three_d.view.constraints_header",
            count = edit3d.constraints.len()
        ))
        .size(12.0),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.checkbox(
            &mut edit3d.use_constraints,
            RichText::new(t!("three_d.view.use_constraints")).size(11.0),
        );
        let mut remove = None;
        for (n, c) in edit3d.constraints.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui
                    .small_button("×")
                    .on_hover_text(t!("three_d.view.remove_constraint"))
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
                        RichText::new(t!(
                            "three_d.view.constraint_status",
                            target = format!("{target:.2}"),
                            now = format!("{now:.2}"),
                            unit = c.unit()
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
    });
}

fn clash_row(ui: &mut egui::Ui, model: &Model3d, view: &mut View3d, ink: &Ink) {
    let clashes = view.clashes_for(model);
    match clashes.first() {
        None => {
            ui.label(ink.muted(t!("three_d.view.no_clashes")));
        }
        Some(worst) => {
            ui.label(
                RichText::new(tn!(
                    "three_d.view.clashes",
                    clashes.len(),
                    a = edit::atom_label(model, worst.a),
                    b = edit::atom_label(model, worst.b),
                    distance = format!("{:.2}", worst.distance)
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
    ui.add_enabled_ui(doc.is_some() && !running, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(t!("three_d.view.force_field"))
                    .size(11.0)
                    .color(ink.muted),
            );
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
                .button(t!("three_d.view.regenerate"))
                .on_hover_text(t!("three_d.view.regenerate_tip"))
                .clicked()
            {
                out.push(Command::Generate3d);
            }
            let constraints = job.map_or(0, |j| {
                j.model()
                    .map_or(0, |m| j.edit.active_constraints(m.atoms.len()).len())
            });
            let tip = if constraints > 0 {
                tn!("three_d.view.optimize_constrained_tip", constraints)
            } else {
                t!("three_d.view.optimize_tip").to_owned()
            };
            let optimize = ui
                .add_enabled(
                    job.is_some_and(|j| j.model().is_some()),
                    egui::Button::new(
                        RichText::new(t!("three_d.view.optimize")).color(Color32::WHITE),
                    )
                    .fill(theme::ORANGE_DEEP),
                )
                .on_hover_text(tip);
            if optimize.clicked() {
                out.push(Command::Optimize3d);
            }
            if ui
                .add_enabled(
                    job.is_some_and(|j| j.model().is_some()),
                    egui::Button::new(t!("three_d.view.conformer_search")),
                )
                .on_hover_text(t!("three_d.view.conformer_search_tip"))
                .clicked()
            {
                out.push(Command::OpenConformers);
            }
            let softcore_running = doc
                .and_then(|d| d.softcore.as_ref())
                .is_some_and(|r| r.is_running());
            let refused = match model {
                Some(m) if job.is_some_and(|j| j.model().is_some()) => {
                    crate::softcore::refusal(crate::softcore::molecule_count(m))
                }
                _ => Some(t!("conformer.window.no_model").to_owned()),
            };
            let button = ui
                .add_enabled(
                    refused.is_none() && !softcore_running,
                    egui::Button::new(t!("softcore.model_button")),
                )
                .on_hover_text(t!("softcore.model_tip"));
            let button = match &refused {
                Some(why) => button.on_disabled_hover_text(why),
                None => button,
            };
            if button.clicked() {
                out.push(Command::SoftCore3d);
            }
        });
    });
    if let Some(run) = doc
        .and_then(|d| d.softcore.as_ref())
        .filter(|r| r.is_running() && r.target == crate::softcore::Target::Model)
    {
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            ui.label(ink.muted(&t!("softcore.running", stage = run.stage_label())));
            if ui.small_button(t!("common.cancel")).clicked() {
                out.push(Command::CancelSoftCore);
            }
        });
    }
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
    let Some(m) = model else {
        if !running {
            ui.label(ink.muted(t!("three_d.view.no_model_hint")));
        }
        return;
    };
    let pending = job.is_some_and(|j| j.energy_pending());
    let energy = if pending {
        t!(
            "three_d.view.energy_recalculating",
            energy = format!("{:.1}", m.energy),
            unit = m.energy_unit
        )
    } else {
        format!("E = {:.1} {}", m.energy, m.energy_unit)
    };
    ui.label(RichText::new(energy).monospace().size(12.0));
    let mut about = tn!(
        "three_d.view.about",
        m.atoms.len(),
        formula = m.formula,
        hydrogens = m.hydrogen_count()
    );
    if job.is_some_and(|j| j.scope().is_some()) {
        about += t!("three_d.view.about_part");
    }
    ui.label(ink.muted(&about));
    if doc.is_some_and(|d| d.three_d.is_stale(&d.editor.molecule)) {
        ui.label(
            RichText::new(t!("three_d.view.stale"))
                .size(11.0)
                .color(ink.warn),
        )
        .on_hover_text(t!("three_d.view.stale_tip"));
    }
}

/// The panel body when there is no model to show: why, and what to do.
fn summary(ui: &mut egui::Ui, state: Option<&JobState>, stale: bool, can_render: bool) {
    match state {
        None | Some(JobState::Idle) => {
            ui.label(muted(t!("three_d.view.idle_hint")));
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
            ui.label(muted(t!("three_d.view.retry_hint")));
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
                    row(ui, t!("three_d.view.formula"), model.formula.clone());
                    row(
                        ui,
                        t!("three_d.view.atom_count"),
                        t!(
                            "three_d.view.atom_count_value",
                            atoms = model.atoms.len(),
                            hydrogens = model.hydrogen_count()
                        ),
                    );
                    row(
                        ui,
                        t!("three_d.view.energy"),
                        format!("{:.1} {}", model.energy, model.energy_unit),
                    );
                    row(
                        ui,
                        t!("three_d.view.force_field"),
                        model.force_field.id().to_owned(),
                    );
                });
            ui.add_space(10.0);
            if stale {
                ui.label(
                    RichText::new(t!("three_d.view.stale_long"))
                        .size(12.0)
                        .color(theme::ORANGE_ON_DARK),
                );
            }
            if !can_render {
                ui.label(muted(t!("three_d.view.no_wgpu")));
            }
        }
    }
}

fn muted(text: &str) -> RichText {
    RichText::new(text).size(11.0).color(dark::MUTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Atoms 0–1 bonded (and, with `second`, 2–3 as another molecule).
    fn molecules(second: bool) -> Model3d {
        let count: usize = if second { 4 } else { 2 };
        Model3d {
            atoms: (0..count)
                .map(|i| three_d::Atom3d {
                    element: "C".to_owned(),
                    pos: [i as f64 * 1.5, 0.0, 0.0],
                })
                .collect(),
            bonds: (0..count / 2)
                .map(|i| three_d::Bond3d {
                    begin: 2 * i,
                    end: 2 * i + 1,
                    order: 1,
                })
                .collect(),
            force_field: ForceField::default(),
            energy: 0.0,
            energy_unit: String::new(),
            formula: String::new(),
            molfile: String::new(),
        }
    }

    fn held_keys() -> Vec<egui::Modifiers> {
        use egui::Modifiers;
        vec![
            Modifiers::NONE,
            Modifiers::SHIFT,
            Modifiers::ALT,
            Modifiers::COMMAND,
            Modifiers::SHIFT | Modifiers::ALT,
            Modifiers::SHIFT | Modifiers::COMMAND,
            Modifiers::ALT | Modifiers::COMMAND,
        ]
    }

    /// A model of one molecule never hands a drag to the molecule under it, whatever is held:
    /// its drags stay the camera's and the selection rectangle's.
    #[test]
    fn a_single_molecule_is_never_taken_hold_of() {
        let single = molecules(false);
        for modifiers in held_keys() {
            for hover in [
                None,
                Some(Hit::Nothing),
                Some(Hit::Atom(0)),
                Some(Hit::Atom(1)),
                Some(Hit::Bond(0)),
            ] {
                assert_eq!(grab_reach(&single, hover, modifiers), None);
            }
        }
    }

    #[test]
    fn shift_or_alt_takes_hold_of_the_molecule_under_the_pointer() {
        use egui::Modifiers;
        let pair = molecules(true);
        for modifiers in held_keys() {
            let grabs = !modifiers.command && (modifiers.shift || modifiers.alt);
            assert_eq!(
                grab_reach(&pair, Some(Hit::Atom(3)), modifiers),
                grabs.then(|| (3, vec![2, 3])),
                "{modifiers:?}"
            );
            // Empty space, and no pointer over the view, leave the drag alone.
            assert_eq!(grab_reach(&pair, Some(Hit::Nothing), modifiers), None);
            assert_eq!(grab_reach(&pair, None, modifiers), None);
        }
        // A bond is a handle too, by its first atom.
        assert_eq!(
            grab_reach(&pair, Some(Hit::Bond(0)), Modifiers::SHIFT),
            Some((0, vec![0, 1]))
        );
        assert_eq!(
            grab_reach(&pair, Some(Hit::Bond(7)), Modifiers::SHIFT),
            None
        );
        assert_eq!(grab_reach(&pair, Some(Hit::Atom(9)), Modifiers::ALT), None);
    }

    /// Pentane as the 3D view generates it (C0–C4 in a chain, then the hydrogens).
    fn pentane() -> Model3d {
        let drawn = include_str!("../../tests/data/drawn/pentane.mol");
        three_d::generate_from_molfile(drawn, ForceField::Uff).expect("pentane")
    }

    /// A molecule a deletion cut in two is taken hold of as either half; before the deletion
    /// and after undoing it there is nothing to take hold of.
    #[test]
    fn the_halves_of_a_molecule_cut_in_two_are_taken_hold_of() {
        use egui::Modifiers;
        let mut model = pentane();
        let mut edit3d = three_d::Edit3d::default();
        let hold = |model: &Model3d, hover| grab_reach(model, Some(hover), Modifiers::SHIFT);
        assert_eq!(hold(&model, Hit::Atom(0)), None);

        assert!(edit::delete_atoms(&mut model, &mut edit3d, &[2]));
        let (handle, left) = hold(&model, Hit::Atom(0)).expect("the left half");
        assert_eq!((handle, left.len()), (0, 8));
        let (_, right) = hold(&model, Hit::Atom(3)).expect("the right half");
        assert_eq!(right.len(), 8);
        assert!(left.iter().all(|a| !right.contains(a)));
        // By any of its atoms (the hydrogens put on the cut bonds too) and any of its bonds.
        for atom in 0..model.atoms.len() {
            let (_, held) = hold(&model, Hit::Atom(atom)).expect("a half");
            assert!(held == left || held == right, "atom {atom}");
        }
        for bond in 0..model.bonds.len() {
            let (handle, held) = hold(&model, Hit::Bond(bond)).expect("a half");
            assert!(held.contains(&handle) && held.len() == 8, "bond {bond}");
        }

        assert!(edit3d.undo(&mut model).is_some());
        assert_eq!(hold(&model, Hit::Atom(0)), None);
        assert!(edit3d.redo(&mut model).is_some());
        assert_eq!(hold(&model, Hit::Atom(0)), Some((0, left)));
    }

    /// The viewer on a real GPU target, fed pointer and key events a frame at a time.
    struct Rig {
        ctx: egui::Context,
        render_state: RenderState,
        view: View3d,
        doc: Document,
        time: f64,
    }

    impl Rig {
        const SCREEN: egui::Rect = egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(800.0, 600.0));

        /// `None` on a machine without a GPU.
        fn showing(model: Model3d) -> Option<Rig> {
            fn block_on<F: std::future::Future>(future: F) -> F::Output {
                let mut future = std::pin::pin!(future);
                let mut context = std::task::Context::from_waker(std::task::Waker::noop());
                loop {
                    if let std::task::Poll::Ready(out) = future.as_mut().poll(&mut context) {
                        return out;
                    }
                    std::thread::yield_now();
                }
            }
            let render_state = block_on(RenderState::create(
                &eframe::egui_wgpu::WgpuConfiguration::default(),
                &eframe::wgpu::Instance::default(),
                None,
                eframe::egui_wgpu::RendererOptions::default(),
            ))
            .ok()?;
            let mut doc = Document::new(&chembuider_rs::Config::default(), 1);
            doc.three_d = three_d::Job::with_model(model, &doc.editor.molecule);
            let mut rig = Rig {
                ctx: egui::Context::default(),
                render_state,
                view: View3d::default(),
                doc,
                time: 0.0,
            };
            rig.frame(Vec::new(), egui::Modifiers::NONE);
            Some(rig)
        }

        /// One frame of the viewer (and of the job's polling, as the app does).
        fn frame(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
            self.time += 0.05;
            let raw = egui::RawInput {
                screen_rect: Some(Self::SCREEN),
                time: Some(self.time),
                events: std::iter::once(egui::Event::ModifiersChanged(modifiers))
                    .chain(events)
                    .collect(),
                ..Default::default()
            };
            let (doc, view, render_state) = (&mut self.doc, &mut self.view, &self.render_state);
            let mut output = self.ctx.run_ui(raw, |ui| {
                viewer_area(ui, "test", Some(doc), view, Some(render_state));
            });
            output.textures_delta.clear();
            self.doc.three_d.poll();
        }

        fn model(&self) -> &Model3d {
            self.doc.three_d.model().expect("a model")
        }

        /// Where `atom` is drawn.
        fn at(&self, atom: usize) -> Pos2 {
            let viewport = self.view.viewport.as_ref().expect("shown");
            viewport
                .project(Self::SCREEN, self.model().atoms[atom].pos)
                .expect("in front of the camera")
        }

        fn point(&mut self, pos: Pos2, modifiers: egui::Modifiers) {
            self.frame(vec![egui::Event::PointerMoved(pos)], modifiers);
            self.frame(Vec::new(), modifiers);
        }

        fn button(&mut self, pos: Pos2, pressed: bool, modifiers: egui::Modifiers) {
            let event = egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers,
            };
            self.frame(vec![event], modifiers);
        }

        fn click(&mut self, atom: usize, modifiers: egui::Modifiers) {
            let pos = self.at(atom);
            self.point(pos, modifiers);
            self.button(pos, true, modifiers);
            self.button(pos, false, modifiers);
            self.frame(Vec::new(), egui::Modifiers::NONE);
        }

        fn key(&mut self, key: egui::Key) {
            let event = egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(vec![event], egui::Modifiers::NONE);
            self.frame(Vec::new(), egui::Modifiers::NONE);
        }

        /// Drag from `atom` with `modifiers` held; the atoms that moved.
        fn drag(&mut self, atom: usize, modifiers: egui::Modifiers) -> Vec<usize> {
            let before = edit::coords(self.model());
            let from = self.at(atom);
            self.point(from, modifiers);
            self.button(from, true, modifiers);
            for step in 1..=6 {
                self.point(from + vec2(8.0 * step as f32, 3.0 * step as f32), modifiers);
            }
            self.button(from + vec2(48.0, 18.0), false, modifiers);
            self.frame(Vec::new(), egui::Modifiers::NONE);
            let after = edit::coords(self.model());
            assert_eq!(before.len(), after.len());
            (0..after.len())
                .filter(|&i| before[i] != after[i])
                .collect()
        }
    }

    /// The whole path through the viewer: a molecule is cut in two with Ctrl+click and Delete,
    /// and each half then follows a Shift+drag (and turns with Alt) while the other stays —
    /// also after an optimisation, and not after the deletion is undone.
    #[test]
    #[ignore = "draws on a GPU: run with --ignored"]
    fn a_molecule_cut_in_two_in_the_viewer_is_dragged_by_its_halves() {
        use egui::{Key, Modifiers};
        let Some(mut rig) = Rig::showing(pentane()) else {
            eprintln!("no GPU: skipped");
            return;
        };
        let ctrl = Modifiers::CTRL | Modifiers::COMMAND;
        // One molecule: Shift+drag is the camera's, nothing moves.
        assert_eq!(rig.drag(0, Modifiers::SHIFT), Vec::<usize>::new());

        rig.click(2, ctrl);
        assert_eq!(
            rig.doc.three_d.edit.selected.iter().collect::<Vec<_>>(),
            [&2]
        );
        rig.key(Key::Delete);
        assert_eq!(rig.model().formula, "C4H12");
        let half = |rig: &Rig, atom| edit::separate_molecule(rig.model(), atom).expect("a half");
        for (atom, modifiers) in [
            (0, Modifiers::SHIFT),
            (3, Modifiers::SHIFT),
            (1, Modifiers::ALT),
            (2, Modifiers::ALT),
        ] {
            assert_eq!(rig.drag(atom, modifiers), half(&rig, atom), "atom {atom}");
        }

        // An optimisation keeps them two.
        rig.doc.three_d.optimize();
        while rig.doc.three_d.is_running() {
            std::thread::sleep(std::time::Duration::from_millis(10));
            rig.frame(Vec::new(), Modifiers::NONE);
        }
        assert_eq!(rig.drag(0, Modifiers::SHIFT), half(&rig, 0));

        // The half holding the fixed atom stays; the other still moves.
        rig.doc.three_d.edit.anchor = Some(0);
        assert_eq!(rig.drag(1, Modifiers::SHIFT), Vec::<usize>::new());
        assert_eq!(rig.drag(3, Modifiers::SHIFT), half(&rig, 3));
        rig.doc.three_d.edit.anchor = None;

        // Undone back to one molecule, there is nothing to take hold of; redone, there is.
        while rig.model().formula != "C5H12" {
            let (model, edit3d) = rig.doc.three_d.model_and_edit().unwrap();
            edit3d.undo(model).expect("the deletion is in the history");
        }
        rig.frame(Vec::new(), Modifiers::NONE);
        assert_eq!(rig.drag(0, Modifiers::SHIFT), Vec::<usize>::new());
        let (model, edit3d) = rig.doc.three_d.model_and_edit().unwrap();
        edit3d.redo(model).expect("redo");
        rig.frame(Vec::new(), Modifiers::NONE);
        assert_eq!(rig.drag(0, Modifiers::SHIFT), half(&rig, 0));
    }

    #[test]
    fn a_drag_moves_the_molecule_with_the_pointer_or_turns_it() {
        // Looking down −z: twenty points to the ångström.
        let basis = ScreenBasis {
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            toward_viewer: [0.0, 0.0, 1.0],
            angstrom_per_point: 0.05,
        };
        // Right and down on the screen (y grows downward) is +x and −y in the scene.
        let Some(Motion::Move(by)) = drag_motion(&basis, vec2(20.0, 10.0), false) else {
            panic!("a move");
        };
        assert!(geom::distance(by, [1.0, -0.5, 0.0]) < 1e-12, "{by:?}");
        // A drag to the right swings the side facing the viewer to the right.
        let Some(Motion::Turn(rotation)) = drag_motion(&basis, vec2(60.0, 0.0), true) else {
            panic!("a turn");
        };
        let near = rotation.map(|row| geom::dot(row, [0.0, 0.0, 1.0]));
        let degrees = 60.0 * place::DEGREES_PER_POINT;
        let (sin, cos) = degrees.to_radians().sin_cos();
        assert!(geom::distance(near, [sin, 0.0, cos]) < 1e-12, "{near:?}");
        // A pointer that has not moved moves nothing.
        assert_eq!(drag_motion(&basis, vec2(0.0, 0.0), false), None);
        assert_eq!(drag_motion(&basis, vec2(0.0, 0.0), true), None);
    }

    #[test]
    fn a_rectangle_takes_in_the_atoms_drawn_inside_it() {
        let model = Model3d {
            atoms: [0.0, 1.0, 2.0, 3.0]
                .into_iter()
                .map(|x| three_d::Atom3d {
                    element: "C".to_owned(),
                    pos: [x, 0.0, 0.0],
                })
                .collect(),
            bonds: Vec::new(),
            force_field: ForceField::default(),
            energy: 0.0,
            energy_unit: String::new(),
            formula: "C4".to_owned(),
            molfile: String::new(),
        };
        // Ten points to the ångström; the atom at x = 2 is behind the camera.
        let project = |p: [f64; 3]| (p[0] != 2.0).then(|| egui::pos2(p[0] as f32 * 10.0, 5.0));
        let area = egui::Rect::from_two_pos(egui::pos2(25.0, 10.0), egui::pos2(5.0, 0.0));
        assert_eq!(atoms_inside(&model, area, project), vec![1]);
        let all = egui::Rect::from_two_pos(egui::pos2(-5.0, 0.0), egui::pos2(35.0, 10.0));
        assert_eq!(atoms_inside(&model, all, project), vec![0, 1, 3]);
    }

    #[test]
    fn typed_values_accept_full_width_input() {
        assert_eq!(parse_value(" 1.54 "), Some(1.54));
        assert_eq!(parse_value("－６０"), Some(-60.0));
        assert_eq!(parse_value("１０９．５°"), Some(109.5));
        assert_eq!(parse_value("abc"), None);
        assert_eq!(parse_value("NaN"), None);
    }
}
