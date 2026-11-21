//! A graph evaluated into placed molecules.
//!
//! The output node pulls what it needs: each node reached is evaluated once (per custom node it
//! is inside) and remembered, so a set fed to two nodes is made once. A node that cannot be
//! evaluated (a crystal with impossible constants, an expression that does not parse) is
//! reported and passes its molecules through unchanged, as a switched-off step does — so the
//! rest of the graph still shows something while the user fixes it.
//!
//! Molecules carry attributes, numbers named by the nodes that made them: a ring's copies have
//! `ring`, a stack's `layer`, an array's `row`, a crystal's `cell`, a repeat's its own name, a
//! merge's `input`; each copying node also sets `copy` and `copies`. Expressions in selecting
//! and moving nodes read them, with `k` (the molecule's place in the set), `count`, `comp`
//! (its monomer, from 0), `x`, `y`, `z` (its centroid) and the node's own variables.
//!
//! A number may also be a field: a value per element of whatever uses it, such as the index
//! node's. Nodes that work element by element (select, move each, set attribute, instance on
//! points) take one wherever they take a number; expression and combine nodes fed one give
//! one; other inputs report it.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use super::defs::{self, DefId, NodeDef};
use super::expr::Expr;
use super::field::{StepField, count_of};
use super::kinds::{CurveSample, EachPivot, NodeKind, PortValue, SocketType, Var};
use super::{Graph, InKey, NodeId, OutKey};
use crate::assembly::math::{self, Rigid, Vec3};
use crate::assembly::model::{Component, Crystal, Ids, Instance, Shape, Step, centroid};

/// The most molecules a set may hold: past it the node reports an error instead of running
/// out of memory.
pub const MAX_INSTANCES: usize = 2_000_000;

/// How deep custom nodes may be nested.
pub const MAX_DEPTH: usize = 16;

/// Molecules, with their attributes and the crystal they belong to.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Set {
    pub items: Vec<Instance>,
    /// Columns of numbers, one per molecule each.
    pub attrs: Vec<(Arc<str>, Vec<f64>)>,
    /// The crystal a lattice node made, while the first `z` molecules are still its unit cell.
    pub crystal: Option<Crystal>,
}

impl Set {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The values of attribute `name`.
    pub fn attr(&self, name: &str) -> Option<&[f64]> {
        self.attrs
            .iter()
            .find(|(n, _)| &**n == name)
            .map(|(_, v)| v.as_slice())
    }

    /// Set attribute `name` (replacing it if there is one).
    pub fn set_attr(&mut self, name: &str, values: Vec<f64>) {
        match self.attrs.iter_mut().find(|(n, _)| &**n == name) {
            Some((_, v)) => *v = values,
            None => self.attrs.push((Arc::from(name), values)),
        }
    }

    /// The molecules at `picks`, in that order, with their attributes (no crystal).
    fn gather(&self, picks: &[usize]) -> Set {
        Set {
            items: picks.iter().map(|&k| self.items[k].clone()).collect(),
            attrs: self
                .attrs
                .iter()
                .map(|(n, v)| (n.clone(), picks.iter().map(|&k| v[k]).collect()))
                .collect(),
            crystal: None,
        }
    }

    /// `copies` copies of the set, copy `c` of item `k` moved by `moves[c]` and listed at
    /// `c · len + k`, with `copy`, `copies` and `name` set to the copy.
    fn copied(&self, moves: &[Rigid], name: &str) -> Set {
        let n = self.items.len();
        let mut items = Vec::with_capacity(n * moves.len());
        for t in moves {
            items.extend(self.items.iter().map(|inst| Instance {
                component: inst.component,
                transform: inst.transform.then(t),
            }));
        }
        let mut out = Set {
            items,
            attrs: self
                .attrs
                .iter()
                .map(|(name, v)| (name.clone(), v.repeat(moves.len())))
                .collect(),
            crystal: self.crystal.clone(),
        };
        let index: Vec<f64> = (0..moves.len())
            .flat_map(|c| std::iter::repeat_n(c as f64, n))
            .collect();
        out.set_attr("copies", vec![moves.len() as f64; index.len()]);
        if !name.is_empty() && name != "copy" {
            out.set_attr(name, index.clone());
        }
        out.set_attr("copy", index);
        out
    }
}

/// Curves: lines through points.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Curve {
    pub splines: Vec<Spline>,
}

/// One curve: its points in order, and whether the last joins the first.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Spline {
    pub points: Vec<Vec3>,
    pub closed: bool,
}

/// Points, each with a frame: where it is and how it is turned.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Points {
    pub frames: Vec<Rigid>,
}

/// A number for each element of whatever uses it.
#[derive(Debug, Clone)]
pub enum Field {
    Const(f64),
    /// The element's place, from 0.
    Index,
    /// An expression of other fields (its variables, in order).
    Expr(Rc<Expr>, Vec<Field>),
}

impl Field {
    /// The values for `len` elements.
    pub fn column(&self, len: usize) -> Vec<f64> {
        match self {
            Field::Const(v) => vec![*v; len],
            Field::Index => (0..len).map(|k| k as f64).collect(),
            Field::Expr(expr, args) => {
                let columns: Vec<Vec<f64>> = args.iter().map(|a| a.column(len)).collect();
                let mut values = vec![0.0; args.len()];
                (0..len)
                    .map(|k| {
                        for (v, c) in values.iter_mut().zip(&columns) {
                            *v = c[k];
                        }
                        expr.eval(&values)
                    })
                    .collect()
            }
        }
    }

    fn constant(&self) -> Option<f64> {
        match self {
            Field::Const(v) => Some(*v),
            _ => None,
        }
    }
}

/// What travels along a wire.
#[derive(Debug, Clone)]
pub enum Data {
    Set(Arc<Set>),
    Number(f64),
    Vec3(Vec3),
    Curve(Arc<Curve>),
    Points(Arc<Points>),
    /// A number per element.
    Field(Rc<Field>),
    /// Three numbers per element.
    Field3(Rc<[Field; 3]>),
}

impl Data {
    fn as_number(&self) -> Option<f64> {
        match self {
            Data::Number(v) => Some(*v),
            Data::Vec3(v) => Some(v[0]),
            _ => None,
        }
    }

    fn as_port_value(&self, ty: SocketType) -> Option<PortValue> {
        match (self, ty) {
            (Data::Number(v), SocketType::Vec3) => Some(PortValue::Vec3([*v; 3])),
            (Data::Number(v), _) => Some(PortValue::Number(*v)),
            (Data::Vec3(v), SocketType::Number) => Some(PortValue::Number(v[0])),
            (Data::Vec3(v), _) => Some(PortValue::Vec3(*v)),
            _ => None,
        }
    }

    fn is_field(&self) -> bool {
        matches!(self, Data::Field(_) | Data::Field3(_))
    }

    /// The data as a socket of type `ty` takes it, if it can.
    fn conform(self, ty: SocketType) -> Option<Data> {
        match (self, ty) {
            (d @ Data::Set(_), SocketType::Set)
            | (d @ Data::Curve(_), SocketType::Curve)
            | (d @ Data::Points(_), SocketType::Points) => Some(d),
            (d @ (Data::Field(_) | Data::Field3(_)), SocketType::Number | SocketType::Vec3) => {
                Some(d)
            }
            (d, SocketType::Number | SocketType::Vec3) => {
                d.as_port_value(ty).map(Data::from_port_value)
            }
            _ => None,
        }
    }

    fn from_port_value(v: PortValue) -> Data {
        match v {
            PortValue::Number(v) => Data::Number(v),
            PortValue::Vec3(v) => Data::Vec3(v),
        }
    }
}

/// Where a node is: the custom nodes entered to reach it, outermost first (empty at the top).
pub type Scope = Vec<NodeId>;

/// Something wrong with one node.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub scope: Scope,
    pub node: NodeId,
    /// In the UI language.
    pub message: String,
    /// A crystal that cannot be built (reported as such by the build).
    pub lattice: bool,
}

/// What an output gave: how many molecules, or the number.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shown {
    Count(usize),
    Number(f64),
    Vec3(Vec3),
    /// A value per element.
    Field,
}

/// The result of evaluating a graph.
#[derive(Debug, Clone, Default)]
pub struct Evaluation {
    /// The assembly: the molecules the output node was fed.
    pub output: Arc<Set>,
    pub errors: Vec<Issue>,
    pub warnings: Vec<Issue>,
    /// What each output of each node reached gave.
    pub shown: HashMap<(Scope, NodeId, OutKey), Shown>,
    /// The molecules each set output of each node reached gave (for previewing one).
    pub sets: HashMap<(Scope, NodeId, OutKey), Arc<Set>>,
}

impl Evaluation {
    /// Whether node `node` in `scope` was evaluated (it feeds the output).
    pub fn reached(&self, scope: &[NodeId], node: NodeId) -> bool {
        self.shown
            .keys()
            .any(|(s, n, _)| *n == node && s.as_slice() == scope)
            || self
                .errors
                .iter()
                .chain(&self.warnings)
                .any(|i| i.node == node && i.scope == scope)
    }

