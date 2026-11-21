//! The assembly as a graph of nodes: monomers feed copying, moving, merging and selecting
//! nodes, numbers and expressions feed their parameters, and the output node is the assembly.
//! Nodes can be gathered into custom nodes ([`defs::NodeDef`]) with inputs and outputs of
//! their own, kept in the design and in the user's library ([`library`]).
//!
//! Pure code like the rest of [`crate::assembly`]:
//!
//! - [`kinds`]: what each node is, its sockets and settings;
//! - [`field`]: the numbers of the building steps as sockets;
//! - [`expr`]: the expressions typed into nodes;
//! - [`eval`]: the graph evaluated into placed molecules;
//! - [`ops`]: the edits the node editor makes (connect, group, …);
//! - [`defs`], [`library`]: custom nodes and where they are kept.
//!
//! A design made of a monomer and a list of steps (every file before version 5) is the chain
//! [`Graph::chain`] makes; it becomes a graph of its own only when the graph is edited.

pub mod access;
pub mod defs;
pub mod eval;
pub mod expr;
pub mod field;
pub mod kinds;
pub mod library;
pub mod ops;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

pub use defs::{DefId, NodeDef};
pub use eval::{Evaluation, Issue, Scope, Set, Shown};
pub use field::StepField;
pub use kinds::{NodeKind, PortValue, SocketType};

use crate::assembly::model::Step;

/// A node, unique within its graph: ids are never handed out twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u64);

/// An input socket of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum InKey {
    /// Molecules (a merge has several).
    Set(u8),
    /// A building step's number.
    Field(StepField),
    /// An expression's variable, by its key.
    Var(u32),
    /// A custom node's input, by its key.
    Port(u32),
    /// x, y or z of a combine node.
    Axis(u8),
    /// How many copies a repeat makes.
    Count,
    /// The triple a separate node splits.
    Value,
}

/// An output socket of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum OutKey {
    Set,
    /// A selection's chosen molecules.
    Selected,
    /// A selection's other molecules.
    Rest,
    Number,
    Vec3,
    /// A custom node's output (or a group input node's), by its key.
    Port(u32),
    /// x, y or z of a separate node.
    Axis(u8),
}

/// One node: what it is, where it is drawn, and whether it is bypassed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    /// The top-left corner on the canvas.
    #[serde(default)]
    pub pos: [f32; 2],
    /// Passes its molecules through unchanged (a step switched off).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bypass: bool,
    /// A name the user gave it, shown instead of the kind's.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

/// A wire: output `out` of node `from` feeds input `input` of node `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Link {
    pub from: NodeId,
    pub out: OutKey,
    pub to: NodeId,
    pub input: InKey,
}

/// Nodes and the wires between them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub links: Vec<Link>,
    /// The id the next node gets.
    #[serde(default)]
    pub next_id: u64,
}

/// Horizontal distance between the nodes of a chain.
pub const CHAIN_SPACING: f32 = 220.0;

impl Graph {
    /// A monomer straight into the output.
    pub fn new(monomer: Option<u64>) -> Graph {
        let mut g = Graph::default();
        let out = g.add(NodeKind::Output, [CHAIN_SPACING, 0.0]);
        if let Some(component) = monomer {
            let m = g.add(NodeKind::Monomer { component }, [0.0, 0.0]);
            g.links.push(Link {
                from: m,
                out: OutKey::Set,
                to: out,
                input: InKey::Set(0),
            });
        }
        g
    }

