//! What a scan can vary: a number of the assembly's design (a step's parameter, a cell
//! constant, a hand-placed molecule's pose) or a picked bond length, angle or dihedral of the
//! 3D model — and how a value is read from and written into each.

use std::fmt;

use crate::assembly::model::{Design, Step};
use crate::assembly::place;
use crate::three_d::edit;
use crate::three_d::geom::{self, Graph, Vec3};
use crate::three_d::{Constraint, Model3d};

/// One number of a building step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepField {
    RingRadius,
    RingTilt,
    StackRise,
    StackTwist,
    StackSlip,
    /// The array's offset along x, y or z.
    ArrayOffset(usize),
    /// The array's per-copy rotation about x, y or z.
    ArrayEuler(usize),
    /// The move/rotate step's offset along x, y or z.
    TransformOffset(usize),
    /// The move/rotate step's rotation about x, y or z.
    TransformEuler(usize),
    CellA,
    CellB,
    CellC,
    CellAlpha,
    CellBeta,
    CellGamma,
}

const XYZ: [&str; 3] = ["x", "y", "z"];

impl StepField {
    /// The fields `step` has, in panel order.
    pub fn of(step: &Step) -> Vec<StepField> {
        use StepField::*;
        match step {
            Step::Ring(_) => vec![RingRadius, RingTilt],
            Step::Stack(_) => vec![StackRise, StackTwist, StackSlip],
            Step::Array(_) => (0..3)
                .map(ArrayOffset)
                .chain((0..3).map(ArrayEuler))
                .collect(),
            Step::Lattice(_) => vec![CellA, CellB, CellC, CellAlpha, CellBeta, CellGamma],
            Step::Transform(_) => (0..3)
                .map(TransformOffset)
                .chain((0..3).map(TransformEuler))
                .collect(),
        }
    }

    pub fn label(self) -> String {
        use StepField::*;
        match self {
            RingRadius => t!("scan.param.ring_radius").into(),
            RingTilt => t!("scan.param.ring_tilt").into(),
            StackRise => t!("scan.param.stack_rise").into(),
            StackTwist => t!("scan.param.stack_twist").into(),
            StackSlip => t!("scan.param.stack_slip").into(),
            ArrayOffset(i) | TransformOffset(i) => t!("scan.param.array_offset", axis = XYZ[i]),
            ArrayEuler(i) | TransformEuler(i) => t!("scan.param.array_rotation", axis = XYZ[i]),
            CellA => "a".into(),
            CellB => "b".into(),
            CellC => "c".into(),
            CellAlpha => "α".into(),
            CellBeta => "β".into(),
            CellGamma => "γ".into(),
        }
    }

    pub fn is_angle(self) -> bool {
        use StepField::*;
        matches!(
            self,
            RingTilt
                | StackTwist
                | ArrayEuler(_)
                | TransformEuler(_)
                | CellAlpha
                | CellBeta
                | CellGamma
        )
    }

    /// The field's slot in `step`, or `None` when the step is of another kind.
    fn slot(self, step: &mut Step) -> Option<&mut f64> {
        use StepField::*;
        Some(match (self, step) {
            (RingRadius, Step::Ring(s)) => &mut s.radius,
            (RingTilt, Step::Ring(s)) => &mut s.tilt_deg,
            (StackRise, Step::Stack(s)) => &mut s.rise,
            (StackTwist, Step::Stack(s)) => &mut s.twist_deg,
            (StackSlip, Step::Stack(s)) => &mut s.slip,
            (ArrayOffset(i), Step::Array(s)) => s.offset.get_mut(i)?,
            (ArrayEuler(i), Step::Array(s)) => s.euler_deg.get_mut(i)?,
            (TransformOffset(i), Step::Transform(s)) => s.offset.get_mut(i)?,
            (TransformEuler(i), Step::Transform(s)) => s.euler_deg.get_mut(i)?,
            (CellA, Step::Lattice(l)) => &mut l.cell.a,
            (CellB, Step::Lattice(l)) => &mut l.cell.b,
            (CellC, Step::Lattice(l)) => &mut l.cell.c,
            (CellAlpha, Step::Lattice(l)) => &mut l.cell.alpha,
            (CellBeta, Step::Lattice(l)) => &mut l.cell.beta,
            (CellGamma, Step::Lattice(l)) => &mut l.cell.gamma,
            _ => return None,
        })
    }
}