    /// The first error of a crystal that cannot be built.
    pub fn lattice_error(&self) -> Option<&Issue> {
        self.errors.iter().find(|i| i.lattice)
    }

    /// The first error that is not a crystal's.
    pub fn node_error(&self) -> Option<&Issue> {
        self.errors.iter().find(|i| !i.lattice)
    }
}

/// What the graph's nodes refer to: the monomers and the custom nodes.
pub struct Source<'a> {
    pub components: &'a [Component],
    pub component_ids: &'a Ids,
    pub defs: &'a [NodeDef],
    /// Each component's atoms as the steps place them.
    pub shapes: &'a [Option<Shape>],
}

/// Evaluate `graph` (a design's top-level graph) into the molecules its output is fed.
pub fn evaluate(source: &Source<'_>, graph: &Graph) -> Evaluation {
    let centroids = source
        .shapes
        .iter()
        .map(|s| s.as_ref().map_or([0.0; 3], |(p, _)| centroid(p)))
        .collect();
    let mut ev = Evaluator {
        src: source,
        centroids,
        memo: HashMap::new(),
        visiting: HashSet::new(),
        bindings: HashMap::new(),
        def_stack: Vec::new(),
        out: Evaluation::default(),
    };
    let scope: Scope = Vec::new();
    let output = match graph.output() {
        Some(out) => {
            ev.mark(&scope, out);
            ev.set_input(graph, None, &scope, out, InKey::Set(0))
        }
        None => Arc::new(Set::default()),
    };
    let mut result = ev.out;
    result.output = output;
    result
}

type Outputs = Rc<Vec<(OutKey, Data)>>;

/// Why a node could not be evaluated.
struct Fail {
    message: String,
    lattice: bool,
}

impl Fail {
    fn new(message: impl Into<String>) -> Fail {
        Fail {
            message: message.into(),
            lattice: false,
        }
    }
}

struct Evaluator<'s, 'a> {
    src: &'s Source<'a>,
    /// Each component's centroid in its own frame.
    centroids: Vec<Vec3>,
    memo: HashMap<(Scope, NodeId), Outputs>,
    visiting: HashSet<(Scope, NodeId)>,
    /// The values a custom node's inputs have, by the scope inside it.
    bindings: HashMap<Scope, Rc<Vec<(u32, Data)>>>,
    def_stack: Vec<DefId>,
    out: Evaluation,
}

impl<'s, 'a> Evaluator<'s, 'a> {
    /// Note that the node was reached (for nodes with no outputs, such as the output node).
    fn mark(&mut self, scope: &Scope, node: NodeId) {
        self.out
            .shown
            .entry((scope.clone(), node, OutKey::Set))
            .or_insert(Shown::Count(0));
    }

    fn error(&mut self, scope: &Scope, node: NodeId, fail: Fail) {
        self.out.errors.push(Issue {
            scope: scope.clone(),
            node,
            message: fail.message,
            lattice: fail.lattice,
        });
    }

    fn warn(&mut self, scope: &Scope, node: NodeId, message: String) {
        self.out.warnings.push(Issue {
            scope: scope.clone(),
            node,
            message,
            lattice: false,
        });
    }

    /// What feeds input `key` of node `id`, if a link does.
    fn linked(
        &mut self,
        graph: &Graph,
        owner: Option<&NodeDef>,
        scope: &Scope,
        id: NodeId,
        key: InKey,
    ) -> Option<Data> {
        let link = *graph.link_into(id, key)?;
        let outs = self.node(graph, owner, scope, link.from);
        outs.iter()
            .find(|(k, _)| *k == link.out)
            .map(|(_, d)| d.clone())
    }

    /// The molecules fed to input `key` (none when unlinked).
    fn set_input(
        &mut self,
        graph: &Graph,
        owner: Option<&NodeDef>,
        scope: &Scope,
        id: NodeId,
        key: InKey,
    ) -> Arc<Set> {
        match self.linked(graph, owner, scope, id, key) {
            Some(Data::Set(s)) => s,
            _ => Arc::new(Set::default()),
        }
    }

    /// The value of a number input: what feeds it, else `inline`.
    fn number_input(
        &mut self,
        graph: &Graph,
        owner: Option<&NodeDef>,
        scope: &Scope,
        id: NodeId,
        key: InKey,
        inline: f64,
    ) -> f64 {
        self.linked(graph, owner, scope, id, key)
            .and_then(|d| d.as_number())
            .unwrap_or(inline)
    }

    /// The value of a number input that may be a field: what feeds it, else `inline`.
    fn field_input(
        &mut self,
        graph: &Graph,
        owner: Option<&NodeDef>,
        scope: &Scope,
        id: NodeId,
        key: InKey,
        inline: f64,
    ) -> Field {
        match self.linked(graph, owner, scope, id, key) {
            Some(Data::Field(f)) => (*f).clone(),
            Some(Data::Field3(f)) => f[0].clone(),
            Some(d) => Field::Const(d.as_number().unwrap_or(inline)),
            None => Field::Const(inline),
        }
    }

    /// The value of a triple input that may be a field: what feeds it, else `inline`.
    fn field3_input(
        &mut self,
        graph: &Graph,
        owner: Option<&NodeDef>,
        scope: &Scope,
        id: NodeId,
        key: InKey,
        inline: Vec3,
    ) -> [Field; 3] {
        match self.linked(graph, owner, scope, id, key) {
            Some(Data::Field3(f)) => (*f).clone(),
            Some(Data::Field(f)) => [(*f).clone(), (*f).clone(), (*f).clone()],
            Some(Data::Vec3(v)) => v.map(Field::Const),
            Some(Data::Number(v)) => [v; 3].map(Field::Const),
            _ => inline.map(Field::Const),
        }
    }

    /// The values of an expression node's variables (fields where fields feed them).
    fn var_fields(
        &mut self,
        graph: &Graph,
        owner: Option<&NodeDef>,
        scope: &Scope,
        id: NodeId,
        vars: &[Var],
    ) -> Vec<(String, Field)> {
        vars.iter()
            .map(|v| {
                (
                    v.name.clone(),
                    self.field_input(graph, owner, scope, id, InKey::Var(v.key), v.value),
                )
            })
            .collect()
    }

    /// The values of an expression node's variables, which must be plain numbers.
    fn var_values(
        &mut self,
        graph: &Graph,
        owner: Option<&NodeDef>,
        scope: &Scope,
        id: NodeId,
        vars: &[Var],
    ) -> Result<Vec<(String, f64)>, Fail> {
        self.var_fields(graph, owner, scope, id, vars)
            .into_iter()
            .map(|(name, f)| match f.constant() {
                Some(v) => Ok((name, v)),
                None => Err(Fail::new(t!("assembly.node.error.field_here"))),
            })
            .collect()
    }

    /// A field fed to an input that takes one number for all.
    fn check_fields(
        &mut self,
        graph: &Graph,
        owner: Option<&NodeDef>,
        scope: &Scope,
        id: NodeId,
        kind: &NodeKind,
    ) -> Result<(), Fail> {
        for decl in kind.inputs(self.src.defs, owner) {
            if matches!(decl.ty, SocketType::Number | SocketType::Vec3)
                && !takes_field(kind, decl.key)
                && self
                    .linked(graph, owner, scope, id, decl.key)
                    .is_some_and(|d| d.is_field())
            {
                return Err(Fail::new(t!("assembly.node.error.field_here")));
            }
        }
        Ok(())
    }

    /// Node `id`'s outputs, evaluated once per scope.
    fn node(
        &mut self,
        graph: &Graph,
        owner: Option<&NodeDef>,
        scope: &Scope,
        id: NodeId,
    ) -> Outputs {
        let key = (scope.clone(), id);
        if let Some(done) = self.memo.get(&key) {
            return done.clone();
        }
        let Some(node) = graph.node(id) else {
            return Rc::new(Vec::new());
        };
        if !self.visiting.insert(key.clone()) {
            self.error(scope, id, Fail::new(t!("assembly.node.error.cycle")));
            return Rc::new(Vec::new());
        }
        let outputs = if node.bypass {
            self.pass_through(graph, owner, scope, id, &node.kind)
        } else {
            let computed = self
                .check_fields(graph, owner, scope, id, &node.kind)
                .and_then(|()| self.compute(graph, owner, scope, id, &node.kind));
            match computed {
                Ok(outputs) => outputs,
                Err(fail) => {
                    self.error(scope, id, fail);
                    self.pass_through(graph, owner, scope, id, &node.kind)
                }
            }
        };
        self.visiting.remove(&key);
        for (out, data) in &outputs {
            let shown = match data {
                Data::Set(s) => {
                    self.out.sets.insert((scope.clone(), id, *out), s.clone());
                    Shown::Count(s.len())
                }
                Data::Number(v) => Shown::Number(*v),
                Data::Vec3(v) => Shown::Vec3(*v),
                Data::Points(p) => Shown::Count(p.frames.len()),
                Data::Curve(c) => Shown::Count(c.splines.iter().map(|s| s.points.len()).sum()),
                Data::Field(_) | Data::Field3(_) => Shown::Field,
            };
            self.out.shown.insert((scope.clone(), id, *out), shown);
        }
        let outputs = Rc::new(outputs);
        self.memo.insert(key, outputs.clone());
        outputs
    }

