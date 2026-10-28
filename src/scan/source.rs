//! The structure a scan evaluates at each grid point.

use std::ops::Range;

use crate::assembly::build::{self, Built};
use crate::assembly::math::{self, Rigid};
use crate::assembly::model::Design;
use crate::three_d::geom::{Graph, Vec3};
use crate::three_d::{Atom3d, Bond3d, Model3d, edit};

use super::Param;
use super::energy;

/// What is scanned: the assembly's design, or the 3D model's coordinates.
#[derive(Debug, Clone)]
pub enum Source {
    /// The design as it was when the scan started; each point is that design with the
    /// parameters set, rebuilt.
    Assembly {
        design: Design,
        params: Vec<Param>,
        /// The elements and bonds of the design as built at the start, to tell a point whose
        /// atoms merely moved from one with other atoms.
        topology: Option<(Vec<String>, Vec<Bond3d>)>,
    },
    /// The model as it was when the scan started; each point edits its coordinates, the
    /// parameters set in turn (the smaller side of each moving), never moving `anchor` — in a
    /// relaxed scan, never the atoms fixed in the 3D view, and the anchor only when it blocks
    /// every side (see [`Source::pinned`]).
    Model {
        model: Model3d,
        graph: Graph,
        anchor: Option<usize>,
        params: Vec<Param>,
    },
}

/// The structure at one grid point.
#[derive(Debug, Clone, PartialEq)]
pub enum Point {
    /// The reference structure's atoms, moved: only the coordinates change.
    Coords(Vec<Vec3>),
    /// A structure with other atoms or bonds than the reference (a crystal whose copies on
    /// special positions changed with the cell): it has to be read afresh.
    Rebuilt {
        atoms: Vec<Atom3d>,
        bonds: Vec<Bond3d>,
    },
}

/// The structure the scan starts from, as OpenBabel reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    pub molfile: String,
    pub atoms: usize,
    /// Its coordinates.
    pub coords: Vec<Vec3>,
}

impl Source {
    pub fn assembly(design: &Design, params: Vec<Param>) -> Self {
        Source::Assembly {
            topology: design_topology(design),
            design: design.clone(),
            params,
        }
    }

    pub fn model(model: &Model3d, anchor: Option<usize>, params: Vec<Param>) -> Self {
        Source::Model {
            graph: edit::graph(model),
            model: model.clone(),
            anchor,
            params,
        }
    }

    pub fn params(&self) -> &[Param] {
        match self {
            Source::Assembly { params, .. } | Source::Model { params, .. } => params,
        }
    }

    /// How many atoms the structure has now (the design as built).
    pub fn atom_count(&self) -> usize {
        match self {
            Source::Assembly { design, .. } => design.atom_count(),
            Source::Model { model, .. } => model.atoms.len(),
        }
    }

    /// The reference structure: the design built as it is, or the model's own molfile.
    pub fn reference(&self) -> Result<Reference, String> {
        match self {
            Source::Assembly { design, .. } => {
                let built = build::build(design, 0).map_err(|e| e.to_string())?;
                if built.is_empty() {
                    return Err(t!("scan.error.empty_assembly").into());
                }
                Ok(Reference {
                    molfile: energy::molfile(&built.atoms, &built.bonds),
                    atoms: built.atoms.len(),
                    coords: built.atoms.iter().map(|a| a.pos).collect(),
                })
            }
            Source::Model { model, .. } => Ok(Reference {
                molfile: model.molfile.clone(),
                atoms: model.atoms.len(),
                coords: edit::coords(model),
            }),
        }
    }

    /// The structure with the parameters at `values` (one per parameter, in order).
    pub fn at(&self, values: &[f64]) -> Result<Point, String> {
        match self {
            Source::Assembly { .. } => {
                let (_, built) = self.built_at(values)?;
                Ok(self.point_of(built))
            }
            Source::Model { model, .. } => self.coords_at(&edit::coords(model), values, None),
        }
    }

