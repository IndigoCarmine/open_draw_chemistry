//! Editing a 3D model by hand: what is picked in the viewer, the restraints the next
//! optimisation keeps, and the undo/redo history of the geometry.
//!
//! Most edits only move atoms, so their history step is a coordinate snapshot. Each is written
//! into both [`Model3d::atoms`] and [`Model3d::molfile`] (see [`set_coords`]), so optimising
//! and exporting start from what is on screen. The arithmetic is in [`super::geom`].
//!
//! Deleting atoms ([`delete_atoms`]) is the one edit that changes the structure: its history
//! step holds the whole model, and what named atoms by index (the picks, the fixed atom, the
//! restraints) is renumbered or let go.

use std::collections::BTreeSet;
use std::fmt;

use super::Model3d;
use super::geom::{self, Graph, Vec3};

/// The most atoms any edit needs: four, for a dihedral.
pub const MAX_PICKED: usize = 4;
/// Geometry snapshots kept for undo.
const HISTORY: usize = 100;

/// A restraint the next optimisation keeps (atom indices into [`Model3d::atoms`], 0-based).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Constraint {
    /// The atom does not move.
    Fix(usize),
    Distance([usize; 2], f64),
    Angle([usize; 3], f64),
    Torsion([usize; 4], f64),
}

impl Constraint {
    /// The restraint over `atoms` (one to four of them) at `value` (Å or degrees; ignored for a
    /// single atom, which is fixed where it is).
    pub fn over(atoms: &[usize], value: f64) -> Option<Self> {
        Some(match *atoms {
            [a] => Constraint::Fix(a),
            [a, b] => Constraint::Distance([a, b], value),
            [a, b, c] => Constraint::Angle([a, b, c], value),
            [a, b, c, d] => Constraint::Torsion([a, b, c, d], value),
            _ => return None,
        })
    }

    pub fn atoms(&self) -> &[usize] {
        match self {
            Constraint::Fix(a) => std::slice::from_ref(a),
            Constraint::Distance(atoms, _) => atoms,
            Constraint::Angle(atoms, _) => atoms,
            Constraint::Torsion(atoms, _) => atoms,
        }
    }

    pub fn target(&self) -> Option<f64> {
        match *self {
            Constraint::Fix(_) => None,
            Constraint::Distance(_, v) | Constraint::Angle(_, v) | Constraint::Torsion(_, v) => {
                Some(v)
            }
        }
    }

    /// Whether this restrains the same atoms (in either direction) as `other`, so setting a new
    /// value replaces the old restraint rather than adding a contradictory one.
    pub fn same_atoms(&self, other: &Constraint) -> bool {
        let (a, b) = (self.atoms(), other.atoms());
        a.len() == b.len() && (a == b || a.iter().rev().eq(b.iter()))
    }

    /// Every index names one of `atoms` atoms, none twice.
    pub fn is_valid_for(&self, atoms: usize) -> bool {
        let list = self.atoms();
        list.iter().all(|&i| i < atoms)
            && list.iter().enumerate().all(|(n, a)| !list[..n].contains(a))
    }

    /// The restraint after the atoms are renumbered by `map` (old index → new, `None` for an
    /// atom that is gone), or `None` when one of its atoms is gone.
    pub fn remapped(&self, map: &[Option<usize>]) -> Option<Constraint> {
        let at = |i: usize| map.get(i).copied().flatten();
        Some(match *self {
            Constraint::Fix(a) => Constraint::Fix(at(a)?),
            Constraint::Distance([a, b], v) => Constraint::Distance([at(a)?, at(b)?], v),
            Constraint::Angle([a, b, c], v) => Constraint::Angle([at(a)?, at(b)?, at(c)?], v),
            Constraint::Torsion([a, b, c, d], v) => {
                Constraint::Torsion([at(a)?, at(b)?, at(c)?, at(d)?], v)
            }
        })
    }

    /// What the restrained quantity measures in `coords`.
    pub fn measure(&self, coords: &[Vec3]) -> Option<f64> {
        if !self.is_valid_for(coords.len()) {
            return None;
        }
        Some(match *self {
            Constraint::Fix(_) => return None,
            Constraint::Distance([a, b], _) => geom::distance(coords[a], coords[b]),
            Constraint::Angle([a, b, c], _) => geom::angle(coords[a], coords[b], coords[c]),
            Constraint::Torsion([a, b, c, d], _) => {
                geom::dihedral(coords[a], coords[b], coords[c], coords[d])
            }
        })
    }

    pub fn unit(&self) -> &'static str {
        match self {
            Constraint::Fix(_) => "",
            Constraint::Distance(..) => "Å",
            Constraint::Angle(..) | Constraint::Torsion(..) => "°",
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            Constraint::Fix(_) => t!("three_d.edit.constraint_fix"),
            Constraint::Distance(..) => t!("three_d.edit.constraint_distance"),
            Constraint::Angle(..) => t!("three_d.edit.constraint_angle"),
            Constraint::Torsion(..) => t!("three_d.edit.constraint_torsion"),
        }
    }
}

/// One history entry: the geometry (and energy) before or after an edit.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    label: String,
    coords: Vec<Vec3>,
    energy: f64,
    energy_stale: bool,
    /// For a step that changed the atoms themselves: everything it replaced.
    structure: Option<Box<Structure>>,
}

/// The model and what names its atoms by index, on one side of an edit that changed the atoms.
#[derive(Debug, Clone, PartialEq)]
struct Structure {
    model: Model3d,
    picked: Vec<usize>,
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
    constraints: Vec<Constraint>,
}

/// The editing state of one document's 3D model.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit3d {
    /// Atoms picked in the viewer, oldest first; at most [`MAX_PICKED`].
    pub picked: Vec<usize>,
    /// Atoms selected to be deleted (Ctrl+click, Ctrl+drag): any number, in no order.
    pub selected: BTreeSet<usize>,
    /// An atom that must not move: edits move whichever side does not hold it.
    pub anchor: Option<usize>,
    /// Restraints for the next optimisation.
    pub constraints: Vec<Constraint>,
    /// Whether optimisation applies [`Edit3d::constraints`].
    pub use_constraints: bool,
    /// Whether setting a value also restrains it, so a following optimisation keeps it.
    pub lock_on_set: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Bumped on every change of geometry, for caches keyed on it (clashes, the viewer).
    revision: u64,
    /// The model's energy is that of an earlier geometry.
    pub energy_stale: bool,
    /// The outcome of the last edit for the panel: the message, and whether it is a problem.
    pub report: Option<(String, bool)>,
}

impl Default for Edit3d {
    fn default() -> Self {
        Self {
            picked: Vec::new(),
            selected: BTreeSet::new(),
            anchor: None,
            constraints: Vec::new(),
            use_constraints: true,
            lock_on_set: true,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            energy_stale: false,
            report: None,
        }
    }
}