    /// A bypassed or failed node: its first molecule input on its first molecule output, none
    /// on the others, numbers as they are without it (0).
    fn pass_through(
        &mut self,
        graph: &Graph,
        owner: Option<&NodeDef>,
        scope: &Scope,
        id: NodeId,
        kind: &NodeKind,
    ) -> Vec<(OutKey, Data)> {
        let first_set = kind
            .inputs(self.src.defs, owner)
            .into_iter()
            .find(|i| i.ty == SocketType::Set)
            .map(|i| i.key);
        let mut through = first_set.map(|key| self.set_input(graph, owner, scope, id, key));
        kind.outputs(self.src.defs, owner)
            .into_iter()
            .map(|o| {
                let data = match o.ty {
                    SocketType::Set => {
                        Data::Set(through.take().unwrap_or_else(|| Arc::new(Set::default())))
                    }
                    ty => default_data(ty, None),
                };
                (o.key, data)
            })
            .collect()
    }

    fn compute(
        &mut self,
        graph: &Graph,
        owner: Option<&NodeDef>,
        scope: &Scope,
        id: NodeId,
        kind: &NodeKind,
    ) -> Result<Vec<(OutKey, Data)>, Fail> {
        let set = |s: Set| -> Result<Vec<(OutKey, Data)>, Fail> {
            if s.len() > MAX_INSTANCES {
                return Err(Fail::new(t!(
                    "assembly.node.error.too_many",
                    count = s.len(),
                    max = MAX_INSTANCES
                )));
            }
            Ok(vec![(OutKey::Set, Data::Set(Arc::new(s)))])
        };
        match kind {
            NodeKind::Monomer { component } => {
                let Some(c) = self
                    .src
                    .component_ids
                    .index(*component, self.src.components.len())
                else {
                    self.warn(
                        scope,
                        id,
                        t!("assembly.node.error.monomer_missing").to_owned(),
                    );
                    return set(Set::default());
                };
                set(Set {
                    items: vec![Instance {
                        component: c,
                        transform: Rigid::IDENTITY,
                    }],
                    attrs: Vec::new(),
                    crystal: None,
                })
            }
            NodeKind::Op(step) => {
                let mut step = step.clone();
                for field in StepField::of(&step) {
                    if let Some(data) = self.linked(graph, owner, scope, id, InKey::Field(field))
                        && let Some(value) = data.as_port_value(field.ty())
                    {
                        field.write(&mut step, value);
                    }
                }
                let input = self.set_input(graph, owner, scope, id, InKey::Set(0));
                set(self.op(&step, &input)?)
            }
            NodeKind::Merge { inputs } => {
                let sets: Vec<Arc<Set>> = (0..*inputs)
                    .map(|i| self.set_input(graph, owner, scope, id, InKey::Set(i)))
                    .collect();
                if sets.iter().skip(1).any(|s| s.crystal.is_some()) {
                    self.warn(
                        scope,
                        id,
                        t!("assembly.node.error.crystal_dropped").to_owned(),
                    );
                }
                set(merge(&sets))
            }
            NodeKind::Select(sel) => {
                let input = self.set_input(graph, owner, scope, id, InKey::Set(0));
                let vars = self.var_fields(graph, owner, scope, id, &sel.vars);
                let keep = self.per_item(&input, &vars, &sel.expr, |v| v != 0.0 && !v.is_nan())?;
                let (chosen, rest): (Vec<usize>, Vec<usize>) =
                    (0..input.len()).partition(|&k| keep[k]);
                let mut selected = input.gather(&chosen);
                if let Some(crystal) = &input.crystal {
                    if chosen.len() >= crystal.z
                        && chosen[..crystal.z].iter().copied().eq(0..crystal.z)
                    {
                        selected.crystal = Some(crystal.clone());
                    } else {
                        self.warn(
                            scope,
                            id,
                            t!("assembly.node.error.crystal_dropped").to_owned(),
                        );
                    }
                }
                Ok(vec![
                    (OutKey::Selected, Data::Set(Arc::new(selected))),
                    (OutKey::Rest, Data::Set(Arc::new(input.gather(&rest)))),
                ])
            }
            NodeKind::Repeat(r) => {
                let input = self.set_input(graph, owner, scope, id, InKey::Set(0));
                let n = count_of(self.number_input(
                    graph,
                    owner,
                    scope,
                    id,
                    InKey::Count,
                    f64::from(r.n),
                ));
                let vars = self.var_values(graph, owner, scope, id, &r.vars)?;
                let mut names: Vec<String> = vec!["i".into(), "n".into()];
                names.extend(vars.iter().map(|(name, _)| name.clone()));
                let resolve = |name: &str| names.iter().rposition(|n| n == name);
                let parse = |src: &String| Expr::parse(src, &resolve).map_err(expr_fail);
                let offset = r.offset.iter().map(parse).collect::<Result<Vec<_>, _>>()?;
                let euler = r
                    .euler_deg
                    .iter()
                    .map(parse)
                    .collect::<Result<Vec<_>, _>>()?;
                let mut values = vec![0.0, f64::from(n)];
                values.extend(vars.iter().map(|(_, v)| *v));
                let mut moves = Vec::with_capacity(n as usize);
                for i in 0..n {
                    values[0] = f64::from(i);
                    let o = [0, 1, 2].map(|a| offset[a].eval(&values));
                    let e = [0, 1, 2].map(|a| euler[a].eval(&values));
                    if o.iter().chain(&e).any(|v| !v.is_finite()) {
                        return Err(Fail::new(t!("assembly.node.error.not_finite", at = i)));
                    }
                    moves.push(Rigid::rotation(math::euler_xyz(e)).then(&Rigid::translation(o)));
                }
                if input.len().saturating_mul(moves.len()) > MAX_INSTANCES {
                    return Err(Fail::new(t!(
                        "assembly.node.error.too_many",
                        count = input.len().saturating_mul(moves.len()),
                        max = MAX_INSTANCES
                    )));
                }
                set(input.copied(&moves, r.name.trim()))
            }
            NodeKind::MoveEach(m) => {
                let input = self.set_input(graph, owner, scope, id, InKey::Set(0));
                let vars = self.var_fields(graph, owner, scope, id, &m.vars);
                let columns: Vec<Vec<f64>> = m
                    .offset
                    .iter()
                    .chain(&m.euler_deg)
                    .map(|src| self.per_item(&input, &vars, src, |v| v))
                    .collect::<Result<_, _>>()?;
                let set_centre = (m.pivot == EachPivot::Set).then(|| self.set_centroid(&input));
                let mut out = (*input).clone();
                for (k, inst) in out.items.iter_mut().enumerate() {
                    let o = [columns[0][k], columns[1][k], columns[2][k]];
                    let e = [columns[3][k], columns[4][k], columns[5][k]];
                    if o.iter().chain(&e).any(|v| !v.is_finite()) {
                        return Err(Fail::new(t!("assembly.node.error.not_finite", at = k)));
                    }
                    let pivot = match m.pivot {
                        EachPivot::Origin => [0.0; 3],
                        EachPivot::Own => self.item_centroid(inst),
                        EachPivot::Set => set_centre.unwrap_or([0.0; 3]),
                    };
                    let t = Rigid::translation(math::scale(pivot, -1.0))
                        .then(&Rigid::rotation(math::euler_xyz(e)))
                        .then(&Rigid::translation(math::add(pivot, o)));
                    inst.transform = inst.transform.then(&t);
                }
                set(out)
            }
            NodeKind::SetAttr(a) => {
                let name = a.name.trim();
                if !is_name(name) {
                    return Err(Fail::new(t!("assembly.node.error.bad_name", name = name)));
                }
                let input = self.set_input(graph, owner, scope, id, InKey::Set(0));
                let vars = self.var_fields(graph, owner, scope, id, &a.vars);
                let values = self.per_item(&input, &vars, &a.expr, |v| v)?;
                let mut out = (*input).clone();
                out.set_attr(name, values);
                set(out)
            }
            NodeKind::Value { value } => Ok(vec![(OutKey::Number, Data::Number(*value))]),
            NodeKind::Vector { value } => Ok(vec![(OutKey::Vec3, Data::Vec3(*value))]),
            NodeKind::Expression(e) => {
                let vars = self.var_fields(graph, owner, scope, id, &e.vars);
                let resolve = |name: &str| vars.iter().rposition(|(n, _)| n == name);
                let expr = Expr::parse(&e.expr, &resolve).map_err(expr_fail)?;
                let Some(values) = vars
                    .iter()
                    .map(|(_, f)| f.constant())
                    .collect::<Option<Vec<f64>>>()
                else {
                    let args = vars.into_iter().map(|(_, f)| f).collect();
                    let field = Field::Expr(Rc::new(expr), args);
                    return Ok(vec![(OutKey::Number, Data::Field(Rc::new(field)))]);
                };
                let v = expr.eval(&values);
                if !v.is_finite() {
                    return Err(Fail::new(t!("assembly.node.error.not_a_number")));
                }
                Ok(vec![(OutKey::Number, Data::Number(v))])
            }
            NodeKind::Combine { value } => {
                let v = [0u8, 1, 2].map(|i| {
                    self.field_input(
                        graph,
                        owner,
                        scope,
                        id,
                        InKey::Axis(i),
                        value[usize::from(i)],
                    )
                });
                match [0, 1, 2].map(|a| v[a].constant()) {
                    [Some(x), Some(y), Some(z)] => Ok(vec![(OutKey::Vec3, Data::Vec3([x, y, z]))]),
                    _ => Ok(vec![(OutKey::Vec3, Data::Field3(Rc::new(v)))]),
                }
            }
            NodeKind::Separate { value } => {
                let v = match self.linked(graph, owner, scope, id, InKey::Value) {
                    Some(Data::Vec3(v)) => v,
                    Some(Data::Number(n)) => [n; 3],
                    Some(d @ (Data::Field(_) | Data::Field3(_))) => {
                        let f = match d {
                            Data::Field3(f) => (*f).clone(),
                            Data::Field(f) => [(*f).clone(), (*f).clone(), (*f).clone()],
                            _ => unreachable!(),
                        };
                        return Ok(f
                            .into_iter()
                            .zip(0u8..)
                            .map(|(f, i)| (OutKey::Axis(i), Data::Field(Rc::new(f))))
                            .collect());
                    }
                    _ => *value,
                };
                Ok((0..3u8)
                    .map(|i| (OutKey::Axis(i), Data::Number(v[usize::from(i)])))
                    .collect())
            }
            NodeKind::Circle(c) => {
                let n = count_of(self.number_input(
                    graph,
                    owner,
                    scope,
                    id,
                    InKey::Count,
                    f64::from(c.resolution),
                ));
                let r = self.number_input(graph, owner, scope, id, InKey::Param(0), c.radius);
                if !r.is_finite() {
                    return Err(Fail::new(t!("assembly.node.error.not_a_number")));
                }
                let points = (0..n)
                    .map(|i| {
                        let a = std::f64::consts::TAU * f64::from(i) / f64::from(n);
                        [r * a.cos(), r * a.sin(), 0.0]
                    })
                    .collect();
                let curve = Curve {
                    splines: vec![Spline {
                        points,
                        closed: true,
                    }],
                };
                Ok(vec![(OutKey::Curve, Data::Curve(Arc::new(curve)))])
            }
            NodeKind::CurveToPoints(c) => {
                let curve = match self.linked(graph, owner, scope, id, InKey::Curve) {
                    Some(Data::Curve(curve)) => curve,
                    _ => Arc::new(Curve::default()),
                };
                let count = count_of(self.number_input(
                    graph,
                    owner,
                    scope,
                    id,
                    InKey::Count,
                    f64::from(c.count),
                ));
                let length = self.number_input(graph, owner, scope, id, InKey::Param(0), c.length);
                let sample = match c.mode {
                    CurveSample::Count => Sample::Count(count as usize),
                    CurveSample::Length if length.is_finite() && length > 1e-3 => {
                        Sample::Length(length)
                    }
                    CurveSample::Length => {
                        return Err(Fail::new(t!("assembly.node.error.bad_length")));
                    }
                    CurveSample::Evaluated => Sample::Evaluated,
                };
                let mut frames = Vec::new();
                for spline in &curve.splines {
                    frames.extend(spline_points(spline, sample));
                    if frames.len() > MAX_INSTANCES {
                        return Err(Fail::new(t!(
                            "assembly.node.error.too_many",
                            count = frames.len(),
                            max = MAX_INSTANCES
                        )));
                    }
                }
                Ok(vec![(
                    OutKey::Points,
                    Data::Points(Arc::new(Points { frames })),
                )])
            }
            NodeKind::InstanceOnPoints(p) => {
                let points = match self.linked(graph, owner, scope, id, InKey::Points) {
                    Some(Data::Points(points)) => points,
                    _ => Arc::new(Points::default()),
                };
                let input = self.set_input(graph, owner, scope, id, InKey::Set(0));
                let n = points.frames.len();
                let chosen = self
                    .field_input(graph, owner, scope, id, InKey::Param(0), p.selection)
                    .column(n);
                let turn = self
                    .field3_input(graph, owner, scope, id, InKey::Param(1), p.rotation_deg)
                    .map(|f| f.column(n));
                let mut moves = Vec::new();
                let mut used = Vec::new();
                for (k, frame) in points.frames.iter().enumerate() {
                    if chosen[k] == 0.0 || chosen[k].is_nan() {
                        continue;
                    }
                    let e = [turn[0][k], turn[1][k], turn[2][k]];
                    if e.iter().any(|v| !v.is_finite()) {
                        return Err(Fail::new(t!("assembly.node.error.not_finite", at = k)));
                    }
                    let place = if p.align {
                        *frame
                    } else {
                        Rigid::translation(frame.trans)
                    };
                    moves.push(Rigid::rotation(math::euler_xyz(e)).then(&place));
                    used.push(k as f64);
                }
                if input.len().saturating_mul(moves.len()) > MAX_INSTANCES {
                    return Err(Fail::new(t!(
                        "assembly.node.error.too_many",
                        count = input.len().saturating_mul(moves.len()),
                        max = MAX_INSTANCES
                    )));
                }
                if input.crystal.is_some() {
                    self.warn(
                        scope,
                        id,
                        t!("assembly.node.error.crystal_dropped").to_owned(),
                    );
                }
                let mut out = input.copied(&moves, "");
                out.crystal = None;
                let point = used
                    .iter()
                    .flat_map(|&k| std::iter::repeat_n(k, input.len()))
                    .collect();
                out.set_attr("point", point);
                set(out)
            }
            NodeKind::Index => Ok(vec![(OutKey::Number, Data::Field(Rc::new(Field::Index)))]),
            NodeKind::Group(g) => self.group(graph, owner, scope, id, &g.def, &g.inputs),
            NodeKind::GroupInput => {
                let bound = self.bindings.get(scope).cloned().unwrap_or_default();
                Ok(owner
                    .map(|d| {
                        d.inputs
                            .iter()
                            .map(|p| {
                                let data = bound
                                    .iter()
                                    .find(|(k, _)| *k == p.key)
                                    .map(|(_, d)| d.clone())
                                    .unwrap_or_else(|| default_data(p.ty, p.default));
                                (OutKey::Port(p.key), data)
                            })
                            .collect()
                    })
                    .unwrap_or_default())
            }
            NodeKind::GroupOutput | NodeKind::Output => Ok(Vec::new()),
        }
    }

