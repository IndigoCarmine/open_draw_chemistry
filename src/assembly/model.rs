//! What an assembly is made of: the monomers ([`Component`]), the steps that copy them into
//! place ([`Step`]), molecules placed by hand ([`Instance`], used from A3 on), and the editing
//! state around them with its undo history ([`AssemblyState`]).
//!
//! The steps are applied in order to a growing set of placements. The set starts as the base
//! monomer at the origin; each step makes `n` copies of the whole set, copy `i` moved by the
//! step's `i`-th transform, so a ring followed by a stack gives a stack of rings — the
//! build_md_rs `make_rosette2` → `make_oligorosette` pipeline.

use serde::{Deserialize, Serialize};

use super::math::{self, Cell, Rigid, Vec3};
use super::symop::{self, SymOp};
use crate::three_d::{Atom3d, Bond3d, Model3d};

/// How many undo steps are kept.
pub const HISTORY_LIMIT: usize = 100;

/// A monomer: atoms and bonds in its own frame, with an optional orientation by three atoms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Component {
    pub name: String,
    pub atoms: Vec<Atom3d>,
    pub bonds: Vec<Bond3d>,
    /// Orient the monomer by these atoms before placing it (see [`orientation`]).
    #[serde(default)]
    pub orient: Option<[usize; 3]>,
}

impl Component {
    /// A monomer from a 3D model: only its atoms and bonds (the energy and the molfile stay
    /// with the model).
    pub fn from_model(name: impl Into<String>, model: &Model3d) -> Self {
        Self {
            name: name.into(),
            atoms: model.atoms.clone(),
            bonds: model.bonds.clone(),
            orient: None,
        }
    }

    /// The transform that orients the monomer ([`Rigid::IDENTITY`] without an orientation).
    pub fn orient_transform(&self) -> Result<Rigid, OrientError> {
        match self.orient {
            Some(atoms) => orientation(&self.atoms, atoms),
            None => Ok(Rigid::IDENTITY),
        }
    }

    /// The atoms' positions after orienting.
    pub fn oriented_positions(&self) -> Result<Vec<Vec3>, OrientError> {
        let t = self.orient_transform()?;
        Ok(self.atoms.iter().map(|a| t.apply(a.pos)).collect())
    }

    /// The residue name written to PDB and MOL2: up to three letters or digits of the name,
    /// upper case, or `MOL`.
    pub fn residue_name(&self) -> String {
        let name: String = self
            .name
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(3)
            .collect::<String>()
            .to_ascii_uppercase();
        if name.is_empty() {
            "MOL".to_owned()
        } else {
            name
        }
    }
}

/// Why three atoms cannot orient a monomer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrientError {
    /// An index past the monomer's atoms.
    OutOfRange,
    /// The same atom twice.
    Repeated,
    /// The atoms coincide or are on one line, so they fix no plane.
    Collinear,
}

impl std::fmt::Display for OrientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OrientError::OutOfRange => t!("assembly.error.orient_out_of_range"),
            OrientError::Repeated => t!("assembly.error.orient_repeated"),
            OrientError::Collinear => t!("assembly.error.orient_collinear"),
        })
    }
}

impl std::error::Error for OrientError {}

/// The transform orienting `atoms` by three of them: the first to the origin, the second onto
/// the +x axis, and the third into the xy plane on the y < 0 side.
///
/// Ported from build_md_rs `src/gromacs/pre_coordinator.rs` `precoordinate2` (itself a port
/// of mylibs' `pre_coordinator.py`): translate the first atom to the origin, yaw about z and
/// pitch about y to put the second on +x, then roll about x to put the third in the plane.
/// gmx_template's build guide calls these PRECO[0..2] and explains the choice.
pub fn orientation(atoms: &[Atom3d], [a, b, c]: [usize; 3]) -> Result<Rigid, OrientError> {
    let pos = |i: usize| atoms.get(i).map(|atom| atom.pos);
    let (Some(pa), Some(pb), Some(pc)) = (pos(a), pos(b), pos(c)) else {
        return Err(OrientError::OutOfRange);
    };
    if a == b || b == c || a == c {
        return Err(OrientError::Repeated);
    }
    let mut t = Rigid::translation(math::scale(pa, -1.0));
    // Yaw the second atom into the xz plane, then pitch it onto +x.
    let v = t.apply(pb);
    if math::norm(v) < 1e-6 {
        return Err(OrientError::Collinear);
    }
    t = t.then(&Rigid::rotation(math::rot_z(
        -v[1].atan2(v[0]).to_degrees(),
    )));
    let v = t.apply(pb);
    t = t.then(&Rigid::rotation(math::rot_y(v[2].atan2(v[0]).to_degrees())));
    // Roll about x so the third atom lands in the xy plane, y < 0.
    let v = t.apply(pc);
    if v[1].hypot(v[2]) < 1e-6 {
        return Err(OrientError::Collinear);
    }
    t = t.then(&Rigid::rotation(math::rot_x(
        v[2].atan2(-v[1]).to_degrees(),
    )));
    Ok(t)
}

/// Copies on a ring about the z axis.
///
/// From build_md_rs `rosette_maker.rs`: copy `i` is tilted by `−tilt` about y
/// (`make_rosette`), pushed `radius` along +x, then turned `360°·i/n` about z
/// (`make_rosette2`). With the monomer oriented by three atoms, the first sits on the circle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ring {
    pub enabled: bool,
    pub n: u32,
    pub radius: f64,
    pub tilt_deg: f64,
}

/// Copies stacked along z, each layer twisted and slipped.
///
/// From build_md_rs `rosette_maker.rs` `make_oligorosette`: layer `i` is lifted `rise·i`
/// along z, turned `twist·i` about z, then moved onto a helix of radius
/// `(slip/2) / sin(twist/2)` about the z axis at angle `twist·i`, so consecutive layers are
/// `slip` apart sideways. Without twist the slip is a straight offset of `slip·i` along x
/// (the limit of the helix).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stack {
    pub enabled: bool,
    pub n: u32,
    pub rise: f64,
    pub twist_deg: f64,
    pub slip: f64,
}