    /// The graph a monomer and a list of steps make: the monomer (id 1), each step in turn
    /// (step `k` is node `k + 2`, bypassed when the step is switched off) and the output.
    pub fn chain(monomer: u64, steps: &[Step]) -> Graph {
        let mut nodes = Vec::with_capacity(steps.len() + 2);
        nodes.push(Node {
            id: NodeId(1),
            kind: NodeKind::Monomer { component: monomer },
            pos: [0.0, 0.0],
            bypass: false,
            label: String::new(),
        });
        for (k, step) in steps.iter().enumerate() {
            let mut step = step.clone();
            let bypass = !step.enabled();
            *step.enabled_mut() = true;
            nodes.push(Node {
                id: chain_step_id(k),
                kind: NodeKind::Op(step),
                pos: [CHAIN_SPACING * (k + 1) as f32, 0.0],
                bypass,
                label: String::new(),
            });
        }
        let out = NodeId(steps.len() as u64 + 2);
        nodes.push(Node {
            id: out,
            kind: NodeKind::Output,
            pos: [CHAIN_SPACING * (steps.len() + 1) as f32, 0.0],
            bypass: false,
            label: String::new(),
        });
        let links = nodes
            .windows(2)
            .map(|w| Link {
                from: w[0].id,
                out: OutKey::Set,
                to: w[1].id,
                input: InKey::Set(0),
            })
            .collect();
        Graph {
            nodes,
            links,
            next_id: out.0 + 1,
        }
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// The link feeding input `input` of node `to`.
    pub fn link_into(&self, to: NodeId, input: InKey) -> Option<&Link> {
        self.links.iter().find(|l| l.to == to && l.input == input)
    }

    /// The output node.
    pub fn output(&self) -> Option<NodeId> {
        self.nodes
            .iter()
            .find(|n| matches!(n.kind, NodeKind::Output))
            .map(|n| n.id)
    }

    /// The group output node (inside a custom node).
    pub fn group_output(&self) -> Option<NodeId> {
        self.nodes
            .iter()
            .find(|n| matches!(n.kind, NodeKind::GroupOutput))
            .map(|n| n.id)
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
    pub fn add(&mut self, kind: NodeKind, pos: [f32; 2]) -> NodeId {
        let id = self.alloc_id();
        self.nodes.push(Node {
            id,
            kind,
            pos,
            bypass: false,
            label: String::new(),
        });
        id
    }

    /// The steps of a chain [`Graph::chain`] could have made, with its monomer: `None` when
    /// the graph is not such a chain.
    pub fn as_chain(&self) -> Option<(u64, Vec<Step>)> {
        let output = self.output()?;
        let mut steps = Vec::new();
        let mut at = self.link_into(output, InKey::Set(0))?.from;
        if self.links.len() != self.nodes.len() - 1 {
            return None;
        }
        loop {
            let node = self.node(at)?;
            match &node.kind {
                NodeKind::Monomer { component } => {
                    steps.reverse();
                    return (steps.len() + 2 == self.nodes.len()).then_some((*component, steps));
                }
                NodeKind::Op(step) => {
                    let mut step = step.clone();
                    *step.enabled_mut() = !node.bypass;
                    steps.push(step);
                    let link = self.link_into(at, InKey::Set(0))?;
                    if link.out != OutKey::Set {
                        return None;
                    }
                    at = link.from;
                }
                _ => return None,
            }
        }
    }

    /// Whether the graph's wiring holds together: ids unique, every link between existing
    /// sockets of the right types, one link at most per input, one output node at the top
    /// level (`owner` is `None`) and none inside a custom node, every custom node defined and
    /// no wire loops. The message is for a damaged file, in English.
    pub fn validate(&self, defs: &[NodeDef], owner: Option<&NodeDef>) -> Result<(), String> {
        let mut ids = HashSet::new();
        for n in &self.nodes {
            if !ids.insert(n.id) {
                return Err(format!("node {} appears twice", n.id.0));
            }
            match (&n.kind, owner) {
                (NodeKind::Output, Some(_)) => {
                    return Err("an output node inside a custom node".into());
                }
                (NodeKind::Monomer { .. }, Some(_)) => {
                    return Err("a monomer node inside a custom node".into());
                }
                (NodeKind::GroupInput | NodeKind::GroupOutput, None) => {
                    return Err("a group input or output outside a custom node".into());
                }
                (NodeKind::Group(g), _) if defs::find(defs, &g.def).is_none() => {
                    return Err(format!("custom node {} is not defined", g.def.0));
                }
                _ => {}
            }
        }
        let outputs = self
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Output))
            .count();
        if owner.is_none() && outputs != 1 {
            return Err(format!("{outputs} output nodes (one is needed)"));
        }
        if owner.is_some()
            && self
                .nodes
                .iter()
                .filter(|n| matches!(n.kind, NodeKind::GroupOutput))
                .count()
                > 1
        {
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
            let out = from
                .kind
                .outputs(defs, owner)
                .into_iter()
                .find(|o| o.key == l.out);
            let input = to
                .kind
                .inputs(defs, owner)
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
        if let Some(id) = ops::find_cycle(self) {
            return Err(format!("the links loop back through node {}", id.0));
        }
        Ok(())
    }
}

/// The id [`Graph::chain`] gives step `k`.
pub fn chain_step_id(k: usize) -> NodeId {
    NodeId(k as u64 + 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_is_read_back_as_its_steps() {
        let mut stack = Step::new_stack();
        *stack.enabled_mut() = false;
        let steps = vec![Step::new_ring(), stack, Step::new_bend()];
        let g = Graph::chain(7, &steps);
        assert_eq!(g.nodes.len(), 5);
        assert!(g.node(chain_step_id(1)).unwrap().bypass);
        assert_eq!(g.as_chain(), Some((7, steps)));
        g.validate(&[], None).unwrap();
        let mut branched = g.clone();
        let m = branched.add(NodeKind::Monomer { component: 7 }, [0.0, 100.0]);
        assert!(branched.as_chain().is_none());
        let _ = m;
        assert_eq!(Graph::chain(0, &[]).as_chain(), Some((0, vec![])));
    }

    #[test]
    fn damaged_graphs_are_refused() {
        let g = Graph::chain(0, &[Step::new_ring()]);
        let mut two_outputs = g.clone();
        two_outputs.add(NodeKind::Output, [0.0; 2]);
        assert!(two_outputs.validate(&[], None).is_err());
        let mut dangling = g.clone();
        dangling.links.push(Link {
            from: NodeId(99),
            out: OutKey::Set,
            to: NodeId(2),
            input: InKey::Set(0),
        });
        assert!(dangling.validate(&[], None).is_err());
        let mut wrong_socket = g.clone();
        wrong_socket.links[0].input = InKey::Count;
        assert!(wrong_socket.validate(&[], None).is_err());
        let mut looped = g.clone();
        looped.links.push(Link {
            from: NodeId(2),
            out: OutKey::Set,
            to: NodeId(2),
            input: InKey::Field(StepField::RingRadius),
        });
        assert!(looped.validate(&[], None).is_err(), "a set into a number");
        let mut ids = g;
        ids.nodes[1].id = NodeId(1);
        assert!(ids.validate(&[], None).is_err());
    }
}