    /// A custom node: its inputs bound, its graph evaluated inside it.
    fn group(
        &mut self,
        graph: &Graph,
        owner: Option<&NodeDef>,
        scope: &Scope,
        id: NodeId,
        def_id: &DefId,
        inline: &std::collections::BTreeMap<u32, PortValue>,
    ) -> Result<Vec<(OutKey, Data)>, Fail> {
        let defs_all = self.src.defs;
        let Some(def) = defs::find(defs_all, def_id) else {
            return Err(Fail::new(t!("assembly.node.error.missing_def")));
        };
        if self.def_stack.contains(def_id) {
            return Err(Fail::new(t!(
                "assembly.node.error.recursion",
                name = def.name
            )));
        }
        if self.def_stack.len() >= MAX_DEPTH {
            return Err(Fail::new(t!(
                "assembly.node.error.too_deep",
                max = MAX_DEPTH
            )));
        }
        let mut bound = Vec::with_capacity(def.inputs.len());
        for p in &def.inputs {
            let data = match self.linked(graph, owner, scope, id, InKey::Port(p.key)) {
                Some(d) => d
                    .conform(p.ty)
                    .unwrap_or_else(|| default_data(p.ty, p.default)),
                None => match (inline.get(&p.key), p.ty) {
                    (Some(v), SocketType::Number) => Data::Number(v.number()),
                    (Some(v), SocketType::Vec3) => Data::Vec3(v.vec3()),
                    _ => default_data(p.ty, p.default),
                },
            };
            bound.push((p.key, data));
        }
        let mut inner = scope.clone();
        inner.push(id);
        self.bindings.insert(inner.clone(), Rc::new(bound));
        self.def_stack.push(def_id.clone());
        let out_node = def.graph.group_output();
        let mut outputs = Vec::with_capacity(def.outputs.len());
        if let Some(out_node) = out_node {
            self.mark(&inner, out_node);
        }
        for p in &def.outputs {
            let data = out_node
                .and_then(|o| self.linked(&def.graph, Some(def), &inner, o, InKey::Port(p.key)))
                .and_then(|d| d.conform(p.ty))
                .unwrap_or_else(|| default_data(p.ty, p.default));
            outputs.push((OutKey::Port(p.key), data));
        }
        self.def_stack.pop();
        Ok(outputs)
    }