impl Edit3d {
    /// Forget everything tied to the old model (indices would point into a different
    /// structure), keeping the user's preferences.
    pub fn reset(&mut self) {
        let (use_constraints, lock_on_set) = (self.use_constraints, self.lock_on_set);
        *self = Self {
            use_constraints,
            lock_on_set,
            revision: self.revision + 1,
            ..Self::default()
        };
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Toggle `atom` in the picked list; the oldest drops out past [`MAX_PICKED`].
    pub fn toggle_pick(&mut self, atom: usize) {
        if let Some(pos) = self.picked.iter().position(|&a| a == atom) {
            self.picked.remove(pos);
        } else {
            self.picked.push(atom);
            if self.picked.len() > MAX_PICKED {
                self.picked.remove(0);
            }
        }
    }

    /// Pick exactly the two ends of a bond.
    pub fn pick_bond(&mut self, a: usize, b: usize) {
        self.picked = vec![a, b];
    }

    pub fn clear_picks(&mut self) {
        self.picked.clear();
    }

    /// Add `atom` to the atoms selected for deletion, or take it out.
    pub fn toggle_selected(&mut self, atom: usize) {
        if !self.selected.remove(&atom) {
            self.selected.insert(atom);
        }
    }

    /// Select both ends of a bond, or deselect them when both already are.
    pub fn toggle_selected_bond(&mut self, a: usize, b: usize) {
        if self.selected.contains(&a) && self.selected.contains(&b) {
            self.selected.remove(&a);
            self.selected.remove(&b);
        } else {
            self.selected.extend([a, b]);
        }
    }

    /// What deleting acts on: the atoms selected for it, or without any the picked ones.
    pub fn delete_targets(&self) -> Vec<usize> {
        if self.selected.is_empty() {
            let mut picked = self.picked.clone();
            picked.sort_unstable();
            picked
        } else {
            self.selected.iter().copied().collect()
        }
    }

    /// Make `atom` the anchor, or clear it when it already is.
    pub fn toggle_anchor(&mut self, atom: usize) {
        self.anchor = if self.anchor == Some(atom) {
            None
        } else {
            Some(atom)
        };
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// What undo would take back, for the button's tooltip.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    /// Move `model`'s atoms to `coords` as one undoable step called `label`.
    pub fn apply(&mut self, model: &mut Model3d, coords: Vec<Vec3>, label: &str) {
        if coords.len() != model.atoms.len() {
            return;
        }
        self.push_undo(snapshot(model, label, self.energy_stale));
        self.redo.clear();
        set_coords(model, &coords);
        self.energy_stale = true;
        self.revision += 1;
    }

    /// Move `model`'s atoms again within the step [`Edit3d::apply`] last began, without another
    /// history entry: a drag in the viewer is many small moves and one step.
    fn adjust(&mut self, model: &mut Model3d, coords: &[Vec3]) {
        if coords.len() != model.atoms.len() {
            return;
        }
        set_coords(model, coords);
        self.energy_stale = true;
        self.revision += 1;
    }

    /// Replace `model` by `next` — the same structure with atoms taken out or added — as one
    /// undoable step called `label`. `map` gives each old atom's index in `next` (`None`: it
    /// is gone); the picks, the fixed atom and the restraints follow it, and those naming an
    /// atom that is gone are dropped.
    pub fn restructure(
        &mut self,
        model: &mut Model3d,
        next: Model3d,
        map: &[Option<usize>],
        label: &str,
    ) {
        let step = self.structural_snapshot(model, label);
        self.push_undo(step);
        self.redo.clear();
        *model = next;
        let at = |i: usize| map.get(i).copied().flatten();
        self.picked = self.picked.iter().filter_map(|&i| at(i)).collect();
        self.selected = self.selected.iter().filter_map(|&i| at(i)).collect();
        self.anchor = self.anchor.and_then(at);
        self.constraints = self
            .constraints
            .iter()
            .filter_map(|c| c.remapped(map))
            .collect();
        self.energy_stale = true;
        self.revision += 1;
    }

    /// The state now, as the other side of `step` in the history: with the whole structure
    /// when `step` replaces it.
    fn counterpart(&self, model: &Model3d, step: &Snapshot) -> Snapshot {
        if step.structure.is_some() {
            self.structural_snapshot(model, &step.label)
        } else {
            snapshot(model, &step.label, self.energy_stale)
        }
    }

    fn structural_snapshot(&self, model: &Model3d, label: &str) -> Snapshot {
        Snapshot {
            structure: Some(Box::new(Structure {
                model: model.clone(),
                picked: self.picked.clone(),
                selected: self.selected.clone(),
                anchor: self.anchor,
                constraints: self.constraints.clone(),
            })),
            ..snapshot(model, label, self.energy_stale)
        }
    }

    /// `model` was replaced by `next` — an optimisation of the same atoms. Record it as an
    /// undoable step (the reference app dropped the history here instead), or start over when
    /// the atoms differ.
    pub fn record_replacement(&mut self, before: &Model3d, next: &Model3d, label: &str) {
        let same_atoms = before.atoms.len() == next.atoms.len()
            && before
                .atoms
                .iter()
                .zip(&next.atoms)
                .all(|(x, y)| x.element == y.element);
        if same_atoms {
            self.push_undo(snapshot(before, label, self.energy_stale));
            self.redo.clear();
            self.energy_stale = false;
            self.revision += 1;
        } else {
            self.reset();
        }
    }

    /// Take back the last step, returning its label.
    pub fn undo(&mut self, model: &mut Model3d) -> Option<String> {
        let step = self.undo.pop()?;
        let label = step.label.clone();
        let back = self.counterpart(model, &step);
        self.redo.push(back);
        self.restore(model, step);
        Some(label)
    }

    pub fn redo(&mut self, model: &mut Model3d) -> Option<String> {
        let step = self.redo.pop()?;
        let label = step.label.clone();
        let back = self.counterpart(model, &step);
        self.push_undo(back);
        self.restore(model, step);
        Some(label)
    }

    fn restore(&mut self, model: &mut Model3d, step: Snapshot) {
        match step.structure {
            Some(structure) => {
                let structure = *structure;
                *model = structure.model;
                self.picked = structure.picked;
                self.selected = structure.selected;
                self.anchor = structure.anchor;
                self.constraints = structure.constraints;
            }
            None => set_coords(model, &step.coords),
        }
        model.energy = step.energy;
        self.energy_stale = step.energy_stale;
        self.revision += 1;
    }

    fn push_undo(&mut self, step: Snapshot) {
        if self.undo.len() == HISTORY {
            self.undo.remove(0);
        }
        self.undo.push(step);
    }

    /// The energy computed for revision `revision` arrived.
    pub fn energy_updated(&mut self, model: &mut Model3d, revision: u64, energy: f64) {
        if revision == self.revision && energy.is_finite() {
            model.energy = energy;
            self.energy_stale = false;
        }
    }

    /// Restrain `atoms` at `value`, replacing a restraint over the same atoms.
    pub fn constrain(&mut self, atoms: &[usize], value: f64) {
        let Some(new) = Constraint::over(atoms, value) else {
            return;
        };
        self.constraints.retain(|c| !c.same_atoms(&new));
        self.constraints.push(new);
    }

    /// The restraints to optimise under: none when switched off, and only those that still
    /// name real atoms.
    pub fn active_constraints(&self, atoms: usize) -> Vec<Constraint> {
        if !self.use_constraints {
            return Vec::new();
        }
        self.constraints
            .iter()
            .copied()
            .filter(|c| c.is_valid_for(atoms))
            .collect()
    }
}

/// What the picked atoms allow: which value can be typed in, and whether a bond can be turned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Nothing to set (no atoms, or one).
    None,
    /// Two bonded atoms: the bond length (the smaller side slides along the bond).
    BondLength([usize; 2]),
    /// Two atoms not bonded to each other: their distance, by turning the bonds between.
    Distance([usize; 2]),
    /// Three atoms in a chain: the angle at the middle one.
    Angle([usize; 3]),
    /// Four atoms in a chain: the dihedral about the middle bond.
    Dihedral([usize; 4]),
    /// Three or four atoms that are not a chain: measured, not settable.
    Unbonded,
}

impl Target {
    pub fn of(picked: &[usize], graph: &Graph) -> Target {
        match *picked {
            [a, b] if graph.bonded(a, b) => Target::BondLength([a, b]),
            [a, b] => Target::Distance([a, b]),
            [a, b, c] if graph.is_chain(&[a, b, c]) => Target::Angle([a, b, c]),
            [a, b, c, d] if graph.is_chain(&[a, b, c, d]) => Target::Dihedral([a, b, c, d]),
            [_, _, _] | [_, _, _, _] => Target::Unbonded,
            _ => Target::None,
        }
    }

    /// The step's name, for the history.
    pub fn label(self) -> &'static str {
        match self {
            Target::BondLength(_) => t!("three_d.edit.undo_bond_length"),
            Target::Distance(_) => t!("three_d.edit.undo_distance"),
            Target::Angle(_) => t!("three_d.edit.undo_bond_angle"),
            Target::Dihedral(_) => t!("three_d.edit.undo_dihedral"),
            Target::None | Target::Unbonded => "",
        }
    }
}

/// The bond the picked atoms name, when it can be turned: `(b, c)` in picking order.
pub fn picked_rotor(model: &Model3d, picked: &[usize]) -> Option<(usize, usize)> {
    let [b, c] = *picked else {
        return None;
    };
    geom::is_rotatable(&graph(model), b, c, bond_order(model, b, c)).ok()?;
    Some((b, c))
}

