//! Free placement in the assembly view (配置): the panel section listing the molecules placed
//! by hand with their position and rotation, the mouse moves in the viewer (Alt+drag turns the
//! selected molecule, Shift+drag moves it), and the clash check between molecules with its
//! highlights and its list in 情報.
//!
//! The clash check runs on the build each time its revision changes. A small assembly is
//! checked at once; a large one on a worker thread (one at a time, the latest revision next),
//! so the UI never waits for it. Until a newer answer arrives, the last one is shown for as
//! long as its atom indices still apply (the same number of atoms).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use egui::{DragValue, Pos2, RichText, Stroke, vec2};

use super::super::view3d::{CLASH_COLOR, CLASH_ON_LIGHT, CLASH_STROKE};
use super::super::viewport3d::{Frame3d, Viewport3d};
use super::{AssemblyView, Ink, tag};
use crate::assembly::clash::{self, Clashes};
use crate::assembly::math::{self, Vec3};
use crate::assembly::place as geometry;
use crate::assembly::{AssemblyState, Built};
use crate::theme::dark;

/// Up to this many atoms the clash check runs on the UI thread (a few milliseconds).
const INLINE_ATOMS: usize = 8_000;
/// How many clashes are drawn as dashed lines, and how many of them get their distance.
const DRAWN_CLASHES: usize = 40;
const LABELLED_CLASHES: usize = 5;
/// How many clashes the panel lists.
const LISTED_CLASHES: usize = 8;
/// The gap left between a molecule and a new one put next to it.
const NEXT_TO_GAP: f64 = 1.5;

/// An assembly (its state id), a revision of it, and its atom count.
type Key = (u64, u64, usize);

/// The clash check's last answer and the one being worked out.
#[derive(Default)]
pub(super) struct ClashCheck {
    done: Option<(Key, Arc<Clashes>)>,
    running: Option<(Key, Receiver<Clashes>)>,
}

impl ClashCheck {
    /// The clashes of `built` (assembly `id`), the check started when its revision is new, and
    /// whether the answer is for this very revision (otherwise it is the last one while this
    /// one is worked out). `None` until an answer for these atoms is ready.
    pub(super) fn get(
        &mut self,
        ctx: &egui::Context,
        id: u64,
        built: &Arc<Built>,
    ) -> Option<(Arc<Clashes>, bool)> {
        if let Some((key, rx)) = &self.running {
            match rx.try_recv() {
                Ok(result) => {
                    self.done = Some((*key, Arc::new(result)));
                    self.running = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.running = None,
            }
        }
        let key = (id, built.revision, built.atoms.len());
        let current = self.done.as_ref().is_some_and(|(k, _)| *k == key);
        if !current && self.running.is_none() {
            if built.atoms.len() <= INLINE_ATOMS {
                self.done = Some((
                    key,
                    Arc::new(clash::find(built, clash::periodic_of(built).as_ref())),
                ));
            } else {
                let (tx, rx) = channel();
                let job = Arc::clone(built);
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    // The receiver may be gone by now: nothing to tell then.
                    let _ = tx.send(clash::find(&job, clash::periodic_of(&job).as_ref()));
                    ctx.request_repaint();
                });
                self.running = Some((key, rx));
            }
        }
        // An older answer still names the right atoms while their number is the same.
        self.done
            .as_ref()
            .filter(|((did, _, atoms), _)| *did == id && *atoms == key.2)
            .map(|(k, c)| (Arc::clone(c), *k == key))
    }
}

/// The world centroid and the local centroid of molecule `m`'s component, for a hand-placed
/// molecule `k`.
fn manual_centroids(state: &AssemblyState, k: usize) -> Option<(Vec3, Vec3)> {
    let inst = state.design.manual.get(k)?;
    let component = state.design.components.get(inst.component)?;
    let local = geometry::centroid(&geometry::local_positions(component));
    Some((inst.transform.apply(local), local))
}

