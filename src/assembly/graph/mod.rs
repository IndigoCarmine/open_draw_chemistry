//! The assembly as a graph of nodes: monomers feed copying, moving, merging and selecting
//! nodes, numbers and expressions feed their parameters, and the output node is the assembly.
//! Nodes can be gathered into custom nodes ([`defs::NodeDef`]) with inputs and outputs of
//! their own, kept in the design and in the user's library ([`library`]).
//!
//! The machinery — graphs, custom nodes, the edits, the evaluator, expressions, the library —
//! is the `flow_nodes_rs` crate; this module is the assembly's kinds of node on top of it:
//!
//! - [`kinds`]: what each node is, its sockets and settings (the crate's `NodeKind`);
//! - [`field`]: the numbers of the building steps as sockets;
//! - [`eval`]: how each kind computes its molecules (the crate's `Evaluate`);
//! - [`expr`], [`ops`], [`defs`], [`library`]: the crate's, worded in the UI language;
//! - [`access`]: the graph being edited, found in the design by its scope.
//!
//! A design made of a monomer and a list of steps (every file before version 5) is the chain
//! [`ChainGraph::chain`] makes; it becomes a graph of its own only when the graph is edited.

pub mod access;
pub mod defs;
pub mod eval;
pub mod expr;
pub mod field;
pub mod kinds;
pub mod library;
pub mod ops;

use serde::{Deserialize, Serialize};

pub use defs::{DefId, NodeDef};
pub use eval::{Evaluation, Issue, Scope, Set, Shown};
pub use field::StepField;
pub use flow_nodes_rs::NodeId;
pub use kinds::{NodeKind, PortValue, SocketType};

use crate::assembly::model::Step;

/// One node: what it is, where it is drawn, and whether it is bypassed.
pub type Node = flow_nodes_rs::Node<NodeKind>;
/// A wire: output `out` of node `from` feeds input `input` of node `to`.
pub type Link = flow_nodes_rs::Link<InKey, OutKey>;
/// Nodes and the wires between them.
pub type Graph = flow_nodes_rs::Graph<NodeKind>;

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
    /// Curves to take points along.
    Curve,
    /// Points to put copies on.
    Points,
    /// Another number or triple of a node, by its place.
    Param(u8),
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
    Curve,
    Points,
}

/// Horizontal distance between the nodes of a chain.
pub const CHAIN_SPACING: f32 = flow_nodes_rs::SPACING;

/// The assembly's own ways of making and reading graphs: a monomer into the output, and the
/// chain of a monomer and a list of steps (how designs before version 5 are read).
pub trait ChainGraph: Sized {
    /// A monomer straight into the output.
    fn new(monomer: Option<u64>) -> Self;
    /// The graph a monomer and a list of steps make: the monomer (id 1), each step in turn
    /// (step `k` is node `k + 2`, bypassed when the step is switched off) and the output.
    fn chain(monomer: u64, steps: &[Step]) -> Self;
    /// The nodes of a chain ([`ChainGraph::as_chain`]) in order: the monomer, the steps, the
    /// output.
    fn chain_order(&self) -> Option<Vec<NodeId>>;
    /// The monomer and steps of a chain [`ChainGraph::chain`] could have made: `None` when the
    /// graph is not such a chain.
    fn as_chain(&self) -> Option<(u64, Vec<Step>)>;
}

impl ChainGraph for Graph {
    /// A monomer straight into the output.
    fn new(monomer: Option<u64>) -> Graph {
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
    fn chain(monomer: u64, steps: &[Step]) -> Graph {
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

    /// The steps of a chain [`Graph::chain`] could have made, with its monomer: `None` when
    /// the graph is not such a chain.
    /// The nodes of a chain ([`Graph::as_chain`]) in order: the monomer, the steps, the output.
    fn chain_order(&self) -> Option<Vec<NodeId>> {
        self.as_chain()?;
        let output = self.output()?;
        let mut order = vec![output];
        let mut at = self.link_into(output, InKey::Set(0))?.from;
        loop {
            order.push(at);
            match self.link_into(at, InKey::Set(0)) {
                Some(link) => at = link.from,
                None => break,
            }
        }
        order.reverse();
        Some(order)
    }

    fn as_chain(&self) -> Option<(u64, Vec<Step>)> {
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