/// Set the value the picked atoms measure to `value` (Å or degrees), as one undoable step.
/// Reports how it went in [`Edit3d::report`] and returns whether the geometry changed.
pub fn set_value(model: &mut Model3d, edit: &mut Edit3d, value: f64) -> bool {
    let graph = graph(model);
    let coords = coords(model);
    let anchor = edit.anchor;
    let target = Target::of(&edit.picked, &graph);
    let result = match target {
        Target::BondLength([a, b]) => geom::set_bond_length(&coords, &graph, a, b, value, anchor),
        Target::Angle(atoms) => geom::set_angle(&coords, &graph, atoms, value, anchor),
        Target::Dihedral(atoms) => geom::set_dihedral(&coords, &graph, atoms, value, anchor),
        Target::Distance([a, b]) => {
            return solve(model, edit, a, b, geom::Goal::Distance(value));
        }
        Target::None | Target::Unbonded => return false,
    };
    match result {
        Ok(next) => {
            let picked = edit.picked.clone();
            edit.apply(model, next, target.label());
            if edit.lock_on_set {
                edit.constrain(&picked, value);
            }
            edit.report = Some((clash_note(model, t!("three_d.edit.value_set")), false));
            true
        }
        Err(e) => {
            edit.report = Some((e.to_string(), true));
            false
        }
    }
}

/// Turn the picked bond by `degrees`.
pub fn rotate_picked(model: &mut Model3d, edit: &mut Edit3d, degrees: f64) -> bool {
    let Some((b, c)) = picked_rotor(model, &edit.picked) else {
        return false;
    };
    match geom::rotate_bond(&coords(model), &graph(model), b, c, degrees, edit.anchor) {
        Ok(next) => {
            edit.apply(model, next, t!("three_d.edit.undo_rotate"));
            edit.report = Some((
                clash_note(
                    model,
                    &t!("three_d.edit.rotated", degrees = format!("{degrees:+.0}")),
                ),
                false,
            ));
            true
        }
        Err(e) => {
            edit.report = Some((e.to_string(), true));
            false
        }
    }
}

/// Bring atoms `a` and `b` to `goal` by turning the single bonds between them.
pub fn solve(model: &mut Model3d, edit: &mut Edit3d, a: usize, b: usize, goal: geom::Goal) -> bool {
    let graph = graph(model);
    let orders = |x: usize, y: usize| bond_order(model, x, y);
    let solution = geom::solve_distance(&coords(model), &graph, orders, a, b, goal);
    match solution {
        Ok(s) if (s.achieved - s.start).abs() < 1e-6 => {
            // Nothing moved: not worth a history step.
            let text = if s.reach.1 - s.reach.0 < 1e-3 {
                t!("three_d.edit.distance_fixed").to_owned()
            } else {
                t!(
                    "three_d.edit.distance_already",
                    distance = format!("{:.2}", s.achieved)
                )
            };
            edit.report = Some((text, s.unreachable));
            false
        }
        Ok(s) => {
            let achieved = s.achieved;
            let (near, far) = s.reach;
            let unreachable = s.unreachable;
            edit.apply(model, s.coords, t!("three_d.edit.undo_distance"));
            if let (geom::Goal::Distance(value), false) = (goal, unreachable)
                && edit.lock_on_set
            {
                edit.constrain(&[a, b], value);
            }
            let text = if unreachable {
                t!(
                    "three_d.edit.distance_unreachable",
                    achieved = format!("{achieved:.2}"),
                    near = format!("{near:.2}"),
                    far = format!("{far:.2}")
                )
            } else {
                t!(
                    "three_d.edit.distance_set",
                    distance = format!("{achieved:.2}")
                )
            };
            edit.report = Some((clash_note(model, &text), unreachable));
            true
        }
        Err(e) => {
            edit.report = Some((e.to_string(), true));
            false
        }
    }
}

/// What deleting some atoms takes out of a model, and the hydrogens it puts in.
///
/// As deleting an atom of the drawing leaves its neighbours' hydrogens filled in, the 3D model
/// stays a whole molecule: the hydrogens on a deleted atom go with it, and each bond cut
/// between a deleted atom and one that stays is closed by a hydrogen on the one that stays,
/// along the old bond. A hydrogen deleted by itself is not put back.
#[derive(Debug, Clone, PartialEq)]
pub struct Removal {
    /// Per atom of the model: whether it goes.
    pub remove: Vec<bool>,
    /// The new hydrogens: the atom (one that stays) each is bonded to, and its position.
    pub caps: Vec<(usize, Vec3)>,
}

impl Removal {
    /// What deleting `atoms` of `model` comes to. Indices outside the model are ignored.
    pub fn of(model: &Model3d, atoms: &[usize]) -> Removal {
        let n = model.atoms.len();
        let graph = graph(model);
        let hydrogen = |i: usize| model.atoms[i].element == "H";
        let mut chosen = vec![false; n];
        for &atom in atoms {
            if atom < n {
                chosen[atom] = true;
            }
        }
        let mut remove = chosen.clone();
        for h in (0..n).filter(|&i| hydrogen(i)) {
            let near = graph.neighbors(h);
            if !near.is_empty() && near.iter().all(|&x| chosen[x] && !hydrogen(x)) {
                remove[h] = true;
            }
        }
        let mut caps = Vec::new();
        for bond in &model.bonds {
            if bond.begin >= n || bond.end >= n {
                continue;
            }
            let (kept, gone) = match (remove[bond.begin], remove[bond.end]) {
                (false, true) => (bond.begin, bond.end),
                (true, false) => (bond.end, bond.begin),
                _ => continue,
            };
            if hydrogen(kept) || hydrogen(gone) {
                continue;
            }
            let from = model.atoms[kept].pos;
            if let Some(along) = geom::unit(geom::sub(model.atoms[gone].pos, from)) {
                let length = hydrogen_bond_length(&model.atoms[kept].element);
                caps.push((kept, geom::add(from, geom::scale(along, length))));
            }
        }
        Removal { remove, caps }
    }

    /// How many atoms go.
    pub fn removed(&self) -> usize {
        self.remove.iter().filter(|&&gone| gone).count()
    }

    /// Each atom's index among those left (`None` for one that goes).
    pub fn map(&self) -> Vec<Option<usize>> {
        let mut next = 0;
        self.remove
            .iter()
            .map(|&gone| {
                (!gone).then(|| {
                    next += 1;
                    next - 1
                })
            })
            .collect()
    }
}

/// A typical X–H bond length in Å, for a hydrogen put on an atom of `element`.
fn hydrogen_bond_length(element: &str) -> f64 {
    match element {
        "C" => 1.09,
        "N" => 1.01,
        "O" => 0.96,
        "B" => 1.19,
        "Si" => 1.48,
        "P" => 1.42,
        "S" => 1.34,
        _ => 1.10,
    }
}

/// Every atom of the molecules (connected through bonds) holding one of `atoms`.
pub fn molecules_of(model: &Model3d, atoms: impl IntoIterator<Item = usize>) -> BTreeSet<usize> {
    let graph = graph(model);
    let mut found = BTreeSet::new();
    let mut stack: Vec<usize> = atoms
        .into_iter()
        .filter(|&a| a < model.atoms.len())
        .collect();
    while let Some(atom) = stack.pop() {
        if found.insert(atom) {
            stack.extend(
                graph
                    .neighbors(atom)
                    .iter()
                    .copied()
                    .filter(|n| !found.contains(n)),
            );
        }
    }
    found
}

/// The molecule holding `atom`, when the model has another one to move it against: `None` for
/// a model that is one molecule (and for an atom it does not have).
pub fn separate_molecule(model: &Model3d, atom: usize) -> Option<Vec<usize>> {
    let molecule = molecules_of(model, [atom]);
    (!molecule.is_empty() && molecule.len() < model.atoms.len())
        .then(|| molecule.into_iter().collect())
}

/// What a drag in the viewer does to the molecule it holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Motion {
    /// Shift it by this much (Å).
    Move(Vec3),
    /// Turn it about its centroid by this rotation matrix (rows).
    Turn([Vec3; 3]),
}

/// One drag of a molecule in the viewer: the atoms it moves, and its place in the history.
#[derive(Debug, Clone, PartialEq)]
pub struct MoleculeDrag {
    atoms: Vec<usize>,
    /// The edit revision this drag's last move left. While it is still the current one the
    /// next move joins the same history step; after anything else (an undo, say) it begins
    /// a new one.
    step: Option<u64>,
}

impl MoleculeDrag {
    /// A drag of `atoms` (a molecule of the model, see [`separate_molecule`]).
    pub fn new(atoms: Vec<usize>) -> Self {
        Self { atoms, step: None }
    }