/// Move or turn the selected hand-placed molecule by a drag in the viewer: Alt turns it about
/// its centroid, otherwise (Shift) it follows the pointer. Part of the gesture's undo step.
pub(super) fn drag_selected(
    state: &mut AssemblyState,
    viewport: &Viewport3d,
    frame: &Frame3d,
    delta: egui::Vec2,
    turn: bool,
) {
    let Some(k) = state.selected.and_then(|m| state.design.manual_index(m)) else {
        return;
    };
    let Some((centre, _)) = manual_centroids(state, k) else {
        return;
    };
    let basis = viewport.screen_basis(frame.rect, centre);
    let delta = [f64::from(delta.x), f64::from(delta.y)];
    if delta == [0.0, 0.0] {
        return;
    }
    let t = &mut state.design.manual[k].transform;
    *t = if turn {
        geometry::rotate_about(t, centre, geometry::drag_rotation(&basis, delta))
    } else {
        geometry::translate(t, geometry::drag_translation(&basis, delta))
    };
    state.touch();
}

/// Each clashing atom, for the highlight.
pub(super) fn clash_atoms(clashes: &Clashes) -> Vec<usize> {
    let mut atoms: Vec<usize> = clashes.pairs.iter().flat_map(|c| [c.a, c.b]).collect();
    atoms.sort_unstable();
    atoms.dedup();
    atoms
}

/// Dashed lines between the worst clashing pairs, the worst few with their distance, and a hint
/// about moving the selected molecule.
pub(super) fn overlay(
    ui: &egui::Ui,
    viewport: &Viewport3d,
    frame: &Frame3d,
    state: &AssemblyState,
    built: &Built,
    clashes: Option<&Clashes>,
) {
    let painter = ui.painter_at(frame.rect);
    let at = |i: usize| {
        built
            .atoms
            .get(i)
            .and_then(|a| viewport.project(frame.rect, a.pos))
    };
    for (n, c) in clashes
        .map_or(&[][..], |c| &c.pairs)
        .iter()
        .take(DRAWN_CLASHES)
        .enumerate()
    {
        if let (Some(p), Some(q)) = (at(c.a), at(c.b)) {
            painter.add(egui::Shape::dashed_line(
                &[p, q],
                Stroke::new(1.5, CLASH_STROKE),
                4.0,
                3.0,
            ));
            if n < LABELLED_CLASHES {
                tag(
                    &painter,
                    p.lerp(q, 0.5) + vec2(4.0, -8.0),
                    format!("{:.2} Å", c.distance),
                    CLASH_STROKE,
                );
            }
        }
    }
    let Some(m) = state.selected.filter(|&m| m < built.molecule_count()) else {
        return;
    };
    let hint = if state.design.manual_index(m).is_some() {
        t!("assembly.place.hint_manual")
    } else {
        t!("assembly.place.hint_step")
    };
    let pos = Pos2::new(frame.rect.left() + 10.0, frame.rect.bottom() - 24.0);
    tag(&painter, pos, hint.to_owned(), dark::TEXT);
}

/// What a button in the section asks for, done once the section has let go of the state.
enum Action {
    Add(usize),
    Duplicate(usize),
    Remove(usize),
    Detach(usize),
    Restore,
}

