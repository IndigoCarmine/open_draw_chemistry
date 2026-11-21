//! What each kind of node is: its settings, the sockets it takes and gives, and the values of
//! its inputs that no link feeds.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::defs::{DefId, NodeDef};
use super::field::StepField;
use super::{InKey, OutKey};
use crate::assembly::math::Vec3;
use crate::assembly::model::Step;

/// What a socket carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SocketType {
    /// Molecules: placed copies of the monomers.
    Set,
    Number,
    /// Three numbers (x, y, z).
    Vec3,
    /// Curves: lines through points, open or closed.
    Curve,
    /// Points, each turned (a frame to put a copy in).
    Points,
}

impl SocketType {
    /// Whether an output of this type may feed an input of `to`: the same type, or a number
    /// into a triple (all three set to it).
    pub fn feeds(self, to: SocketType) -> bool {
        self == to || (self == SocketType::Number && to == SocketType::Vec3)
    }
}

/// The value of an input no link feeds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PortValue {
    Number(f64),
    Vec3(Vec3),
}

impl PortValue {
    /// As one number: the first of a triple.
    pub fn number(self) -> f64 {
        match self {
            PortValue::Number(v) => v,
            PortValue::Vec3(v) => v[0],
        }
    }

    /// As a triple: a number is repeated.
    pub fn vec3(self) -> Vec3 {
        match self {
            PortValue::Number(v) => [v; 3],
            PortValue::Vec3(v) => v,
        }
    }

    /// One component: the number itself, or axis `i` of a triple.
    pub fn component(self, axis: Option<usize>) -> f64 {
        match (self, axis) {
            (PortValue::Vec3(v), Some(i)) => v[i.min(2)],
            _ => self.number(),
        }
    }

    /// The value with one component set.
    pub fn with_component(self, axis: Option<usize>, value: f64) -> PortValue {
        match (self, axis) {
            (PortValue::Vec3(mut v), Some(i)) => {
                v[i.min(2)] = value;
                PortValue::Vec3(v)
            }
            (PortValue::Vec3(_), None) => PortValue::Vec3([value; 3]),
            (PortValue::Number(_), _) => PortValue::Number(value),
        }
    }
}

/// A named number input of an expression: the name the expression uses, and its value when no
/// link feeds it. `key` stays with it when others are removed, so links keep pointing at it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Var {
    pub key: u32,
    pub name: String,
    pub value: f64,
}

/// Keeps the molecules for which an expression is true (not 0) on its first output and the
/// others on the second.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Select {
    pub expr: String,
    #[serde(default)]
    pub vars: Vec<Var>,
}

/// Copies of everything fed in, copy `i` (0 to n − 1) turned by `euler_deg` and then moved by
/// `offset`, each component an expression of `i`, `n` and the variables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Repeat {
    pub n: u32,
    /// The attribute the copy number is kept under (besides `copy`).
    pub name: String,
    pub offset: [String; 3],
    pub euler_deg: [String; 3],
    #[serde(default)]
    pub vars: Vec<Var>,
}

/// What [`MoveEach`] turns a molecule about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EachPivot {
    /// The origin.
    Origin,
    /// The molecule's own centroid.
    #[default]
    Own,
    /// The centroid of all the molecules fed in.
    Set,
}

/// Moves each molecule by its own amount: turned about the pivot, then moved, each component an
/// expression of the molecule's attributes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveEach {
    pub offset: [String; 3],
    pub euler_deg: [String; 3],
    #[serde(default)]
    pub pivot: EachPivot,
    #[serde(default)]
    pub vars: Vec<Var>,
}

/// Gives each molecule an attribute: `name` = the expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetAttr {
    pub name: String,
    pub expr: String,
    #[serde(default)]
    pub vars: Vec<Var>,
}

/// A number from an expression of its variables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expression {
    pub expr: String,
    #[serde(default)]
    pub vars: Vec<Var>,
}

/// A circle of `resolution` points about the origin in the xy plane: a closed curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Circle {
    pub resolution: u32,
    pub radius: f64,
}

/// Where [`CurveToPoints`] puts its points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CurveSample {
    /// `count` points evenly along each curve.
    #[default]
    Count,
    /// A point every `length` Å along each curve.
    Length,
    /// The curve's own points.
    Evaluated,
}