/// Copies in a row: copy `i` turned by `i·euler` (degrees, extrinsic xyz, about the origin)
/// and moved `i·offset`. sp_builder's `MolArray` node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Array {
    pub enabled: bool,
    pub n: u32,
    pub offset: Vec3,
    pub euler_deg: Vec3,
}

/// A crystal: everything built so far is the asymmetric unit, copied by the space group's
/// symmetry operations into a unit cell and the cell repeated `reps` times along its edges.
///
/// The set built so far is taken as Cartesian Å in the cell's frame (**a** along x, **b** in
/// the xy plane). Each operation is applied to each molecule as a whole, and the copy is moved
/// by whole cell vectors so that its centroid lies in `[0, 1)` — molecules are never cut at
/// the cell faces. Copies that coincide with one kept before (a molecule on an inversion
/// centre or a twofold axis maps onto itself or onto another copy) are dropped, so the unit
/// cell holds Z distinct molecules. The cell is then repeated, `a` fastest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lattice {
    pub enabled: bool,
    pub cell: Cell,
    pub reps: [u32; 3],
    pub symops: Vec<SymOp>,
}

/// Two copies are the same molecule when every atom of one lies within this distance (Å) of
/// an atom of the same element of the other (after a whole-cell shift). Generous enough for
/// coordinates rounded to four decimals, far below any distance between distinct atoms.
pub const DUPLICATE_TOLERANCE: f64 = 0.1;

/// Centroids this close below a cell face (in fractional units) count as on it, so rounding
/// does not send a molecule on the face to the far side of the cell.
const WRAP_EPS: f64 = 1e-6;

/// A component's atoms as the steps place them: positions (after orientation) and elements.
pub type Shape = (Vec<Vec3>, Vec<String>);

impl Lattice {
    /// Whether the step can be built: a possible cell, operations including the identity, and
    /// every operation keeping distances in this cell. The message is in the UI language.
    pub fn check(&self) -> Result<(), String> {
        self.cell.check()?;
        if !self.symops.iter().any(SymOp::is_identity) {
            return Err(t!("assembly.symop.error.no_identity").to_owned());
        }
        if let Some(op) = self.symops.iter().find(|op| !op.fits(&self.cell)) {
            return Err(t!("assembly.error.op_misfits_cell", op = op));
        }
        Ok(())
    }

    /// How many unit cells: `reps` multiplied out, each at least one.
    pub fn cell_count(&self) -> usize {
        self.reps
            .iter()
            .fold(1usize, |n, &r| n.saturating_mul(r.max(1) as usize))
    }

    /// The whole-cell translations of the repeats, `a` fastest.
    pub fn translations(&self) -> Vec<Rigid> {
        let [na, nb, nc] = self.reps.map(|r| r.max(1));
        let mut out = Vec::with_capacity(self.cell_count());
        for k in 0..nc {
            for j in 0..nb {
                for i in 0..na {
                    let f = [f64::from(i), f64::from(j), f64::from(k)];
                    out.push(Rigid::translation(self.cell.to_cart(f)));
                }
            }
        }
        out
    }

    /// The unit cell's molecules made from `set` (the asymmetric unit): every operation on
    /// every molecule, moved into the cell by its centroid, duplicates dropped. `shapes[c]` is
    /// component `c`'s atoms (`None` skips its molecules).
    pub fn unit_cell(&self, set: &[Instance], shapes: &[Option<Shape>]) -> Vec<Instance> {
        struct Kept {
            component: usize,
            centroid: Vec3,
            atoms: Vec<Vec3>,
        }
        let mut kept: Vec<Kept> = Vec::new();
        let mut out = Vec::new();
        for op in &self.symops {
            let op = op.cartesian(&self.cell);
            for inst in set {
                let Some(Some((positions, elements))) = shapes.get(inst.component) else {
                    continue;
                };
                let moved = inst.transform.then(&op);
                let atoms: Vec<Vec3> = positions.iter().map(|&p| moved.apply(p)).collect();
                let frac = self.cell.to_frac(centroid(&atoms));
                let shift = frac.map(|f| -(f + WRAP_EPS).floor());
                let shift_cart = self.cell.to_cart(shift);
                let atoms: Vec<Vec3> = atoms.iter().map(|&p| math::add(p, shift_cart)).collect();
                let frac = math::add(frac, shift);
                let duplicate = kept.iter().any(|k| {
                    k.component == inst.component
                        && self.same_molecule(&k.atoms, k.centroid, &atoms, frac, elements)
                });
                if duplicate {
                    continue;
                }
                kept.push(Kept {
                    component: inst.component,
                    centroid: frac,
                    atoms,
                });
                out.push(Instance {
                    component: inst.component,
                    transform: moved.then(&Rigid::translation(shift_cart)),
                });
            }
        }
        out
    }

    /// Whether two copies of one component (`a` and `b`, with fractional centroids) are the
    /// same molecule up to a whole-cell shift.
    fn same_molecule(
        &self,
        a: &[Vec3],
        ca: Vec3,
        b: &[Vec3],
        cb: Vec3,
        elements: &[String],
    ) -> bool {
        let d = math::sub(ca, cb);
        let n = d.map(f64::round);
        if math::norm(self.cell.to_cart(math::sub(d, n))) > DUPLICATE_TOLERANCE {
            return false;
        }
        let shift = self.cell.to_cart(n);
        a.iter().zip(elements).all(|(&p, element)| {
            b.iter().zip(elements).any(|(&q, e)| {
                e == element && math::distance(p, math::add(q, shift)) < DUPLICATE_TOLERANCE
            })
        })
    }
}

/// The mean of `points` (the origin for none).
pub fn centroid(points: &[Vec3]) -> Vec3 {
    if points.is_empty() {
        return [0.0; 3];
    }
    let sum = points.iter().fold([0.0; 3], |s, &p| math::add(s, p));
    math::scale(sum, 1.0 / points.len() as f64)
}

