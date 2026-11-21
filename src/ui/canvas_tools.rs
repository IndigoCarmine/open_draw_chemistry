//! The palette's ring and chain tools (鎖, ベンゼン環, シクロペンタン, シクロヘキサン).
//!
//! The editor widget has no mouse tool for these, so they are worked here, around it
//! ([`CanvasTools::before_show`] and [`CanvasTools::after_show`] bracket the widget's frame). While
//! one is on, the widget is left on its Select tool, which keeps hovering, panning, zooming and the
//! keys working as they always do; what a click or a drag does is then this module's:
//!
//! - **Ring**: a click on a bond fuses the ring onto it, one on an atom grows it from the atom.
//!   Both are the widget's own ring keys (`6`, `5`, `a`), pressed for the user on the next frame
//!   on what was clicked, so the ring comes out exactly as the key would draw it (fusion on any
//!   side, onto atoms already there, heteroatoms kept). A click on empty canvas puts a
//!   free-standing ring there, centred on the click, flat side down.
//! - **Chain**: a drag from an atom or from empty canvas draws a zigzag carbon chain along the
//!   pointer, one carbon per step of the drag, previewed as it goes (Escape gives it up); a
//!   click on an atom adds one carbon to it.
//!
//! The Select tool's own reaction to the same click (toggling the selection) is undone, and its
//! reaction to the drag (a lasso, or moving the atom) is given up as it starts.

use chembuider_rs::config::{AtomAction, AtomShortcut, BondAction, BondShortcut};
use chembuider_rs::molecule::Molecule;
use chembuider_rs::molecule::fragment::Fragment;
use chembuider_rs::{BondOrder, ChemStructEditor, EditorResponse, Tool};
use std::collections::HashSet;

/// The editor's usual bond length (molecule units), as its fragments and ring keys draw.
const BOND_LENGTH: f32 = 1.5;

/// The ring a ring tool draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingKind {
    Benzene,
    Cyclopentane,
    Cyclohexane,
}

impl RingKind {
    /// The ring as a fragment: benzene as the editor's configuration has it (the ring its
    /// benzene key draws), a cycloalkane as a regular polygon.
    fn fragment(self, editor: &ChemStructEditor) -> Fragment {
        match self {
            RingKind::Benzene => editor
                .config
                .resolve_fragment("benzene")
                .unwrap_or_else(Fragment::benzene),
            RingKind::Cyclopentane => Fragment::ring(5),
            RingKind::Cyclohexane => Fragment::ring(6),
        }
    }

    fn atom_action(self) -> AtomAction {
        match self {
            RingKind::Benzene => AtomAction::Fragment {
                fragment: "benzene".into(),
            },
            RingKind::Cyclopentane => AtomAction::Ring { ring: 5 },
            RingKind::Cyclohexane => AtomAction::Ring { ring: 6 },
        }
    }

    fn bond_action(self) -> BondAction {
        match self {
            RingKind::Benzene => BondAction::Fragment {
                fragment: "benzene".into(),
            },
            RingKind::Cyclopentane => BondAction::Ring { ring: 5 },
            RingKind::Cyclohexane => BondAction::Ring { ring: 6 },
        }
    }
}

/// A drawing tool of the palette's that the editor widget does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasTool {
    Chain,
    Ring(RingKind),
}

/// What a ring click landed on, to be drawn on the next frame.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Atom(u32),
    Bond(u32),
}

/// A chain being dragged out: from atom `from` (or from empty canvas) at `start`, towards `end`
/// (molecule coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
struct ChainDrag {
    from: Option<u32>,
    start: [f32; 2],
    end: [f32; 2],
}

/// The editor's selection, kept across a frame in which the Select tool's click is not wanted.
struct Selection {
    atoms: HashSet<u32>,
    bonds: HashSet<u32>,
    objects: HashSet<u32>,
}

impl Selection {
    fn of(editor: &ChemStructEditor) -> Self {
        Self {
            atoms: editor.selected_atoms.clone(),
            bonds: editor.selected_bonds.clone(),
            objects: editor.selected_objects.clone(),
        }
    }

    fn restore(self, editor: &mut ChemStructEditor) {
        editor.selected_atoms = self.atoms;
        editor.selected_bonds = self.bonds;
        editor.selected_objects = self.objects;
    }
}

