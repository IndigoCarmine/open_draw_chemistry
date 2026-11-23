//! A graph evaluated: the output node pulls what it needs.
//!
//! Each node reached is evaluated once (per custom node it is inside) and remembered, so data
//! fed to two nodes is made once; nodes that do not feed the output are never evaluated. The
//! application computes its own nodes ([`Evaluate::compute`]); the engine does the rest:
//!
//! - inputs: what a link feeds, pulled when the node asks for it ([`EvalCx::input`]);
//! - bypassed nodes, and nodes that fail: their primary input passed to their primary output
//!   ([`schema::through`]), other outputs at their default — so the rest of the graph still
//!   gives something while the user fixes the node;
//! - custom nodes: their inputs bound (a link, else the node's value, else the port's default)
//!   and their graph evaluated inside them, each use on its own, nested at most
//!   [`MAX_DEPTH`] deep and never inside themselves;
//! - loops of links, reported on the node that closes them;
//! - what each node gave ([`Outcome::values`]), and which nodes were reached.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::def::{self, DefId, NodeDef};
use crate::graph::Graph;
use crate::id::NodeId;
use crate::schema::{self, NodeKind, Role};
pub use crate::scope::Scope;

/// How deep custom nodes may be nested.
pub const MAX_DEPTH: usize = 16;

/// How an application computes its kinds of node, in an environment `E` it passes to
/// [`evaluate`] (what its nodes refer to: objects, settings, …).
pub trait Evaluate<E: ?Sized>: NodeKind {
    /// What travels along a wire.
    type Data: Clone;
    /// Why a node could not be computed (or a warning about it).
    type Error: Clone + std::fmt::Debug;

    /// The outputs of a [`Role::Plain`] node, by key, in the order it declares them. An output
    /// left out feeds nothing: what it is linked to sees no link.
    fn compute(&self, cx: &mut EvalCx<'_, '_, Self, E>) -> Result<OutputsOf<Self, E>, Self::Error>;

    /// The data of a socket of type `ty` nothing feeds: `value` is its own value (a custom
    /// node's port default, or the value a group node keeps for the port) if it has one.
    fn default_data(ty: Self::Socket, value: Option<&Self::Value>) -> Self::Data;

    /// `data` as a socket of type `ty` takes it, if it can (a custom node's port passing on
    /// whatever was linked to it).
    fn conform(data: Self::Data, ty: Self::Socket) -> Option<Self::Data>;
}

/// What went wrong with one node.
#[derive(Debug, Clone, PartialEq)]
pub enum Problem<X> {
    /// The application's error (or warning) about the node.
    Node(X),
    /// The links loop back through the node.
    Cycle,
    /// A custom node that is not defined.
    MissingDef,
    /// A custom node used inside itself (its name).
    Recursion(String),
    /// Custom nodes nested deeper than this.
    TooDeep(usize),
}

/// Something wrong with one node.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue<X> {
    pub scope: Scope,
    pub node: NodeId,
    pub problem: Problem<X>,
    /// A warning: the node was still computed.
    pub warning: bool,
}

/// What each node gave, by output.
pub type Values<O, D> = HashMap<(Scope, NodeId), Rc<Vec<(O, D)>>>;

/// The data on each output of a node of kind `K`, by key.
pub type OutputsOf<K, E> = Vec<(<K as NodeKind>::Out, <K as Evaluate<E>>::Data)>;

/// A node's outputs, or what went wrong.
type Computed<K, E> = Result<OutputsOf<K, E>, Problem<<K as Evaluate<E>>::Error>>;

/// The result of evaluating a graph.
pub struct Outcome<K: Evaluate<E>, E: ?Sized> {
    /// What the output node's inputs were fed (inputs no link feeds left out).
    pub output: Vec<(K::In, K::Data)>,
    /// What each node evaluated gave.
    pub values: Values<K::Out, K::Data>,
    /// Errors and warnings, in the order they happened.
    pub issues: Vec<Issue<K::Error>>,
    /// Every node evaluated or looked at (an output node with no outputs included).
    pub reached: HashSet<(Scope, NodeId)>,
}