/// The 配置 section of the panel.
pub(super) fn section(
    ui: &mut egui::Ui,
    state: &mut AssemblyState,
    view: &mut AssemblyView,
    ink: &Ink,
) {
    let mut action = None;
    let components = &state.design.components;
    if components.is_empty() {
        ui.label(ink.muted(t!("assembly.place.empty")));
        return;
    }
    if view.add_component >= components.len() {
        view.add_component = 0;
    }
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("assembly_add_component")
            .selected_text(components[view.add_component].name.as_str())
            .width(110.0)
            .show_ui(ui, |ui| {
                for (i, c) in components.iter().enumerate() {
                    ui.selectable_value(&mut view.add_component, i, c.name.as_str());
                }
            })
            .response
            .on_hover_text(t!("assembly.place.component_tip"));
        if ui
            .button(t!("assembly.place.add"))
            .on_hover_text(t!("assembly.place.add_tip"))
            .clicked()
        {
            action = Some(Action::Add(view.add_component));
        }
    });

    let built = state.built().ok();
    let steps = state.design.step_molecule_count();
    if state.design.manual.is_empty() {
        ui.label(ink.muted(t!("assembly.place.no_manual")));
    }
    egui::ScrollArea::vertical()
        .id_salt("assembly_manual_list")
        .max_height(160.0)
        .show(ui, |ui| {
            for (k, inst) in state.design.manual.iter().enumerate() {
                let m = steps + k;
                let name = state
                    .design
                    .components
                    .get(inst.component)
                    .map_or("", |c| c.name.as_str());
                if ui
                    .selectable_label(
                        state.selected == Some(m),
                        t!(
                            "assembly.place.manual_item",
                            index = k + 1,
                            name = name,
                            molecule = m + 1
                        ),
                    )
                    .on_hover_text(t!("assembly.place.click_to_select"))
                    .clicked()
                {
                    state.selected = Some(m);
                }
            }
        });
    if !state.design.excluded.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(ink.muted(tn!("assembly.place.excluded", state.design.excluded.len())));
            if ui
                .small_button(t!("assembly.place.restore"))
                .on_hover_text(t!("assembly.place.restore_tip"))
                .clicked()
            {
                action = Some(Action::Restore);
            }
        });
    }

    let selected = state
        .selected
        .filter(|&m| built.as_ref().is_some_and(|b| m < b.molecule_count()));
    if let Some(m) = selected {
        ui.add_space(2.0);
        match state.design.manual_index(m) {
            Some(k) => {
                ui.label(
                    RichText::new(t!(
                        "assembly.place.manual_title",
                        molecule = m + 1,
                        index = k + 1
                    ))
                    .strong(),
                );
                if pose_editor(ui, state, k) {
                    state.touch();
                }
                ui.label(ink.muted(t!("assembly.place.drag_hint")));
            }
            None => {
                ui.label(RichText::new(t!("assembly.place.step_title", molecule = m + 1)).strong());
                ui.label(ink.muted(t!("assembly.place.step_note")));
                if ui
                    .button(t!("assembly.place.detach"))
                    .on_hover_text(t!("assembly.place.detach_tip"))
                    .clicked()
                {
                    action = Some(Action::Detach(m));
                }
            }
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(t!("assembly.place.duplicate"))
                .on_hover_text(t!("assembly.place.duplicate_tip"))
                .clicked()
            {
                action = Some(Action::Duplicate(m));
            }
            let tip = if state.design.manual_index(m).is_some() {
                t!("assembly.place.remove_manual_tip")
            } else {
                t!("assembly.place.remove_step_tip")
            };
            if ui
                .button(t!("assembly.place.remove"))
                .on_hover_text(tip)
                .clicked()
            {
                action = Some(Action::Remove(m));
            }
        });
    } else {
        ui.label(ink.muted(t!("assembly.place.select_hint")));
    }

    if let Some(action) = action {
        act(action, state, view, built.as_deref());
    }
}

/// The position and rotation fields of hand-placed molecule `k`. Returns whether they changed.
fn pose_editor(ui: &mut egui::Ui, state: &mut AssemblyState, k: usize) -> bool {
    let Some((_, local)) = manual_centroids(state, k) else {
        return false;
    };
    let t = &mut state.design.manual[k].transform;
    let mut pose = geometry::pose_of(t, local);
    let mut changed = false;
    egui::Grid::new("pose")
        .num_columns(2)
        .spacing(vec2(8.0, 4.0))
        .show(ui, |ui| {
            changed |= triple(
                ui,
                t!("assembly.place.position"),
                &mut pose.position,
                0.05,
                " Å",
                t!("assembly.place.position_tip"),
            );
            changed |= triple(
                ui,
                t!("assembly.place.rotation"),
                &mut pose.euler_deg,
                0.5,
                "°",
                t!("assembly.place.rotation_tip"),
            );
        });
    if changed {
        *t = geometry::transform_of(&pose, local);
    }
    changed
}