/// Points along curves, each turned to follow the curve: its x axis along the curve, its z
/// axis normal to the curve's plane (z for a curve in the xy plane).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurveToPoints {
    pub mode: CurveSample,
    pub count: u32,
    pub length: f64,
}

/// A copy of the molecules fed in on each point chosen, turned by `rotation_deg` and then by
/// the point's own turn (when `align`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceOnPoints {
    /// Whether a point is used (not 0), when no link feeds it.
    pub selection: f64,
    pub rotation_deg: Vec3,
    /// Turn each copy as its point is turned.
    pub align: bool,
}

/// A custom node in use: which definition, and the values of its inputs no link feeds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupRef {
    pub def: DefId,
    #[serde(default)]
    pub inputs: BTreeMap<u32, PortValue>,
}

/// One kind of node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NodeKind {
    /// One molecule of a monomer, by the monomer's id.
    Monomer {
        component: u64,
    },
    /// One of the building steps: ring, stack, array, crystal, move/rotate, arc.
    Op(Step),
    /// The molecules of every input, in order.
    Merge {
        inputs: u8,
    },
    Select(Select),
    Repeat(Repeat),
    MoveEach(MoveEach),
    SetAttr(SetAttr),
    Value {
        value: f64,
    },
    Vector {
        value: Vec3,
    },
    Expression(Expression),
    /// Three numbers into a triple.
    Combine {
        value: Vec3,
    },
    /// A triple into three numbers.
    Separate {
        value: Vec3,
    },
    /// A circle (a curve).
    Circle(Circle),
    CurveToPoints(CurveToPoints),
    InstanceOnPoints(InstanceOnPoints),
    /// The place of each element (molecule, point) where it is used: a value per element.
    Index,
    Group(GroupRef),
    /// Inside a custom node: its inputs.
    GroupInput,
    /// Inside a custom node: its outputs.
    GroupOutput,
    /// What the assembly is.
    Output,
}

/// An input socket.
#[derive(Debug, Clone, PartialEq)]
pub struct InDecl {
    pub key: InKey,
    pub ty: SocketType,
    pub label: String,
    /// Degrees rather than ångström (or a plain number).
    pub angle: bool,
    /// Ångström.
    pub length: bool,
}

/// An output socket.
#[derive(Debug, Clone, PartialEq)]
pub struct OutDecl {
    pub key: OutKey,
    pub ty: SocketType,
    pub label: String,
}

/// Where a node is listed in the add menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Input,
    Copy,
    Move,
    Combine,
    Math,
    /// Curves and points.
    Geometry,
    Group,
    Output,
}

const XYZ: [&str; 3] = ["x", "y", "z"];

fn input(key: InKey, ty: SocketType, label: impl Into<String>) -> InDecl {
    InDecl {
        key,
        ty,
        label: label.into(),
        angle: false,
        length: false,
    }
}

fn length_input(key: InKey, label: impl Into<String>) -> InDecl {
    InDecl {
        length: true,
        ..input(key, SocketType::Number, label)
    }
}

fn output(key: OutKey, ty: SocketType, label: impl Into<String>) -> OutDecl {
    OutDecl {
        key,
        ty,
        label: label.into(),
    }
}

fn var_inputs(vars: &[Var]) -> impl Iterator<Item = InDecl> + '_ {
    vars.iter()
        .map(|v| input(InKey::Var(v.key), SocketType::Number, v.name.clone()))
}

impl NodeKind {
    /// The node's name.
    pub fn title(&self, defs: &[NodeDef]) -> String {
        match self {
            NodeKind::Monomer { .. } => t!("assembly.node.monomer").into(),
            NodeKind::Op(step) => step.label().into(),
            NodeKind::Merge { .. } => t!("assembly.node.merge").into(),
            NodeKind::Select(_) => t!("assembly.node.select").into(),
            NodeKind::Repeat(_) => t!("assembly.node.repeat").into(),
            NodeKind::MoveEach(_) => t!("assembly.node.move_each").into(),
            NodeKind::SetAttr(_) => t!("assembly.node.set_attr").into(),
            NodeKind::Value { .. } => t!("assembly.node.value").into(),
            NodeKind::Vector { .. } => t!("assembly.node.vector").into(),
            NodeKind::Expression(_) => t!("assembly.node.expression").into(),
            NodeKind::Combine { .. } => t!("assembly.node.combine").into(),
            NodeKind::Separate { .. } => t!("assembly.node.separate").into(),
            NodeKind::Circle(_) => t!("assembly.node.circle").into(),
            NodeKind::CurveToPoints(_) => t!("assembly.node.curve_to_points").into(),
            NodeKind::InstanceOnPoints(_) => t!("assembly.node.instance_on_points").into(),
            NodeKind::Index => t!("assembly.node.index").into(),
            NodeKind::Group(g) => super::defs::find(defs, &g.def).map_or_else(
                || t!("assembly.node.missing_def").into(),
                |d| d.name.clone(),
            ),
            NodeKind::GroupInput => t!("assembly.node.group_input").into(),
            NodeKind::GroupOutput => t!("assembly.node.group_output").into(),
            NodeKind::Output => t!("assembly.node.output").into(),
        }
    }