/// The key pressed for the user to draw a ring with the widget's own ring code. Its binding is
/// set for that one frame only (see [`CanvasTools::before_show`]), so what the user has bound to
/// it does not matter.
const RING_KEY: egui::Key = egui::Key::Num6;
const RING_KEY_NAME: &str = "6";

fn ring_key_event() -> egui::Event {
    egui::Event::Key {
        key: RING_KEY,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

/// The palette's ring and chain tools, for one document.
#[derive(Default)]
pub struct CanvasTools {
    active: Option<CanvasTool>,
    /// A ring click waiting to be drawn on the next frame.
    pending_ring: Option<(RingKind, Target)>,
    chain: Option<ChainDrag>,
    /// The selection before this frame's widget input.
    selection: Option<Selection>,
    /// The user's shortcuts, while the ring key's binding stands in for them.
    shortcuts: Option<(Vec<AtomShortcut>, Vec<BondShortcut>)>,
    /// The drawing before this frame's widget input, while the button is down with the chain
    /// tool and no chain is being drawn yet: what a drag that starts this frame starts on.
    pressed: Option<Molecule>,
}

impl CanvasTools {
    /// The tool that is on, if one of these is.
    pub fn active(&self) -> Option<CanvasTool> {
        self.active
    }

    /// Turn `tool` on (the editor goes to its Select tool, under it), or with `None` turn off
    /// whichever is on.
    pub fn select(&mut self, editor: &mut ChemStructEditor, tool: Option<CanvasTool>) {
        self.pending_ring = None;
        self.chain = None;
        self.active = tool;
        if tool.is_some() {
            editor.set_tool(Tool::Select);
        }
    }

    /// Before the widget's frame. A tool chosen some other way (a palette element, a menu,
    /// Ctrl+1/2/3) turns these off; a ring clicked last frame is drawn with the ring key.
    pub fn before_show(&mut self, editor: &mut ChemStructEditor, ctx: &egui::Context) {
        let switched = ctx.input(|i| {
            i.events.iter().any(|e| {
                matches!(e, egui::Event::Key { key: egui::Key::Num1 | egui::Key::Num2 | egui::Key::Num3, pressed: true, modifiers, .. } if modifiers.ctrl || modifiers.command)
            })
        });
        if self.active.is_some() && (editor.tool != Tool::Select || switched) {
            self.active = None;
        }
        if self.active.is_none() {
            self.pending_ring = None;
            self.chain = None;
            return;
        }
        self.selection = Some(Selection::of(editor));
        self.pressed = (self.active == Some(CanvasTool::Chain)
            && self.chain.is_none()
            && ctx.input(|i| i.pointer.primary_down()))
        .then(|| editor.molecule.clone());

        if let Some((kind, target)) = self.pending_ring.take() {
            // The pointer has not moved off what was clicked (or the click would not be this
            // frame's); the keyboard target is set to it as well, should it have.
            match target {
                Target::Atom(id) => {
                    editor.hotspot_atom = Some(id);
                    editor.hotspot_bond = None;
                }
                Target::Bond(id) => editor.hotspot_bond = Some(id),
            }
            let atom = std::mem::replace(
                &mut editor.config.atom_shortcuts,
                vec![AtomShortcut {
                    key: RING_KEY_NAME.into(),
                    ctrl: false,
                    alt: false,
                    action: kind.atom_action(),
                }],
            );
            let bond = std::mem::replace(
                &mut editor.config.bond_shortcuts,
                vec![BondShortcut {
                    key: RING_KEY_NAME.into(),
                    ctrl: false,
                    alt: false,
                    action: kind.bond_action(),
                }],
            );
            self.shortcuts = Some((atom, bond));
            ctx.input_mut(|i| i.events.push(ring_key_event()));
        }
    }

    /// After the widget's frame: what this frame's click or drag does with the tool that is on.
    pub fn after_show(
        &mut self,
        editor: &mut ChemStructEditor,
        ui: &egui::Ui,
        response: &mut EditorResponse,
    ) {
        if let Some((atom, bond)) = self.shortcuts.take() {
            editor.config.atom_shortcuts = atom;
            editor.config.bond_shortcuts = bond;
            let key = ring_key_event();
            ui.ctx().input_mut(|i| i.events.retain(|e| *e != key));
        }
        let selection = self.selection.take();
        let Some(tool) = self.active else { return };
        let changed = match tool {
            CanvasTool::Ring(kind) => {
                self.ring_input(editor, ui, &response.response, kind, selection)
            }
            CanvasTool::Chain => self.chain_input(editor, ui, &response.response, selection),
        };
        response.modified |= changed;
    }

    /// A click with a ring tool. Returns whether the drawing changed (now).
    fn ring_input(
        &mut self,
        editor: &mut ChemStructEditor,
        ui: &egui::Ui,
        response: &egui::Response,
        kind: RingKind,
        selection: Option<Selection>,
    ) -> bool {
        if !response.clicked() {
            return false;
        }
        let Some(at) = response.interact_pointer_pos() else {
            return false;
        };
        let center = response.rect.center();
        // A text, table, arrow or bracket is the Select tool's, as with every drawing tool.
        if editor.hit_test_atom(at, center).is_none()
            && editor.hit_test_object(at, center).is_some()
        {
            return false;
        }
        if let Some(selection) = selection {
            selection.restore(editor);
        }
        let target = editor
            .hit_test_atom(at, center)
            .map(Target::Atom)
            .or_else(|| editor.hit_test_bond(at, center).map(Target::Bond));
        match target {
            Some(target) => {
                self.pending_ring = Some((kind, target));
                ui.ctx().request_repaint();
                false
            }
            // The second click of a double-click would put a second ring on top of the first.
            None if response.double_clicked() => false,
            None => {
                let pos = editor.screen_to_mol(at, center);
                as_undo_step(editor, |editor| {
                    place_ring(editor, &kind.fragment(editor), pos)
                })
            }
        }
    }

    /// A click or a drag with the chain tool. Returns whether the drawing changed.
    fn chain_input(
        &mut self,
        editor: &mut ChemStructEditor,
        ui: &egui::Ui,
        response: &egui::Response,
        selection: Option<Selection>,
    ) -> bool {
        let center = response.rect.center();
        if response.drag_started() {
            let origin = ui
                .input(|i| i.pointer.press_origin())
                .or_else(|| response.interact_pointer_pos());
            if let Some(origin) = origin {
                // The Select tool has already begun its drag this frame, and may have moved the
                // atom pressed on: what was pressed on is looked at in the drawing as it was.
                let moved = self
                    .pressed
                    .take()
                    .map(|before| std::mem::replace(&mut editor.molecule, before));
                let from = editor.hit_test_atom(origin, center);
                let on_other = editor.hit_test_object(origin, center).is_some()
                    || editor.hit_test_bond(origin, center).is_some();
                if let Some(moved) = moved {
                    editor.molecule = moved;
                }
                // From a bond or an object the drag is the Select tool's (a lasso, a move).
                if from.is_some() || !on_other {
                    // Give up the lasso or move the Select tool has just begun (switching tools
                    // does that, putting back anything it moved).
                    editor.set_tool(Tool::Eraser);
                    editor.set_tool(Tool::Select);
                    if let Some(selection) = selection {
                        selection.restore(editor);
                    }
                    let start = match from.and_then(|id| editor.molecule.atom_by_id(id)) {
                        Some(atom) => atom.pos,
                        None => editor.screen_to_mol(origin, center),
                    };
                    self.chain = Some(ChainDrag {
                        from,
                        start,
                        end: start,
                    });
                }
            }
            return false;
        }

        if let Some(drag) = self.chain.as_mut() {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.chain = None;
                return false;
            }
            if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
                drag.end = editor.screen_to_mol(p, center);
            }
            let drag = *drag;
            if response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
                self.chain = None;
                return as_undo_step(editor, |editor| {
                    finish_chain(editor, drag.from, drag.start, drag.end)
                });
            }
            paint_chain(
                editor,
                ui,
                response.rect,
                &chain_points(editor, drag.from, drag.start, drag.end),
            );
            return false;
        }

        if response.clicked()
            && let Some(at) = response.interact_pointer_pos()
            && let Some(atom) = editor.hit_test_atom(at, center)
        {
            if let Some(selection) = selection {
                selection.restore(editor);
            }
            return as_undo_step(editor, |editor| add_carbon(editor, atom));
        }
        false
    }

    /// Where the atoms of the chain being dragged will go (molecule coordinates, its first atom
    /// first), or `None` when no chain is being dragged.
    pub fn chain_preview(&self, editor: &ChemStructEditor) -> Option<Vec<[f32; 2]>> {
        self.chain
            .map(|d| chain_points(editor, d.from, d.start, d.end))
    }
}