/// Evaluate `graph` (a top-level graph, whose custom nodes are `defs`) in `env`: everything
/// its output node ([`Role::Output`]) needs.
pub fn evaluate<K: Evaluate<E>, E: ?Sized>(
    graph: &Graph<K>,
    defs: &[NodeDef<K>],
    env: &E,
) -> Outcome<K, E> {
    let mut engine = Engine {
        defs,
        env,
        memo: HashMap::new(),
        visiting: HashSet::new(),
        bindings: HashMap::new(),
        def_stack: Vec::new(),
        issues: Vec::new(),
        reached: HashSet::new(),
    };
    let scope: Scope = Vec::new();
    let mut output = Vec::new();
    if let Some(out) = graph.output() {
        engine.reached.insert((scope.clone(), out));
        let decls = schema::inputs(&graph.node(out).unwrap().kind, defs, None);
        for decl in decls {
            if let Some(data) = engine.linked(graph, None, &scope, out, decl.key) {
                output.push((decl.key, data));
            }
        }
    }
    let mut reached = engine.reached;
    reached.extend(engine.memo.keys().cloned());
    reached.extend(engine.issues.iter().map(|i| (i.scope.clone(), i.node)));
    Outcome {
        output,
        values: engine.memo,
        issues: engine.issues,
        reached,
    }
}

type Outputs<K, E> = Rc<OutputsOf<K, E>>;

/// The data bound to a custom node's inputs, by port.
type Bound<K, E> = Rc<Vec<(u32, <K as Evaluate<E>>::Data)>>;

struct Engine<'g, K: Evaluate<E>, E: ?Sized> {
    defs: &'g [NodeDef<K>],
    env: &'g E,
    memo: HashMap<(Scope, NodeId), Outputs<K, E>>,
    visiting: HashSet<(Scope, NodeId)>,
    /// The data a custom node's inputs have, by the scope inside it.
    bindings: HashMap<Scope, Bound<K, E>>,
    def_stack: Vec<DefId>,
    issues: Vec<Issue<K::Error>>,
    reached: HashSet<(Scope, NodeId)>,
}

impl<'g, K: Evaluate<E>, E: ?Sized> Engine<'g, K, E> {
    fn issue(&mut self, scope: &Scope, node: NodeId, problem: Problem<K::Error>, warning: bool) {
        self.issues.push(Issue {
            scope: scope.clone(),
            node,
            problem,
            warning,
        });
    }

    /// What feeds input `key` of node `id`, if a link does.
    fn linked(
        &mut self,
        graph: &'g Graph<K>,
        owner: Option<&'g NodeDef<K>>,
        scope: &Scope,
        id: NodeId,
        key: K::In,
    ) -> Option<K::Data> {
        let link = *graph.link_into(id, key)?;
        let outs = self.node(graph, owner, scope, link.from);
        outs.iter()
            .find(|(k, _)| *k == link.out)
            .map(|(_, d)| d.clone())
    }

    /// Node `id`'s outputs, evaluated once per scope.
    fn node(
        &mut self,
        graph: &'g Graph<K>,
        owner: Option<&'g NodeDef<K>>,
        scope: &Scope,
        id: NodeId,
    ) -> Outputs<K, E> {
        let key = (scope.clone(), id);
        if let Some(done) = self.memo.get(&key) {
            return done.clone();
        }
        let Some(node) = graph.node(id) else {
            return Rc::new(Vec::new());
        };
        if !self.visiting.insert(key.clone()) {
            self.issue(scope, id, Problem::Cycle, false);
            return Rc::new(Vec::new());
        }
        let outputs = if node.bypass {
            self.pass_through(graph, owner, scope, id, &node.kind)
        } else {
            match self.compute(graph, owner, scope, id, &node.kind) {
                Ok(outputs) => outputs,
                Err(problem) => {
                    self.issue(scope, id, problem, false);
                    self.pass_through(graph, owner, scope, id, &node.kind)
                }
            }
        };
        self.visiting.remove(&key);
        let outputs = Rc::new(outputs);
        self.memo.insert(key, outputs.clone());
        outputs
    }

    /// A bypassed or failed node: its primary input on its primary output, the other outputs
    /// at their default.
    fn pass_through(
        &mut self,
        graph: &'g Graph<K>,
        owner: Option<&'g NodeDef<K>>,
        scope: &Scope,
        id: NodeId,
        kind: &K,
    ) -> OutputsOf<K, E> {
        let outputs = schema::outputs(kind, self.defs, owner);
        let through = schema::through(&schema::inputs(kind, self.defs, owner), &outputs);
        let mut passed =
            through.map(|(input, out)| (out, self.linked(graph, owner, scope, id, input)));
        outputs
            .into_iter()
            .map(|o| {
                let data = match passed.take_if(|(out, _)| *out == o.key) {
                    Some((_, Some(d))) => K::conform(d, o.ty),
                    _ => None,
                };
                (o.key, data.unwrap_or_else(|| K::default_data(o.ty, None)))
            })
            .collect()
    }

