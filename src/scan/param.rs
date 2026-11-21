//! What a scan can vary: a number of the assembly's design (a node's input, such as a step's
//! parameter or a cell constant, a number node, a custom node's input, a hand-placed
//! molecule's pose) or a picked bond length, angle or dihedral of the 3D model — and how a
//! value is read from and written into each.

use std::fmt;

pub use crate::assembly::graph::StepField;
use crate::assembly::graph::{self, InKey, NodeId, NodeKind, SocketType, chain_step_id};
use crate::assembly::model::{Design, node_name};
use crate::assembly::place;
use crate::three_d::edit;
use crate::three_d::geom::{self, Graph, Vec3};
use crate::three_d::{Constraint, Model3d};

const XYZ: [&str; 3] = ["x", "y", "z"];

/// A step field's name in the scan's words ("Offset x").
fn field_label(field: StepField, axis: Option<usize>) -> String {
    use StepField::*;
    let axis_name = XYZ[axis.unwrap_or(0).min(2)];
    match field {
        RingRadius => t!("scan.param.ring_radius").into(),
        RingTilt => t!("scan.param.ring_tilt").into(),
        StackRise => t!("scan.param.stack_rise").into(),
        StackTwist => t!("scan.param.stack_twist").into(),
        StackSlip => t!("scan.param.stack_slip").into(),
        ArrayOffset | TransformOffset => t!("scan.param.array_offset", axis = axis_name),
        ArrayEuler | TransformEuler => t!("scan.param.array_rotation", axis = axis_name),
        BendRadius => t!("scan.param.bend_radius").into(),
        BendDirection => t!("scan.param.bend_direction").into(),
        other => other.axis_label(axis),
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
    /// The value of input `key` of node `node` of the design's graph (top level), one axis of
    /// it for a triple. `InKey::Value` is a number or vector node's own value.
    Input {
        node: NodeId,
        key: InKey,
        axis: Option<u8>,
    },
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

/// The value of input `key` (axis `axis`) of a node of kind `kind`.
fn read_kind(kind: &NodeKind, key: InKey, axis: Option<usize>) -> Option<f64> {
    match (kind, key) {
        (NodeKind::Value { value }, InKey::Value) => Some(*value),
        (NodeKind::Vector { value }, InKey::Value) => Some(value[axis?.min(2)]),
        _ => kind.inline(key).map(|v| v.component(axis)),
    }
}

/// Set input `key` (axis `axis`) of a node of kind `kind`.
fn write_kind(kind: &mut NodeKind, key: InKey, axis: Option<usize>, value: f64) -> bool {
    match (&mut *kind, key) {
        (NodeKind::Value { value: v }, InKey::Value) => {
            *v = value;
            true
        }
        (NodeKind::Vector { value: v }, InKey::Value) => match axis {
            Some(i) => {
                v[i.min(2)] = value;
                true
            }
            None => false,
        },
        _ => match kind.inline(key) {
            Some(old) => kind.set_inline(key, old.with_component(axis, value)),
            None => false,
        },
    }
}

impl Param {
    /// Field `field` of step `k` of a design's steps (the node the chain makes of it).
    pub fn step(k: usize, field: StepField) -> Param {
        Param::Input {
            node: chain_step_id(k),
            key: InKey::Field(field),
            axis: None,
        }
    }

    /// Axis `axis` of field `field` (a triple) of step `k`.
    pub fn step_axis(k: usize, field: StepField, axis: usize) -> Param {
        Param::Input {
            node: chain_step_id(k),
            key: InKey::Field(field),
            axis: Some(axis as u8),
        }
    }

    /// Whether the parameter belongs to the assembly (else to the 3D model).
    pub fn is_assembly(&self) -> bool {
        matches!(self, Param::Input { .. } | Param::Pose { .. })
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

    /// Å, °, or nothing (a plain number).
    pub fn unit(&self) -> &'static str {
        let angle = match self {
            Param::Input { key, .. } => match key {
                InKey::Field(f) if f.is_angle() => true,
                InKey::Field(_) => false,
                _ => return "",
            },
            Param::Pose { field, .. } => matches!(field, PoseField::Rotation(_)),
            Param::Bond(_) => false,
            Param::Angle(_) | Param::Dihedral(_) => true,
        };
        if angle { "°" } else { "Å" }
    }

    /// The parameter's name, for the axis and the CSV header. `design` names the node;
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
            Param::Input { node, key, axis } => {
                let axis = axis.map(usize::from);
                let field = match key {
                    InKey::Field(f) => field_label(*f, axis),
                    InKey::Value => axis.map_or_else(String::new, |i| XYZ[i.min(2)].to_owned()),
                    other => design
                        .and_then(|d| {
                            let g = d.effective_graph();
                            let n = g.node(*node)?;
                            let decl = n
                                .kind
                                .inputs(&d.defs, None)
                                .into_iter()
                                .find(|i| i.key == *other)?;
                            Some(match axis {
                                Some(i) if decl.ty == SocketType::Vec3 => {
                                    format!("{} {}", decl.label, XYZ[i.min(2)])
                                }
                                _ => decl.label,
                            })
                        })
                        .unwrap_or_default(),
                };
                let name = design.and_then(|d| {
                    if d.is_linear() {
                        let k = node.0.checked_sub(2)? as usize;
                        let step = d.steps.get(k)?;
                        return Some(format!("{} {}", k + 1, step.label()));
                    }
                    let g = d.effective_graph();
                    g.node(*node)?;
                    Some(node_name(&g, &d.defs, &[], *node))
                });
                let name = name.unwrap_or_else(|| t!("scan.param.step").to_owned());
                if field.is_empty() {
                    name
                } else {
                    format!("{name}: {field}")
                }
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
            Param::Input { node, key, axis } => {
                let axis = axis.map(usize::from);
                if design.is_linear() {
                    let step = design.steps.get(node.0.checked_sub(2)? as usize)?;
                    let InKey::Field(field) = key else {
                        return None;
                    };
                    return field.read(step).map(|v| v.component(axis));
                }
                let g = design.graph.as_ref()?;
                read_kind(&g.node(node)?.kind, key, axis)
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

    /// Put `value` into `design`. A design still made of steps stays so.
    pub fn write_design(&self, design: &mut Design, value: f64) -> Result<(), ApplyError> {
        let missing = || ApplyError(t!("scan.error.item_missing").into());
        match *self {
            Param::Input { node, key, axis } => {
                let axis = axis.map(usize::from);
                if design.is_linear() {
                    let k = node.0.checked_sub(2).ok_or_else(missing)? as usize;
                    let step = design.steps.get_mut(k).ok_or_else(missing)?;
                    let InKey::Field(field) = key else {
                        return Err(missing());
                    };
                    let old = field.read(step).ok_or_else(missing)?;
                    field.write(step, old.with_component(axis, value));
                    return Ok(());
                }
                let n = design
                    .graph
                    .as_mut()
                    .and_then(|g| g.node_mut(node))
                    .ok_or_else(missing)?;
                if write_kind(&mut n.kind, key, axis, value) {
                    Ok(())
                } else {
                    Err(missing())
                }
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
            _ if self.unit().is_empty() => (current - 1.0, current + 1.0),
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

/// Every parameter of `design` that can be scanned, then the pose of hand-placed molecule
/// `selected_manual` (the one selected in the viewer). For a design made of steps: each step's
/// numbers. For a graph: the number and vector nodes, then the unlinked number inputs of the
/// other nodes that feed the assembly (counts left out: they take whole values).
pub fn design_params(design: &Design, selected_manual: Option<usize>) -> Vec<Param> {
    let mut out = Vec::new();
    let mut push = |node: NodeId, key: InKey, ty: SocketType| match ty {
        SocketType::Number => out.push(Param::Input {
            node,
            key,
            axis: None,
        }),
        SocketType::Vec3 => out.extend((0..3).map(|i| Param::Input {
            node,
            key,
            axis: Some(i),
        })),
        SocketType::Set | SocketType::Curve | SocketType::Points => {}
    };
    match &design.graph {
        None => {
            for (k, step) in design.steps.iter().enumerate() {
                for field in StepField::of(step) {
                    if !field.is_count() {
                        push(chain_step_id(k), InKey::Field(field), field.ty());
                    }
                }
            }
        }
        Some(g) => {
            let ev = design.evaluation();
            let rank = |kind: &NodeKind| match kind {
                NodeKind::Value { .. } | NodeKind::Vector { .. } => 0,
                NodeKind::Group(_) => 1,
                _ => 2,
            };
            let mut nodes: Vec<&graph::Node> =
                g.nodes.iter().filter(|n| ev.reached(&[], n.id)).collect();
            nodes.sort_by_key(|n| (rank(&n.kind), n.id));
            for n in nodes {
                match &n.kind {
                    NodeKind::Value { .. } => push(n.id, InKey::Value, SocketType::Number),
                    NodeKind::Vector { .. } => push(n.id, InKey::Value, SocketType::Vec3),
                    kind => {
                        for decl in kind.inputs(&design.defs, None) {
                            let count = matches!(decl.key, InKey::Count)
                                || matches!(decl.key, InKey::Field(f) if f.is_count());
                            if !count && g.link_into(n.id, decl.key).is_none() {
                                push(n.id, decl.key, decl.ty);
                            }
                        }
                    }
                }
            }
        }
    }
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
    use crate::assembly::graph::{OutKey, ops};
    use crate::assembly::math::Rigid;
    use crate::assembly::model::{Component, Instance, Step};
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
                Step::new_bend(),
            ],
            manual: vec![Instance {
                component: 0,
                transform: Rigid::translation([1.0, 2.0, 3.0]),
            }],
            ..Default::default()
        }
    }

    /// Every assembly parameter reads back the value written into it, and writing one leaves
    /// the others alone — for a design of steps and for the same design as a graph.
    #[test]
    fn every_assembly_parameter_round_trips() {
        let linear = design();
        let mut as_graph = linear.clone();
        as_graph.graph_mut();
        for base in [linear, as_graph] {
            let params = design_params(&base, Some(0));
            assert_eq!(params.len(), 2 + 3 + 6 + 6 + 6 + 2 + 6, "{params:?}");
            for (n, param) in params.iter().enumerate() {
                let mut d = base.clone();
                let value = 37.0 + n as f64 * 0.25;
                param.write_design(&mut d, value).unwrap();
                assert_eq!(d.is_linear(), base.is_linear());
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
        let expected: Vec<Param> = (0..3)
            .map(|i| Param::step_axis(0, StepField::TransformOffset, i))
            .chain((0..3).map(|i| Param::step_axis(0, StepField::TransformEuler, i)))
            .collect();
        assert_eq!(params, expected);
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
        assert!(params[0].label(Some(&d), None).starts_with("1 "));
    }

    /// A number node feeding a stack's rise is a parameter of its own; the rise it feeds is
    /// not offered.
    #[test]
    fn a_value_node_can_be_scanned() {
        let mut d = Design {
            components: vec![component()],
            steps: vec![Step::new_stack()],
            ..Default::default()
        };
        let g = d.graph_mut();
        let v = g.add(NodeKind::Value { value: 3.5 }, [0.0; 2]);
        ops::connect(
            g,
            &[],
            None,
            v,
            OutKey::Number,
            chain_step_id(0),
            InKey::Field(StepField::StackRise),
        )
        .unwrap();
        let params = design_params(&d, None);
        assert_eq!(
            params[0],
            Param::Input {
                node: v,
                key: InKey::Value,
                axis: None
            }
        );
        assert!(!params.contains(&Param::step(0, StepField::StackRise)));
        assert_eq!(params[0].unit(), "");
        let top = |d: &Design| d.instances().last().unwrap().transform.apply([0.0; 3])[2];
        params[0].write_design(&mut d, 4.0).unwrap();
        assert!((top(&d) - 36.0).abs() < 1e-9);
    }

    #[test]
    fn a_missing_step_is_an_error_not_a_panic() {
        let mut d = design();
        assert!(
            Param::step(9, StepField::RingRadius)
                .write_design(&mut d, 1.0)
                .is_err()
        );
        let wrong_kind = Param::step(0, StepField::StackRise);
        assert!(wrong_kind.write_design(&mut d, 1.0).is_err());
        assert!(
            Param::Dihedral([0, 1, 2, 3])
                .write_design(&mut d, 1.0)
                .is_err()
        );
        let mut g = design();
        g.graph_mut();
        assert!(
            Param::step(9, StepField::RingRadius)
                .write_design(&mut g, 1.0)
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