/// One step of building the assembly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Step {
    Ring(Ring),
    Stack(Stack),
    Array(Array),
    Lattice(Lattice),
}

impl Step {
    /// The defaults a new step starts from: a hexagonal rosette, a 3.5 Å π stack, a row.
    pub fn new_ring() -> Self {
        Step::Ring(Ring {
            enabled: true,
            n: 6,
            radius: 5.0,
            tilt_deg: 0.0,
        })
    }

    pub fn new_stack() -> Self {
        Step::Stack(Stack {
            enabled: true,
            n: 10,
            rise: 3.5,
            twist_deg: 0.0,
            slip: 0.0,
        })
    }

    pub fn new_array() -> Self {
        Step::Array(Array {
            enabled: true,
            n: 4,
            offset: [5.0, 0.0, 0.0],
            euler_deg: [0.0; 3],
        })
    }

    /// One 10 Å cubic P1 cell: the user then picks the space group and the constants.
    pub fn new_lattice() -> Self {
        Step::Lattice(Lattice {
            enabled: true,
            cell: Cell::default(),
            reps: [1, 1, 1],
            symops: symop::PRESETS[0].symops(),
        })
    }

    /// The step's name in the panel.
    pub fn label(&self) -> &'static str {
        match self {
            Step::Ring(_) => t!("assembly.model.step_ring"),
            Step::Stack(_) => t!("assembly.model.step_stack"),
            Step::Array(_) => t!("assembly.model.step_array"),
            Step::Lattice(_) => t!("assembly.model.step_lattice"),
        }
    }

    pub fn enabled(&self) -> bool {
        match self {
            Step::Ring(s) => s.enabled,
            Step::Stack(s) => s.enabled,
            Step::Array(s) => s.enabled,
            Step::Lattice(s) => s.enabled,
        }
    }

    pub fn enabled_mut(&mut self) -> &mut bool {
        match self {
            Step::Ring(s) => &mut s.enabled,
            Step::Stack(s) => &mut s.enabled,
            Step::Array(s) => &mut s.enabled,
            Step::Lattice(s) => &mut s.enabled,
        }
    }

    /// How many copies the step makes (at least one). For a lattice, the most it can make:
    /// copies on special positions are dropped when it is built.
    pub fn copies(&self) -> usize {
        let n = match self {
            Step::Ring(s) => s.n,
            Step::Stack(s) => s.n,
            Step::Array(s) => s.n,
            Step::Lattice(l) => {
                return l.symops.len().max(1).saturating_mul(l.cell_count());
            }
        };
        n.max(1) as usize
    }

    /// The transform of each copy, in order. A lattice's symmetry copies depend on where each
    /// molecule is, so they are made in [`Design::placements`]; here it gives only the
    /// whole-cell repeats.
    pub fn transforms(&self) -> Vec<Rigid> {
        if let Step::Lattice(l) = self {
            return l.translations();
        }
        let n = self.copies();
        (0..n)
            .map(|i| {
                let i = i as f64;
                match self {
                    Step::Ring(s) => Rigid::rotation(math::rot_y(-s.tilt_deg))
                        .then(&Rigid::translation([s.radius, 0.0, 0.0]))
                        .then(&Rigid::rotation(math::rot_z(360.0 / n as f64 * i))),
                    Step::Stack(s) => Rigid::translation([0.0, 0.0, s.rise * i])
                        .then(&Rigid::rotation(math::rot_z(s.twist_deg * i)))
                        .then(&Rigid::translation(slip_offset(s, i))),
                    Step::Array(s) => Rigid::rotation(math::euler_xyz(math::scale(s.euler_deg, i)))
                        .then(&Rigid::translation(math::scale(s.offset, i))),
                    Step::Lattice(_) => unreachable!("handled above"),
                }
            })
            .collect()
    }
}

/// The helix radius of a slipped, twisted stack: `(slip/2) / sin(twist/2)` (build_md_rs
/// `make_oligorosette`), or `None` when it is infinite (no twist) or there is no slip.
pub fn helix_radius(slip: f64, twist_deg: f64) -> Option<f64> {
    let half = (twist_deg / 2.0).to_radians().sin();
    (slip != 0.0 && half.abs() > 1e-9).then(|| (slip / 2.0) / half)
}

/// Where layer `i` of a stack is moved sideways.
fn slip_offset(s: &Stack, i: f64) -> Vec3 {
    if s.slip == 0.0 {
        return [0.0; 3];
    }
    match helix_radius(s.slip, s.twist_deg) {
        Some(r) => {
            let theta = (s.twist_deg * i).to_radians();
            [r * theta.sin(), r * theta.cos(), 0.0]
        }
        None => [s.slip * i, 0.0, 0.0],
    }
}

/// One placed molecule: a component moved by a transform (applied after its orientation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    pub component: usize,
    pub transform: Rigid,
}

/// One molecule of the assembly named by where it comes from rather than by its place in the
/// build, so the name still holds when molecules before it are removed or added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// A copy the steps make, by its index among all their copies (the excluded ones counted,
    /// as [`Design::excluded`] names them). Detached into a hand-placed molecule, it is that
    /// molecule.
    Step(usize),
    /// A hand-placed molecule, by its id in [`Design::manual`] (see [`Ids`]).
    Manual(u64),
}

/// Ids that stay with the items of a list (the monomers, the hand-placed molecules) while
/// items before them are removed — unlike their indices, which shift.
///
/// Items are named by their index until one is removed: an item past the assigned ids has
/// the id `next + (its index - assigned.len())`, so appending an item (which the app may do
/// to the list directly) changes no item's id. Removing one, through [`Ids::remove`], first
/// gives every item its id for good. A design never edited that way, an old file's included,
/// needs none stored. An id is never handed out twice within a design's history: `next` only
/// grows.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ids {
    /// The ids of the first items, in order.
    assigned: Vec<u64>,
    /// The id of the first item past them.
    next: u64,
}

impl Ids {
    fn is_unused(&self) -> bool {
        self.assigned.is_empty() && self.next == 0
    }