    /// The assembly's design with the parameters at `values`, and it built.
    fn built_at(&self, values: &[f64]) -> Result<(Design, Built), String> {
        let Source::Assembly { design, params, .. } = self else {
            return Err(t!("scan.error.model_param").into());
        };
        let mut d = design.clone();
        for (p, &v) in params.iter().zip(values) {
            p.write_design(&mut d, v).map_err(|e| e.to_string())?;
        }
        let built = build::build(&d, 0).map_err(|e| e.to_string())?;
        Ok((d, built))
    }

    /// The assembly `built` at a point: its coordinates when it has the atoms and bonds the
    /// design had at the start, else the whole structure.
    fn point_of(&self, built: Built) -> Point {
        let same = match self {
            Source::Assembly { topology, .. } => {
                topology.as_ref().is_some_and(|(elements, bonds)| {
                    elements.len() == built.atoms.len()
                        && elements
                            .iter()
                            .zip(&built.atoms)
                            .all(|(e, a)| *e == a.element)
                        && *bonds == built.bonds
                })
            }
            Source::Model { .. } => false,
        };
        if same {
            Point::Coords(built.atoms.iter().map(|a| a.pos).collect())
        } else {
            Point::Rebuilt {
                atoms: built.atoms,
                bonds: built.bonds,
            }
        }
    }

    /// The assembly at `values` as a relaxed scan minimises it: built rigidly, with where each
    /// molecule is placed (see [`Placed`]).
    pub fn placed_at(&self, values: &[f64]) -> Result<Placed, String> {
        let (design, built) = self.built_at(values)?;
        // The build places the instances whose component exists, in order.
        let placements: Vec<Rigid> = design
            .instances()
            .into_iter()
            .filter(|i| design.components.get(i.component).is_some())
            .map(|i| i.transform)
            .collect();
        if placements.len() != built.molecule_count() {
            return Err(t!("scan.error.item_missing").into());
        }
        let molecules = placements
            .into_iter()
            .enumerate()
            .map(|(m, placement)| (placement, built.molecule_atoms(m)))
            .collect();
        Ok(Placed {
            rigid: built.atoms.iter().map(|a| a.pos).collect(),
            point: self.point_of(built),
            molecules,
        })
    }

    /// The most atoms the structure has at any of `points` (the values of each, one per
    /// parameter): the model's are always its own; the assembly's change where a crystal's
    /// copies on special positions merge or part, so each point's design is counted (a point
    /// that cannot be set counts as the design as it is).
    pub fn most_atoms(&self, points: &[Vec<f64>]) -> usize {
        let Source::Assembly { design, params, .. } = self else {
            return self.atom_count();
        };
        let at = |values: &[f64]| {
            let mut d = design.clone();
            for (p, &v) in params.iter().zip(values) {
                p.write_design(&mut d, v).ok()?;
            }
            Some(d.atom_count())
        };
        points
            .iter()
            .filter_map(|values| at(values))
            .fold(design.atom_count(), usize::max)
    }

    /// The model's atoms at `from` (another structure of them, such as a neighbouring point's
    /// relaxed one) with the parameters set to `values`, none of the `fixed` atoms moving (see
    /// [`Source::pinned`]). The assembly's points are always rebuilt from its design, so for
    /// it this is [`Source::at`] (a relaxed scan of it starts from [`Source::placed_at`]).
    pub fn at_from(&self, from: &[Vec3], values: &[f64], fixed: &[usize]) -> Result<Point, String> {
        match self {
            Source::Assembly { .. } => self.at(values),
            Source::Model { .. } => self.coords_at(from, values, Some(fixed)),
        }
    }