    /// A building step applied to `input`.
    fn op(&self, step: &Step, input: &Set) -> Result<Set, Fail> {
        let shapes = self.src.shapes;
        match step {
            Step::Lattice(lattice) => {
                lattice.check().map_err(|message| Fail {
                    message,
                    lattice: true,
                })?;
                let unit = lattice.unit_cell_sources(&input.items, shapes);
                let translations = lattice.translations();
                if unit.len().saturating_mul(translations.len()) > MAX_INSTANCES {
                    return Err(Fail::new(t!(
                        "assembly.node.error.too_many",
                        count = unit.len().saturating_mul(translations.len()),
                        max = MAX_INSTANCES
                    )));
                }
                let picks: Vec<usize> = unit.iter().map(|(_, src)| *src).collect();
                let mut cell = input.gather(&picks);
                cell.items = unit.iter().map(|(inst, _)| inst.clone()).collect();
                let mut out = cell.copied(&translations, "cell");
                out.crystal = Some(Crystal {
                    cell: lattice.cell,
                    reps: lattice.reps.map(|r| r.max(1)),
                    symops: lattice.symops.clone(),
                    z: unit.len(),
                    asymmetric: input.items.clone(),
                });
                Ok(out)
            }
            Step::Transform(transform) => {
                let t = transform.of_set(&input.items, shapes);
                let mut out = input.clone();
                for inst in &mut out.items {
                    inst.transform = inst.transform.then(&t);
                }
                Ok(out)
            }
            Step::Bend(bend) => {
                let moves = bend.of_set(&input.items, shapes);
                let mut out = input.clone();
                for (inst, t) in out.items.iter_mut().zip(&moves) {
                    inst.transform = inst.transform.then(t);
                }
                Ok(out)
            }
            Step::Ring(_) | Step::Stack(_) | Step::Array(_) => {
                let moves = step.transforms();
                if input.len().saturating_mul(moves.len()) > MAX_INSTANCES {
                    return Err(Fail::new(t!(
                        "assembly.node.error.too_many",
                        count = input.len().saturating_mul(moves.len()),
                        max = MAX_INSTANCES
                    )));
                }
                let name = match step {
                    Step::Ring(_) => "ring",
                    Step::Stack(_) => "layer",
                    _ => "row",
                };
                Ok(input.copied(&moves, name))
            }
        }
    }

    /// The centroid of molecule `inst` (rigid, so its local centroid moved).
    fn item_centroid(&self, inst: &Instance) -> Vec3 {
        inst.transform.apply(
            self.centroids
                .get(inst.component)
                .copied()
                .unwrap_or([0.0; 3]),
        )
    }

    /// The centroid of every atom of `set`, as a move/rotate step takes it.
    fn set_centroid(&self, set: &Set) -> Vec3 {
        let atoms: Vec<Vec3> = set
            .items
            .iter()
            .filter_map(|inst| {
                let (positions, _) = self.src.shapes.get(inst.component)?.as_ref()?;
                Some(positions.iter().map(|&p| inst.transform.apply(p)))
            })
            .flatten()
            .collect();
        centroid(&atoms)
    }

    /// `src` evaluated for each molecule of `set`, turned into `T` by `map`. The variables are
    /// the node's own (`vars`), the molecule's attributes, `k`, `count`, `comp`, `x`, `y`,
    /// `z`, and `i`, `n` for its `copy` and `copies`.
    fn per_item<T>(
        &self,
        set: &Set,
        vars: &[(String, Field)],
        src: &str,
        map: impl Fn(f64) -> T,
    ) -> Result<Vec<T>, Fail> {
        #[derive(Clone, Copy)]
        enum Slot {
            Var(f64),
            /// A variable fed a field: its column.
            Column(usize),
            Attr(usize),
            K,
            Count,
            Comp,
            Axis(usize),
            Copy,
            Copies,
        }
        let slots = std::cell::RefCell::new(Vec::<(String, Slot)>::new());
        let columns = std::cell::RefCell::new(Vec::<Vec<f64>>::new());
        let resolve = |name: &str| {
            let mut slots = slots.borrow_mut();
            if let Some(k) = slots.iter().position(|(n, _)| n == name) {
                return Some(k);
            }
            let slot = if let Some((_, f)) = vars.iter().rev().find(|(n, _)| n == name) {
                match f.constant() {
                    Some(v) => Slot::Var(v),
                    None => {
                        let mut columns = columns.borrow_mut();
                        columns.push(f.column(set.len()));
                        Slot::Column(columns.len() - 1)
                    }
                }
            } else if let Some(a) = set.attrs.iter().position(|(n, _)| &**n == name) {
                Slot::Attr(a)
            } else {
                match name {
                    "k" => Slot::K,
                    "count" => Slot::Count,
                    "comp" => Slot::Comp,
                    "x" => Slot::Axis(0),
                    "y" => Slot::Axis(1),
                    "z" => Slot::Axis(2),
                    "i" => Slot::Copy,
                    "n" => Slot::Copies,
                    _ => return None,
                }
            };
            slots.push((name.to_owned(), slot));
            Some(slots.len() - 1)
        };
        let expr = Expr::parse(src, &resolve).map_err(expr_fail)?;
        let slots: Vec<Slot> = slots.into_inner().into_iter().map(|(_, s)| s).collect();
        let columns = columns.into_inner();
        let copy = set.attr("copy");
        let copies = set.attr("copies");
        let needs_centroid = slots.iter().any(|s| matches!(s, Slot::Axis(_)));
        let mut values = vec![0.0; slots.len()];
        let mut out = Vec::with_capacity(set.len());
        for (k, inst) in set.items.iter().enumerate() {
            let c = if needs_centroid {
                self.item_centroid(inst)
            } else {
                [0.0; 3]
            };
            for (v, slot) in values.iter_mut().zip(&slots) {
                *v = match *slot {
                    Slot::Var(v) => v,
                    Slot::Column(c) => columns[c][k],
                    Slot::Attr(a) => set.attrs[a].1[k],
                    Slot::K => k as f64,
                    Slot::Count => set.len() as f64,
                    Slot::Comp => inst.component as f64,
                    Slot::Axis(a) => c[a],
                    Slot::Copy => copy.map_or(0.0, |v| v[k]),
                    Slot::Copies => copies.map_or(1.0, |v| v[k]),
                };
            }
            out.push(map(expr.eval(&values)));
        }
        Ok(out)
    }
}

/// The molecules of every set in order, with attribute `input` the set's index. Attributes
/// some sets lack are 0 for their molecules. The first set's crystal is kept.
fn merge(sets: &[Arc<Set>]) -> Set {
    let total: usize = sets.iter().map(|s| s.len()).sum();
    let mut names: Vec<Arc<str>> = Vec::new();
    for s in sets {
        for (n, _) in &s.attrs {
            if !names.iter().any(|m| m == n) {
                names.push(n.clone());
            }
        }
    }
    let mut items = Vec::with_capacity(total);
    let mut attrs: Vec<(Arc<str>, Vec<f64>)> = names
        .into_iter()
        .map(|n| (n, Vec::with_capacity(total)))
        .collect();
    let mut input = Vec::with_capacity(total);
    for (i, s) in sets.iter().enumerate() {
        items.extend(s.items.iter().cloned());
        for (name, column) in &mut attrs {
            match s.attr(name) {
                Some(v) => column.extend_from_slice(v),
                None => column.extend(std::iter::repeat_n(0.0, s.len())),
            }
        }
        input.extend(std::iter::repeat_n(i as f64, s.len()));
    }
    let mut out = Set {
        items,
        attrs,
        crystal: sets.first().and_then(|s| s.crystal.clone()),
    };
    out.set_attr("input", input);
    out
}

/// Whether input `key` of a node of kind `kind` takes a field (a value per element).
fn takes_field(kind: &NodeKind, key: InKey) -> bool {
    matches!(
        (kind, key),
        (
            NodeKind::Select(_)
                | NodeKind::MoveEach(_)
                | NodeKind::SetAttr(_)
                | NodeKind::Expression(_),
            InKey::Var(_),
        ) | (NodeKind::Combine { .. }, InKey::Axis(_))
            | (NodeKind::Separate { .. }, InKey::Value)
            | (NodeKind::InstanceOnPoints(_), InKey::Param(_))
            | (NodeKind::Group(_) | NodeKind::GroupOutput, InKey::Port(_))
    )
}

/// How [`spline_points`] places points.
#[derive(Clone, Copy)]
enum Sample {
    Count(usize),
    Length(f64),
    Evaluated,
}

