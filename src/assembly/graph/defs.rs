//! Custom nodes: a graph of nodes behind one node, with inputs and outputs of its own.
//!
//! A design keeps a copy of every custom node it uses ([`crate::assembly::Design::defs`]), so
//! a file opens the same anywhere. The user's library ([`super::library`]) keeps them to be
//! used again; the copy remembers which library version it came from ([`LibraryStamp`]), so
//! the two can be told apart when either changes ([`status`]). The machinery is the
//! `flow_nodes_rs` crate's ([`flow_nodes_rs::def`]).

pub use flow_nodes_rs::def::{
    DefId, LibraryStamp, Status, content_hash, depends_on, find, find_mut, graph_uses,
    graph_uses_all, status, validate,
};

use super::kinds::{NodeKind, PortValue, SocketType};

/// A custom node.
pub type NodeDef = flow_nodes_rs::NodeDef<NodeKind>;

/// An input or output of a custom node.
pub type Port = flow_nodes_rs::Port<SocketType, PortValue>;

/// An empty custom node with one molecule input and output, wired straight through.
pub fn new_def(name: impl Into<String>) -> NodeDef {
    NodeDef::passing(
        name,
        SocketType::Set,
        t!("assembly.node.socket.molecules"),
        super::CHAIN_SPACING,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::model::Step;

    use crate::assembly::graph::{InKey, OutKey};

    fn def() -> NodeDef {
        let mut d = new_def("ring");
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