    /// The model's atoms at `from` with the parameters set to `values`, keeping still the atoms
    /// [`Source::pinned`] gives for `fixed` (`None` in a rigid scan).
    fn coords_at(
        &self,
        from: &[Vec3],
        values: &[f64],
        fixed: Option<&[usize]>,
    ) -> Result<Point, String> {
        let Source::Model { graph, params, .. } = self else {
            return self.at(values);
        };
        let mut coords = from.to_vec();
        for (p, &v) in params.iter().zip(values) {
            coords = p
                .write_coords(&coords, graph, v, &self.pinned(p, fixed)?)
                .map_err(|e| e.to_string())?;
        }
        Ok(Point::Coords(coords))
    }

    /// The atoms setting the model's parameter `param` keeps still. In a rigid scan (`fixed`
    /// is `None`) that is the anchor, as the 編集 tools keep it: a parameter it blocks on every
    /// side cannot be set. In a relaxed one it is the `fixed` atoms (those held where they were
    /// when it started) and the anchor — or the `fixed` ones alone when with the anchor no side
    /// is free, for the minimiser moves the anchor anyway. Fails when the `fixed` atoms are on
    /// both sides of the parameter.
    fn pinned(&self, param: &Param, fixed: Option<&[usize]>) -> Result<Vec<usize>, String> {
        let Source::Model {
            model,
            graph,
            anchor,
            ..
        } = self
        else {
            return Ok(Vec::new());
        };
        let Some(fixed) = fixed else {
            return Ok(anchor.as_slice().to_vec());
        };
        let mut pinned = fixed.to_vec();
        pinned.extend(anchor.filter(|a| !fixed.contains(a)));
        if !param.blocked_by(graph, &pinned) {
            Ok(pinned)
        } else if !param.blocked_by(graph, fixed) {
            Ok(fixed.to_vec())
        } else {
            Err(t!(
                "scan.window.relax_fixed_both_sides",
                param = param.label(None, Some(model))
            ))
        }
    }

    /// Why the parameters cannot be set at all without moving one of the `fixed` atoms, if
    /// they cannot (checked before a relaxed scan starts; which side moves does not depend on
    /// the values).
    pub fn fixed_conflict(&self, fixed: &[usize]) -> Option<String> {
        self.params()
            .iter()
            .find_map(|p| self.pinned(p, Some(fixed)).err())
    }
}

/// An assembly point as a relaxed scan minimises it. Each molecule keeps the place and the
/// orientation the design gives it — what the scanned parameters (a step's number, a cell
/// constant, a hand-placed molecule's pose) decide — while its shape relaxes freely. OpenBabel
/// can fix atoms but not a centroid or an orientation, so the minimiser moves every atom and
/// [`Placed::hold`] then puts each molecule back rigidly: its centroid where the rigid
/// structure has it, turned to fit the rigid molecule best (see [`math::fit_rotation`]).
///
/// The centroid is the plain mean of the atoms' positions, every atom weighted alike (not by
/// mass), as the assembly places a molecule (a hand-placed one's position is that centroid,
/// see [`crate::assembly::place::pose_of`]); the orientation's fit weights them alike too.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    /// The structure built rigidly.
    pub point: Point,
    /// Each molecule's placement (the transform of its component's oriented atoms) and atoms.
    molecules: Vec<(Rigid, Range<usize>)>,
    /// Where the rigid structure has each atom: the molecules' places.
    rigid: Vec<Vec3>,
}

impl Placed {
    /// Each molecule's atoms.
    pub fn molecules(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        self.molecules.iter().map(|(_, atoms)| atoms.clone())
    }

    /// Where the rigid structure has each atom.
    pub fn rigid(&self) -> &[Vec3] {
        &self.rigid
    }