    pub fn category(&self) -> Category {
        match self {
            NodeKind::Monomer { .. } | NodeKind::GroupInput | NodeKind::Index => Category::Input,
            NodeKind::Circle(_) | NodeKind::CurveToPoints(_) | NodeKind::InstanceOnPoints(_) => {
                Category::Geometry
            }
            NodeKind::Op(Step::Transform(_) | Step::Bend(_)) | NodeKind::MoveEach(_) => {
                Category::Move
            }
            NodeKind::Op(_) | NodeKind::Repeat(_) => Category::Copy,
            NodeKind::Merge { .. } | NodeKind::Select(_) | NodeKind::SetAttr(_) => {
                Category::Combine
            }
            NodeKind::Value { .. }
            | NodeKind::Vector { .. }
            | NodeKind::Expression(_)
            | NodeKind::Combine { .. }
            | NodeKind::Separate { .. } => Category::Math,
            NodeKind::Group(_) => Category::Group,
            NodeKind::GroupOutput | NodeKind::Output => Category::Output,
        }
    }

    /// The input sockets, in order. `owner` is the custom node whose graph the node is in
    /// (its outputs are a group output node's inputs).
    pub fn inputs(&self, defs: &[NodeDef], owner: Option<&NodeDef>) -> Vec<InDecl> {
        let set = || {
            input(
                InKey::Set(0),
                SocketType::Set,
                t!("assembly.node.socket.molecules"),
            )
        };
        match self {
            NodeKind::Monomer { .. }
            | NodeKind::Value { .. }
            | NodeKind::Vector { .. }
            | NodeKind::Index
            | NodeKind::GroupInput => Vec::new(),
            NodeKind::Circle(_) => vec![
                input(
                    InKey::Count,
                    SocketType::Number,
                    t!("assembly.node.socket.resolution"),
                ),
                length_input(InKey::Param(0), t!("assembly.node.socket.radius")),
            ],
            NodeKind::CurveToPoints(c) => {
                let mut v = vec![input(
                    InKey::Curve,
                    SocketType::Curve,
                    t!("assembly.node.socket.curve"),
                )];
                match c.mode {
                    CurveSample::Count => v.push(input(
                        InKey::Count,
                        SocketType::Number,
                        t!("assembly.node.socket.count"),
                    )),
                    CurveSample::Length => v.push(length_input(
                        InKey::Param(0),
                        t!("assembly.node.socket.length"),
                    )),
                    CurveSample::Evaluated => {}
                }
                v
            }
            NodeKind::InstanceOnPoints(_) => vec![
                input(
                    InKey::Points,
                    SocketType::Points,
                    t!("assembly.node.socket.points"),
                ),
                input(
                    InKey::Set(0),
                    SocketType::Set,
                    t!("assembly.node.socket.instance"),
                ),
                input(
                    InKey::Param(0),
                    SocketType::Number,
                    t!("assembly.node.socket.selection"),
                ),
                InDecl {
                    angle: true,
                    ..input(
                        InKey::Param(1),
                        SocketType::Vec3,
                        t!("assembly.node.socket.rotation"),
                    )
                },
            ],
            NodeKind::Op(step) => std::iter::once(set())
                .chain(StepField::of(step).into_iter().map(|f| InDecl {
                    key: InKey::Field(f),
                    ty: f.ty(),
                    label: f.label().into(),
                    angle: f.is_angle(),
                    length: !f.is_angle() && !f.is_count(),
                }))
                .collect(),
            NodeKind::Merge { inputs } => (0..*inputs)
                .map(|i| {
                    input(
                        InKey::Set(i),
                        SocketType::Set,
                        t!("assembly.node.socket.molecules"),
                    )
                })
                .collect(),
            NodeKind::Select(s) => std::iter::once(set()).chain(var_inputs(&s.vars)).collect(),
            NodeKind::Repeat(r) => [
                set(),
                input(
                    InKey::Count,
                    SocketType::Number,
                    t!("assembly.node.socket.count"),
                ),
            ]
            .into_iter()
            .chain(var_inputs(&r.vars))
            .collect(),
            NodeKind::MoveEach(m) => std::iter::once(set()).chain(var_inputs(&m.vars)).collect(),
            NodeKind::SetAttr(s) => std::iter::once(set()).chain(var_inputs(&s.vars)).collect(),
            NodeKind::Expression(e) => var_inputs(&e.vars).collect(),
            NodeKind::Combine { .. } => (0..3)
                .map(|i| input(InKey::Axis(i), SocketType::Number, XYZ[usize::from(i)]))
                .collect(),
            NodeKind::Separate { .. } => {
                vec![input(
                    InKey::Value,
                    SocketType::Vec3,
                    t!("assembly.node.socket.vector"),
                )]
            }
            NodeKind::Group(g) => super::defs::find(defs, &g.def)
                .map(|d| {
                    d.inputs
                        .iter()
                        .map(|p| input(InKey::Port(p.key), p.ty, p.name.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            NodeKind::GroupOutput => owner
                .map(|d| {
                    d.outputs
                        .iter()
                        .map(|p| input(InKey::Port(p.key), p.ty, p.name.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            NodeKind::Output => vec![set()],
        }
    }

    /// The output sockets, in order. `owner` as for [`NodeKind::inputs`] (its inputs are a
    /// group input node's outputs).
    pub fn outputs(&self, defs: &[NodeDef], owner: Option<&NodeDef>) -> Vec<OutDecl> {
        let set = || {
            output(
                OutKey::Set,
                SocketType::Set,
                t!("assembly.node.socket.molecules"),
            )
        };
        match self {
            NodeKind::Monomer { .. }
            | NodeKind::Op(_)
            | NodeKind::Merge { .. }
            | NodeKind::Repeat(_)
            | NodeKind::MoveEach(_)
            | NodeKind::InstanceOnPoints(_)
            | NodeKind::SetAttr(_) => vec![set()],
            NodeKind::Circle(_) => vec![output(
                OutKey::Curve,
                SocketType::Curve,
                t!("assembly.node.socket.curve"),
            )],
            NodeKind::CurveToPoints(_) => vec![output(
                OutKey::Points,
                SocketType::Points,
                t!("assembly.node.socket.points"),
            )],
            NodeKind::Index => vec![output(
                OutKey::Number,
                SocketType::Number,
                t!("assembly.node.socket.index"),
            )],
            NodeKind::Select(_) => vec![
                output(
                    OutKey::Selected,
                    SocketType::Set,
                    t!("assembly.node.socket.selected"),
                ),
                output(
                    OutKey::Rest,
                    SocketType::Set,
                    t!("assembly.node.socket.rest"),
                ),
            ],
            NodeKind::Value { .. } | NodeKind::Expression(_) => vec![output(
                OutKey::Number,
                SocketType::Number,
                t!("assembly.node.socket.value"),
            )],
            NodeKind::Vector { .. } | NodeKind::Combine { .. } => vec![output(
                OutKey::Vec3,
                SocketType::Vec3,
                t!("assembly.node.socket.vector"),
            )],
            NodeKind::Separate { .. } => (0..3)
                .map(|i| output(OutKey::Axis(i), SocketType::Number, XYZ[usize::from(i)]))
                .collect(),
            NodeKind::Group(g) => super::defs::find(defs, &g.def)
                .map(|d| {
                    d.outputs
                        .iter()
                        .map(|p| output(OutKey::Port(p.key), p.ty, p.name.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            NodeKind::GroupInput => owner
                .map(|d| {
                    d.inputs
                        .iter()
                        .map(|p| output(OutKey::Port(p.key), p.ty, p.name.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            NodeKind::GroupOutput | NodeKind::Output => Vec::new(),
        }
    }

    /// The value an unlinked input has.
    pub fn inline(&self, key: InKey) -> Option<PortValue> {
        match (self, key) {
            (NodeKind::Op(step), InKey::Field(f)) => f.read(step),
            (NodeKind::Repeat(r), InKey::Count) => Some(PortValue::Number(f64::from(r.n))),
            (NodeKind::Combine { value }, InKey::Axis(i)) => {
                value.get(usize::from(i)).map(|&v| PortValue::Number(v))
            }
            (NodeKind::Separate { value }, InKey::Value) => Some(PortValue::Vec3(*value)),
            (NodeKind::Group(g), InKey::Port(p)) => g.inputs.get(&p).copied(),
            (NodeKind::Circle(c), InKey::Count) => Some(PortValue::Number(f64::from(c.resolution))),
            (NodeKind::Circle(c), InKey::Param(0)) => Some(PortValue::Number(c.radius)),
            (NodeKind::CurveToPoints(c), InKey::Count) => {
                Some(PortValue::Number(f64::from(c.count)))
            }
            (NodeKind::CurveToPoints(c), InKey::Param(0)) => Some(PortValue::Number(c.length)),
            (NodeKind::InstanceOnPoints(p), InKey::Param(0)) => {
                Some(PortValue::Number(p.selection))
            }
            (NodeKind::InstanceOnPoints(p), InKey::Param(1)) => {
                Some(PortValue::Vec3(p.rotation_deg))
            }
            (kind, InKey::Var(k)) => kind
                .vars()?
                .iter()
                .find(|v| v.key == k)
                .map(|v| PortValue::Number(v.value)),
            _ => None,
        }
    }

    /// Set the value an unlinked input has. Returns whether the node has that input.
    pub fn set_inline(&mut self, key: InKey, value: PortValue) -> bool {
        match (self, key) {
            (NodeKind::Op(step), InKey::Field(f)) => f.write(step, value),
            (NodeKind::Repeat(r), InKey::Count) => {
                r.n = super::field::count_of(value.number());
                true
            }
            (NodeKind::Combine { value: v }, InKey::Axis(i)) if i < 3 => {
                v[usize::from(i)] = value.number();
                true
            }
            (NodeKind::Separate { value: v }, InKey::Value) => {
                *v = value.vec3();
                true
            }
            (NodeKind::Group(g), InKey::Port(p)) => {
                g.inputs.insert(p, value);
                true
            }
            (NodeKind::Circle(c), InKey::Count) => {
                c.resolution = super::field::count_of(value.number());
                true
            }
            (NodeKind::Circle(c), InKey::Param(0)) => {
                c.radius = value.number();
                true
            }
            (NodeKind::CurveToPoints(c), InKey::Count) => {
                c.count = super::field::count_of(value.number());
                true
            }
            (NodeKind::CurveToPoints(c), InKey::Param(0)) => {
                c.length = value.number();
                true
            }
            (NodeKind::InstanceOnPoints(p), InKey::Param(0)) => {
                p.selection = value.number();
                true
            }
            (NodeKind::InstanceOnPoints(p), InKey::Param(1)) => {
                p.rotation_deg = value.vec3();
                true
            }
            (kind, InKey::Var(k)) => match kind
                .vars_mut()
                .and_then(|vars| vars.iter_mut().find(|v| v.key == k))
            {
                Some(var) => {
                    var.value = value.number();
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    /// The node's expression variables, if it has any.
    pub fn vars(&self) -> Option<&Vec<Var>> {
        match self {
            NodeKind::Select(s) => Some(&s.vars),
            NodeKind::Repeat(r) => Some(&r.vars),
            NodeKind::MoveEach(m) => Some(&m.vars),
            NodeKind::SetAttr(s) => Some(&s.vars),
            NodeKind::Expression(e) => Some(&e.vars),
            _ => None,
        }
    }

    pub fn vars_mut(&mut self) -> Option<&mut Vec<Var>> {
        match self {
            NodeKind::Select(s) => Some(&mut s.vars),
            NodeKind::Repeat(r) => Some(&mut r.vars),
            NodeKind::MoveEach(m) => Some(&mut m.vars),
            NodeKind::SetAttr(s) => Some(&mut s.vars),
            NodeKind::Expression(e) => Some(&mut e.vars),
            _ => None,
        }
    }

    /// A new node of each kind the add menu offers (the monomers and custom nodes apart).
    pub fn menu() -> Vec<NodeKind> {
        vec![
            NodeKind::Op(Step::new_array()),
            NodeKind::Op(Step::new_ring()),
            NodeKind::Op(Step::new_stack()),
            NodeKind::Op(Step::new_lattice()),
            NodeKind::Repeat(Repeat::new()),
            NodeKind::Op(Step::new_transform()),
            NodeKind::Op(Step::new_bend()),
            NodeKind::MoveEach(MoveEach::new()),
            NodeKind::Merge { inputs: 2 },
            NodeKind::Select(Select::new()),
            NodeKind::SetAttr(SetAttr::new()),
            NodeKind::Value { value: 1.0 },
            NodeKind::Vector { value: [0.0; 3] },
            NodeKind::Expression(Expression::new()),
            NodeKind::Combine { value: [0.0; 3] },
            NodeKind::Separate { value: [0.0; 3] },
            NodeKind::Index,
            NodeKind::Circle(Circle::new()),
            NodeKind::CurveToPoints(CurveToPoints::new()),
            NodeKind::InstanceOnPoints(InstanceOnPoints::new()),
        ]
    }
}

impl Circle {
    /// Eight points 10 Å from the centre.
    pub fn new() -> Self {
        Self {
            resolution: 8,
            radius: 10.0,
        }
    }
}

impl Default for Circle {
    fn default() -> Self {
        Self::new()
    }
}

impl CurveToPoints {
    pub fn new() -> Self {
        Self {
            mode: CurveSample::Count,
            count: 10,
            length: 5.0,
        }
    }
}

impl Default for CurveToPoints {
    fn default() -> Self {
        Self::new()
    }
}

impl InstanceOnPoints {
    /// On every point, turned as the point is.
    pub fn new() -> Self {
        Self {
            selection: 1.0,
            rotation_deg: [0.0; 3],
            align: true,
        }
    }
}

impl Default for InstanceOnPoints {
    fn default() -> Self {
        Self::new()
    }
}

fn zeros() -> [String; 3] {
    ["0".to_owned(), "0".to_owned(), "0".to_owned()]
}

impl Select {
    /// Every other molecule.
    pub fn new() -> Self {
        Self {
            expr: "k % 2 == 0".into(),
            vars: Vec::new(),
        }
    }
}

impl Default for Select {
    fn default() -> Self {
        Self::new()
    }
}

impl Repeat {
    /// Four copies 5 Å apart along x.
    pub fn new() -> Self {
        Self {
            n: 4,
            name: "rep".into(),
            offset: ["5*i".into(), "0".into(), "0".into()],
            euler_deg: zeros(),
            vars: Vec::new(),
        }
    }
}

impl Default for Repeat {
    fn default() -> Self {
        Self::new()
    }
}

impl MoveEach {
    /// No move.
    pub fn new() -> Self {
        Self {
            offset: zeros(),
            euler_deg: zeros(),
            pivot: EachPivot::Own,
            vars: Vec::new(),
        }
    }
}

impl Default for MoveEach {
    fn default() -> Self {
        Self::new()
    }
}

impl SetAttr {
    pub fn new() -> Self {
        Self {
            name: "tag".into(),
            expr: "1".into(),
            vars: Vec::new(),
        }
    }
}

impl Default for SetAttr {
    fn default() -> Self {
        Self::new()
    }
}

impl Expression {
    /// `a * 2`, with `a` an input.
    pub fn new() -> Self {
        Self {
            expr: "a * 2".into(),
            vars: vec![Var {
                key: 0,
                name: "a".into(),
                value: 1.0,
            }],
        }
    }
}

impl Default for Expression {
    fn default() -> Self {
        Self::new()
    }
}

/// The next key for a new variable of `vars`.
pub fn next_var_key(vars: &[Var]) -> u32 {
    vars.iter().map(|v| v.key + 1).max().unwrap_or(0)
}

/// A name for a new variable of `vars`: the first of a, b, c, … not taken.
pub fn next_var_name(vars: &[Var]) -> String {
    let taken = |n: &str| vars.iter().any(|v| v.name == n);
    for c in 'a'..='h' {
        let name = c.to_string();
        if !taken(&name) {
            return name;
        }
    }
    let mut k = 1;
    while taken(&format!("v{k}")) {
        k += 1;
    }
    format!("v{k}")
}