/// Points along `spline`, each framed: x along the curve, z normal to its plane.
fn spline_points(spline: &Spline, sample: Sample) -> Vec<Rigid> {
    let pts = &spline.points;
    let n = pts.len();
    if n == 0 {
        return Vec::new();
    }
    let closed = spline.closed && n > 2;
    let segments = if closed { n } else { n - 1 };
    let seg = |i: usize| (pts[i], pts[(i + 1) % n]);
    // Newell's normal of the polygon; z for a straight line or a point.
    let mut normal = [0.0; 3];
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        normal = math::add(normal, math::cross(a, b));
    }
    let normal = math::unit(normal).unwrap_or([0.0, 0.0, 1.0]);
    let dir = |i: usize| {
        let (a, b) = seg(i);
        math::unit(math::sub(b, a))
    };
    // The tangent at vertex `i`: between the segments meeting there.
    let vertex_tangent = |i: usize| {
        let before = if i > 0 {
            dir(i - 1)
        } else if closed {
            dir(n - 1)
        } else {
            None
        };
        let after = if i < segments { dir(i) } else { None };
        match (before, after) {
            (Some(a), Some(b)) => math::unit(math::add(a, b)).or(Some(b)),
            (a, b) => a.or(b),
        }
    };
    let frame = |at: Vec3, tangent: Option<Vec3>| {
        let x = tangent.unwrap_or([1.0, 0.0, 0.0]);
        let z = math::unit(math::sub(normal, math::scale(x, math::dot(normal, x))))
            .or_else(|| math::unit(math::cross(x, [0.0, 0.0, 1.0])))
            .or_else(|| math::unit(math::cross(x, [0.0, 1.0, 0.0])))
            .unwrap_or([0.0, 0.0, 1.0]);
        let y = math::cross(z, x);
        Rigid {
            rot: [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]],
            trans: at,
        }
    };
    if let Sample::Evaluated = sample {
        return (0..n).map(|i| frame(pts[i], vertex_tangent(i))).collect();
    }
    let lengths: Vec<f64> = (0..segments)
        .map(|i| {
            let (a, b) = seg(i);
            math::distance(a, b)
        })
        .collect();
    let total: f64 = lengths.iter().sum();
    let at_length = |s: f64| {
        let mut left = s;
        for (i, &l) in lengths.iter().enumerate() {
            if left <= l * (1.0 - 1e-9) || i + 1 == segments {
                if left <= l * 1e-9 {
                    return frame(pts[i], vertex_tangent(i));
                }
                let (a, b) = seg(i);
                let t = if l > 0.0 { (left / l).min(1.0) } else { 0.0 };
                if t >= 1.0 - 1e-9 {
                    return frame(b, vertex_tangent((i + 1) % n.max(1)).or(dir(i)));
                }
                return frame(math::add(a, math::scale(math::sub(b, a), t)), dir(i));
            }
            left -= l;
        }
        frame(pts[0], vertex_tangent(0))
    };
    if total <= 0.0 || segments == 0 {
        return vec![frame(pts[0], vertex_tangent(0))];
    }
    let places: Vec<f64> = match sample {
        Sample::Count(0) => Vec::new(),
        Sample::Count(1) => vec![0.0],
        Sample::Count(c) if closed => (0..c).map(|i| total * i as f64 / c as f64).collect(),
        Sample::Count(c) => (0..c).map(|i| total * i as f64 / (c - 1) as f64).collect(),
        Sample::Length(step) => {
            let m = (total / step + 1e-9).floor() as usize;
            let m = if closed { m.max(1) } else { m + 1 };
            (0..m.min(MAX_INSTANCES + 1))
                .map(|i| i as f64 * step)
                .collect()
        }
        Sample::Evaluated => Vec::new(),
    };
    places.into_iter().map(at_length).collect()
}

fn default_data(ty: SocketType, default: Option<PortValue>) -> Data {
    match ty {
        SocketType::Set => Data::Set(Arc::new(Set::default())),
        SocketType::Curve => Data::Curve(Arc::new(Curve::default())),
        SocketType::Points => Data::Points(Arc::new(Points::default())),
        SocketType::Number => Data::Number(default.map_or(0.0, PortValue::number)),
        SocketType::Vec3 => Data::Vec3(default.map_or([0.0; 3], PortValue::vec3)),
    }
}

fn expr_fail(e: super::expr::ExprError) -> Fail {
    Fail::new(e.to_string())
}