/// Run `change` as one undo step, if it changes the drawing. Returns whether it did.
fn as_undo_step(editor: &mut ChemStructEditor, change: impl FnOnce(&mut ChemStructEditor)) -> bool {
    let before = editor.molecule.clone();
    change(editor);
    if editor.molecule == before {
        return false;
    }
    let after = std::mem::replace(&mut editor.molecule, before);
    editor.push_undo();
    editor.molecule = after;
    true
}

/// A free-standing copy of `frag`, centred on `center`. Its last atom becomes the hotspot.
fn place_ring(editor: &mut ChemStructEditor, frag: &Fragment, center: [f32; 2]) {
    let n = frag.atoms.len() as f32;
    let c = frag.atoms.iter().fold([0.0, 0.0], |c, a| {
        [c[0] + a.pos[0] / n, c[1] + a.pos[1] / n]
    });
    let ids: Vec<u32> = frag
        .atoms
        .iter()
        .map(|a| {
            let pos = [a.pos[0] - c[0] + center[0], a.pos[1] - c[1] + center[1]];
            editor.molecule.add_atom(a.element.clone(), pos, a.charge)
        })
        .collect();
    for b in &frag.bonds {
        editor
            .molecule
            .add_bond(ids[b.begin], ids[b.end], b.order.clone());
    }
    editor.hotspot_atom = ids.last().copied();
}

