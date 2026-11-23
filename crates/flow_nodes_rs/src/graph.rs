//! Nodes and the wires between them.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::def::{self, NodeDef};
use crate::id::NodeId;
use crate::schema::{self, NodeKind, Role, SocketType};

/// One node: what it is, where it is drawn, and whether it is bypassed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Node<K: NodeKind> {
    pub id: NodeId,
    pub kind: K,
    /// The top-left corner on the canvas.
    #[serde(default)]
    pub pos: [f32; 2],
    /// Passes its data through unchanged (see [`schema::through`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bypass: bool,
    /// A name the user gave it, shown instead of the kind's.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

impl<K: NodeKind> Node<K> {
    pub fn new(id: NodeId, kind: K, pos: [f32; 2]) -> Self {
        Self {
            id,
            kind,
            pos,
            bypass: false,
            label: String::new(),
        }
    }
}

/// A wire: output `out` of node `from` feeds input `input` of node `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Link<I, O> {
    pub from: NodeId,
    pub out: O,
    pub to: NodeId,
    pub input: I,
}

/// [`Link`] of a kind of node.
pub type LinkOf<K> = Link<<K as NodeKind>::In, <K as NodeKind>::Out>;

/// Nodes and the wires between them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Graph<K: NodeKind> {
    pub nodes: Vec<Node<K>>,
    #[serde(default)]
    pub links: Vec<LinkOf<K>>,
    /// The id the next node gets.
    #[serde(default)]
    pub next_id: u64,
}

impl<K: NodeKind> Default for Graph<K> {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            links: Vec::new(),
            next_id: 0,
        }
    }
}

impl<K: NodeKind> Graph<K> {
    pub fn node(&self, id: NodeId) -> Option<&Node<K>> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node<K>> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// The link feeding input `input` of node `to`.
    pub fn link_into(&self, to: NodeId, input: K::In) -> Option<&LinkOf<K>> {
        self.links.iter().find(|l| l.to == to && l.input == input)
    }