/// Whether `name` can be an attribute: a letter or `_`, then letters, digits and `_`.
pub fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::assembly::graph::kinds::{GroupRef, MoveEach, Repeat, Select};
    use crate::assembly::graph::{Link, chain_step_id, ops};
    use crate::assembly::math::Cell;
    use crate::assembly::model::{Array, Design, Lattice, Stack};
    use crate::assembly::symop;
    use crate::three_d::{Atom3d, Bond3d};

    fn component(name: &str) -> Component {
        let atom = |e: &str, pos| Atom3d {
            element: e.into(),
            pos,
        };
        Component {
            name: name.into(),
            atoms: vec![
                atom("C", [0.3, 0.1, 0.2]),
                atom("O", [1.5, 0.2, 0.1]),
                atom("N", [0.1, 1.3, -0.4]),
            ],
            bonds: vec![Bond3d {
                begin: 0,
                end: 1,
                order: 1,
            }],
            orient: None,
        }
    }

    fn design(steps: Vec<Step>) -> Design {
        Design {
            components: vec![component("a"), component("b")],
            steps,
            ..Default::default()
        }
    }

    fn same(a: &[Instance], b: &[Instance]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(x, y)| {
                x.component == y.component && x.transform.approx_eq(&y.transform, 1e-9)
            })
    }

    fn lattice() -> Step {
        let mut l = Step::new_lattice();
        if let Step::Lattice(Lattice {
            cell, reps, symops, ..
        }) = &mut l
        {
            *cell = Cell {
                a: 8.0,
                b: 9.0,
                c: 10.0,
                alpha: 90.0,
                beta: 100.0,
                gamma: 90.0,
            };
            *reps = [2, 1, 2];
            *symops = symop::PRESETS
                .iter()
                .find(|p| p.name == "P2₁/c")
                .expect("P2₁/c is a preset")
                .symops();
        }
        l
    }

    /// Every list of steps evaluates as a chain to what the steps made before.
    #[test]
    fn a_chain_makes_what_the_steps_made() {
        let all = [
            Step::new_ring(),
            Step::new_stack(),
            Step::new_array(),
            Step::new_transform(),
            Step::new_bend(),
            lattice(),
        ];
        let mut tilted = Step::new_stack();
        if let Step::Stack(Stack {
            twist_deg, slip, ..
        }) = &mut tilted
        {
            *twist_deg = 17.0;
            *slip = 1.3;
        }
        let mut cases: Vec<Vec<Step>> = vec![vec![]];
        for (i, a) in all.iter().enumerate() {
            cases.push(vec![a.clone()]);
            for b in &all[i..] {
                cases.push(vec![a.clone(), b.clone()]);
                cases.push(vec![b.clone(), tilted.clone(), a.clone()]);
            }
        }
        for mut steps in cases {
            for off in [None, Some(0)] {
                if let Some(k) = off
                    && let Some(s) = steps.get_mut(k)
                {
                    *s.enabled_mut() = false;
                }
                let d = design(steps.clone());
                let mut counts = Vec::new();
                let (old, old_crystal) = d.trace_step_copies(|_, n| counts.push(n));
                let (new, new_crystal) = d.placements();
                assert!(same(&old, &new), "{steps:?}");
                assert_eq!(old_crystal.map(|c| c.z), new_crystal.map(|c| c.z));
                assert_eq!(d.running_counts(), counts, "{steps:?}");
                // Made into a graph, it still builds the same.
                let mut g = d.clone();
                g.graph_mut();
                assert!(g.steps.is_empty());
                assert!(same(&g.placements().0, &new));
            }
        }
    }

    #[test]
    fn an_impossible_crystal_is_reported_and_passed_through() {
        let mut bad = lattice();
        if let Step::Lattice(l) = &mut bad {
            l.cell.a = -1.0;
        }
        let d = design(vec![Step::new_ring(), bad]);
        assert!(d.lattice_error().is_some());
        assert_eq!(d.instances().len(), 6);
    }

    fn link(g: &mut Graph, from: NodeId, out: OutKey, to: NodeId, input: InKey) {
        ops::connect(g, &[], None, from, out, to, input).unwrap();
    }

    /// A on the bottom, B 3.5 Å above it turned 30°, stacked: an alternating column.
    #[test]
    fn two_monomers_merge_into_an_alternating_stack() {
        let mut d = design(vec![]);
        let b_id = d.component_id(1);
        let g = d.graph_mut();
        let out = g.output().unwrap();
        let a = NodeId(1);
        let b = g.add(NodeKind::Monomer { component: b_id }, [0.0; 2]);
        let mut lift = Step::new_transform();
        if let Step::Transform(t) = &mut lift {
            t.offset = [0.0, 0.0, 3.5];
            t.euler_deg = [0.0, 0.0, 30.0];
        }
        let lift = g.add(NodeKind::Op(lift), [0.0; 2]);
        let merge = g.add(NodeKind::Merge { inputs: 2 }, [0.0; 2]);
        let mut stack = Step::new_stack();
        if let Step::Stack(s) = &mut stack {
            s.n = 5;
            s.rise = 7.0;
            s.twist_deg = 60.0;
        }
        let stack = g.add(NodeKind::Op(stack), [0.0; 2]);
        link(g, b, OutKey::Set, lift, InKey::Set(0));
        link(g, a, OutKey::Set, merge, InKey::Set(0));
        link(g, lift, OutKey::Set, merge, InKey::Set(1));
        link(g, merge, OutKey::Set, stack, InKey::Set(0));
        link(g, stack, OutKey::Set, out, InKey::Set(0));
        let ev = d.evaluation();
        assert!(ev.errors.is_empty(), "{:?}", ev.errors);
        let set = &ev.output;
        assert_eq!(set.len(), 10);
        let comps: Vec<usize> = set.items.iter().map(|i| i.component).collect();
        assert_eq!(comps, [0, 1, 0, 1, 0, 1, 0, 1, 0, 1]);
        assert_eq!(set.attr("input").unwrap()[..4], [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(set.attr("layer").unwrap()[..4], [0.0, 0.0, 1.0, 1.0]);
        // B of layer 2 sits at z = 14 + 3.5 (the move/rotate turns about its centroid).
        let c = centroid(&d.shapes()[1].as_ref().unwrap().0);
        let p = set.items[5].transform.apply(c);
        assert!((p[2] - (17.5 + c[2])).abs() < 1e-9, "{p:?}");
        assert_eq!(d.atom_count(), 30);
        assert_eq!(
            ev.shown.get(&(vec![], merge, OutKey::Set)),
            Some(&Shown::Count(2))
        );
    }

    /// Every other layer of a stack is turned, the rest kept: the halves add up.
    #[test]
    fn selecting_splits_the_set_and_merging_puts_it_back() {
        let mut stack = Step::new_stack();
        if let Step::Stack(s) = &mut stack {
            s.n = 6;
        }
        let mut d = design(vec![stack]);
        let g = d.graph_mut();
        let out = g.output().unwrap();
        let stack = chain_step_id(0);
        let select = g.add(
            NodeKind::Select(Select {
                expr: "layer % 2 == 0".into(),
                vars: vec![],
            }),
            [0.0; 2],
        );
        let mut turn = MoveEach::new();
        turn.euler_deg[2] = "15".into();
        let turn = g.add(NodeKind::MoveEach(turn), [0.0; 2]);
        let merge = g.add(NodeKind::Merge { inputs: 2 }, [0.0; 2]);
        link(g, select, OutKey::Selected, merge, InKey::Set(0));
        link(g, select, OutKey::Rest, turn, InKey::Set(0));
        link(g, turn, OutKey::Set, merge, InKey::Set(1));
        link(g, stack, OutKey::Set, select, InKey::Set(0));
        link(g, merge, OutKey::Set, out, InKey::Set(0));
        let ev = d.evaluation();
        assert!(ev.errors.is_empty(), "{:?}", ev.errors);
        assert_eq!(ev.output.len(), 6);
        assert_eq!(
            ev.output.attr("layer").unwrap(),
            &[0.0, 2.0, 4.0, 1.0, 3.0, 5.0]
        );
        // A turned molecule keeps its centroid.
        let local = centroid(&d.shapes()[0].as_ref().unwrap().0);
        let before = ev.sets[&(vec![], stack, OutKey::Set)].items[1].clone();
        let after = &ev.output.items[3];
        assert!(math::distance(before.transform.apply(local), after.transform.apply(local)) < 1e-9);
        assert!(!before.transform.approx_eq(&after.transform, 1e-6));
    }

    /// A repeat with expressions linear in i is an array.
    #[test]
    fn a_linear_repeat_is_an_array() {
        let array = Step::Array(Array {
            enabled: true,
            n: 5,
            offset: [1.0, 2.0, 3.5],
            euler_deg: [0.0, 10.0, 30.0],
        });
        let d = design(vec![array]);
        let mut r = design(vec![]);
        let g = r.graph_mut();
        let out = g.output().unwrap();
        g.links.clear();
        let rep = g.add(
            NodeKind::Repeat(Repeat {
                n: 5,
                name: "row".into(),
                offset: ["1*i".into(), "2*i".into(), "3.5*i".into()],
                euler_deg: ["0".into(), "10*i".into(), "30*i".into()],
                vars: vec![],
            }),
            [0.0; 2],
        );
        link(g, NodeId(1), OutKey::Set, rep, InKey::Set(0));
        link(g, rep, OutKey::Set, out, InKey::Set(0));
        assert!(same(&d.instances(), &r.instances()));
        // A twist that is not linear: copy 3 turned by 90 + 5 sin 3.
        if let Some(n) = r.graph_mut().node_mut(rep)
            && let NodeKind::Repeat(rep) = &mut n.kind
        {
            rep.euler_deg = ["0".into(), "0".into(), "30*i + 5*sin(i)".into()];
        }
        let x = r.instances()[3].transform.rot;
        let angle = x[1][0].atan2(x[0][0]).to_degrees();
        assert!((angle - (90.0 + 5.0 * 3f64.sin())).abs() < 1e-9, "{angle}");
    }

    /// A monomer instanced on a circle's points (turned back a quarter, since a point's x axis
    /// runs along the circle) is a ring.
    #[test]
    fn instances_on_a_circle_are_a_ring() {
        use crate::assembly::graph::kinds::{Circle, CurveToPoints, InstanceOnPoints};
        use crate::assembly::model::Ring;
        let ring = Step::Ring(Ring {
            enabled: true,
            n: 6,
            radius: 7.0,
            tilt_deg: 0.0,
        });
        let d = design(vec![ring]);
        for (resolution, count) in [(6, 6), (12, 6), (6, 12)] {
            let mut r = design(vec![]);
            let g = r.graph_mut();
            let out = g.output().unwrap();
            g.links.clear();
            let circle = g.add(
                NodeKind::Circle(Circle {
                    resolution,
                    radius: 7.0,
                }),
                [0.0; 2],
            );
            let mut sample = CurveToPoints::new();
            sample.count = count;
            let points = g.add(NodeKind::CurveToPoints(sample), [0.0; 2]);
            let mut put = InstanceOnPoints::new();
            put.rotation_deg = [0.0, 0.0, -90.0];
            let put = g.add(NodeKind::InstanceOnPoints(put), [0.0; 2]);
            link(g, circle, OutKey::Curve, points, InKey::Curve);
            link(g, points, OutKey::Points, put, InKey::Points);
            link(g, NodeId(1), OutKey::Set, put, InKey::Set(0));
            link(g, put, OutKey::Set, out, InKey::Set(0));
            let ev = r.evaluation();
            assert!(ev.errors.is_empty(), "{:?}", ev.errors);
            let want = d.instances();
            let got = r.instances();
            if count == 6 {
                assert!(same(&want, &got), "{resolution} points, {count} copies");
                assert_eq!(
                    ev.output.attr("point").unwrap(),
                    &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0]
                );
            } else {
                // Twelve copies on a hexagon: every other one on a corner, the others midway
                // along a side, all turned along it.
                assert_eq!(got.len(), 12);
                for (k, w) in want.iter().enumerate() {
                    assert!(same(std::slice::from_ref(w), &got[2 * k..2 * k + 1]));
                }
                let mid = got[1].transform.trans;
                let apothem = 7.0 * (std::f64::consts::PI / 6.0).cos();
                assert!((math::norm(mid) - apothem).abs() < 1e-9);
            }
        }
    }

    /// The index is a value per element: per point in instance on points (choosing every
    /// third point and turning each copy by 10° more), per molecule in move each.
    #[test]
    fn the_index_is_a_value_per_element() {
        use crate::assembly::graph::kinds::{
            Circle, CurveSample, CurveToPoints, Expression, InstanceOnPoints, Var,
        };
        let mut d = design(vec![]);
        let g = d.graph_mut();
        let out = g.output().unwrap();
        g.links.clear();
        let circle = g.add(NodeKind::Circle(Circle::new()), [0.0; 2]);
        let points = g.add(
            NodeKind::CurveToPoints(CurveToPoints {
                mode: CurveSample::Evaluated,
                count: 1,
                length: 1.0,
            }),
            [0.0; 2],
        );
        let put = g.add(
            NodeKind::InstanceOnPoints(InstanceOnPoints {
                align: false,
                ..InstanceOnPoints::new()
            }),
            [0.0; 2],
        );
        let index = g.add(NodeKind::Index, [0.0; 2]);
        let expr = |e: &str| {
            NodeKind::Expression(Expression {
                expr: e.into(),
                vars: vec![Var {
                    key: 0,
                    name: "a".into(),
                    value: 0.0,
                }],
            })
        };
        let third = g.add(expr("a % 3 == 0"), [0.0; 2]);
        let turn = g.add(expr("10 * a"), [0.0; 2]);
        let z = g.add(NodeKind::Combine { value: [0.0; 3] }, [0.0; 2]);
        link(g, circle, OutKey::Curve, points, InKey::Curve);
        link(g, points, OutKey::Points, put, InKey::Points);
        link(g, NodeId(1), OutKey::Set, put, InKey::Set(0));
        link(g, index, OutKey::Number, third, InKey::Var(0));
        link(g, third, OutKey::Number, put, InKey::Param(0));
        link(g, index, OutKey::Number, turn, InKey::Var(0));
        link(g, turn, OutKey::Number, z, InKey::Axis(2));
        link(g, z, OutKey::Vec3, put, InKey::Param(1));
        link(g, put, OutKey::Set, out, InKey::Set(0));
        let ev = d.evaluation();
        assert!(ev.errors.is_empty(), "{:?}", ev.errors);
        assert_eq!(ev.output.attr("point").unwrap(), &[0.0, 3.0, 6.0]);
        assert_eq!(ev.shown[&(vec![], third, OutKey::Number)], Shown::Field);
        for (inst, k) in ev.output.items.iter().zip([0.0, 3.0, 6.0]) {
            let rot = inst.transform.rot;
            let angle = rot[1][0].atan2(rot[0][0]).to_degrees();
            assert!((angle - 10.0 * k).abs() < 1e-9, "{angle}");
        }

        // A field where one value is wanted is reported.
        let g = d.graph_mut();
        let ring = g.add(NodeKind::Op(Step::new_ring()), [0.0; 2]);
        link(g, put, OutKey::Set, ring, InKey::Set(0));
        link(
            g,
            turn,
            OutKey::Number,
            ring,
            InKey::Field(StepField::RingRadius),
        );
        link(g, ring, OutKey::Set, out, InKey::Set(0));
        let ev = d.evaluation();
        assert_eq!(ev.errors.len(), 1);
        assert_eq!(ev.errors[0].node, ring);
    }

    /// Points every 2 Å along an open curve: both ends and the evenly spaced points between.
    #[test]
    fn points_along_a_line_by_length() {
        let line = Spline {
            points: vec![[0.0; 3], [5.0, 0.0, 0.0]],
            closed: false,
        };
        let frames = spline_points(&line, Sample::Length(2.0));
        let xs: Vec<f64> = frames.iter().map(|f| f.trans[0]).collect();
        assert_eq!(xs, [0.0, 2.0, 4.0]);
        assert!(frames.iter().all(|f| f.rot[0][0] > 0.999));
        let ends = spline_points(&line, Sample::Count(3));
        let xs: Vec<f64> = ends.iter().map(|f| f.trans[0]).collect();
        assert_eq!(xs, [0.0, 2.5, 5.0]);
    }

    #[test]
    fn numbers_feed_parameters() {
        let mut d = design(vec![Step::new_ring()]);
        let g = d.graph_mut();
        let ring = chain_step_id(0);
        let v = g.add(NodeKind::Value { value: 3.0 }, [0.0; 2]);
        let e = g.add(
            NodeKind::Expression(crate::assembly::graph::kinds::Expression::new()),
            [0.0; 2],
        );
        link(g, v, OutKey::Number, e, InKey::Var(0));
        link(
            g,
            e,
            OutKey::Number,
            ring,
            InKey::Field(StepField::RingCount),
        );
        assert_eq!(d.instances().len(), 6, "3 × 2");
        // A broken expression is reported, and gives 0: a count of one.
        if let NodeKind::Expression(x) = &mut d.graph_mut().node_mut(e).unwrap().kind {
            x.expr = "a *".into();
        }
        let ev = d.evaluation();
        assert_eq!(ev.errors.len(), 1);
        assert_eq!(ev.errors[0].node, e);
        assert_eq!(d.instances().len(), 1);
    }

    #[test]
    fn a_loop_is_an_error_not_a_hang() {
        let mut d = design(vec![Step::new_ring(), Step::new_stack()]);
        let g = d.graph_mut();
        g.links.retain(|l| l.from != NodeId(1));
        g.links.push(Link {
            from: chain_step_id(1),
            out: OutKey::Set,
            to: chain_step_id(0),
            input: InKey::Set(0),
        });
        let ev = d.evaluation();
        assert!(!ev.errors.is_empty());
    }

    /// Grouping nodes changes nothing that is built; ungrouping puts them back.
    #[test]
    fn grouping_keeps_what_is_built() {
        let mut d = design(vec![Step::new_ring(), Step::new_stack(), Step::new_bend()]);
        let before = d.instances();
        let ids: BTreeSet<NodeId> = [chain_step_id(0), chain_step_id(1)].into();
        let mut defs = std::mem::take(&mut d.defs);
        let (def, node) = ops::group(d.graph_mut(), &mut defs, None, &ids, "rosette").unwrap();
        d.defs = defs;
        let ev = d.evaluation();
        assert!(ev.errors.is_empty(), "{:?}", ev.errors);
        assert!(same(&before, &d.instances()));
        assert_eq!(d.defs.len(), 1);
        crate::assembly::graph::defs::validate(&d.defs).unwrap();
        d.graph.as_ref().unwrap().validate(&d.defs, None).unwrap();
        assert!(ev.shown.keys().any(|(s, _, _)| s == &vec![node]));

        // Used twice: the second is evaluated in a scope of its own.
        let defs = d.defs.clone();
        let g = d.graph_mut();
        let second = g.add(
            NodeKind::Group(GroupRef {
                def: def.clone(),
                inputs: Default::default(),
            }),
            [0.0; 2],
        );
        let merge = g.add(NodeKind::Merge { inputs: 2 }, [0.0; 2]);
        let out = g.output().unwrap();
        let last = g.link_into(out, InKey::Set(0)).unwrap().from;
        ops::connect(
            g,
            &defs,
            None,
            NodeId(1),
            OutKey::Set,
            second,
            InKey::Port(0),
        )
        .unwrap();
        ops::connect(g, &defs, None, last, OutKey::Set, merge, InKey::Set(0)).unwrap();
        ops::connect(
            g,
            &defs,
            None,
            second,
            OutKey::Port(1),
            merge,
            InKey::Set(1),
        )
        .unwrap();
        ops::connect(g, &defs, None, merge, OutKey::Set, out, InKey::Set(0)).unwrap();
        let ev = d.evaluation();
        assert!(ev.errors.is_empty(), "{:?}", ev.errors);
        assert_eq!(ev.output.len(), 2 * before.len());
        assert!(ev.shown.keys().any(|(s, _, _)| s == &vec![second]));

        let defs = d.defs.clone();
        let g = d.graph_mut();
        ops::remove(g, &[second, merge].into());
        ops::ungroup(g, &defs, node).unwrap();
        assert!(same(&before, &d.instances()));
    }

    #[test]
    fn a_custom_node_using_itself_is_an_error() {
        let mut d = design(vec![Step::new_ring()]);
        let ids: BTreeSet<NodeId> = [chain_step_id(0)].into();
        let mut defs = std::mem::take(&mut d.defs);
        let (def, _) = ops::group(d.graph_mut(), &mut defs, None, &ids, "r").unwrap();
        // Hand-edit the definition to use itself.
        let inner = defs[0].graph.add(
            NodeKind::Group(GroupRef {
                def: def.clone(),
                inputs: Default::default(),
            }),
            [0.0; 2],
        );
        let gout = defs[0].graph.group_output().unwrap();
        defs[0].graph.links.retain(|l| l.to != gout);
        defs[0].graph.links.push(Link {
            from: inner,
            out: OutKey::Port(1),
            to: gout,
            input: InKey::Port(1),
        });
        d.defs = defs;
        let ev = d.evaluation();
        assert!(
            ev.errors.iter().any(|e| !e.scope.is_empty()),
            "{:?}",
            ev.errors
        );
    }

    #[test]
    fn a_crystal_survives_selection_of_whole_cells_only() {
        let d = design(vec![lattice()]);
        let z = d.placements().1.unwrap().z;
        let mut keep = d.clone();
        let g = keep.graph_mut();
        let out = g.output().unwrap();
        let select = g.add(
            NodeKind::Select(Select {
                expr: "cell == 0".into(),
                vars: vec![],
            }),
            [0.0; 2],
        );
        let lat = chain_step_id(0);
        link(g, lat, OutKey::Set, select, InKey::Set(0));
        link(g, select, OutKey::Selected, out, InKey::Set(0));
        let (set, crystal) = keep.placements();
        assert_eq!(set.len(), z);
        assert!(crystal.is_some());
        if let NodeKind::Select(s) = &mut keep.graph_mut().node_mut(select).unwrap().kind {
            s.expr = "k != 0".into();
        }
        assert!(keep.placements().1.is_none());
        assert_eq!(keep.evaluation().warnings.len(), 1);
    }
}