    /// The id of item `k`.
    pub fn id(&self, k: usize) -> u64 {
        match k.checked_sub(self.assigned.len()) {
            None => self.assigned[k],
            Some(past) => self.next + past as u64,
        }
    }

    /// The item with id `id` in a list of `len` items, if it is still there.
    pub fn index(&self, id: u64, len: usize) -> Option<usize> {
        let k = match self.assigned.iter().position(|&a| a == id) {
            Some(k) => k,
            None => self.assigned.len() + usize::try_from(id.checked_sub(self.next)?).ok()?,
        };
        (k < len).then_some(k)
    }

    /// Note that item `k` of a list of `len` is removed.
    fn remove(&mut self, len: usize, k: usize) {
        while self.assigned.len() < len {
            self.assigned.push(self.next);
            self.next += 1;
        }
        self.assigned.remove(k);
    }
}

/// Everything that determines the assembly: what is saved, and what undo restores.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Design {
    pub components: Vec<Component>,
    /// The component the steps copy.
    pub base: usize,
    pub steps: Vec<Step>,
    /// Molecules placed by hand, after those the steps make (free placement).
    pub manual: Vec<Instance>,
    /// Molecules the steps make that are left out — deleted, or detached into
    /// [`Design::manual`] to be moved by hand — by their index among all the steps' copies
    /// (counting the excluded ones), sorted. The index stays when the steps change, so it then
    /// names whichever copy has that place; indices past the steps' count are ignored.
    pub excluded: Vec<usize>,
    /// The ids of [`Design::components`]. Remove a component with
    /// [`Design::remove_component`] so they stay right.
    #[serde(skip_serializing_if = "Ids::is_unused")]
    pub component_ids: Ids,
    /// The ids of [`Design::manual`]. Remove a hand-placed molecule with
    /// [`Design::remove_molecule`] so they stay right.
    #[serde(skip_serializing_if = "Ids::is_unused")]
    pub manual_ids: Ids,
    /// The hand-placed molecules detached from the steps ([`Design::detach`]): each one's id
    /// and the step copy it was, by [`Design::excluded`]'s index.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub detached: Vec<(u64, usize)>,
}

impl Design {
    /// How many copies the steps make, counting the excluded ones. Saturates instead of
    /// overflowing on absurd step counts.
    fn all_step_copies(&self) -> usize {
        if self.components.get(self.base).is_none() {
            return 0;
        }
        self.running_counts().last().copied().unwrap_or(1)
    }

    /// How many copies there are after each step (one entry per step, the excluded copies
    /// counted): what the panel shows between the steps. A disabled step, or a lattice step
    /// that cannot be built, leaves the count as it was. Without a base component every entry
    /// is zero. Saturates instead of overflowing.
    pub fn running_counts(&self) -> Vec<usize> {
        if self.components.get(self.base).is_none() {
            return vec![0; self.steps.len()];
        }
        // A lattice step drops symmetry copies that coincide (special positions), so its
        // `copies()` is only an upper bound: count what the steps really make.
        if self
            .steps
            .iter()
            .any(|s| s.enabled() && matches!(s, Step::Lattice(_)))
        {
            let mut counts = vec![0; self.steps.len()];
            self.trace_step_copies(|i, n| counts[i] = n);
            return counts;
        }
        let mut n = 1usize;
        self.steps
            .iter()
            .map(|s| {
                if s.enabled() {
                    n = n.saturating_mul(s.copies());
                }
                n
            })
            .collect()
    }

    /// How many molecules the steps make, the excluded ones left out.
    pub fn step_molecule_count(&self) -> usize {
        let all = self.all_step_copies();
        all - self.excluded.iter().filter(|&&i| i < all).count()
    }

    /// How many molecules the steps and the hand-placed ones make, without making them.
    pub fn molecule_count(&self) -> usize {
        self.step_molecule_count().saturating_add(self.manual.len())
    }

    /// How many atoms the assembly has, without building it.
    pub fn atom_count(&self) -> usize {
        let from_steps = self
            .components
            .get(self.base)
            .map_or(0, |c| c.atoms.len())
            .saturating_mul(self.step_molecule_count());
        self.manual
            .iter()
            .filter_map(|m| self.components.get(m.component))
            .fold(from_steps, |n, c| n.saturating_add(c.atoms.len()))
    }

    /// The first enabled lattice step that cannot be built, and why (in the UI language).
    pub fn lattice_error(&self) -> Option<String> {
        self.steps.iter().find_map(|s| match s {
            Step::Lattice(l) if l.enabled => l.check().err(),
            _ => None,
        })
    }

    /// Each component's atoms as the steps place them (oriented; as read when the
    /// orientation is unusable, which the build reports separately).
    pub fn shapes(&self) -> Vec<Option<Shape>> {
        self.components
            .iter()
            .map(|c| {
                let positions = c
                    .oriented_positions()
                    .unwrap_or_else(|_| c.atoms.iter().map(|a| a.pos).collect());
                Some((
                    positions,
                    c.atoms.iter().map(|a| a.element.clone()).collect(),
                ))
            })
            .collect()
    }

    /// Every copy the steps make, the excluded ones included (the base component in order, the
    /// last step's copies outermost), with the crystal the last enabled lattice step made.
    /// Lattice steps that cannot be built ([`Design::lattice_error`]) are skipped.
    fn step_copies_with_crystal(&self) -> (Vec<Instance>, Option<Crystal>) {
        self.trace_step_copies(|_, _| {})
    }

