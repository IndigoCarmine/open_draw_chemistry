//! Custom nodes: a graph of nodes behind one node, with inputs and outputs of its own.
//!
//! A graph's owner keeps a copy of every custom node it uses, so a saved graph opens the same
//! anywhere. A library ([`crate::library`]) keeps them to be used again; the copy remembers
//! which library version it came from ([`LibraryStamp`]), so the two can be told apart when
//! either changes ([`status`]).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::graph::Graph;
pub use crate::id::DefId;
use crate::schema::{NodeKind, Role};

/// A custom node in use: which definition, and the values of its inputs no link feeds. The
/// [`Role::Group`] kind of node holds one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupRef<V> {
    pub def: DefId,
    #[serde(default = "BTreeMap::new")]
    pub inputs: BTreeMap<u32, V>,
}

impl<V> GroupRef<V> {
    pub fn new(def: DefId) -> Self {
        Self {
            def,
            inputs: BTreeMap::new(),
        }
    }
}

/// An input or output of a custom node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Port<S, V> {
    /// Stays with the port when others are added, removed or reordered.
    pub key: u32,
    pub name: String,
    pub ty: S,
    /// The value of an input no link feeds (for types that have one).
    #[serde(default = "Option::default", skip_serializing_if = "Option::is_none")]
    pub default: Option<V>,
}

/// [`Port`] of a kind of node.
pub type PortOf<K> = Port<<K as NodeKind>::Socket, <K as NodeKind>::Value>;

/// Which version of the library's node a copy was taken from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryStamp {
    pub revision: u32,
    /// [`content_hash`] of the library's node then.
    pub hash: String,
}

/// A custom node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct NodeDef<K: NodeKind> {
    pub id: DefId,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Raised each time the node is saved to the library.
    #[serde(default)]
    pub revision: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<LibraryStamp>,
    #[serde(default = "Vec::new")]
    pub inputs: Vec<PortOf<K>>,
    #[serde(default = "Vec::new")]
    pub outputs: Vec<PortOf<K>>,
    /// Holds group input and output nodes standing for the ports.
    pub graph: Graph<K>,
}

impl<K: NodeKind> NodeDef<K> {
    /// A custom node with a fresh id, no ports and an empty graph.
    pub fn empty(name: impl Into<String>) -> Self {
        NodeDef {
            id: DefId::fresh(),
            name: name.into(),
            description: String::new(),
            revision: 0,
            stamp: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            graph: Graph::default(),
        }
    }

    /// A custom node with one input and one output of type `ty`, both named `port`, wired
    /// straight through: the group input node (id 0) at the origin, the group output node
    /// (id 1) `spacing` to its right.
    pub fn passing(name: impl Into<String>, ty: K::Socket, port: &str, spacing: f32) -> Self {
        let mut def = Self::empty(name);
        let input = def.graph.add(K::group_input(), [0.0, 0.0]);
        let output = def.graph.add(K::group_output(), [spacing, 0.0]);
        def.graph.link(input, K::port_out(0), output, K::port_in(0));
        let port = Port {
            key: 0,
            name: port.to_owned(),
            ty,
            default: None,
        };
        def.inputs.push(port.clone());
        def.outputs.push(port);
        def
    }

    /// The next key for a new port.
    pub fn next_port_key(&self) -> u32 {
        self.inputs
            .iter()
            .chain(&self.outputs)
            .map(|p| p.key + 1)
            .max()
            .unwrap_or(0)
    }

    /// The custom nodes this one uses directly.
    pub fn uses(&self) -> BTreeSet<DefId> {
        graph_uses(&self.graph)
    }
}

/// The custom nodes `graph` uses directly.
pub fn graph_uses<K: NodeKind>(graph: &Graph<K>) -> BTreeSet<DefId> {
    graph
        .nodes
        .iter()
        .filter_map(|n| match n.kind.role() {
            Role::Group(g) => Some(g.def.clone()),
            _ => None,
        })
        .collect()
}

