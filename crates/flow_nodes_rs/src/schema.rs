//! What a kind of node is to the graph: its sockets, the type of each, the values of inputs no
//! link feeds, and the part it plays (an ordinary node, the graph's output, a custom node or
//! the inside ends of one).
//!
//! An application describes its nodes by implementing [`NodeKind`] (and [`SocketType`]) for
//! its own types, usually one `enum` of kinds. The crate never matches on the kinds: it asks
//! them through the trait, so a new kind is a new variant and its arms. The custom-node kinds
//! ([`Role::Group`], [`Role::GroupInput`], [`Role::GroupOutput`]) are variants of the same enum,
//! so a graph saves as that enum does; their sockets come from the custom node's ports, which
//! [`inputs`] and [`outputs`] work out.

use std::fmt::Debug;
use std::hash::Hash;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::def::{self, GroupRef, NodeDef};

/// A socket's key: names an input or an output of a node.
pub trait Key: Copy + Eq + Ord + Hash + Debug + Serialize + DeserializeOwned {}

impl<T: Copy + Eq + Ord + Hash + Debug + Serialize + DeserializeOwned> Key for T {}

/// What a socket carries.
pub trait SocketType: Copy + Eq + Hash + Debug + Serialize + DeserializeOwned {
    /// Whether an output of this type may feed an input of type `to`. The same type by
    /// default; an application adds the conversions it allows (a number into a vector, …).
    fn feeds(self, to: Self) -> bool {
        self == to
    }

    /// Whether an input of this type has a value of its own when no link feeds it (a number
    /// does; a set of objects does not). Such inputs keep their value in the node
    /// ([`NodeKind::inline`]) and custom nodes' ports keep a default.
    fn has_value(self) -> bool;

    /// Whether this is the data the nodes work on, which a bypassed node passes through and
    /// deleting a node joins around it (see [`through`]).
    fn is_primary(self) -> bool {
        false
    }
}

/// How the editor treats a number input: as a plain number, a count, an angle or a length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ValueHint {
    #[default]
    Plain,
    /// A whole number of things.
    Count,
    /// Degrees.
    Angle,
    /// A distance.
    Length,
}

/// An input socket.
#[derive(Debug, Clone, PartialEq)]
pub struct InDecl<I, S> {
    pub key: I,
    pub ty: S,
    pub label: String,
    pub hint: ValueHint,
}

impl<I, S> InDecl<I, S> {
    pub fn new(key: I, ty: S, label: impl Into<String>) -> Self {
        Self {
            key,
            ty,
            label: label.into(),
            hint: ValueHint::Plain,
        }
    }

    pub fn hint(self, hint: ValueHint) -> Self {
        Self { hint, ..self }
    }
}

/// An output socket.
#[derive(Debug, Clone, PartialEq)]
pub struct OutDecl<O, S> {
    pub key: O,
    pub ty: S,
    pub label: String,
}

impl<O, S> OutDecl<O, S> {
    pub fn new(key: O, ty: S, label: impl Into<String>) -> Self {
        Self {
            key,
            ty,
            label: label.into(),
        }
    }
}

/// [`InDecl`] of a kind of node.
pub type InDeclOf<K> = InDecl<<K as NodeKind>::In, <K as NodeKind>::Socket>;
/// [`OutDecl`] of a kind of node.
pub type OutDeclOf<K> = OutDecl<<K as NodeKind>::Out, <K as NodeKind>::Socket>;

/// The part a node plays in its graph.
#[derive(Debug, PartialEq)]
pub enum Role<'a, V> {
    /// An ordinary node: the application computes it.
    Plain,
    /// What the top-level graph makes: evaluation pulls its inputs. One per top-level graph,
    /// none inside a custom node.
    Output,
    /// Inside a custom node: the custom node's inputs, as outputs.
    GroupInput,
    /// Inside a custom node: the custom node's outputs, as inputs.
    GroupOutput,
    /// A custom node in use.
    Group(&'a GroupRef<V>),
}

/// A kind of node, as the graph, the edits and the evaluator see it.
///
/// Implemented by the application for its own type of node settings. `Serialize` and
/// `Deserialize` define how graphs are saved.
pub trait NodeKind: Clone + PartialEq + Debug + Serialize + DeserializeOwned {
    /// The key of an input socket.
    type In: Key;
    /// The key of an output socket.
    type Out: Key;
    type Socket: SocketType;
    /// The value of an input no link feeds.
    type Value: Clone + PartialEq + Debug + Serialize + DeserializeOwned;