    /// The first node playing `role`.
    fn find_role(&self, pick: impl Fn(&Role<'_, K::Value>) -> bool) -> Option<NodeId> {
        self.nodes
            .iter()
            .find(|n| pick(&n.kind.role()))
            .map(|n| n.id)
    }

    /// The output node ([`Role::Output`]).
    pub fn output(&self) -> Option<NodeId> {
        self.find_role(|r| matches!(r, Role::Output))
    }

    /// The group output node (inside a custom node).
    pub fn group_output(&self) -> Option<NodeId> {
        self.find_role(|r| matches!(r, Role::GroupOutput))
    }

    /// The group input node (inside a custom node).
    pub fn group_input(&self) -> Option<NodeId> {
        self.find_role(|r| matches!(r, Role::GroupInput))
    }

    /// A fresh id.
    pub fn alloc_id(&mut self) -> NodeId {
        let floor = self.nodes.iter().map(|n| n.id.0 + 1).max().unwrap_or(0);
        self.next_id = self.next_id.max(floor);
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Add a node at `pos`; returns its id.
    pub fn add(&mut self, kind: K, pos: [f32; 2]) -> NodeId {
        let id = self.alloc_id();
        self.nodes.push(Node::new(id, kind, pos));
        id
    }

    /// Link output `out` of `from` to input `input` of `to`, without any check (see
    /// [`crate::ops::connect`] for the checked edit).
    pub fn link(&mut self, from: NodeId, out: K::Out, to: NodeId, input: K::In) {
        self.links.push(Link {
            from,
            out,
            to,
            input,
        });
    }

    /// Whether the graph's wiring holds together: ids unique, every link between existing
    /// sockets of fitting types, one link at most per input, one output node at the top level
    /// (`owner` is `None`) and none inside a custom node, every custom node defined and no wire
    /// loops. The message is for a damaged file, in English.
    pub fn validate(&self, defs: &[NodeDef<K>], owner: Option<&NodeDef<K>>) -> Result<(), String> {
        let mut ids = HashSet::new();
        for n in &self.nodes {
            if !ids.insert(n.id) {
                return Err(format!("node {} appears twice", n.id.0));
            }
            match (n.kind.role(), owner) {
                (Role::Output, Some(_)) => {
                    return Err("an output node inside a custom node".into());
                }
                (Role::Plain, Some(_)) if n.kind.top_level_only() => {
                    return Err(format!("node {} belongs outside custom nodes", n.id.0));
                }
                (Role::GroupInput | Role::GroupOutput, None) => {
                    return Err("a group input or output outside a custom node".into());
                }
                (Role::Group(g), _) if def::find(defs, &g.def).is_none() => {
                    return Err(format!("custom node {} is not defined", g.def.0));
                }
                _ => {}
            }
        }
        let count = |pick: fn(&Role<'_, K::Value>) -> bool| {
            self.nodes.iter().filter(|n| pick(&n.kind.role())).count()
        };
        let outputs = count(|r| matches!(r, Role::Output));
        if owner.is_none() && outputs != 1 {
            return Err(format!("{outputs} output nodes (one is needed)"));
        }
        if owner.is_some() && count(|r| matches!(r, Role::GroupOutput)) > 1 {
            return Err("more than one group output node".into());
        }
        let mut fed = HashSet::new();
        for l in &self.links {
            let (Some(from), Some(to)) = (self.node(l.from), self.node(l.to)) else {
                return Err(format!(
                    "a link to a missing node ({} → {})",
                    l.from.0, l.to.0
                ));
            };
            let out = schema::outputs(&from.kind, defs, owner)
                .into_iter()
                .find(|o| o.key == l.out);
            let input = schema::inputs(&to.kind, defs, owner)
                .into_iter()
                .find(|i| i.key == l.input);
            let (Some(out), Some(input)) = (out, input) else {
                return Err(format!(
                    "a link between missing sockets ({} → {})",
                    l.from.0, l.to.0
                ));
            };
            if !out.ty.feeds(input.ty) {
                return Err(format!(
                    "a link of the wrong type ({} → {})",
                    l.from.0, l.to.0
                ));
            }
            if !fed.insert((l.to, l.input)) {
                return Err(format!("two links into one input of node {}", l.to.0));
            }
        }
        if let Some(id) = find_cycle(self) {
            return Err(format!("the links loop back through node {}", id.0));
        }
        Ok(())
    }
}

/// A node on a loop of links, if there is one.
pub fn find_cycle<K: NodeKind>(graph: &Graph<K>) -> Option<NodeId> {
    let mut next: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for l in &graph.links {
        next.entry(l.from).or_default().push(l.to);
    }
    // 0 unseen, 1 on the path, 2 done.
    let mut state: HashMap<NodeId, u8> = HashMap::new();
    for start in graph.nodes.iter().map(|n| n.id) {
        if state.get(&start).copied().unwrap_or(0) != 0 {
            continue;
        }
        let mut stack = vec![(start, 0usize)];
        state.insert(start, 1);
        while let Some((node, k)) = stack.pop() {
            let succ = next.get(&node).map_or(&[][..], Vec::as_slice);
            if let Some(&to) = succ.get(k) {
                stack.push((node, k + 1));
                match state.get(&to).copied().unwrap_or(0) {
                    0 => {
                        state.insert(to, 1);
                        stack.push((to, 0));
                    }
                    1 => return Some(to),
                    _ => {}
                }
            } else {
                state.insert(node, 2);
            }
        }
    }
    None
}

/// Whether `to` can be reached from `from` along the links (so a link `to → from` would loop).
pub fn reaches<K: NodeKind>(graph: &Graph<K>, from: NodeId, to: NodeId) -> bool {
    let mut seen = HashSet::new();
    let mut todo = vec![from];
    while let Some(n) = todo.pop() {
        if n == to {
            return true;
        }
        if seen.insert(n) {
            todo.extend(graph.links.iter().filter(|l| l.from == n).map(|l| l.to));
        }
    }
    false
}