    /// [`Design::step_copies_with_crystal`], telling `after` how many copies there are after
    /// each step, by the step's index (disabled steps included).
    fn trace_step_copies(
        &self,
        mut after: impl FnMut(usize, usize),
    ) -> (Vec<Instance>, Option<Crystal>) {
        let mut set = Vec::new();
        let mut crystal = None;
        if self.components.get(self.base).is_some() {
            set.push(Instance {
                component: self.base,
                transform: Rigid::IDENTITY,
            });
            let mut shapes = None;
            for (i, step) in self.steps.iter().enumerate() {
                if !step.enabled() {
                    after(i, set.len());
                    continue;
                }
                if let Step::Lattice(lattice) = step {
                    if lattice.check().is_err() {
                        after(i, set.len());
                        continue;
                    }
                    let shapes = shapes.get_or_insert_with(|| self.shapes());
                    let unit = lattice.unit_cell(&set, shapes);
                    let mut next = Vec::with_capacity(unit.len() * lattice.cell_count());
                    for t in lattice.translations() {
                        next.extend(unit.iter().map(|inst| Instance {
                            component: inst.component,
                            transform: inst.transform.then(&t),
                        }));
                    }
                    crystal = Some(Crystal {
                        cell: lattice.cell,
                        reps: lattice.reps.map(|r| r.max(1)),
                        symops: lattice.symops.clone(),
                        z: unit.len(),
                        asymmetric: std::mem::replace(&mut set, next),
                    });
                    after(i, set.len());
                    continue;
                }
                let transforms = step.transforms();
                let mut next = Vec::with_capacity(set.len() * transforms.len());
                for t in &transforms {
                    for inst in &set {
                        next.push(Instance {
                            component: inst.component,
                            transform: inst.transform.then(t),
                        });
                    }
                }
                set = next;
                after(i, set.len());
            }
        }
        (set, crystal)
    }

    /// Every copy the steps make, the excluded ones included.
    fn step_copies(&self) -> Vec<Instance> {
        self.step_copies_with_crystal().0
    }

    /// Every placed molecule: the steps' copies (less the excluded ones), then the hand-placed
    /// ones. A built molecule's index is its place in this list.
    pub fn instances(&self) -> Vec<Instance> {
        self.placements().0
    }

    /// [`Design::instances`], with the crystal the last enabled lattice step made.
    pub fn placements(&self) -> (Vec<Instance>, Option<Crystal>) {
        let (copies, crystal) = self.step_copies_with_crystal();
        let mut set: Vec<Instance> = copies
            .into_iter()
            .enumerate()
            .filter(|(i, _)| self.excluded.binary_search(i).is_err())
            .map(|(_, inst)| inst)
            .collect();
        set.extend(self.manual.iter().cloned());
        (set, crystal)
    }

    /// The hand-placed molecule that molecule `m` of the build is, if it is one.
    pub fn manual_index(&self, m: usize) -> Option<usize> {
        m.checked_sub(self.step_molecule_count())
            .filter(|&k| k < self.manual.len())
    }

    /// The molecule of the build that hand-placed molecule `k` is.
    pub fn manual_molecule(&self, k: usize) -> usize {
        self.step_molecule_count() + k
    }

    /// The index among all the steps' copies of molecule `m`, if the steps made it.
    fn step_copy_index(&self, m: usize) -> Option<usize> {
        if m >= self.step_molecule_count() {
            return None;
        }
        // Walk past the excluded copies (sorted) that come before it.
        let mut copy = m;
        for &e in &self.excluded {
            if e <= copy {
                copy += 1;
            } else {
                break;
            }
        }
        Some(copy)
    }

    /// Which copy molecule `m` of the build is, by a name that survives the design changing
    /// around it (see [`Origin`]). A molecule detached from the steps keeps the name of the
    /// copy it was, so it is the same before and after [`Design::detach`] (and its undo).
    pub fn origin_of(&self, m: usize) -> Option<Origin> {
        if let Some(k) = self.manual_index(m) {
            let id = self.manual_ids.id(k);
            return Some(match self.detached.iter().find(|&&(d, _)| d == id) {
                Some(&(_, copy)) => Origin::Step(copy),
                None => Origin::Manual(id),
            });
        }
        self.step_copy_index(m).map(Origin::Step)
    }

    /// The molecule of the build that `origin` is, if the design still makes it.
    pub fn molecule_of(&self, origin: Origin) -> Option<usize> {
        match origin {
            Origin::Step(c) => {
                if let Some(&(id, _)) = self.detached.iter().find(|&&(_, from)| from == c) {
                    return self.molecule_of(Origin::Manual(id));
                }
                if c >= self.all_step_copies() || self.excluded.binary_search(&c).is_ok() {
                    return None;
                }
                Some(c - self.excluded.iter().take_while(|&&e| e < c).count())
            }
            Origin::Manual(id) => self
                .manual_ids
                .index(id, self.manual.len())
                .map(|k| self.manual_molecule(k)),
        }
    }

    /// The id of component `i` (see [`Ids`]).
    pub fn component_id(&self, i: usize) -> u64 {
        self.component_ids.id(i)
    }

    /// The component with id `id`, if the design still has it.
    pub fn component_index(&self, id: u64) -> Option<usize> {
        self.component_ids.index(id, self.components.len())
    }

    /// Turn molecule `m`, made by the steps, into a hand-placed one where it is, so it can be
    /// moved on its own: the steps leave it out and [`Design::manual`] gains it, remembering
    /// which copy it was (so [`Origin::Step`] still finds it). Returns its new molecule index,
    /// or `None` when `m` is not a step's molecule.
    pub fn detach(&mut self, m: usize) -> Option<usize> {
        let copy = self.step_copy_index(m)?;
        let instance = self.step_copies().swap_remove(copy);
        self.exclude(copy);
        self.manual.push(instance);
        let k = self.manual.len() - 1;
        self.detached.push((self.manual_ids.id(k), copy));
        Some(self.manual_molecule(k))
    }

    /// Remove molecule `m`: a hand-placed one is deleted, a step's is left out of the steps.
    /// Returns whether there was such a molecule.
    pub fn remove_molecule(&mut self, m: usize) -> bool {
        if let Some(k) = self.manual_index(m) {
            self.remove_manual(k);
            true
        } else if let Some(copy) = self.step_copy_index(m) {
            self.exclude(copy);
            true
        } else {
            false
        }
    }