/// One number of a hand-placed molecule's pose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoseField {
    /// The centroid's x, y or z (Å).
    Position(usize),
    /// The Euler angle about x, y or z (degrees, as the 配置 panel shows them).
    Rotation(usize),
}

impl PoseField {
    pub const ALL: [PoseField; 6] = [
        PoseField::Position(0),
        PoseField::Position(1),
        PoseField::Position(2),
        PoseField::Rotation(0),
        PoseField::Rotation(1),
        PoseField::Rotation(2),
    ];

    pub fn label(self) -> String {
        match self {
            PoseField::Position(i) => t!("scan.param.pose_position", axis = XYZ[i]),
            PoseField::Rotation(i) => t!("scan.param.pose_rotation", axis = XYZ[i]),
        }
    }
}

/// A scanned parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    /// A number of building step `step` (an index into [`Design::steps`]).
    Step { step: usize, field: StepField },
    /// A number of hand-placed molecule `index` (into [`Design::manual`]).
    Pose { index: usize, field: PoseField },
    /// A bond length of the 3D model: the smaller side slides along the bond.
    Bond([usize; 2]),
    /// A bond angle of the 3D model, at the middle atom.
    Angle([usize; 3]),
    /// A dihedral of the 3D model, about the middle bond.
    Dihedral([usize; 4]),
}

/// Why a value could not be put into the design or the model.
#[derive(Debug, Clone, PartialEq)]
pub struct ApplyError(pub String);

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ApplyError {}

impl Param {
    /// Whether the parameter belongs to the assembly (else to the 3D model).
    pub fn is_assembly(&self) -> bool {
        matches!(self, Param::Step { .. } | Param::Pose { .. })
    }

    /// The parameter of the 3D model the picked atoms name, if they name a settable one.
    pub fn of_picked(picked: &[usize], graph: &Graph) -> Option<Param> {
        match edit::Target::of(picked, graph) {
            edit::Target::BondLength(a) => Some(Param::Bond(a)),
            edit::Target::Angle(a) => Some(Param::Angle(a)),
            edit::Target::Dihedral(a) => Some(Param::Dihedral(a)),
            _ => None,
        }
    }