    /// Put every molecule of `coords` (a structure of this point's atoms; another number of
    /// atoms is left as it is) back in its place, rigidly, keeping its shape: its centroid on
    /// the rigid molecule's, and turned about it to fit the rigid molecule best. Of a straight
    /// molecule only its line is held (turning about it changes nothing), of one atom only its
    /// position.
    pub fn hold(&self, coords: &mut [Vec3]) {
        if coords.len() != self.rigid.len() {
            return;
        }
        for (_, atoms) in &self.molecules {
            let (Some(now), Some(want)) =
                (coords.get_mut(atoms.clone()), self.rigid.get(atoms.clone()))
            else {
                continue;
            };
            let at = crate::assembly::place::centroid(now);
            let centre = crate::assembly::place::centroid(want);
            let from: Vec<Vec3> = now.iter().map(|&p| math::sub(p, at)).collect();
            let to: Vec<Vec3> = want.iter().map(|&p| math::sub(p, centre)).collect();
            let rot = math::fit_rotation(&from, &to);
            for (p, d) in now.iter_mut().zip(&from) {
                *p = math::add(math::mat_vec(&rot, *d), centre);
            }
        }
    }

    /// `coords` (a structure of this point's atoms) with each molecule's atoms taken back into
    /// its component's frame: the molecules' shapes, without their places.
    pub fn to_local(&self, coords: &[Vec3]) -> Vec<Vec3> {
        let mut local = coords.to_vec();
        for (placement, atoms) in &self.molecules {
            let back = placement.inverse();
            for p in local.get_mut(atoms.clone()).into_iter().flatten() {
                *p = back.apply(*p);
            }
        }
        local
    }

    /// The molecules' shapes `local` (from [`Placed::to_local`], at another point) placed where
    /// this point has the molecules, and held there ([`Placed::hold`]); `None` unless this
    /// point is [`Point::Coords`] of as many atoms.
    pub fn place(&self, local: &[Vec3]) -> Option<Vec<Vec3>> {
        if !matches!(self.point, Point::Coords(_)) || local.len() != self.rigid.len() {
            return None;
        }
        let mut coords = local.to_vec();
        for (placement, atoms) in &self.molecules {
            for p in coords.get_mut(atoms.clone()).into_iter().flatten() {
                *p = placement.apply(*p);
            }
        }
        self.hold(&mut coords);
        Some(coords)
    }
}