    /// Delete hand-placed molecule `k`.
    fn remove_manual(&mut self, k: usize) {
        let id = self.manual_ids.id(k);
        self.manual_ids.remove(self.manual.len(), k);
        self.manual.remove(k);
        self.detached.retain(|&(d, _)| d != id);
    }

    /// Remove component `i` and the hand-placed molecules of it. When it is the base, the
    /// steps copy the one before it (or the next, for the first).
    pub fn remove_component(&mut self, i: usize) {
        if i >= self.components.len() {
            return;
        }
        self.component_ids.remove(self.components.len(), i);
        self.components.remove(i);
        let mut k = 0;
        while k < self.manual.len() {
            if self.manual[k].component == i {
                self.remove_manual(k);
            } else {
                k += 1;
            }
        }
        for m in &mut self.manual {
            if m.component > i {
                m.component -= 1;
            }
        }
        if self.base > i || self.base >= self.components.len() {
            self.base = self.base.saturating_sub(1);
        }
    }

    fn exclude(&mut self, copy: usize) {
        if let Err(at) = self.excluded.binary_search(&copy) {
            self.excluded.insert(at, copy);
        }
    }
}

/// What a lattice step made: the unit cell, its repeats, the operations, the asymmetric unit
/// they were applied to, and how many molecules one unit cell holds. The steps' molecules
/// start with the unit cell at the origin (`z` molecules), then its repeats.
#[derive(Debug, Clone, PartialEq)]
pub struct Crystal {
    pub cell: Cell,
    pub reps: [u32; 3],
    pub symops: Vec<SymOp>,
    /// The molecules the operations were applied to, as placed before the lattice step.
    pub asymmetric: Vec<Instance>,
    /// Molecules per unit cell (Z for a one-molecule asymmetric unit).
    pub z: usize,
}

/// A document's assembly: the design being edited, the molecule selected in the viewer, the
/// undo history, and a revision that changes with every edit (the build and the viewer are
/// redone only when it changes).
///
/// Edits made in a single gesture (dragging a value) are gathered into one undo step: the UI
/// calls [`AssemblyState::touch`] on every change and [`AssemblyState::settle`] once the
/// pointer is released; [`AssemblyState::edit`] does both for a one-off change.
#[derive(Debug)]
pub struct AssemblyState {
    /// Tells this state apart from every other in the process (a viewer showing several
    /// documents' assemblies in turn keys on it).
    id: u64,
    /// Which design is open: changed by [`AssemblyState::replace`] and kept by every edit, so
    /// the designs sharing it are one design's history, in which an item's id
    /// ([`Ids`]) always names the same item. Unique in the process.
    lineage: u64,
    pub design: Design,
    /// The molecule selected in the viewer, as an index into the build's molecules.
    pub selected: Option<usize>,
    /// The design as of the last undo step.
    committed: Design,
    /// The design has changed since [`AssemblyState::committed`].
    pending: bool,
    /// The designs undo and redo restore, each with its lineage.
    undo: Vec<(Design, u64)>,
    redo: Vec<(Design, u64)>,
    revision: u64,
    /// The last build and the revision it was made at (see [`super::build`]).
    pub(super) cache: Option<super::build::Cached>,
}

/// A lineage no state has had (see [`AssemblyState::lineage`]).
fn new_lineage() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Default for AssemblyState {
    fn default() -> Self {
        Self::with_design(Design::default())
    }
}