    pub fn atoms(&self) -> &[usize] {
        &self.atoms
    }
}

/// Move the molecule `drag` holds by `motion`, leaving every other atom where it is. All the
/// moves of one drag make one undoable step. Returns whether the geometry changed.
pub fn drag_molecule(
    model: &mut Model3d,
    edit: &mut Edit3d,
    drag: &mut MoleculeDrag,
    motion: Motion,
) -> bool {
    let mut next = coords(model);
    if drag.atoms.is_empty() || drag.atoms.iter().any(|&a| a >= next.len()) {
        return false;
    }
    match motion {
        Motion::Move(by) => {
            for &a in &drag.atoms {
                next[a] = geom::add(next[a], by);
            }
        }
        Motion::Turn(rotation) => {
            let sum = drag
                .atoms
                .iter()
                .fold([0.0; 3], |sum, &a| geom::add(sum, next[a]));
            let centre = geom::scale(sum, 1.0 / drag.atoms.len() as f64);
            for &a in &drag.atoms {
                let v = geom::sub(next[a], centre);
                next[a] = geom::add(centre, rotation.map(|row| geom::dot(row, v)));
            }
        }
    }
    if drag.step == Some(edit.revision) {
        edit.adjust(model, &next);
    } else {
        edit.apply(model, next, t!("three_d.edit.undo_move_molecule"));
    }
    drag.step = Some(edit.revision);
    edit.report = None;
    true
}

/// The drag `drag` ended: report it, with the clashes the molecule's new place makes.
pub fn finish_molecule_drag(model: &Model3d, edit: &mut Edit3d, drag: &MoleculeDrag) {
    if drag.step.is_some() {
        let text = clash_note(model, t!("three_d.edit.molecule_moved"));
        edit.report = Some((text, false));
    }
}

/// Delete `atoms` from the model as one undoable step (see [`Removal`] for the hydrogens that
/// go and come with them). Reports how it went in [`Edit3d::report`] and returns whether the
/// model changed; a model is never emptied.
pub fn delete_atoms(model: &mut Model3d, edit: &mut Edit3d, atoms: &[usize]) -> bool {
    let removal = Removal::of(model, atoms);
    let removed = removal.removed();
    if removed == 0 {
        return false;
    }
    if removed == model.atoms.len() {
        edit.report = Some((t!("three_d.edit.delete_all").to_owned(), true));
        return false;
    }
    match super::engine::without_atoms(model, &removal.remove, &removal.caps) {
        Ok(next) => {
            let label = t!("three_d.edit.undo_delete");
            edit.restructure(model, next, &removal.map(), label);
            let mut text = tn!("three_d.edit.deleted", removed);
            if !removal.caps.is_empty() {
                text = tn!("three_d.edit.capped", removal.caps.len(), text = text);
            }
            edit.report = Some((text, false));
            true
        }
        Err(e) => {
            edit.report = Some((e.to_string(), true));
            false
        }
    }
}

/// `text`, with a warning appended when the geometry now has clashes.
fn clash_note(model: &Model3d, text: &str) -> String {
    let elements: Vec<&str> = model.atoms.iter().map(|a| a.element.as_str()).collect();
    let found = geom::clashes(&coords(model), &elements, &graph(model));
    match found.first() {
        None => text.to_owned(),
        Some(worst) => tn!(
            "three_d.edit.clash_note",
            found.len(),
            text = text,
            a = atom_label(model, worst.a),
            b = atom_label(model, worst.b),
            distance = format!("{:.2}", worst.distance)
        ),
    }
}

/// An atom's name for the user: element and 1-based number, as in a molfile ("C3").
pub fn atom_label(model: &Model3d, atom: usize) -> String {
    match model.atoms.get(atom) {
        Some(a) => format!("{}{}", a.element, atom + 1),
        None => format!("#{}", atom + 1),
    }
}

fn snapshot(model: &Model3d, label: &str, energy_stale: bool) -> Snapshot {
    Snapshot {
        label: label.to_owned(),
        coords: coords(model),
        energy: model.energy,
        energy_stale,
        structure: None,
    }
}

/// The model's atom positions.
pub fn coords(model: &Model3d) -> Vec<Vec3> {
    model.atoms.iter().map(|a| a.pos).collect()
}

/// The model's bond graph.
pub fn graph(model: &Model3d) -> Graph {
    Graph::new(
        model.atoms.len(),
        model.bonds.iter().map(|b| (b.begin, b.end)),
    )
}

/// The order of the bond between `a` and `b` (0 when there is none).
pub fn bond_order(model: &Model3d, a: usize, b: usize) -> u8 {
    model
        .bonds
        .iter()
        .find(|x| (x.begin, x.end) == (a, b) || (x.begin, x.end) == (b, a))
        .map_or(0, |x| x.order)
}

/// Move `model`'s atoms to `coords`, in [`Model3d::atoms`] and in [`Model3d::molfile`] alike.
pub fn set_coords(model: &mut Model3d, coords: &[Vec3]) {
    for (atom, &pos) in model.atoms.iter_mut().zip(coords) {
        atom.pos = pos;
    }
    if let Some(molfile) = molfile_with_coords(&model.molfile, coords) {
        model.molfile = molfile;
    }
}

/// `molfile` (V2000 or V3000) with its atom coordinates replaced by `coords`, or `None` when
/// its atom block does not have exactly that many atoms.
pub fn molfile_with_coords(molfile: &str, coords: &[Vec3]) -> Option<String> {
    let lines: Vec<&str> = molfile.lines().collect();
    let counts = lines.get(3)?;
    let mut out: Vec<String> = lines.iter().map(|l| (*l).to_owned()).collect();
    if counts.contains("V3000") {
        let begin = lines
            .iter()
            .position(|l| l.trim_end() == "M  V30 BEGIN ATOM")?;
        let mut n = 0;
        for line in out.iter_mut().skip(begin + 1) {
            if line.trim_end() == "M  V30 END ATOM" {
                break;
            }
            // "M  V30 index element x y z rest…"
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 7 || fields[0] != "M" || fields[1] != "V30" {
                return None;
            }
            let p = coords.get(n)?;
            let mut rebuilt = format!(
                "M  V30 {} {} {:.4} {:.4} {:.4}",
                fields[2], fields[3], p[0], p[1], p[2]
            );
            for rest in &fields[7..] {
                rebuilt.push(' ');
                rebuilt.push_str(rest);
            }
            *line = rebuilt;
            n += 1;
        }
        return (n == coords.len()).then(|| join(&out, molfile));
    }
    let atoms: usize = counts.get(0..3)?.trim().parse().ok()?;
    if atoms != coords.len() || lines.len() < 4 + atoms {
        return None;
    }
    for (line, p) in out[4..4 + atoms].iter_mut().zip(coords) {
        let rest = line.get(30..)?.to_owned();
        *line = format!("{:10.4}{:10.4}{:10.4}{rest}", p[0], p[1], p[2]);
    }
    Some(join(&out, molfile))
}

/// Lines joined back with the original's line ending and trailing newline.
fn join(lines: &[String], original: &str) -> String {
    let newline = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut text = lines.join(newline);
    if original.ends_with('\n') {
        text.push_str(newline);
    }
    text
}

/// A measurement over the picked atoms, for the panel and the on-screen labels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Measure {
    Distance(f64),
    Angle(f64),
    Dihedral(f64),
}

impl Measure {
    /// The measurement over `atoms` (two to four) in `coords`.
    pub fn of(atoms: &[usize], coords: &[Vec3]) -> Option<Measure> {
        let points = atoms
            .iter()
            .map(|&i| coords.get(i).copied())
            .collect::<Option<Vec<_>>>()?;
        Measure::of_points(&points)
    }

    /// The measurement over two to four points, in order.
    pub fn of_points(points: &[Vec3]) -> Option<Measure> {
        Some(match *points {
            [a, b] => Measure::Distance(geom::distance(a, b)),
            [a, b, c] => Measure::Angle(geom::angle(a, b, c)),
            [a, b, c, d] => Measure::Dihedral(geom::dihedral(a, b, c, d)),
            _ => return None,
        })
    }

    pub fn value(self) -> f64 {
        match self {
            Measure::Distance(v) | Measure::Angle(v) | Measure::Dihedral(v) => v,
        }
    }
}