    /// Å or °.
    pub fn unit(&self) -> &'static str {
        let angle = match self {
            Param::Step { field, .. } => field.is_angle(),
            Param::Pose { field, .. } => matches!(field, PoseField::Rotation(_)),
            Param::Bond(_) => false,
            Param::Angle(_) | Param::Dihedral(_) => true,
        };
        if angle { "°" } else { "Å" }
    }

    /// The parameter's name, for the axis and the CSV header. `design` names the step;
    /// `model` the atoms (both optional: without them a generic name is given).
    pub fn label(&self, design: Option<&Design>, model: Option<&Model3d>) -> String {
        let atoms = |list: &[usize]| {
            list.iter()
                .map(|&a| match model {
                    Some(m) if a < m.atoms.len() => edit::atom_label(m, a),
                    _ => (a + 1).to_string(),
                })
                .collect::<Vec<_>>()
                .join("–")
        };
        match self {
            Param::Step { step, field } => {
                let name = design
                    .and_then(|d| d.steps.get(*step))
                    .map_or(t!("scan.param.step"), Step::label);
                format!("{} {}: {}", step + 1, name, field.label())
            }
            Param::Pose { index, field } => {
                t!("scan.param.pose", number = index + 1, field = field.label())
            }
            Param::Bond(a) => t!("scan.param.bond", atoms = atoms(a)),
            Param::Angle(a) => t!("scan.param.angle", atoms = atoms(a)),
            Param::Dihedral(a) => t!("scan.param.dihedral", atoms = atoms(a)),
        }
    }

    /// The parameter's value in `design` now.
    pub fn read_design(&self, design: &Design) -> Option<f64> {
        match *self {
            Param::Step { step, field } => {
                let mut step = design.steps.get(step)?.clone();
                field.slot(&mut step).map(|v| *v)
            }
            Param::Pose { index, field } => {
                let (pose, _) = pose(design, index)?;
                Some(match field {
                    PoseField::Position(i) => pose.position[i],
                    PoseField::Rotation(i) => pose.euler_deg[i],
                })
            }
            _ => None,
        }
    }

    /// Put `value` into `design`.
    pub fn write_design(&self, design: &mut Design, value: f64) -> Result<(), ApplyError> {
        let missing = || ApplyError(t!("scan.error.item_missing").into());
        match *self {
            Param::Step { step, field } => {
                let slot = design
                    .steps
                    .get_mut(step)
                    .and_then(|s| field.slot(s))
                    .ok_or_else(missing)?;
                *slot = value;
                Ok(())
            }
            Param::Pose { index, field } => {
                let (mut pose, local) = pose(design, index).ok_or_else(missing)?;
                match field {
                    PoseField::Position(i) => pose.position[i] = value,
                    PoseField::Rotation(i) => pose.euler_deg[i] = value,
                }
                design.manual[index].transform = place::transform_of(&pose, local);
                Ok(())
            }
            _ => Err(ApplyError(t!("scan.error.model_param").into())),
        }
    }

    /// The parameter's value in the 3D model's `coords`.
    pub fn read_coords(&self, coords: &[Vec3]) -> Option<f64> {
        let p = |i: usize| coords.get(i).copied();
        Some(match *self {
            Param::Bond([a, b]) => geom::distance(p(a)?, p(b)?),
            Param::Angle([a, b, c]) => geom::angle(p(a)?, p(b)?, p(c)?),
            Param::Dihedral([a, b, c, d]) => geom::dihedral(p(a)?, p(b)?, p(c)?, p(d)?),
            _ => return None,
        })
    }

    /// `coords` with the parameter set to `value`, moving the smaller side as the 編集 tools
    /// do (never an atom of `pinned`: the anchor, or the atoms a relaxed scan holds fixed).
    pub fn write_coords(
        &self,
        coords: &[Vec3],
        graph: &Graph,
        value: f64,
        pinned: &[usize],
    ) -> Result<Vec<Vec3>, ApplyError> {
        let result = match *self {
            Param::Bond([a, b]) => {
                geom::set_bond_length_avoiding(coords, graph, a, b, value, pinned)
            }
            Param::Angle(atoms) => geom::set_angle_avoiding(coords, graph, atoms, value, pinned),
            Param::Dihedral(atoms) => {
                geom::set_dihedral_avoiding(coords, graph, atoms, value, pinned)
            }
            _ => return Err(ApplyError(t!("scan.error.assembly_param").into())),
        };
        result.map_err(|e| ApplyError(e.to_string()))
    }

    /// Whether every side that setting the 3D model's parameter could move holds an atom of
    /// `pinned`, so it cannot be set without moving one (false for the assembly's, and for one
    /// that cannot be set for another reason).
    pub fn blocked_by(&self, graph: &Graph, pinned: &[usize]) -> bool {
        let side = match *self {
            Param::Bond([a, b]) => geom::moving_side_avoiding(graph, a, b, pinned).err(),
            Param::Angle(atoms) => geom::swinging_arm(graph, atoms, pinned).err(),
            Param::Dihedral([_, b, c, _]) => geom::moving_side_avoiding(graph, b, c, pinned).err(),
            _ => None,
        };
        side == Some(geom::EditError::AnchorBlocks)
    }

    /// The restraint holding the 3D model's parameter at `value` through an optimisation (none
    /// for the assembly's).
    pub fn restraint(&self, value: f64) -> Option<Constraint> {
        match *self {
            Param::Bond(atoms) => Some(Constraint::Distance(atoms, value)),
            Param::Angle(atoms) => Some(Constraint::Angle(atoms, value)),
            Param::Dihedral(atoms) => Some(Constraint::Torsion(atoms, value)),
            _ => None,
        }
    }

    /// A sensible default range around `current`: ±1 Å for a length (positive), the full turn
    /// for a dihedral, ±20° for an angle.
    pub fn default_range(&self, current: f64) -> (f64, f64) {
        match self {
            Param::Dihedral(_) => (-180.0, 180.0),
            Param::Angle(_) => ((current - 20.0).max(1.0), (current + 20.0).min(179.0)),
            Param::Bond(_) => ((current - 0.3).max(0.5), current + 0.5),
            _ if self.unit() == "°" => (current - 30.0, current + 30.0),
            _ => ((current - 1.0).max(0.0), current + 1.0),
        }
    }
}