/// The custom nodes `graph` uses, directly or inside the ones it uses.
pub fn graph_uses_all<K: NodeKind>(graph: &Graph<K>, defs: &[NodeDef<K>]) -> BTreeSet<DefId> {
    let mut all = BTreeSet::new();
    let mut todo: Vec<DefId> = graph_uses(graph).into_iter().collect();
    while let Some(id) = todo.pop() {
        if all.insert(id.clone())
            && let Some(def) = find(defs, &id)
        {
            todo.extend(def.uses());
        }
    }
    all
}

/// Whether custom node `a` uses `b` (directly or through others, or is it).
pub fn depends_on<K: NodeKind>(defs: &[NodeDef<K>], a: &DefId, b: &DefId) -> bool {
    if a == b {
        return true;
    }
    find(defs, a).is_some_and(|d| graph_uses_all(&d.graph, defs).contains(b))
}

/// The custom node with id `id`.
pub fn find<'a, K: NodeKind>(defs: &'a [NodeDef<K>], id: &DefId) -> Option<&'a NodeDef<K>> {
    defs.iter().find(|d| &d.id == id)
}

pub fn find_mut<'a, K: NodeKind>(
    defs: &'a mut [NodeDef<K>],
    id: &DefId,
) -> Option<&'a mut NodeDef<K>> {
    defs.iter_mut().find(|d| &d.id == id)
}

/// Whether the custom nodes hold together: unique ids, each graph valid, none using itself.
/// The message is for a damaged file, in English.
pub fn validate<K: NodeKind>(defs: &[NodeDef<K>]) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    for def in defs {
        if !ids.insert(&def.id) {
            return Err(format!("custom node {} appears twice", def.id.0));
        }
    }
    for def in defs {
        def.graph
            .validate(defs, Some(def))
            .map_err(|e| format!("{}: {e}", def.name))?;
        if graph_uses_all(&def.graph, defs).contains(&def.id) {
            return Err(format!("custom node {} uses itself", def.name));
        }
    }
    Ok(())
}

/// A fingerprint of what the node does: its ports and graph, without where its nodes are
/// drawn, their labels, its name and description or its library bookkeeping. SHA-256 of its
/// JSON, in hex.
#[cfg(feature = "library")]
pub fn content_hash<K: NodeKind>(def: &NodeDef<K>) -> String {
    use sha2::{Digest, Sha256};
    let mut bare = def.clone();
    bare.name.clear();
    bare.description.clear();
    bare.revision = 0;
    bare.stamp = None;
    bare.id = DefId(String::new());
    for n in &mut bare.graph.nodes {
        n.pos = [0.0; 2];
        n.label.clear();
    }
    let json = serde_json::to_vec(&bare).expect("a node definition always serialises");
    let digest = Sha256::digest(&json);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// How a copy of a custom node stands to the library's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Not in the library (never saved there, or removed from it).
    Local,
    /// The same as the library's.
    InSync,
    /// The library's has changed since the copy was taken; the copy has not.
    UpdateAvailable,
    /// The copy has changed since it was taken; the library's has not.
    Modified,
    /// Both have changed.
    Conflict,
}

/// How `embedded` stands to the library's node of the same id (`None`: not in the library).
#[cfg(feature = "library")]
pub fn status<K: NodeKind>(embedded: &NodeDef<K>, library: Option<&NodeDef<K>>) -> Status {
    let Some(library) = library else {
        return Status::Local;
    };
    let mine = content_hash(embedded);
    let theirs = content_hash(library);
    if mine == theirs {
        return Status::InSync;
    }
    let Some(stamp) = &embedded.stamp else {
        return Status::Conflict;
    };
    let changed_here = mine != stamp.hash;
    let changed_there = theirs != stamp.hash;
    match (changed_here, changed_there) {
        (false, true) => Status::UpdateAvailable,
        (true, false) => Status::Modified,
        _ => Status::Conflict,
    }
}