/// Where a zigzag chain dragged from `start` to `end` puts its atoms, `start` first. It runs
/// along the drag, its direction snapped to 30° steps, with bonds of the usual length at 120°
/// to each other, and has as many bonds as fit the drag's length (none for a drag shorter than
/// half a step). Its first bond leans to the side with the most room beside the bonds atom
/// `from` already has.
fn chain_points(
    editor: &ChemStructEditor,
    from: Option<u32>,
    start: [f32; 2],
    end: [f32; 2],
) -> Vec<[f32; 2]> {
    use std::f32::consts::{FRAC_PI_6, TAU};
    let (dx, dy) = (end[0] - start[0], end[1] - start[1]);
    let step = BOND_LENGTH * FRAC_PI_6.cos();
    let rise = BOND_LENGTH * FRAC_PI_6.sin();
    let n = ((dx * dx + dy * dy).sqrt() / step).round() as usize;
    if n == 0 {
        return vec![start];
    }
    let angle = (dy.atan2(dx) / FRAC_PI_6).round() * FRAC_PI_6;
    let u = [angle.cos(), angle.sin()];
    // Screen y points down, so this is to the left of the drag: up, for a drag to the right.
    let perp = [u[1], -u[0]];

    let existing: Vec<f32> = from
        .and_then(|id| editor.molecule.atom_by_id(id))
        .map(|atom| {
            editor
                .molecule
                .neighbor_atom_ids(atom.id)
                .iter()
                .filter_map(|&nb| editor.molecule.atom_by_id(nb))
                .map(|nb| (nb.pos[1] - atom.pos[1]).atan2(nb.pos[0] - atom.pos[0]))
                .collect()
        })
        .unwrap_or_default();
    let clearance = |side: f32| {
        let first =
            (u[1] * step + side * perp[1] * rise).atan2(u[0] * step + side * perp[0] * rise);
        existing
            .iter()
            .map(|&e| {
                let d = (first - e).rem_euclid(TAU);
                d.min(TAU - d)
            })
            .fold(f32::INFINITY, f32::min)
    };
    let side = if clearance(-1.0) > clearance(1.0) + 1e-3 {
        -1.0
    } else {
        1.0
    };

    (0..=n)
        .map(|k| {
            let along = k as f32 * step;
            let off = if k % 2 == 1 { side * rise } else { 0.0 };
            [
                start[0] + u[0] * along + perp[0] * off,
                start[1] + u[1] * along + perp[1] * off,
            ]
        })
        .collect()
}