    fn compute(
        &mut self,
        graph: &'g Graph<K>,
        owner: Option<&'g NodeDef<K>>,
        scope: &Scope,
        id: NodeId,
        kind: &K,
    ) -> Computed<K, E> {
        match kind.role() {
            Role::Plain => {
                let mut cx = EvalCx {
                    engine: self,
                    graph,
                    owner,
                    scope,
                    id,
                };
                kind.compute(&mut cx).map_err(Problem::Node)
            }
            Role::Group(g) => self.group(graph, owner, scope, id, &g.def, kind),
            Role::GroupInput => {
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
                                    .unwrap_or_else(|| K::default_data(p.ty, p.default.as_ref()));
                                (K::port_out(p.key), data)
                            })
                            .collect()
                    })
                    .unwrap_or_default())
            }
            Role::GroupOutput | Role::Output => Ok(Vec::new()),
        }
    }

    /// A custom node: its inputs bound, its graph evaluated inside it.
    fn group(
        &mut self,
        graph: &'g Graph<K>,
        owner: Option<&'g NodeDef<K>>,
        scope: &Scope,
        id: NodeId,
        def_id: &DefId,
        kind: &K,
    ) -> Computed<K, E> {
        let Some(def) = def::find(self.defs, def_id) else {
            return Err(Problem::MissingDef);
        };
        if self.def_stack.contains(def_id) {
            return Err(Problem::Recursion(def.name.clone()));
        }
        if self.def_stack.len() >= MAX_DEPTH {
            return Err(Problem::TooDeep(MAX_DEPTH));
        }
        let mut bound = Vec::with_capacity(def.inputs.len());
        for p in &def.inputs {
            let data = match self.linked(graph, owner, scope, id, K::port_in(p.key)) {
                Some(d) => {
                    K::conform(d, p.ty).unwrap_or_else(|| K::default_data(p.ty, p.default.as_ref()))
                }
                None => {
                    let own = kind.inline(K::port_in(p.key));
                    K::default_data(p.ty, own.as_ref().or(p.default.as_ref()))
                }
            };
            bound.push((p.key, data));
        }
        let mut inner = scope.clone();
        inner.push(id);
        self.bindings.insert(inner.clone(), Rc::new(bound));
        self.def_stack.push(def_id.clone());
        let out_node = def.graph.group_output();
        if let Some(out_node) = out_node {
            self.reached.insert((inner.clone(), out_node));
        }
        let mut outputs = Vec::with_capacity(def.outputs.len());
        for p in &def.outputs {
            let data = out_node
                .and_then(|o| self.linked(&def.graph, Some(def), &inner, o, K::port_in(p.key)))
                .and_then(|d| K::conform(d, p.ty))
                .unwrap_or_else(|| K::default_data(p.ty, p.default.as_ref()));
            outputs.push((K::port_out(p.key), data));
        }
        self.def_stack.pop();
        Ok(outputs)
    }
}

/// What a node being computed may ask of the evaluation.
pub struct EvalCx<'c, 'g, K: Evaluate<E>, E: ?Sized> {
    engine: &'c mut Engine<'g, K, E>,
    graph: &'g Graph<K>,
    owner: Option<&'g NodeDef<K>>,
    scope: &'c Scope,
    id: NodeId,
}

impl<'c, 'g, K: Evaluate<E>, E: ?Sized> EvalCx<'c, 'g, K, E> {
    /// What feeds input `key`, if a link does: the node feeding it is evaluated now (once).
    pub fn input(&mut self, key: K::In) -> Option<K::Data> {
        self.engine
            .linked(self.graph, self.owner, self.scope, self.id, key)
    }

    /// Whether a link feeds input `key` (without evaluating anything).
    pub fn is_linked(&self, key: K::In) -> bool {
        self.graph.link_into(self.id, key).is_some()
    }

    /// Report something about the node that does not stop it.
    pub fn warn(&mut self, warning: K::Error) {
        let scope = self.scope.clone();
        self.engine
            .issue(&scope, self.id, Problem::Node(warning), true);
    }

    /// The environment the graph is evaluated in.
    pub fn env(&self) -> &'g E {
        self.engine.env
    }

    /// Every custom node.
    pub fn defs(&self) -> &'g [NodeDef<K>] {
        self.engine.defs
    }

    /// The custom node whose graph the node is in.
    pub fn owner(&self) -> Option<&'g NodeDef<K>> {
        self.owner
    }

    pub fn scope(&self) -> &Scope {
        self.scope
    }

    pub fn id(&self) -> NodeId {
        self.id
    }

    /// The node's input sockets.
    pub fn inputs(&self) -> Vec<schema::InDeclOf<K>> {
        let kind = &self.graph.node(self.id).expect("the node computed").kind;
        schema::inputs(kind, self.engine.defs, self.owner)
    }
}