/// The elements and bonds of `design` as built.
fn design_topology(design: &Design) -> Option<(Vec<String>, Vec<Bond3d>)> {
    let built = build::build(design, 0).ok()?;
    Some((
        built.atoms.into_iter().map(|a| a.element).collect(),
        built.bonds,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::model::{Component, Instance, Step};
    use crate::scan::{PoseField, StepField};

    /// A bent three-atom molecule with a hydrogen off its plane.
    fn component() -> Component {
        let atom = |e: &str, pos| Atom3d {
            element: e.into(),
            pos,
        };
        let bond = |begin, end| Bond3d {
            begin,
            end,
            order: 1,
        };
        Component {
            name: "x".into(),
            atoms: vec![
                atom("C", [0.0, 0.0, 0.0]),
                atom("O", [1.4, 0.0, 0.0]),
                atom("H", [1.8, 0.9, 0.0]),
                atom("H", [-0.4, -0.5, 0.8]),
            ],
            bonds: vec![bond(0, 1), bond(1, 2), bond(0, 3)],
            orient: None,
        }
    }

    fn design() -> Design {
        let mut stack = Step::new_stack();
        if let Step::Stack(s) = &mut stack {
            s.n = 3;
            s.twist_deg = 20.0;
        }
        Design {
            components: vec![component()],
            steps: vec![stack],
            manual: vec![Instance {
                component: 0,
                transform: Rigid::translation([6.0, 0.0, 0.0]),
            }],
            ..Default::default()
        }
    }

    /// The distances between every two atoms of `atoms`: the shape, wherever it is.
    fn shape(coords: &[Vec3], atoms: Range<usize>) -> Vec<f64> {
        let atoms: Vec<usize> = atoms.collect();
        atoms
            .iter()
            .flat_map(|&a| atoms.iter().map(move |&b| (a, b)))
            .map(|(a, b)| math::distance(coords[a], coords[b]))
            .collect()
    }

    fn same_shape(a: &[Vec3], b: &[Vec3], atoms: Range<usize>) -> bool {
        let (a, b) = (shape(a, atoms.clone()), shape(b, atoms));
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-9)
    }

    /// Every molecule of `coords` where `placed` has it: its centroid on the rigid one's and
    /// its best-fitting rotation onto the rigid one none.
    fn assert_held(placed: &Placed, coords: &[Vec3]) {
        use crate::assembly::place::centroid;
        let rigid = placed.rigid();
        for atoms in placed.molecules() {
            let (now, want) = (&coords[atoms.clone()], &rigid[atoms.clone()]);
            let (at, centre) = (centroid(now), centroid(want));
            let off = math::distance(at, centre);
            assert!(off < 1e-9, "{atoms:?}: centroid {off} Å off");
            let from: Vec<Vec3> = now.iter().map(|&p| math::sub(p, at)).collect();
            let to: Vec<Vec3> = want.iter().map(|&p| math::sub(p, centre)).collect();
            let rot = math::fit_rotation(&from, &to);
            assert!(
                Rigid::rotation(rot).approx_eq(&Rigid::IDENTITY, 1e-9),
                "{atoms:?}: turned {rot:?}"
            );
        }
    }

    /// Holding puts every molecule — the steps' copies and the hand-placed one — back on its
    /// place and orientation rigidly, its shape kept; and the molecules' shapes taken from one
    /// point are moved to where another point places the molecules, held there.
    #[test]
    fn a_placed_point_holds_each_molecule_and_carries_shapes_to_another_point() {
        let rise = Param::Step {
            step: 0,
            field: StepField::StackRise,
        };
        let pose = Param::Pose {
            index: 0,
            field: PoseField::Rotation(2),
        };
        let source = Source::assembly(&design(), vec![rise, pose]);
        let at = source.placed_at(&[3.5, 0.0]).unwrap();
        let Point::Coords(rigid) = &at.point else {
            panic!("the same atoms")
        };
        assert_eq!(Ok(at.point.clone()), source.at(&[3.5, 0.0]));
        assert_eq!(at.rigid(), &rigid[..]);
        let molecules: Vec<Range<usize>> = at.molecules().collect();
        assert_eq!(
            molecules,
            (0..4).map(|m| 4 * m..4 * m + 4).collect::<Vec<_>>()
        );
        // A minimised structure: every molecule's shape changed (one atom pushed out), then
        // the molecule moved off and turned, as a free minimiser leaves it.
        let mut relaxed = rigid.clone();
        for (m, atoms) in molecules.iter().enumerate() {
            relaxed[atoms.start + 1] = math::add(relaxed[atoms.start + 1], [0.2, -0.1, 0.15]);
            let turn = Rigid {
                rot: math::euler_xyz([3.0 * m as f64 + 2.0, -4.0, 5.0]),
                trans: [0.3, -0.2, 0.1 * m as f64],
            };
            for p in &mut relaxed[atoms.clone()] {
                *p = turn.apply(*p);
            }
        }
        let mut held = relaxed.clone();
        at.hold(&mut held);
        assert_held(&at, &held);
        for atoms in &molecules {
            assert!(same_shape(&held, &relaxed, atoms.clone()), "kept its shape");
            assert!(
                !same_shape(&held, rigid, atoms.clone()),
                "not the rigid shape"
            );
        }
        // Holding a held structure leaves it.
        let mut again = held.clone();
        at.hold(&mut again);
        assert!(
            again
                .iter()
                .zip(&held)
                .all(|(a, b)| math::distance(*a, *b) < 1e-9)
        );

        let shapes = at.to_local(&held);
        let there = source.placed_at(&[4.2, 35.0]).unwrap();
        let moved = there.place(&shapes).unwrap();
        assert_held(&there, &moved);
        for atoms in there.molecules() {
            assert!(same_shape(&moved, &held, atoms), "kept its shape");
        }
        assert_eq!(there.place(&shapes[1..]), None, "another number of atoms");
    }

    /// A straight molecule is held by its line — laid back along it by the smallest turn, not
    /// spun about it — a two-atom one likewise, and a lone atom at its place.
    #[test]
    fn straight_molecules_and_lone_atoms_are_held_by_what_fixes_them() {
        use crate::assembly::place::centroid;
        let atom = |e: &str, pos| Atom3d {
            element: e.into(),
            pos,
        };
        let bond = |begin, end| Bond3d {
            begin,
            end,
            order: 2,
        };
        let co2 = Component {
            name: "co2".into(),
            atoms: vec![
                atom("O", [-1.16, 0.0, 0.0]),
                atom("C", [0.0, 0.0, 0.0]),
                atom("O", [1.16, 0.0, 0.0]),
            ],
            bonds: vec![bond(0, 1), bond(1, 2)],
            orient: None,
        };
        let mut co = co2.clone();
        co.atoms.pop();
        co.bonds.pop();
        let mut ar = co2.clone();
        ar.atoms = vec![atom("Ar", [0.0, 0.0, 0.0])];
        ar.bonds.clear();
        let design = Design {
            components: vec![co2, co, ar],
            manual: vec![
                Instance {
                    component: 1,
                    transform: Rigid {
                        rot: math::euler_xyz([0.0, 30.0, 60.0]),
                        trans: [4.0, 0.0, 0.0],
                    },
                },
                Instance {
                    component: 2,
                    transform: Rigid::translation([0.0, 4.0, 0.0]),
                },
            ],
            ..Default::default()
        };
        let source = Source::assembly(&design, Vec::new());
        let at = source.placed_at(&[]).unwrap();
        let rigid = at.rigid().to_vec();
        assert_eq!(rigid.len(), 3 + 2 + 1);
        // CO2 bent, CO stretched, then both turned; Ar moved.
        let mut bent = rigid.clone();
        bent[1] = math::add(bent[1], [0.0, 0.1, 0.0]);
        bent[3] = math::add(bent[3], [0.02, 0.0, 0.0]);
        bent[5] = math::add(bent[5], [0.3, 0.0, -0.2]);
        let turn = math::euler_xyz([7.0, 4.0, -3.0]);
        let mut relaxed = bent.clone();
        for p in &mut relaxed[..5] {
            *p = math::mat_vec(&turn, *p);
        }
        let mut held = relaxed.clone();
        at.hold(&mut held);
        assert_held(&at, &held);
        for atoms in [0..3, 3..5] {
            let line = math::unit(math::sub(rigid[atoms.end - 1], rigid[atoms.start])).unwrap();
            let now = math::unit(math::sub(held[atoms.end - 1], held[atoms.start])).unwrap();
            assert!(
                math::norm(math::cross(line, now)) < 1e-9,
                "{atoms:?} along its line"
            );
            assert!(math::dot(line, now) > 0.0, "{atoms:?} not reversed");
            assert!(same_shape(&held, &relaxed, atoms), "kept its shape");
        }
        // The bent CO2's carbon: turned back by the smallest turn laying the molecule on its
        // line (x), not spun about it.
        let back = math::mat_mul(
            &math::align(math::mat_vec(&turn, [1.0, 0.0, 0.0]), [1.0, 0.0, 0.0]),
            &turn,
        );
        let expect = math::mat_vec(&back, math::sub(bent[1], centroid(&bent[..3])));
        let off = math::sub(held[1], centroid(&held[..3]));
        assert!(
            math::distance(off, expect) < 1e-9,
            "{off:?}, not {expect:?}"
        );
        assert!(math::norm(off) > 0.05, "still bent");
        assert!(
            math::distance(held[5], rigid[5]) < 1e-12,
            "the lone atom back in place"
        );
    }
}