impl AssemblyState {
    /// A state opening `design`, with no history.
    pub fn with_design(design: Design) -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            lineage: new_lineage(),
            committed: design.clone(),
            design,
            selected: None,
            pending: false,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 1,
            cache: None,
        }
    }

    /// Unique to this state.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Which design is open (see [`AssemblyState::lineage`]): what refers to the design's items
    /// by id, such as the atoms picked to measure, holds only while it is the same. Undo and
    /// redo across an opened file bring back the lineage of the design they restore.
    pub fn lineage(&self) -> u64 {
        self.lineage
    }

    /// Changes with every change to the design.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Note that [`AssemblyState::design`] was changed in place.
    pub fn touch(&mut self) {
        self.pending = true;
        self.revision += 1;
    }

    /// Close the current gesture: the changes since the last undo step become one.
    pub fn settle(&mut self) {
        if !std::mem::take(&mut self.pending) || self.design == self.committed {
            return;
        }
        let previous = std::mem::replace(&mut self.committed, self.design.clone());
        self.undo.push((previous, self.lineage));
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Change the design as one undo step.
    pub fn edit<R>(&mut self, change: impl FnOnce(&mut Design) -> R) -> R {
        self.settle();
        let result = change(&mut self.design);
        self.touch();
        self.settle();
        result
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || (self.pending && self.design != self.committed)
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Go back one step. Returns whether there was one.
    pub fn undo(&mut self) -> bool {
        self.settle();
        let Some((previous, lineage)) = self.undo.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.design, previous);
        self.redo
            .push((current, std::mem::replace(&mut self.lineage, lineage)));
        self.committed = self.design.clone();
        self.revision += 1;
        true
    }

    pub fn redo(&mut self) -> bool {
        self.settle();
        let Some((next, lineage)) = self.redo.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.design, next);
        self.undo
            .push((current, std::mem::replace(&mut self.lineage, lineage)));
        self.committed = self.design.clone();
        self.revision += 1;
        true
    }

    /// Replace the design (an opened file) as one undo step. The new design is another
    /// lineage: its ids are its own, not the replaced design's.
    pub fn replace(&mut self, design: Design) {
        self.edit(|d| *d = design);
        self.lineage = new_lineage();
        self.selected = None;
    }

    pub fn is_empty(&self) -> bool {
        self.design.components.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn atom(element: &str, pos: Vec3) -> Atom3d {
        Atom3d {
            element: element.into(),
            pos,
        }
    }

    fn origins(step: &Step) -> Vec<Vec3> {
        step.transforms()
            .iter()
            .map(|t| t.apply([0.0; 3]))
            .collect()
    }

    #[test]
    fn ring_neighbours_are_a_chord_apart() {
        for n in [3u32, 6, 8] {
            let step = Step::Ring(Ring {
                enabled: true,
                n,
                radius: 4.2,
                tilt_deg: 20.0,
            });
            let p = origins(&step);
            assert_eq!(p.len(), n as usize);
            let chord = 2.0 * 4.2 * (PI / n as f64).sin();
            for i in 0..p.len() {
                let d = math::distance(p[i], p[(i + 1) % p.len()]);
                assert!((d - chord).abs() < 1e-9, "n={n}: {d} vs {chord}");
                assert!((math::norm(p[i]) - 4.2).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn a_stack_rises_and_twists_on_a_helix_of_the_stated_radius() {
        let s = Stack {
            enabled: true,
            n: 5,
            rise: 3.4,
            twist_deg: 30.0,
            slip: 1.2,
        };
        let step = Step::Stack(s.clone());
        let ts = step.transforms();
        let r = helix_radius(1.2, 30.0).unwrap();
        // build_md_rs: (slip/2) / sin(angle/2 · π/180).
        assert!((r - 0.6 / (15.0f64).to_radians().sin()).abs() < 1e-12);
        for (i, t) in ts.iter().enumerate() {
            let o = t.apply([0.0; 3]);
            assert!((o[2] - 3.4 * i as f64).abs() < 1e-9, "rise");
            assert!((o[0].hypot(o[1]) - r).abs() < 1e-9, "on the helix");
            // The twist: x turns by 30° per layer.
            let x = math::mat_vec(&t.rot, [1.0, 0.0, 0.0]);
            let angle = x[1].atan2(x[0]).to_degrees();
            let expected = 30.0 * i as f64;
            let diff = (angle - expected).rem_euclid(360.0);
            assert!(diff < 1e-6 || diff > 360.0 - 1e-6, "twist {angle}");
        }
        // Consecutive layers are `slip` apart sideways.
        for w in ts.windows(2) {
            let (a, b) = (w[0].apply([0.0; 3]), w[1].apply([0.0; 3]));
            assert!(((a[0] - b[0]).hypot(a[1] - b[1]) - 1.2).abs() < 1e-9);
        }
        // No twist: a straight slipped column.
        let straight = Step::Stack(Stack {
            twist_deg: 0.0,
            ..s
        });
        let p = origins(&straight);
        assert!(math::distance(p[2], [2.4, 0.0, 6.8]) < 1e-9);
    }

    #[test]
    fn an_array_steps_by_its_offset_and_rotation() {
        let step = Step::Array(Array {
            enabled: true,
            n: 3,
            offset: [1.0, 2.0, 0.0],
            euler_deg: [0.0, 0.0, 90.0],
        });
        let ts = step.transforms();
        assert!(math::distance(ts[2].apply([0.0; 3]), [2.0, 4.0, 0.0]) < 1e-9);
        assert!(math::distance(ts[1].apply([1.0, 0.0, 0.0]), [1.0, 3.0, 0.0]) < 1e-9);
    }

    #[test]
    fn three_atoms_orient_a_monomer() {
        let atoms = vec![
            atom("O", [1.0, 1.0, 1.0]),
            atom("N", [1.0, 3.0, 2.0]),
            atom("C", [4.0, 0.0, -1.0]),
            atom("H", [2.0, 2.0, 2.0]),
        ];
        let t = orientation(&atoms, [0, 1, 2]).unwrap();
        let p: Vec<Vec3> = atoms.iter().map(|a| t.apply(a.pos)).collect();
        assert!(math::norm(p[0]) < 1e-9, "first at the origin");
        assert!(
            p[1][0] > 0.0 && p[1][1].abs() < 1e-9 && p[1][2].abs() < 1e-9,
            "second on +x"
        );
        assert!(
            p[2][2].abs() < 1e-9 && p[2][1] < 0.0,
            "third in xy, y < 0: {:?}",
            p[2]
        );
        // A rigid move: distances are kept.
        let d = |i: usize, j: usize| math::distance(p[i], p[j]);
        let d0 = |i: usize, j: usize| math::distance(atoms[i].pos, atoms[j].pos);
        assert!((d(1, 3) - d0(1, 3)).abs() < 1e-9 && (d(2, 3) - d0(2, 3)).abs() < 1e-9);

        assert_eq!(orientation(&atoms, [0, 1, 9]), Err(OrientError::OutOfRange));
        assert_eq!(orientation(&atoms, [0, 1, 1]), Err(OrientError::Repeated));
        let line = vec![
            atom("C", [0.0; 3]),
            atom("C", [1.0, 0.0, 0.0]),
            atom("C", [2.0, 0.0, 0.0]),
        ];
        assert_eq!(orientation(&line, [0, 1, 2]), Err(OrientError::Collinear));
    }

    fn design_with_two_steps() -> Design {
        Design {
            components: vec![Component {
                name: "water".into(),
                atoms: vec![atom("O", [0.0; 3]), atom("H", [0.96, 0.0, 0.0])],
                bonds: vec![Bond3d {
                    begin: 0,
                    end: 1,
                    order: 1,
                }],
                orient: None,
            }],
            base: 0,
            steps: vec![Step::new_ring(), Step::new_stack()],
            manual: Vec::new(),
            excluded: Vec::new(),
            ..Default::default()
        }
    }

    #[test]
    fn steps_multiply_the_molecules_and_disabled_ones_are_skipped() {
        let mut d = design_with_two_steps();
        assert_eq!(d.molecule_count(), 60);
        assert_eq!(d.instances().len(), 60);
        assert_eq!(d.atom_count(), 120);
        *d.steps[1].enabled_mut() = false;
        assert_eq!(d.instances().len(), 6);
        d.manual.push(Instance {
            component: 0,
            transform: Rigid::translation([0.0, 0.0, 20.0]),
        });
        assert_eq!((d.molecule_count(), d.atom_count()), (7, 14));
        // The stack's layers are outermost: molecule 6 is ring copy 0 of layer 1.
        let d = design_with_two_steps();
        let inst = d.instances();
        let o = inst[6].transform.apply([0.0; 3]);
        assert!(math::distance(o, [5.0, 0.0, 3.5]) < 1e-9, "{o:?}");
        assert_eq!(Design::default().molecule_count(), 0);
    }

    #[test]
    fn running_counts_follow_each_step() {
        let mut d = design_with_two_steps();
        assert_eq!(d.running_counts(), vec![6, 60]);
        *d.steps[0].enabled_mut() = false;
        assert_eq!(
            d.running_counts(),
            vec![1, 10],
            "a disabled step keeps the count"
        );
        *d.steps[0].enabled_mut() = true;

        // A crystal counts the molecules it really makes, and the steps before it multiply.
        let p21c = crate::assembly::symop::PRESETS
            .iter()
            .find(|p| p.name == "P2₁/c")
            .unwrap();
        let mut lattice = Step::new_lattice();
        if let Step::Lattice(l) = &mut lattice {
            l.symops = p21c.symops();
            l.reps = [2, 1, 1];
        }
        d.steps = vec![Step::new_ring(), lattice];
        let counts = d.running_counts();
        assert_eq!(counts[0], 6);
        assert_eq!(counts[1], d.instances().len());
        assert_eq!(counts[1], d.step_molecule_count());
        assert!(counts[1] <= 6 * 4 * 2);

        // No monomer: nothing anywhere.
        d.components.clear();
        assert_eq!(d.running_counts(), vec![0, 0]);
    }

    #[test]
    fn a_step_molecule_is_detached_in_place_and_can_be_put_back() {
        let mut d = design_with_two_steps();
        d.steps.truncate(1); // six in a ring
        d.manual.push(Instance {
            component: 0,
            transform: Rigid::translation([0.0, 0.0, 9.0]),
        });
        let before = d.instances();
        assert_eq!(d.manual_index(6), Some(0));
        assert_eq!(d.manual_index(5), None);

        // Molecule 2 leaves the ring and becomes the second hand-placed one, where it was.
        let moved = d.detach(2).unwrap();
        assert_eq!(d.excluded, vec![2]);
        assert_eq!(
            moved, 6,
            "five ring molecules, then the two hand-placed ones"
        );
        assert_eq!(d.molecule_count(), 7);
        let after = d.instances();
        assert_eq!(after[moved], before[2]);
        assert_eq!(after[2], before[3], "the ring closes up behind it");
        assert_eq!(d.manual_molecule(0), 5);
        assert_eq!(d.atom_count(), 14);

        // Detaching another counts past the excluded one: ring molecule 2 now is copy 3.
        let again = d.detach(2).unwrap();
        assert_eq!(d.excluded, vec![2, 3]);
        assert_eq!(d.instances()[again], before[3]);
        assert_eq!(
            d.detach(again),
            None,
            "a hand-placed molecule is not detached"
        );

        // Removing: a step molecule is left out, a hand-placed one deleted.
        assert!(d.remove_molecule(0));
        assert_eq!(d.excluded, vec![0, 2, 3]);
        assert_eq!(d.step_molecule_count(), 3);
        let manual = d.manual.len();
        assert!(d.remove_molecule(d.manual_molecule(0)));
        assert_eq!(d.manual.len(), manual - 1);
        assert!(!d.remove_molecule(99));

        // Fewer copies: exclusions past the count are ignored.
        if let Step::Ring(r) = &mut d.steps[0] {
            r.n = 2;
        }
        assert_eq!(d.step_molecule_count(), 1);
        assert_eq!(d.instances().len(), 1 + d.manual.len());
    }

    #[test]
    fn opening_a_file_starts_another_lineage_that_undo_and_redo_cross() {
        let mut state = AssemblyState::with_design(design_with_two_steps());
        let first = state.lineage();
        state.edit(|d| d.steps.pop());
        assert_eq!(state.lineage(), first, "an edit is the same design");
        state.replace(design_with_two_steps());
        let opened = state.lineage();
        assert_ne!(opened, first);
        assert!(state.undo());
        assert_eq!(state.lineage(), first);
        assert!(state.undo());
        assert_eq!(state.lineage(), first);
        assert!(state.redo() && state.redo());
        assert_eq!(state.lineage(), opened);
        // Opening the same design again is still another.
        state.replace(design_with_two_steps());
        assert_ne!(state.lineage(), opened);
        assert_ne!(
            AssemblyState::with_design(design_with_two_steps()).lineage(),
            state.lineage()
        );
    }

    #[test]
    fn undo_and_redo_walk_the_history_one_gesture_at_a_time() {
        let mut state = AssemblyState::with_design(design_with_two_steps());
        assert!(!state.can_undo());
        let r0 = state.revision();
        // A drag: many changes, one step.
        for k in 1..=5 {
            if let Step::Ring(r) = &mut state.design.steps[0] {
                r.n = 6 + k;
            }
            state.touch();
        }
        state.settle();
        assert!(state.revision() > r0);
        state.edit(|d| d.steps.pop());
        assert_eq!(state.design.steps.len(), 1);

        assert!(state.undo());
        assert_eq!(state.design.steps.len(), 2);
        assert!(matches!(&state.design.steps[0], Step::Ring(r) if r.n == 11));
        assert!(state.undo());
        assert!(matches!(&state.design.steps[0], Step::Ring(r) if r.n == 6));
        assert!(!state.undo());
        assert!(state.redo() && state.redo());
        assert_eq!(state.design.steps.len(), 1);
        assert!(!state.redo());

        // A new edit drops the redo branch; the history is capped.
        state.undo();
        state.edit(|d| d.base = 0);
        assert!(
            state.can_redo(),
            "an edit that changes nothing keeps the redo branch"
        );
        state.edit(|d| d.components[0].name = "ice".into());
        assert!(!state.can_redo());
        for i in 0..HISTORY_LIMIT + 20 {
            state.edit(|d| d.components[0].name = format!("w{i}"));
        }
        let mut undone = 0;
        while state.undo() {
            undone += 1;
        }
        assert_eq!(undone, HISTORY_LIMIT);
    }
}