    /// The part the node plays: [`Role::Plain`] unless it is one of the four special kinds.
    fn role(&self) -> Role<'_, Self::Value>;

    /// A node using custom node `r.def`.
    fn group(r: GroupRef<Self::Value>) -> Self;
    /// The node standing for a custom node's inputs inside it.
    fn group_input() -> Self;
    /// The node standing for a custom node's outputs inside it.
    fn group_output() -> Self;

    /// The input socket of custom-node port `key` (on a group node and a group output node).
    fn port_in(key: u32) -> Self::In;
    /// The port an input socket is, if it is one.
    fn as_port_in(key: Self::In) -> Option<u32>;
    /// The output socket of custom-node port `key` (on a group node and a group input node).
    fn port_out(key: u32) -> Self::Out;
    /// The port an output socket is, if it is one.
    fn as_port_out(key: Self::Out) -> Option<u32>;

    /// The input sockets of a [`Role::Plain`] or [`Role::Output`] node, in order. The other
    /// roles' sockets come from the custom node's ports (see [`inputs`]).
    fn own_inputs(&self) -> Vec<InDeclOf<Self>>;
    /// The output sockets of a [`Role::Plain`] node, in order.
    fn own_outputs(&self) -> Vec<OutDeclOf<Self>>;

    /// The value input `key` has when no link feeds it (a group node's from its
    /// [`GroupRef::inputs`]).
    fn inline(&self, key: Self::In) -> Option<Self::Value>;
    /// Set the value input `key` has when no link feeds it. Returns whether the node has it.
    fn set_inline(&mut self, key: Self::In, value: Self::Value) -> bool;

    /// Whether a [`Role::Plain`] node can be gathered into a custom node.
    fn groupable(&self) -> bool {
        true
    }

    /// Whether the node belongs only in the top-level graph (never inside a custom node).
    fn top_level_only(&self) -> bool {
        false
    }
}

/// The input sockets of `kind`, in order. `owner` is the custom node whose graph the node is
/// in (`None` at the top level): its outputs are a group output node's inputs.
pub fn inputs<K: NodeKind>(
    kind: &K,
    defs: &[NodeDef<K>],
    owner: Option<&NodeDef<K>>,
) -> Vec<InDeclOf<K>> {
    let ports = |ports: &[def::Port<K::Socket, K::Value>]| {
        ports
            .iter()
            .map(|p| InDecl::new(K::port_in(p.key), p.ty, p.name.clone()))
            .collect()
    };
    match kind.role() {
        Role::Plain | Role::Output => kind.own_inputs(),
        Role::GroupInput => Vec::new(),
        Role::Group(g) => def::find(defs, &g.def)
            .map(|d| ports(&d.inputs))
            .unwrap_or_default(),
        Role::GroupOutput => owner.map(|d| ports(&d.outputs)).unwrap_or_default(),
    }
}

/// The output sockets of `kind`, in order. `owner` as for [`inputs`] (its inputs are a group
/// input node's outputs).
pub fn outputs<K: NodeKind>(
    kind: &K,
    defs: &[NodeDef<K>],
    owner: Option<&NodeDef<K>>,
) -> Vec<OutDeclOf<K>> {
    let ports = |ports: &[def::Port<K::Socket, K::Value>]| {
        ports
            .iter()
            .map(|p| OutDecl::new(K::port_out(p.key), p.ty, p.name.clone()))
            .collect()
    };
    match kind.role() {
        Role::Plain => kind.own_outputs(),
        Role::Output | Role::GroupOutput => Vec::new(),
        Role::Group(g) => def::find(defs, &g.def)
            .map(|d| ports(&d.outputs))
            .unwrap_or_default(),
        Role::GroupInput => owner.map(|d| ports(&d.inputs)).unwrap_or_default(),
    }
}

/// The input a bypassed node passes on, and the output it passes it to: its first input of a
/// primary type ([`SocketType::is_primary`]) and its first output of that type. Deleting a
/// node joins the same two around it.
pub fn through<I: Copy, O: Copy, S: SocketType>(
    inputs: &[InDecl<I, S>],
    outputs: &[OutDecl<O, S>],
) -> Option<(I, O)> {
    let input = inputs.iter().find(|i| i.ty.is_primary())?;
    let output = outputs.iter().find(|o| o.ty == input.ty)?;
    Some((input.key, output.key))
}
