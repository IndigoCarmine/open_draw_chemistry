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
    /// molecule is placed and the atoms that hold it there (see [`Placed`]).
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
        let spanning: Vec<Vec<usize>> = design
            .components
            .iter()
            .map(|c| {
                c.oriented_positions()
                    .map(|p| spanning_atoms(&p))
                    .unwrap_or_default()
            })
            .collect();
        let mut held = Vec::new();
        let mut molecules = Vec::with_capacity(placements.len());
        for (m, placement) in placements.into_iter().enumerate() {
            let atoms = built.molecule_atoms(m);
            let span = &spanning[built.component_of_molecule[m]];
            held.extend(span.iter().map(|&k| atoms.start + k));
            molecules.push((placement, atoms));
        }
        Ok(Placed {
            point: self.point_of(built),
            molecules,
            held,
        })
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
/// constant, a hand-placed molecule's pose) decide — while its shape relaxes: OpenBabel can fix
/// atoms but not a centroid or an orientation, so three atoms spanning each molecule (see
/// [`spanning_atoms`]) are held where the rigid structure has them, which fixes its frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    /// The structure built rigidly.
    pub point: Point,
    /// Each molecule's placement (the transform of its component's oriented atoms) and atoms.
    molecules: Vec<(Rigid, Range<usize>)>,
    /// The atoms held: those spanning each molecule.
    pub held: Vec<usize>,
}

impl Placed {
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
    /// this point has the molecules, the held atoms exactly where the rigid structure has them;
    /// `None` unless this point is [`Point::Coords`] of as many atoms.
    pub fn place(&self, local: &[Vec3]) -> Option<Vec<Vec3>> {
        let Point::Coords(rigid) = &self.point else {
            return None;
        };
        if local.len() != rigid.len() {
            return None;
        }
        let mut coords = local.to_vec();
        for (placement, atoms) in &self.molecules {
            for p in coords.get_mut(atoms.clone()).into_iter().flatten() {
                *p = placement.apply(*p);
            }
        }
        for &a in &self.held {
            coords[a] = rigid[a];
        }
        Some(coords)
    }
}

/// How far (Å) an atom must be off the line through two others for the three to span a plane.
const STRAIGHT: f64 = 0.05;

/// Of a molecule's atoms at `points`, those whose places fix its frame: the atom farthest from
/// the centroid, the one farthest from that, and the one farthest from the line through both —
/// spread out, so that holding them restricts the shape the least. A straight molecule gives
/// the two ends (turning it about its line changes nothing), one of fewer atoms all of them.
pub fn spanning_atoms(points: &[Vec3]) -> Vec<usize> {
    if points.len() <= 2 {
        return (0..points.len()).collect();
    }
    // The atom farthest by `far`, the earliest on a tie.
    let farthest = |far: &dyn Fn(Vec3) -> f64| {
        (0..points.len()).fold((0, f64::NEG_INFINITY), |best, i| {
            let d = far(points[i]);
            if d > best.1 { (i, d) } else { best }
        })
    };
    let centre = crate::assembly::place::centroid(points);
    let (a, _) = farthest(&|p| math::distance(p, centre));
    let (b, _) = farthest(&|p| math::distance(p, points[a]));
    let Some(axis) = math::unit(math::sub(points[b], points[a])) else {
        // Every atom in one place.
        return vec![a];
    };
    let (c, off) = farthest(&|p| math::norm(math::cross(math::sub(p, points[a]), axis)));
    if off > STRAIGHT {
        vec![a, b, c]
    } else {
        vec![a, b]
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

    #[test]
    fn spanning_atoms_are_spread_out_and_fix_the_frame() {
        let points: Vec<Vec3> = component().atoms.iter().map(|a| a.pos).collect();
        let span = spanning_atoms(&points);
        assert_eq!(span.len(), 3, "{span:?}");
        // The two ends, and the one off their line.
        assert!(span.contains(&2) && span.contains(&3), "{span:?}");
        // A straight molecule: its two ends; two atoms or one: all of them.
        let line = [[0.0, 0.0, 0.0], [1.2, 0.0, 0.0], [2.4, 0.0, 0.0]];
        let mut ends = spanning_atoms(&line);
        ends.sort();
        assert_eq!(ends, vec![0, 2]);
        assert_eq!(spanning_atoms(&line[..2]), vec![0, 1]);
        assert_eq!(spanning_atoms(&line[..1]), vec![0]);
        assert!(spanning_atoms(&[]).is_empty());
    }

    /// Of every molecule — the steps' copies and the hand-placed one — three atoms are held,
    /// where the rigid structure has them; and the molecules' shapes taken from one point are
    /// moved rigidly to where another point places the molecules.
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
        assert_eq!(at.held.len(), 4 * 3, "three of each of the four molecules");
        for m in 0..4 {
            let held = at.held.iter().filter(|&&a| (4 * m..4 * m + 4).contains(&a));
            assert_eq!(held.count(), 3, "molecule {m}");
        }
        // Each molecule's free atom and one of its held ones.
        let free_and_held = |m: usize| {
            let atoms = 4 * m..4 * m + 4;
            let free = atoms.clone().find(|a| !at.held.contains(a)).unwrap();
            let held = atoms.clone().find(|a| at.held.contains(a)).unwrap();
            (free, held)
        };
        // A shape: every molecule's free atom pushed away from its held one; the held atoms
        // a little off (the minimiser keeps them, but not to the last digit).
        let mut relaxed = rigid.clone();
        for m in 0..4 {
            let (free, held) = free_and_held(m);
            let away = math::sub(rigid[free], rigid[held]);
            relaxed[free] = math::add(rigid[free], math::scale(away, 0.1));
        }
        for &a in &at.held {
            relaxed[a] = math::add(relaxed[a], [1e-7, 0.0, 0.0]);
        }
        let shapes = at.to_local(&relaxed);
        let there = source.placed_at(&[4.2, 35.0]).unwrap();
        let Point::Coords(rigid_there) = &there.point else {
            panic!("the same atoms")
        };
        assert_eq!(there.held, at.held);
        let moved = there.place(&shapes).unwrap();
        for &a in &there.held {
            assert_eq!(
                moved[a], rigid_there[a],
                "held atom {a} not where it is built"
            );
        }
        for m in 0..4 {
            // The free atom is where the molecule's placement there takes the shape.
            let (free, held) = free_and_held(m);
            let away = math::sub(rigid_there[free], rigid_there[held]);
            let expect = math::add(rigid_there[free], math::scale(away, 0.1));
            let off = math::distance(moved[free], expect);
            assert!(off < 1e-9, "molecule {m}: {off}");
            assert!(math::distance(moved[free], rigid_there[free]) > 0.05);
        }
        assert_eq!(there.place(&shapes[1..]), None, "another number of atoms");
    }
}