fn triple(
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
                        .max_decimals(2)
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

/// Where to put a new molecule: beside the selected one (to the right on screen), or at the
/// centre of the view. The point and the direction to look further along for room.
fn spot(
    view: &AssemblyView,
    state: &AssemblyState,
    built: Option<&Built>,
    new: f64,
) -> (Vec3, Vec3) {
    let selected = state.selected.zip(built).and_then(|(m, b)| {
        (m < b.molecule_count()).then(|| {
            let points: Vec<Vec3> = b.molecule_atoms(m).map(|a| b.atoms[a].pos).collect();
            (geometry::centroid(&points), geometry::radius(&points))
        })
    });
    let (centre, right) = match &view.viewport {
        Some(viewport) => {
            let rect = egui::Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
            let at = selected.map_or_else(|| viewport.view_center(), |s| s.0);
            (
                viewport.view_center(),
                viewport.screen_basis(rect, at).right,
            )
        }
        None => ([0.0; 3], [1.0, 0.0, 0.0]),
    };
    match selected {
        Some((c, r)) => (
            math::add(c, math::scale(right, r + new + NEXT_TO_GAP)),
            right,
        ),
        None => (centre, right),
    }
}

/// Put a new hand-placed molecule of component `c`, turned by `rot`, where it clashes with
/// nothing, and select it. Returns its molecule index.
fn put(
    state: &mut AssemblyState,
    view: &AssemblyView,
    built: Option<&Built>,
    c: usize,
    rot: math::Mat3,
) -> Option<usize> {
    let component = state.design.components.get(c)?;
    let size = geometry::radius(&geometry::local_positions(component));
    let (near, direction) = spot(view, state, built, size);
    let empty = Built::default();
    let inst = geometry::place_new(built.unwrap_or(&empty), component, c, rot, near, direction);
    let m = state.edit(|d| {
        d.manual.push(inst);
        d.manual_molecule(d.manual.len() - 1)
    });
    state.selected = Some(m);
    Some(m)
}

fn act(action: Action, state: &mut AssemblyState, view: &mut AssemblyView, built: Option<&Built>) {
    match action {
        Action::Add(c) => {
            if let Some(m) = put(state, view, built, c, math::IDENTITY) {
                let name = &state.design.components[c].name;
                view.report = Some((
                    t!("assembly.place.report.added", name = name, molecule = m + 1),
                    false,
                ));
            }
        }
        Action::Duplicate(m) => {
            let Some(source) = state.design.instances().into_iter().nth(m) else {
                return;
            };
            if let Some(copy) = put(state, view, built, source.component, source.transform.rot) {
                view.report = Some((
                    t!(
                        "assembly.place.report.duplicated",
                        source = m + 1,
                        molecule = copy + 1
                    ),
                    false,
                ));
            }
        }
        Action::Remove(m) => {
            if state.edit(|d| d.remove_molecule(m)) {
                state.selected = None;
                view.report = Some((t!("assembly.place.report.removed", molecule = m + 1), false));
            }
        }
        Action::Detach(m) => {
            if let Some(moved) = state.edit(|d| d.detach(m)) {
                state.selected = Some(moved);
                view.report = Some((
                    t!(
                        "assembly.place.report.detached",
                        source = m + 1,
                        molecule = moved + 1
                    ),
                    false,
                ));
            }
        }
        Action::Restore => {
            state.edit(|d| d.excluded.clear());
            state.selected = None;
            view.report = Some((t!("assembly.place.report.restored").to_owned(), false));
        }
    }
}

/// The clash check's answer in the 情報 section.
pub(super) fn clash_info(
    ui: &mut egui::Ui,
    state: &mut AssemblyState,
    view: &mut AssemblyView,
    ink: &Ink,
) {
    let Ok(built) = state.built() else { return };
    let warn = if ui.visuals().dark_mode {
        CLASH_STROKE
    } else {
        CLASH_ON_LIGHT
    };
    let Some((clashes, current)) = view.clashes.get(ui.ctx(), state.id(), &built) else {
        ui.label(ink.muted(t!("assembly.place.clash.checking")));
        return;
    };
    let pending = if current {
        ""
    } else {
        t!("assembly.place.clash.updating")
    };
    if clashes.pairs.is_empty() {
        ui.label(ink.muted(t!("assembly.place.clash.none", pending = pending)));
        return;
    }
    let more = if clashes.truncated {
        t!("assembly.place.clash.at_least")
    } else {
        ""
    };
    ui.label(
        RichText::new(tn!(
            "assembly.place.clash.count",
            clashes.pairs.len(),
            more = more,
            pending = pending
        ))
        .size(12.0)
        .color(warn),
    )
    .on_hover_text(t!("assembly.place.clash.tip"));
    let label = |a: usize| {
        let m = built.molecule_of_atom[a];
        t!(
            "assembly.place.clash.atom",
            molecule = m + 1,
            element = built.atoms[a].element,
            index = a - built.molecule_start[m] + 1
        )
    };
    for c in clashes.pairs.iter().take(LISTED_CLASHES) {
        let image = if clash::through_image(&built, c) {
            t!("assembly.place.clash.periodic_image")
        } else {
            ""
        };
        let text = t!(
            "assembly.place.clash.pair",
            a = label(c.a),
            b = label(c.b),
            image = image,
            distance = format!("{:.2}", c.distance)
        );
        if ui
            .add(egui::Button::new(RichText::new(text).size(11.0).color(warn)).frame(false))
            .on_hover_text(t!("assembly.place.clash.pick_tip"))
            .clicked()
        {
            state.selected = Some(built.molecule_of_atom[c.a]);
        }
    }
    if clashes.pairs.len() > LISTED_CLASHES {
        ui.label(ink.muted(tn!(
            "assembly.place.clash.more",
            clashes.pairs.len() - LISTED_CLASHES
        )));
    }
}

/// The highlight colour of clashing atoms (the 3D view's).
pub(super) const CLASH_HIGHLIGHT: (f32, f32, f32) = CLASH_COLOR;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::{Component, Design, Step};
    use crate::three_d::{Atom3d, Bond3d};

    fn component(name: &str, element: &str) -> Component {
        let atom = |e: &str, pos| Atom3d {
            element: e.into(),
            pos,
        };
        Component {
            name: name.into(),
            atoms: vec![atom(element, [0.0; 3]), atom("H", [1.0, 0.0, 0.0])],
            bonds: vec![Bond3d {
                begin: 0,
                end: 1,
                order: 1,
            }],
            orient: None,
        }
    }

    fn host_and_guest() -> AssemblyState {
        AssemblyState::with_design(Design {
            components: vec![component("host", "C"), component("guest", "O")],
            steps: vec![Step::new_ring()],
            ..Default::default()
        })
    }

    fn act_on(state: &mut AssemblyState, view: &mut AssemblyView, action: Action) {
        let built = state.built().ok();
        act(action, state, view, built.as_deref());
    }

    fn frame() -> Frame3d {
        Frame3d {
            rect: egui::Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0)),
            primary: None,
            secondary: None,
            hovered: None,
            wheel_notches: 0.0,
            drag: None,
        }
    }

    #[test]
    fn molecules_are_added_duplicated_and_removed_one_undo_step_each() {
        let mut state = host_and_guest();
        let mut view = AssemblyView::default();
        // With a ring molecule selected, the guest goes beside it, clear of everything.
        state.selected = Some(0);
        act_on(&mut state, &mut view, Action::Add(1));
        assert_eq!(state.design.manual.len(), 1);
        assert_eq!(state.design.manual[0].component, 1);
        assert_eq!(state.selected, Some(6));
        let built = state.built().unwrap();
        assert_eq!(built.component_of_molecule[6], 1);
        assert!(clash::find(&built, None).pairs.is_empty(), "placed clear");

        act_on(&mut state, &mut view, Action::Duplicate(6));
        assert_eq!(state.design.manual.len(), 2);
        assert_eq!(state.selected, Some(7));
        assert_eq!(state.design.manual[1].component, 1);
        assert!(clash::find(&state.built().unwrap(), None).pairs.is_empty());

        act_on(&mut state, &mut view, Action::Remove(6));
        assert_eq!(state.design.manual.len(), 1);
        assert_eq!(state.selected, None);

        // Each action was one step.
        assert!(state.undo());
        assert_eq!(state.design.manual.len(), 2);
        assert!(state.undo() && state.undo());
        assert!(state.design.manual.is_empty());
        assert!(!state.undo());
    }

    #[test]
    fn a_ring_molecule_is_detached_moved_and_put_back() {
        let mut state = host_and_guest();
        let mut view = AssemblyView::default();
        let before = state.built().unwrap().atoms[4].pos;
        act_on(&mut state, &mut view, Action::Detach(2));
        assert_eq!(state.selected, Some(5));
        let built = state.built().unwrap();
        assert_eq!(built.molecule_count(), 6);
        // Molecule 2's first atom is now molecule 5's, in the same place.
        assert_eq!(built.atoms[10].pos, before);

        // A mouse drag (Shift: move) over several frames, then released: one undo step.
        let viewport = Viewport3d::new();
        for _ in 0..5 {
            drag_selected(&mut state, &viewport, &frame(), vec2(10.0, 0.0), false);
        }
        // Alt: turn about the centroid, which stays put.
        let (centre, _) = manual_centroids(&state, 0).unwrap();
        drag_selected(&mut state, &viewport, &frame(), vec2(0.0, 40.0), true);
        let (turned, _) = manual_centroids(&state, 0).unwrap();
        assert!(math::distance(centre, turned) < 1e-9);
        state.settle();
        let moved = state.built().unwrap().atoms[10].pos;
        assert!(math::distance(moved, before) > 0.5, "{moved:?}");
        assert!(state.undo());
        assert_eq!(state.built().unwrap().atoms[10].pos, before);

        act_on(&mut state, &mut view, Action::Restore);
        assert!(state.design.excluded.is_empty());
        assert_eq!(state.built().unwrap().molecule_count(), 7);
    }

    #[test]
    fn step_molecules_do_not_move_with_the_mouse() {
        let mut state = host_and_guest();
        state.selected = Some(1);
        let revision = state.revision();
        drag_selected(
            &mut state,
            &Viewport3d::new(),
            &frame(),
            vec2(10.0, 0.0),
            false,
        );
        assert_eq!(state.revision(), revision);
    }

    /// Headless frames of the section and the clash list, the guest first clear and then on
    /// top of a ring molecule.
    #[test]
    fn the_placement_section_and_clash_list_draw_headless() {
        let ctx = egui::Context::default();
        let mut state = host_and_guest();
        let mut view = AssemblyView::default();
        act_on(&mut state, &mut view, Action::Add(1));
        let ink = Ink {
            muted: dark::MUTED,
            warn: CLASH_STROKE,
        };
        for round in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                section(ui, &mut state, &mut view, &ink);
                clash_info(ui, &mut state, &mut view, &ink);
            });
            output.textures_delta.clear();
            let built = state.built().unwrap();
            let (clashes, current) = view.clashes.get(&ctx, state.id(), &built).unwrap();
            assert!(current);
            assert_eq!(clashes.pairs.is_empty(), round == 0);
            // Put the guest's centroid on ring molecule 0's.
            let local = geometry::centroid(&geometry::local_positions(&state.design.components[1]));
            let k = state.design.manual.len() - 1;
            state.design.manual[k].transform =
                math::Rigid::translation(math::sub([5.5, 0.0, 0.0], local));
            state.touch();
        }
    }
}