/// Take off hydrogens the new bonds on `atoms` leave no room for, and forget them.
fn consume_h(editor: &mut ChemStructEditor, atoms: &[u32]) {
    for &id in atoms {
        for h in editor.molecule.consume_excess_group_h(id) {
            forget(editor, h);
        }
    }
}

/// Make room on `atom` for a bond about to be drawn out of it.
fn make_room(editor: &mut ChemStructEditor, atom: u32) {
    for h in editor.molecule.make_room_for_bond(atom) {
        forget(editor, h);
    }
}

fn forget(editor: &mut ChemStructEditor, atom: u32) {
    editor.selected_atoms.remove(&atom);
    if editor.hovered_atom == Some(atom) {
        editor.hovered_atom = None;
    }
}

/// A carbon at `pos`, or the atom already there.
fn atom_at(editor: &mut ChemStructEditor, pos: [f32; 2], before: u32) -> u32 {
    match editor.molecule.find_atom_near(pos, Molecule::SNAP_RADIUS) {
        Some(id) => id,
        None => {
            make_room(editor, before);
            editor.molecule.add_atom("C".to_string(), pos, 0)
        }
    }
}

/// Bond `a` to `b` with a single bond, if they are not the same atom or bonded already.
fn join(editor: &mut ChemStructEditor, a: u32, b: u32) {
    if a != b && editor.molecule.add_bond(a, b, BondOrder::Single) != 0 {
        consume_h(editor, &[a, b]);
    }
}

/// Draw the zigzag [`chain_points`] gives, from atom `from` (or a new carbon at `start`),
/// joining any atom one of its points lands on. Its last atom becomes the hotspot.
fn finish_chain(editor: &mut ChemStructEditor, from: Option<u32>, start: [f32; 2], end: [f32; 2]) {
    let points = chain_points(editor, from, start, end);
    if points.len() < 2 {
        return;
    }
    let mut prev = match from.filter(|&id| editor.molecule.atom_by_id(id).is_some()) {
        Some(id) => id,
        None => editor
            .molecule
            .find_atom_near(start, Molecule::SNAP_RADIUS)
            .unwrap_or_else(|| editor.molecule.add_atom("C".to_string(), start, 0)),
    };
    for &p in &points[1..] {
        let id = atom_at(editor, p, prev);
        join(editor, prev, id);
        prev = id;
    }
    editor.hotspot_atom = Some(prev);
}

/// One carbon more on `atom`, in its best free direction; it becomes the hotspot.
fn add_carbon(editor: &mut ChemStructEditor, atom: u32) {
    if editor.molecule.atom_by_id(atom).is_none() {
        return;
    }
    make_room(editor, atom);
    let angle = editor.best_new_bond_angle(atom);
    let Some(src) = editor.molecule.atom_by_id(atom) else {
        return;
    };
    let pos = [
        src.pos[0] + angle.cos() * BOND_LENGTH,
        src.pos[1] + angle.sin() * BOND_LENGTH,
    ];
    let id = atom_at(editor, pos, atom);
    join(editor, atom, id);
    editor.hotspot_atom = Some(id);
}

/// The chain preview: the zigzag as it will be drawn, with its number of carbons at the end.
fn paint_chain(editor: &ChemStructEditor, ui: &egui::Ui, rect: egui::Rect, points: &[[f32; 2]]) {
    if points.len() < 2 {
        return;
    }
    let center = rect.center();
    let color = egui::Color32::from_rgba_unmultiplied(80, 140, 255, 180);
    let screen: Vec<egui::Pos2> = points
        .iter()
        .map(|&p| editor.mol_to_screen(p, center))
        .collect();
    let last = screen[screen.len() - 1];
    let painter = ui.painter_at(rect);
    painter.add(egui::Shape::line(
        screen,
        egui::Stroke::new(1.5 * editor.zoom, color),
    ));
    painter.text(
        last + egui::vec2(8.0, -8.0),
        egui::Align2::LEFT_BOTTOM,
        format!("{}", points.len() - 1),
        egui::FontId::proportional(12.0),
        color,
    );
}

#[cfg(test)]
mod tests;