/// Hand-placed molecule `index`'s pose and its component's local centroid.
fn pose(design: &Design, index: usize) -> Option<(place::Pose, Vec3)> {
    let inst = design.manual.get(index)?;
    let component = design.components.get(inst.component)?;
    let local = place::centroid(&place::local_positions(component));
    Some((place::pose_of(&inst.transform, local), local))
}

/// Every parameter of `design` that can be scanned: each enabled step's numbers, then the
/// pose of hand-placed molecule `selected_manual` (the one selected in the viewer).
pub fn design_params(design: &Design, selected_manual: Option<usize>) -> Vec<Param> {
    let mut out: Vec<Param> = design
        .steps
        .iter()
        .enumerate()
        .flat_map(|(step, s)| {
            StepField::of(s)
                .into_iter()
                .map(move |field| Param::Step { step, field })
        })
        .collect();
    if let Some(index) = selected_manual.filter(|&k| k < design.manual.len()) {
        out.extend(
            PoseField::ALL
                .into_iter()
                .map(|field| Param::Pose { index, field }),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::math::Rigid;
    use crate::assembly::model::{Component, Instance};
    use crate::three_d::{Atom3d, Bond3d};

    fn component() -> Component {
        let atom = |e: &str, pos| Atom3d {
            element: e.into(),
            pos,
        };
        Component {
            name: "x".into(),
            atoms: vec![atom("C", [0.0, 0.0, 0.0]), atom("O", [1.2, 0.0, 0.0])],
            bonds: vec![Bond3d {
                begin: 0,
                end: 1,
                order: 2,
            }],
            orient: None,
        }
    }

    fn design() -> Design {
        Design {
            components: vec![component()],
            steps: vec![
                Step::new_ring(),
                Step::new_stack(),
                Step::new_array(),
                Step::new_lattice(),
                Step::new_transform(),
            ],
            manual: vec![Instance {
                component: 0,
                transform: Rigid::translation([1.0, 2.0, 3.0]),
            }],
            ..Default::default()
        }
    }

    /// Every assembly parameter reads back the value written into it, and writing one leaves
    /// the others alone.
    #[test]
    fn every_assembly_parameter_round_trips() {
        let base = design();
        let params = design_params(&base, Some(0));
        assert_eq!(params.len(), 2 + 3 + 6 + 6 + 6 + 6);
        for (n, param) in params.iter().enumerate() {
            let mut d = base.clone();
            let value = 37.0 + n as f64 * 0.25;
            param.write_design(&mut d, value).unwrap();
            let back = param.read_design(&d).unwrap();
            assert!((back - value).abs() < 1e-9, "{param:?}: {back}");
            for other in params.iter().filter(|p| *p != param) {
                let (a, b) = (other.read_design(&base), other.read_design(&d));
                let (a, b) = (a.unwrap(), b.unwrap());
                // Euler angles of a pose are coupled only through gimbal lock; 37° is not.
                assert!((a - b).abs() < 1e-6, "{param:?} changed {other:?}");
            }
            assert!(!param.label(Some(&d), None).is_empty());
        }
    }

    /// Scanning a move/rotate step's numbers moves the built molecules.
    #[test]
    fn a_transform_step_can_be_scanned() {
        let d = Design {
            components: vec![component()],
            steps: vec![Step::new_transform()],
            ..Default::default()
        };
        let params = design_params(&d, None);
        let fields: Vec<StepField> = params
            .iter()
            .map(|p| match p {
                Param::Step { field, .. } => *field,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            fields,
            [
                StepField::TransformOffset(0),
                StepField::TransformOffset(1),
                StepField::TransformOffset(2),
                StepField::TransformEuler(0),
                StepField::TransformEuler(1),
                StepField::TransformEuler(2),
            ]
        );
        assert_eq!(params[2].unit(), "Å");
        assert_eq!(params[5].unit(), "°");
        let origin = |d: &Design| d.instances()[0].transform.apply([0.0; 3]);
        let mut moved = d.clone();
        params[2].write_design(&mut moved, 4.0).unwrap();
        assert!(geom::distance(origin(&moved), [0.0, 0.0, 4.0]) < 1e-9);
        // Turned 180° about z through the centroid (0.6, 0, 0): the carbon lands at 1.2.
        params[5].write_design(&mut moved, 180.0).unwrap();
        assert!(geom::distance(origin(&moved), [1.2, 0.0, 4.0]) < 1e-9);
        assert_eq!(params[5].read_design(&moved), Some(180.0));
    }

    #[test]
    fn a_missing_step_is_an_error_not_a_panic() {
        let mut d = design();
        let p = Param::Step {
            step: 9,
            field: StepField::RingRadius,
        };
        assert!(p.write_design(&mut d, 1.0).is_err());
        let wrong_kind = Param::Step {
            step: 0,
            field: StepField::StackRise,
        };
        assert!(wrong_kind.write_design(&mut d, 1.0).is_err());
        assert!(
            Param::Dihedral([0, 1, 2, 3])
                .write_design(&mut d, 1.0)
                .is_err()
        );
    }

    /// A dihedral written into a chain's coordinates is read back, for any target.
    #[test]
    fn a_dihedral_round_trips_through_the_coordinates() {
        // Butane's carbons with one hydrogen on each end, zig-zag.
        let coords = vec![
            [0.0, 0.0, 0.0],
            [1.5, 0.0, 0.0],
            [2.0, 1.4, 0.0],
            [3.5, 1.4, 0.0],
            [-0.5, -0.9, 0.3],
            [4.0, 2.3, 0.3],
        ];
        let graph = Graph::new(6, [(0, 1), (1, 2), (2, 3), (0, 4), (3, 5)]);
        let param = Param::Dihedral([0, 1, 2, 3]);
        for target in [-180.0, -95.0, 0.0, 60.0, 179.0] {
            let next = param.write_coords(&coords, &graph, target, &[]).unwrap();
            let back = param.read_coords(&next).unwrap();
            assert!(geom::wrap_degrees(back - target).abs() < 1e-6, "{back}");
        }
        let bond = Param::Bond([1, 2]);
        let next = bond.write_coords(&coords, &graph, 1.7, &[]).unwrap();
        assert!((bond.read_coords(&next).unwrap() - 1.7).abs() < 1e-9);
        let angle = Param::Angle([0, 1, 2]);
        let next = angle.write_coords(&coords, &graph, 100.0, &[]).unwrap();
        assert!((angle.read_coords(&next).unwrap() - 100.0).abs() < 1e-6);
        assert_eq!(
            Param::of_picked(&[0, 1, 2, 3], &graph),
            Some(Param::Dihedral([0, 1, 2, 3]))
        );
        assert_eq!(Param::of_picked(&[0, 3], &graph), None, "not bonded");
    }
}