impl fmt::Display for Measure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Measure::Distance(v) => write!(f, "{v:.3} Å"),
            Measure::Angle(v) => write!(f, "{v:.1}°"),
            Measure::Dihedral(v) => write!(f, "{v:.1}°"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::three_d::{Atom3d, Bond3d, ForceField};

    /// Ethane-ish: C0–C1 with an H on each.
    fn model() -> Model3d {
        let atoms = vec![
            ("C", [0.0, 0.0, 0.0]),
            ("C", [1.5, 0.0, 0.0]),
            ("H", [-0.5, 0.9, 0.0]),
            ("H", [2.0, 0.9, 0.3]),
        ];
        let mut molfile = String::from("\n  test\n\n  4  3  0  0  0  0  0  0  0  0999 V2000\n");
        for (element, p) in &atoms {
            molfile.push_str(&format!(
                "{:10.4}{:10.4}{:10.4} {:<3} 0  0  0  0  0  0  0  0  0  0  0  0\n",
                p[0], p[1], p[2], element
            ));
        }
        molfile.push_str(
            "  1  2  1  0  0  0  0\n  1  3  1  0  0  0  0\n  2  4  1  0  0  0  0\nM  END\n",
        );
        Model3d {
            atoms: atoms
                .iter()
                .map(|(e, p)| Atom3d {
                    element: (*e).to_owned(),
                    pos: *p,
                })
                .collect(),
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
                Bond3d {
                    begin: 1,
                    end: 3,
                    order: 1,
                },
            ],
            force_field: ForceField::Uff,
            energy: 1.0,
            energy_unit: "kJ/mol".to_owned(),
            formula: "C2H2".to_owned(),
            molfile,
        }
    }

    #[test]
    fn coordinates_are_rewritten_in_a_v2000_molfile() {
        let m = model();
        let mut coords = coords(&m);
        coords[3] = [12.5, -3.25, 0.125];
        let text = molfile_with_coords(&m.molfile, &coords).unwrap();
        let line = text.lines().nth(7).unwrap();
        assert!(
            line.starts_with("   12.5000   -3.2500    0.1250 H"),
            "{line}"
        );
        // Everything else is untouched.
        assert_eq!(text.lines().count(), m.molfile.lines().count());
        assert_eq!(text.lines().nth(4), m.molfile.lines().nth(4));
        assert!(text.ends_with("M  END\n"));
        // A coordinate count that does not match is refused.
        assert_eq!(molfile_with_coords(&m.molfile, &coords[..3]), None);
    }

    #[test]
    fn coordinates_are_rewritten_in_a_v3000_molfile() {
        let text = "\n  test\n\n  0  0  0     0  0            999 V3000\nM  V30 BEGIN CTAB\n\
                    M  V30 COUNTS 2 1 0 0 0\nM  V30 BEGIN ATOM\nM  V30 1 C 0 0 0 0\n\
                    M  V30 2 O 1.2 0 0 0 CHG=-1\nM  V30 END ATOM\nM  V30 END CTAB\nM  END\n";
        let out = molfile_with_coords(text, &[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]).unwrap();
        assert!(out.contains("M  V30 1 C 1.0000 2.0000 3.0000 0\n"), "{out}");
        assert!(
            out.contains("M  V30 2 O 4.0000 5.0000 6.0000 0 CHG=-1\n"),
            "{out}"
        );
    }

    #[test]
    fn edits_undo_and_redo_with_the_energy() {
        let mut m = model();
        let mut edit = Edit3d::default();
        let original = coords(&m);
        let g = graph(&m);
        let stretched = geom::set_bond_length(&original, &g, 0, 1, 2.0, None).unwrap();
        edit.apply(&mut m, stretched.clone(), "結合長");
        assert!(edit.energy_stale);
        assert!((m.distance(0, 1) - 2.0).abs() < 1e-9);
        assert!(m.molfile.contains("    2.0000"), "the molfile follows");

        edit.energy_updated(&mut m, edit.revision(), 5.0);
        assert_eq!(m.energy, 5.0);
        assert!(!edit.energy_stale);
        // A result for an older geometry is ignored.
        edit.energy_updated(&mut m, edit.revision() - 1, 9.0);
        assert_eq!(m.energy, 5.0);

        assert_eq!(edit.undo(&mut m).as_deref(), Some("結合長"));
        assert_eq!(coords(&m), original);
        assert_eq!(m.energy, 1.0);
        assert!(!edit.energy_stale);
        assert!(!edit.can_undo() && edit.can_redo());

        assert_eq!(edit.redo(&mut m).as_deref(), Some("結合長"));
        assert_eq!(coords(&m), stretched);
        assert_eq!(m.energy, 5.0);

        // A new edit drops the redo branch.
        edit.undo(&mut m);
        edit.apply(&mut m, original.clone(), "別の編集");
        assert!(!edit.can_redo());
    }

    #[test]
    fn an_optimisation_of_the_same_atoms_is_undoable() {
        let before = model();
        let mut after = model();
        after.atoms[0].pos = [0.1, 0.0, 0.0];
        after.energy = -3.0;
        let mut edit = Edit3d {
            picked: vec![0, 1],
            ..Default::default()
        };
        edit.record_replacement(&before, &after, "構造最適化");
        assert!(edit.can_undo());
        assert_eq!(edit.picked, vec![0, 1], "the selection survives");

        let mut current = after.clone();
        edit.undo(&mut current);
        assert_eq!(current.atoms[0].pos, [0.0, 0.0, 0.0]);
        assert_eq!(current.energy, 1.0);

        // A different structure starts the history over.
        let mut other = model();
        other.atoms.pop();
        edit.record_replacement(&before, &other, "生成");
        assert!(!edit.can_undo() && edit.picked.is_empty());
    }

    #[test]
    fn picking_toggles_and_keeps_the_newest_four() {
        let mut edit = Edit3d::default();
        for a in [1, 2, 3, 4, 5] {
            edit.toggle_pick(a);
        }
        assert_eq!(edit.picked, vec![2, 3, 4, 5]);
        edit.toggle_pick(3);
        assert_eq!(edit.picked, vec![2, 4, 5]);
        edit.toggle_anchor(7);
        assert_eq!(edit.anchor, Some(7));
        edit.toggle_anchor(7);
        assert_eq!(edit.anchor, None);
    }

    #[test]
    fn a_new_value_replaces_the_restraint_over_the_same_atoms() {
        let mut edit = Edit3d::default();
        edit.constrain(&[0, 1, 2, 3], 60.0);
        edit.constrain(&[3, 2, 1, 0], 90.0);
        edit.constrain(&[0, 1], 1.5);
        assert_eq!(edit.constraints.len(), 2);
        assert_eq!(edit.constraints[0].target(), Some(90.0));
        assert_eq!(edit.active_constraints(4).len(), 2);
        assert_eq!(edit.active_constraints(3).len(), 1, "atom 3 does not exist");
        edit.use_constraints = false;
        assert!(edit.active_constraints(4).is_empty());
    }

    #[test]
    fn the_picked_atoms_decide_what_can_be_set() {
        let mut m = model();
        let mut edit = Edit3d::default();
        let g = graph(&m);
        assert_eq!(Target::of(&[0, 1], &g), Target::BondLength([0, 1]));
        assert_eq!(Target::of(&[2, 3], &g), Target::Distance([2, 3]));
        assert_eq!(Target::of(&[2, 0, 1], &g), Target::Angle([2, 0, 1]));
        assert_eq!(
            Target::of(&[2, 0, 1, 3], &g),
            Target::Dihedral([2, 0, 1, 3])
        );
        assert_eq!(Target::of(&[2, 1, 0, 3], &g), Target::Unbonded);
        assert_eq!(Target::of(&[2], &g), Target::None);

        edit.picked = vec![2, 0, 1, 3];
        assert!(set_value(&mut m, &mut edit, 75.0));
        let c = coords(&m);
        assert!((geom::dihedral(c[2], c[0], c[1], c[3]) - 75.0).abs() < 1e-6);
        assert_eq!(
            edit.constraints,
            vec![Constraint::Torsion([2, 0, 1, 3], 75.0)]
        );
        assert_eq!(edit.undo_label(), Some(t!("three_d.edit.undo_dihedral")));

        // The C–C bond can be turned; a C–H bond is terminal.
        edit.picked = vec![0, 1];
        assert_eq!(picked_rotor(&m, &edit.picked), Some((0, 1)));
        assert!(rotate_picked(&mut m, &mut edit, 15.0));
        let c = coords(&m);
        assert!((geom::dihedral(c[2], c[0], c[1], c[3]) - 90.0).abs() < 1e-6);
        edit.picked = vec![0, 2];
        assert_eq!(picked_rotor(&m, &edit.picked), None);

        // Two unbonded atoms: the distance is solved by turning C0–C1.
        edit.picked = vec![2, 3];
        let before = m.distance(2, 3);
        assert!(set_value(&mut m, &mut edit, before - 0.3));
        assert!(
            (m.distance(2, 3) - (before - 0.3)).abs() < 1e-3,
            "{:?}",
            edit.report
        );

        // A failed edit says why and changes nothing.
        edit.picked = vec![0, 1];
        let undo_depth = edit.undo.len();
        assert!(!set_value(&mut m, &mut edit, -2.0));
        assert!(edit.report.as_ref().is_some_and(|(_, problem)| *problem));
        assert_eq!(edit.undo.len(), undo_depth);
    }

    #[test]
    fn a_double_bond_is_turned_like_a_single_one() {
        // H2–C0=C1–H3.
        let mut m = model();
        m.bonds[0].order = 2;
        let mut edit = Edit3d {
            picked: vec![0, 1],
            ..Edit3d::default()
        };
        assert_eq!(picked_rotor(&m, &edit.picked), Some((0, 1)));
        let c = coords(&m);
        let before = geom::dihedral(c[2], c[0], c[1], c[3]);
        assert!(rotate_picked(&mut m, &mut edit, 180.0));
        let c = coords(&m);
        let after = geom::dihedral(c[2], c[0], c[1], c[3]);
        assert!((geom::wrap_degrees(after - before).abs() - 180.0).abs() < 1e-6);
        assert_eq!(edit.undo_label(), Some(t!("three_d.edit.undo_rotate")));
        assert!(edit.undo(&mut m).is_some());
        let c = coords(&m);
        assert!((geom::dihedral(c[2], c[0], c[1], c[3]) - before).abs() < 1e-6);

        // The distance between the hydrogens is not solved by turning it.
        edit.picked = vec![2, 3];
        let start = m.distance(2, 3);
        assert!(!set_value(&mut m, &mut edit, start + 0.5));
        assert!(edit.report.as_ref().is_some_and(|(_, problem)| *problem));
        assert!((m.distance(2, 3) - start).abs() < 1e-9);

        // A triple bond does not turn.
        m.bonds[0].order = 3;
        edit.picked = vec![0, 1];
        assert_eq!(picked_rotor(&m, &edit.picked), None);
    }

    /// Ethanol with its hydrogens and, apart from it, a water molecule:
    /// C0 C1 O2, H3–H5 on C0, H6 H7 on C1, H8 on O2; O9 with H10 H11.
    fn ethanol_and_water() -> Model3d {
        let atoms: Vec<Atom3d> = [
            ("C", [0.0, 0.0, 0.0]),
            ("C", [1.52, 0.0, 0.0]),
            ("O", [2.0, 1.35, 0.0]),
            ("H", [-0.4, 1.0, 0.0]),
            ("H", [-0.4, -0.5, 0.9]),
            ("H", [-0.4, -0.5, -0.9]),
            ("H", [1.9, -0.5, 0.9]),
            ("H", [1.9, -0.5, -0.9]),
            ("H", [2.95, 1.3, 0.0]),
            ("O", [6.0, 0.0, 0.0]),
            ("H", [6.96, 0.0, 0.0]),
            ("H", [5.76, 0.93, 0.0]),
        ]
        .into_iter()
        .map(|(element, pos)| Atom3d {
            element: element.to_owned(),
            pos,
        })
        .collect();
        let bonds: Vec<Bond3d> = [
            (0, 1),
            (1, 2),
            (0, 3),
            (0, 4),
            (0, 5),
            (1, 6),
            (1, 7),
            (2, 8),
            (9, 10),
            (9, 11),
        ]
        .into_iter()
        .map(|(begin, end)| Bond3d {
            begin,
            end,
            order: 1,
        })
        .collect();
        let mol = crate::three_d::read_molfile(&crate::scan::energy::molfile(&atoms, &bonds))
            .expect("molfile");
        crate::three_d::assemble(&mol, &mol.coords, ForceField::Uff, 1.0)
    }

    /// The model is whole: every bond joins two of its atoms, and its molfile, formula and
    /// atom list say the same thing.
    fn assert_consistent(m: &Model3d) {
        let n = m.atoms.len();
        assert!(
            m.bonds
                .iter()
                .all(|b| b.begin < n && b.end < n && b.begin != b.end)
        );
        let read = crate::three_d::read_molfile(&m.molfile).expect("the molfile reads");
        assert_eq!(read.num_atoms(), n);
        assert_eq!(read.num_bonds(), m.bonds.len());
        for (atom, (read, pos)) in m.atoms.iter().zip(read.atoms().iter().zip(&read.coords)) {
            assert_eq!(atom.element, read.symbol());
            assert!(geom::distance(atom.pos, *pos) < 1e-3);
        }
        assert_eq!(m.formula, crate::three_d::engine::formula(&m.atoms));
    }

    #[test]
    fn deleting_an_atom_takes_its_hydrogens_and_caps_the_cut_bond() {
        let mut m = ethanol_and_water();
        assert_eq!(m.formula, "C2H8O2");
        let before = m.clone();
        let mut edit = Edit3d {
            picked: vec![8, 2, 1, 0],
            anchor: Some(9),
            constraints: vec![
                Constraint::Torsion([8, 2, 1, 0], 60.0),
                Constraint::Distance([0, 9], 6.0),
                Constraint::Fix(10),
                Constraint::Fix(2),
            ],
            ..Edit3d::default()
        };
        edit.selected.insert(2);
        let state = edit.clone();

        // The hydroxyl goes: O2 and, with it, H8. C1 gets a hydrogen where the oxygen was.
        assert!(delete_atoms(&mut m, &mut edit, &[2]));
        assert_consistent(&m);
        assert_eq!(m.atoms.len(), 11);
        assert_eq!(m.formula, "C2H8O", "ethane and water");
        let cap = m.atoms.len() - 1;
        assert_eq!(m.atoms[cap].element, "H");
        assert_eq!(bond_order(&m, 1, cap), 1);
        assert!((m.distance(1, cap) - 1.09).abs() < 1e-3);
        let towards_oxygen = geom::unit(geom::sub(before.atoms[2].pos, before.atoms[1].pos));
        let towards_cap = geom::unit(geom::sub(m.atoms[cap].pos, m.atoms[1].pos));
        assert!(geom::distance(towards_oxygen.unwrap(), towards_cap.unwrap()) < 1e-3);
        // The atoms left keep their places and their order; those after the gap move down.
        assert_eq!(m.atoms[..2], before.atoms[..2]);
        assert_eq!(m.atoms[2..7], before.atoms[3..8]);
        assert_eq!(m.atoms[7..10], before.atoms[9..12]);
        // What named the deleted atoms is let go, the rest is renumbered.
        assert_eq!(edit.picked, vec![1, 0]);
        assert!(edit.selected.is_empty());
        assert_eq!(edit.anchor, Some(7));
        assert_eq!(
            edit.constraints,
            vec![Constraint::Distance([0, 7], 6.0), Constraint::Fix(8)]
        );
        assert!(
            edit.constraints
                .iter()
                .all(|c| c.is_valid_for(m.atoms.len()))
        );
        assert!(edit.energy_stale);
        assert_eq!(edit.undo_label(), Some(t!("three_d.edit.undo_delete")));
        assert!(edit.report.as_ref().is_some_and(|(_, problem)| !*problem));
        // The force field still takes it.
        assert!(crate::three_d::energy(&m, ForceField::Uff).is_ok());
        assert!(crate::three_d::energy(&m, ForceField::Mmff94).is_ok());

        // One undo puts everything back, one redo deletes again.
        let after = m.clone();
        let after_state = (edit.picked.clone(), edit.anchor, edit.constraints.clone());
        assert!(edit.undo(&mut m).is_some());
        assert_eq!(m, before);
        assert_eq!(edit.picked, state.picked);
        assert_eq!(edit.selected, state.selected);
        assert_eq!(edit.anchor, state.anchor);
        assert_eq!(edit.constraints, state.constraints);
        assert!(!edit.energy_stale && !edit.can_undo());
        assert!(edit.redo(&mut m).is_some());
        assert_eq!(m, after);
        assert_eq!(
            (edit.picked.clone(), edit.anchor, edit.constraints.clone()),
            after_state
        );
        assert!(edit.energy_stale);
    }

    #[test]
    fn a_whole_molecule_is_deleted_without_a_trace() {
        let mut m = ethanol_and_water();
        let mut edit = Edit3d::default();
        // One atom of the water names all of it, and nothing of the ethanol.
        let water = molecules_of(&m, [10]);
        assert_eq!(water, BTreeSet::from([9, 10, 11]));
        assert_eq!(molecules_of(&m, [4, 99]).len(), 9);
        edit.selected = water;
        let targets = edit.delete_targets();
        assert!(delete_atoms(&mut m, &mut edit, &targets));
        assert_consistent(&m);
        assert_eq!(m.formula, "C2H6O");
        assert_eq!(m.atoms.len(), 9, "no hydrogen is added: no bond was cut");
        assert_eq!(m.bonds.len(), 8);
    }

    #[test]
    fn a_hydrogen_deleted_by_itself_is_not_put_back() {
        let mut m = ethanol_and_water();
        let mut edit = Edit3d {
            picked: vec![8, 3],
            ..Edit3d::default()
        };
        // Without a selection, deleting acts on the picked atoms.
        let targets = edit.delete_targets();
        assert_eq!(targets, vec![3, 8]);
        assert!(delete_atoms(&mut m, &mut edit, &targets));
        assert_consistent(&m);
        assert_eq!(m.formula, "C2H6O2");
        assert_eq!(m.atoms.len(), 10);
        assert!(edit.picked.is_empty());
        // The open valences are left as they are: an optimisation keeps the same atoms, so it
        // stays one step of the same history.
        for ff in ForceField::ALL {
            assert!(crate::three_d::energy(&m, ff).is_ok(), "{ff}");
            let relaxed = crate::three_d::optimize(&m, ff).expect("optimises");
            assert_eq!(relaxed.formula, "C2H6O2", "{ff}");
        }
    }

    #[test]
    fn deleting_nothing_or_everything_changes_nothing() {
        let mut m = ethanol_and_water();
        let before = m.clone();
        let mut edit = Edit3d::default();
        assert!(!delete_atoms(&mut m, &mut edit, &[]));
        assert!(!delete_atoms(&mut m, &mut edit, &[99]));
        assert_eq!(edit.report, None);
        let all: Vec<usize> = (0..m.atoms.len()).collect();
        assert!(!delete_atoms(&mut m, &mut edit, &all));
        assert!(edit.report.as_ref().is_some_and(|(_, problem)| *problem));
        // The heavy atoms alone are everything too: their hydrogens go with them.
        assert!(!delete_atoms(&mut m, &mut edit, &[0, 1, 2, 9]));
        assert_eq!(m, before);
        assert!(!edit.can_undo());
    }

    #[test]
    fn a_deletion_sits_in_the_history_between_the_moves() {
        let mut m = ethanol_and_water();
        let original = m.clone();
        let mut edit = Edit3d::default();
        // A move, the deletion of the water, another move.
        let stretched = geom::set_bond_length(&coords(&m), &graph(&m), 0, 1, 1.6, None).unwrap();
        edit.apply(&mut m, stretched, "move");
        let moved = m.clone();
        assert!(delete_atoms(&mut m, &mut edit, &[9]));
        let deleted = m.clone();
        let turned = geom::rotate_bond(&coords(&m), &graph(&m), 0, 1, 30.0, None).unwrap();
        edit.apply(&mut m, turned, "turn");
        let last = m.clone();
        assert_eq!(m.atoms.len(), 9);

        // Back through all three, each step restoring the structure it belongs to.
        assert_eq!(edit.undo(&mut m).as_deref(), Some("turn"));
        assert_eq!(m, deleted);
        assert!(edit.undo(&mut m).is_some());
        assert_eq!(m, moved);
        assert_consistent(&m);
        assert_eq!(edit.undo(&mut m).as_deref(), Some("move"));
        assert_eq!(coords(&m), coords(&original));
        assert_eq!(m.atoms.len(), 12);
        // And forward again.
        for expected in [&moved, &deleted, &last] {
            assert!(edit.redo(&mut m).is_some());
            assert_eq!(coords(&m), coords(expected));
            assert_eq!(m.bonds, expected.bonds);
            assert_consistent(&m);
        }
        assert!(!edit.can_redo());
    }

    #[test]
    fn only_a_model_of_several_molecules_has_one_to_move() {
        // One molecule: nothing to move it against, whichever atom is grabbed.
        let single = model();
        for atom in 0..single.atoms.len() {
            assert_eq!(separate_molecule(&single, atom), None);
        }
        let pair = ethanol_and_water();
        assert_eq!(separate_molecule(&pair, 10), Some(vec![9, 10, 11]));
        assert_eq!(separate_molecule(&pair, 4).map(|m| m.len()), Some(9));
        assert_eq!(separate_molecule(&pair, 99), None);
        // Deleting the water leaves one molecule again.
        let mut rest = pair.clone();
        assert!(delete_atoms(&mut rest, &mut Edit3d::default(), &[9]));
        assert_eq!(rest.atoms.len(), 9);
        assert_eq!(separate_molecule(&rest, 0), None);
    }

    /// Pentane as the 3D view generates it: C0–C4 in a chain, then their hydrogens.
    fn pentane() -> Model3d {
        let drawn = include_str!("../../tests/data/drawn/pentane.mol");
        let m = crate::three_d::generate_from_molfile(drawn, ForceField::Uff).expect("pentane");
        assert_eq!(m.formula, "C5H12");
        assert!(m.atoms[..5].iter().all(|a| a.element == "C"));
        m
    }

    /// The molecules of `m`, each as its sorted atoms.
    fn molecules(m: &Model3d) -> BTreeSet<Vec<usize>> {
        (0..m.atoms.len())
            .map(|a| molecules_of(m, [a]).into_iter().collect())
            .collect()
    }

    #[test]
    fn a_molecule_cut_in_two_by_a_deletion_is_two_molecules() {
        let mut m = pentane();
        let whole = m.clone();
        let mut edit = Edit3d::default();
        assert_eq!(molecules(&m).len(), 1);
        assert_eq!(separate_molecule(&m, 0), None);

        // The middle carbon goes: an ethane on each side of the gap.
        assert!(delete_atoms(&mut m, &mut edit, &[2]));
        assert_consistent(&m);
        assert_eq!(m.formula, "C4H12");
        let parts = molecules(&m);
        assert_eq!(parts.len(), 2, "{parts:?}");
        let left = separate_molecule(&m, 0).expect("the left ethane is a molecule to move");
        let right = separate_molecule(&m, 3).expect("the right one too");
        assert_eq!(left.len(), 8);
        assert_eq!(right.len(), 8);
        assert!(left.contains(&1) && right.contains(&2));
        assert!(left.iter().all(|a| !right.contains(a)));
        // Whichever atom it is taken by, hydrogens and the new ones included.
        for atom in 0..m.atoms.len() {
            let found = separate_molecule(&m, atom).expect("every atom has a molecule");
            assert!(found == left || found == right, "atom {atom}");
        }
        // "Whole molecule" widens a selection to one of them only.
        assert_eq!(molecules_of(&m, [0]).len(), 8);
        // The force fields see two molecules too, and still take the model.
        let read = crate::three_d::read_molfile(&m.molfile).expect("reads");
        assert_eq!(crate::three_d::engine::fragment_count(&read), 2);
        for ff in ForceField::ALL {
            assert!(crate::three_d::energy(&m, ff).is_ok(), "{ff}");
        }

        // One of them is dragged away; the other stays.
        let split = coords(&m);
        let mut drag = MoleculeDrag::new(right.clone());
        let by = Motion::Move([0.0, 0.0, 3.0]);
        assert!(drag_molecule(&mut m, &mut edit, &mut drag, by));
        for (i, (now, was)) in coords(&m).iter().zip(&split).enumerate() {
            let expected = if right.contains(&i) { 3.0 } else { 0.0 };
            assert!(
                geom::distance(geom::sub(*now, *was), [0.0, 0.0, expected]) < 1e-12,
                "atom {i}"
            );
        }
        assert_consistent(&m);
        let dragged = m.clone();

        // Undo the drag, then the deletion: one molecule again, with nothing to take hold of.
        assert!(edit.undo(&mut m).is_some());
        assert_eq!(coords(&m), split);
        assert!(separate_molecule(&m, 0).is_some());
        assert!(edit.undo(&mut m).is_some());
        assert_eq!(m, whole);
        assert_eq!(separate_molecule(&m, 0), None);
        // Redo cuts it again.
        assert!(edit.redo(&mut m).is_some());
        assert_eq!(separate_molecule(&m, 0), Some(left));
        assert_eq!(separate_molecule(&m, 3), Some(right));
        assert!(edit.redo(&mut m).is_some());
        assert_eq!(m, dragged);
    }

    #[test]
    fn a_deletion_that_cuts_nothing_off_leaves_one_molecule() {
        // The end of a chain.
        let mut m = pentane();
        assert!(delete_atoms(&mut m, &mut Edit3d::default(), &[4]));
        assert_eq!(m.formula, "C4H10");
        assert_eq!(molecules(&m).len(), 1);
        assert_eq!(separate_molecule(&m, 0), None);
        // A ring opened: still one chain.
        let drawn = include_str!("../../tests/data/drawn/benzene.mol");
        let mut ring = crate::three_d::generate_from_molfile(drawn, ForceField::Uff).unwrap();
        assert!(delete_atoms(&mut ring, &mut Edit3d::default(), &[0]));
        assert_consistent(&ring);
        assert_eq!(molecules(&ring).len(), 1);
        assert_eq!(separate_molecule(&ring, 0), None);
    }

    #[test]
    fn a_dragged_molecule_moves_alone_and_is_undone_as_one_step() {
        let mut m = ethanol_and_water();
        let original = coords(&m);
        let mut edit = Edit3d::default();
        let mut drag = MoleculeDrag::new(separate_molecule(&m, 10).unwrap());
        assert_eq!(drag.atoms(), [9, 10, 11]);
        // Three frames of one drag.
        for by in [[0.5, 0.0, 0.0], [0.5, 1.0, 0.0], [0.0, 0.0, -2.0]] {
            assert!(drag_molecule(
                &mut m,
                &mut edit,
                &mut drag,
                Motion::Move(by)
            ));
        }
        let moved = coords(&m);
        for (i, (now, was)) in moved.iter().zip(&original).enumerate() {
            let shift = geom::sub(*now, *was);
            let expected = if i >= 9 { [1.0, 1.0, -2.0] } else { [0.0; 3] };
            assert!(
                geom::distance(shift, expected) < 1e-12,
                "atom {i}: {shift:?}"
            );
        }
        assert_consistent(&m);
        assert!(edit.energy_stale);
        finish_molecule_drag(&m, &mut edit, &drag);
        assert!(edit.report.is_some());

        // One step back takes the whole drag back, and one forward redoes it.
        assert_eq!(
            edit.undo(&mut m).as_deref(),
            Some(t!("three_d.edit.undo_move_molecule"))
        );
        assert_eq!(coords(&m), original);
        assert_consistent(&m);
        assert!(!edit.can_undo());
        assert!(edit.redo(&mut m).is_some());
        assert_eq!(coords(&m), moved);

        // A second drag is a second step.
        let mut again = MoleculeDrag::new(separate_molecule(&m, 0).unwrap());
        let up = Motion::Move([0.0, 3.0, 0.0]);
        assert!(drag_molecule(&mut m, &mut edit, &mut again, up));
        assert!(edit.undo(&mut m).is_some());
        assert_eq!(coords(&m), moved);
        assert!(edit.undo(&mut m).is_some());
        assert_eq!(coords(&m), original);
    }

    #[test]
    fn a_drag_interrupted_by_an_undo_begins_a_new_step() {
        let mut m = ethanol_and_water();
        let original = coords(&m);
        let mut edit = Edit3d::default();
        let mut drag = MoleculeDrag::new(vec![9, 10, 11]);
        let by = Motion::Move([1.0, 0.0, 0.0]);
        assert!(drag_molecule(&mut m, &mut edit, &mut drag, by));
        edit.undo(&mut m);
        // The drag goes on: what it moves now can be undone too.
        assert!(drag_molecule(&mut m, &mut edit, &mut drag, by));
        assert!(drag_molecule(&mut m, &mut edit, &mut drag, by));
        assert!((m.atoms[9].pos[0] - original[9][0] - 2.0).abs() < 1e-12);
        assert!(edit.undo(&mut m).is_some());
        assert_eq!(coords(&m), original);
        assert!(!edit.can_undo());

        // Atoms the model does not have are not moved, and nothing is recorded.
        let mut stray = MoleculeDrag::new(vec![11, 12]);
        assert!(!drag_molecule(&mut m, &mut edit, &mut stray, by));
        let mut none = MoleculeDrag::new(Vec::new());
        assert!(!drag_molecule(&mut m, &mut edit, &mut none, by));
        assert_eq!(coords(&m), original);
        assert!(!edit.can_undo());
        edit.report = None;
        finish_molecule_drag(&m, &mut edit, &stray);
        assert_eq!(edit.report, None);
    }

    #[test]
    fn a_turned_molecule_keeps_its_shape_and_its_centroid() {
        let mut m = ethanol_and_water();
        let original = coords(&m);
        let mut edit = Edit3d::default();
        let water = [9, 10, 11];
        let centroid = |c: &[Vec3]| {
            let sum = water.iter().fold([0.0; 3], |s, &a| geom::add(s, c[a]));
            geom::scale(sum, 1.0 / 3.0)
        };
        // A quarter turn about z: x → y.
        let quarter = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let mut drag = MoleculeDrag::new(water.to_vec());
        let turn = Motion::Turn(quarter);
        assert!(drag_molecule(&mut m, &mut edit, &mut drag, turn));
        let turned = coords(&m);
        assert!(geom::distance(centroid(&turned), centroid(&original)) < 1e-12);
        for (a, b) in [(9, 10), (9, 11), (10, 11)] {
            let now = geom::distance(turned[a], turned[b]);
            let was = geom::distance(original[a], original[b]);
            assert!((now - was).abs() < 1e-12);
        }
        let arm = |c: &[Vec3]| geom::sub(c[10], centroid(c));
        let (now, was) = (arm(&turned), arm(&original));
        assert!(geom::distance(now, [-was[1], was[0], was[2]]) < 1e-12);
        assert_eq!(turned[..9], original[..9], "the ethanol stays");
        assert_consistent(&m);
        assert!(edit.undo(&mut m).is_some());
        assert_eq!(coords(&m), original);
    }

    #[test]
    fn selecting_for_deletion_toggles_atoms_and_bonds() {
        let mut edit = Edit3d::default();
        edit.toggle_selected(4);
        edit.toggle_selected(2);
        edit.toggle_selected(4);
        assert_eq!(edit.selected, BTreeSet::from([2]));
        edit.toggle_selected_bond(2, 3);
        assert_eq!(edit.selected, BTreeSet::from([2, 3]));
        edit.toggle_selected_bond(3, 2);
        assert!(edit.selected.is_empty());
        // The selection wins over the picks; a reset forgets it.
        edit.picked = vec![7, 5];
        assert_eq!(edit.delete_targets(), vec![5, 7]);
        edit.toggle_selected(1);
        assert_eq!(edit.delete_targets(), vec![1]);
        edit.reset();
        assert!(edit.delete_targets().is_empty());
    }

    #[test]
    fn measures_follow_the_number_of_atoms() {
        let m = model();
        let c = coords(&m);
        assert!(
            matches!(Measure::of(&[0, 1], &c), Some(Measure::Distance(d)) if (d - 1.5).abs() < 1e-9)
        );
        assert!(matches!(
            Measure::of(&[2, 0, 1], &c),
            Some(Measure::Angle(_))
        ));
        assert!(matches!(
            Measure::of(&[2, 0, 1, 3], &c),
            Some(Measure::Dihedral(_))
        ));
        assert_eq!(Measure::of(&[0], &c), None);
        assert_eq!(Measure::of(&[0, 9], &c), None);
        // The same over the points themselves.
        assert_eq!(
            Measure::of_points(&[c[2], c[0], c[1], c[3]]),
            Measure::of(&[2, 0, 1, 3], &c)
        );
        assert_eq!(Measure::of_points(&[c[0]]), None);
    }
}
