//! Editing a 3D model by hand: what is picked in the viewer, the restraints the next
//! optimisation keeps, and the undo/redo history of the geometry.
//!
//! Edits only move atoms; the atoms, bonds and their order never change, so a history step is
//! a coordinate snapshot. Each edit is written into both [`Model3d::atoms`] and
//! [`Model3d::molfile`] (see [`set_coords`]), so optimising and exporting start from what is on
//! screen. The arithmetic is in [`super::geom`].

use std::fmt;

use super::Model3d;
use super::geom::{self, Graph, Vec3};

/// The most atoms any edit needs: four, for a dihedral.
pub const MAX_PICKED: usize = 4;
/// Geometry snapshots kept for undo.
const HISTORY: usize = 100;

/// A restraint the next optimisation keeps (atom indices into [`Model3d::atoms`], 0-based as
/// OpenBabel's constraints are).
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

/// OpenBabel ignores torsion restraints when an angle restraint is present in the same set, so
/// the panel warns rather than letting one silently not apply.
pub fn conflict(constraints: &[Constraint]) -> Option<&'static str> {
    let has = |f: fn(&Constraint) -> bool| constraints.iter().any(f);
    (has(|c| matches!(c, Constraint::Angle(..))) && has(|c| matches!(c, Constraint::Torsion(..))))
        .then(|| t!("three_d.edit.constraint_conflict"))
}

/// One history entry: the geometry (and energy) before or after an edit.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    label: String,
    coords: Vec<Vec3>,
    energy: f64,
    energy_stale: bool,
}

/// The editing state of one document's 3D model.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit3d {
    /// Atoms picked in the viewer, oldest first; at most [`MAX_PICKED`].
    pub picked: Vec<usize>,
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
        self.redo
            .push(snapshot(model, &step.label, self.energy_stale));
        self.restore(model, step);
        Some(label)
    }

    pub fn redo(&mut self, model: &mut Model3d) -> Option<String> {
        let step = self.redo.pop()?;
        let label = step.label.clone();
        self.push_undo(snapshot(model, &step.label, self.energy_stale));
        self.restore(model, step);
        Some(label)
    }

    fn restore(&mut self, model: &mut Model3d, step: Snapshot) {
        set_coords(model, &step.coords);
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

        assert!(conflict(&[Constraint::Angle([0, 1, 2], 100.0)]).is_none());
        assert!(
            conflict(&[
                Constraint::Angle([0, 1, 2], 100.0),
                Constraint::Torsion([0, 1, 2, 3], 10.0)
            ])
            .is_some()
        );
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
