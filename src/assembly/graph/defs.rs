//! Custom nodes: a graph of nodes behind one node, with inputs and outputs of its own.
//!
//! A design keeps a copy of every custom node it uses ([`crate::assembly::Design::defs`]), so
//! a file opens the same anywhere. The user's library ([`super::library`]) keeps them to be
//! used again; the copy remembers which library version it came from ([`LibraryStamp`]), so
//! the two can be told apart when either changes ([`status`]).

use std::collections::BTreeSet;
use std::hash::{BuildHasher, Hasher};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::kinds::{NodeKind, PortValue, SocketType};
use super::{Graph, InKey, OutKey};

/// A custom node's identity: 32 hex digits, random, the same in every design and library that
/// holds the node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DefId(pub String);

impl DefId {
    /// A new id no other custom node has (128 random bits).
    pub fn fresh() -> DefId {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut words = [0u64; 2];
        for (k, w) in words.iter_mut().enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) ^ (k as u64) << 63,
            );
            if let Ok(t) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                h.write_u128(t.as_nanos());
            }
            h.write_usize(std::process::id() as usize);
            *w = h.finish();
        }
        DefId(format!("{:016x}{:016x}", words[0], words[1]))
    }

    /// The first eight digits, for file names.
    pub fn short(&self) -> &str {
        self.0.get(..8).unwrap_or(&self.0)
    }
}

/// An input or output of a custom node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Port {
    /// Stays with the port when others are added, removed or reordered.
    pub key: u32,
    pub name: String,
    pub ty: SocketType,
    /// The value of an input no link feeds (numbers and triples).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<PortValue>,
}

/// Which version of the library's node a design's copy was taken from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryStamp {
    pub revision: u32,
    /// [`content_hash`] of the library's node then.
    pub hash: String,
}

/// A custom node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeDef {
    pub id: DefId,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Raised each time the node is saved to the library.
    #[serde(default)]
    pub revision: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<LibraryStamp>,
    #[serde(default)]
    pub inputs: Vec<Port>,
    #[serde(default)]
    pub outputs: Vec<Port>,
    /// Holds group input and output nodes standing for the ports.
    pub graph: Graph,
}

impl NodeDef {
    /// An empty custom node with one molecule input and output, wired straight through.
    pub fn new(name: impl Into<String>) -> NodeDef {
        let mut graph = Graph::default();
        let input = graph.add(NodeKind::GroupInput, [0.0, 0.0]);
        let output = graph.add(NodeKind::GroupOutput, [super::CHAIN_SPACING, 0.0]);
        graph.links.push(super::Link {
            from: input,
            out: OutKey::Port(0),
            to: output,
            input: InKey::Port(0),
        });
        let port = |name: &str| Port {
            key: 0,
            name: name.to_owned(),
            ty: SocketType::Set,
            default: None,
        };
        NodeDef {
            id: DefId::fresh(),
            name: name.into(),
            description: String::new(),
            revision: 0,
            stamp: None,
            inputs: vec![port(t!("assembly.node.socket.molecules"))],
            outputs: vec![port(t!("assembly.node.socket.molecules"))],
            graph,
        }
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
pub fn graph_uses(graph: &Graph) -> BTreeSet<DefId> {
    graph
        .nodes
        .iter()
        .filter_map(|n| match &n.kind {
            NodeKind::Group(g) => Some(g.def.clone()),
            _ => None,
        })
        .collect()
}

/// The custom nodes `graph` uses, directly or inside the ones it uses.
pub fn graph_uses_all(graph: &Graph, defs: &[NodeDef]) -> BTreeSet<DefId> {
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
pub fn depends_on(defs: &[NodeDef], a: &DefId, b: &DefId) -> bool {
    if a == b {
        return true;
    }
    find(defs, a).is_some_and(|d| graph_uses_all(&d.graph, defs).contains(b))
}

/// The custom node with id `id`.
pub fn find<'a>(defs: &'a [NodeDef], id: &DefId) -> Option<&'a NodeDef> {
    defs.iter().find(|d| &d.id == id)
}

pub fn find_mut<'a>(defs: &'a mut [NodeDef], id: &DefId) -> Option<&'a mut NodeDef> {
    defs.iter_mut().find(|d| &d.id == id)
}

/// Whether the custom nodes hold together: unique ids, each graph valid, none using itself.
pub fn validate(defs: &[NodeDef]) -> Result<(), String> {
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
/// drawn, their labels, its name and description or its library bookkeeping.
pub fn content_hash(def: &NodeDef) -> String {
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

/// How a design's copy of a custom node stands to the library's.
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
pub fn status(embedded: &NodeDef, library: Option<&NodeDef>) -> Status {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::model::Step;

    fn def() -> NodeDef {
        let mut d = NodeDef::new("ring");
        let ring = d.graph.add(NodeKind::Op(Step::new_ring()), [100.0, 0.0]);
        d.graph.links[0].to = ring;
        d.graph.links[0].input = InKey::Set(0);
        let out = d.graph.group_output().unwrap();
        d.graph.links.push(super::super::Link {
            from: ring,
            out: OutKey::Set,
            to: out,
            input: InKey::Port(0),
        });
        d
    }

    #[test]
    fn fresh_ids_differ() {
        let a = DefId::fresh();
        let b = DefId::fresh();
        assert_ne!(a, b);
        assert_eq!(a.0.len(), 32);
    }

    #[test]
    fn the_hash_ignores_layout_and_names() {
        let a = def();
        let mut b = a.clone();
        b.name = "renamed".into();
        b.graph.nodes[0].pos = [50.0, 50.0];
        b.revision = 3;
        assert_eq!(content_hash(&a), content_hash(&b));
        if let NodeKind::Op(Step::Ring(r)) = &mut b.graph.nodes[2].kind {
            r.radius = 9.0;
        }
        assert_ne!(content_hash(&a), content_hash(&b));
        validate(&[a]).unwrap();
    }

    #[test]
    fn statuses() {
        let lib = def();
        let mut copy = lib.clone();
        copy.stamp = Some(LibraryStamp {
            revision: 0,
            hash: content_hash(&lib),
        });
        assert_eq!(status(&copy, None), Status::Local);
        assert_eq!(status(&copy, Some(&lib)), Status::InSync);
        let mut newer = lib.clone();
        newer
            .graph
            .nodes
            .retain(|n| !matches!(n.kind, NodeKind::GroupOutput));
        newer.graph.links.pop();
        assert_eq!(status(&copy, Some(&newer)), Status::UpdateAvailable);
        let mut edited = copy.clone();
        edited.inputs[0].name = "in".into();
        assert_eq!(status(&edited, Some(&lib)), Status::Modified);
        assert_eq!(status(&edited, Some(&newer)), Status::Conflict);
    }

    #[test]
    fn a_node_using_itself_is_refused() {
        let mut a = def();
        let id = a.id.clone();
        a.graph.add(
            NodeKind::Group(super::super::kinds::GroupRef {
                def: id,
                inputs: Default::default(),
            }),
            [0.0; 2],
        );
        assert!(validate(&[a]).is_err());
    }
}
