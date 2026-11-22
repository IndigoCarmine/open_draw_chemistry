//! The edits the node editor makes: the `flow_nodes_rs` crate's ([`flow_nodes_rs::ops`]),
//! with their errors in the UI language.

pub use flow_nodes_rs::ops::{
    ConnectError, GroupError, connect, disconnect, duplicate, expose, find_cycle, group, groupable,
    layout, prune_all, prune_links, reaches, removable, remove, ungroup,
};

/// Why two sockets cannot be linked, in the UI language.
pub fn connect_message(e: ConnectError) -> &'static str {
    match e {
        ConnectError::Missing => t!("assembly.editor.error.missing_socket"),
        ConnectError::Type => t!("assembly.editor.error.type"),
        ConnectError::Cycle => t!("assembly.editor.error.cycle"),
    }
}

/// Why nodes cannot be gathered into a custom node, in the UI language.
pub fn group_message(e: GroupError) -> &'static str {
    match e {
        GroupError::Empty => t!("assembly.editor.error.group_empty"),
        GroupError::Recursion => t!("assembly.editor.error.recursion"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::assembly::graph::{ChainGraph, Graph, InKey, NodeId, NodeKind, OutKey, StepField};
    use crate::assembly::model::Step;

    fn chain() -> Graph {
        Graph::chain(0, &[Step::new_ring(), Step::new_stack(), Step::new_bend()])
    }

    #[test]
    fn connecting_checks_types_and_loops() {
        let mut g = chain();
        let v = g.add(NodeKind::Value { value: 3.0 }, [0.0; 2]);
        let ring = super::super::chain_step_id(0);
        let stack = super::super::chain_step_id(1);
        connect(
            &mut g,
            &[],
            None,
            v,
            OutKey::Number,
            ring,
            InKey::Field(StepField::RingRadius),
        )
        .unwrap();
        assert_eq!(
            connect(&mut g, &[], None, v, OutKey::Number, stack, InKey::Set(0)),
            Err(ConnectError::Type)
        );
        assert_eq!(
            connect(&mut g, &[], None, stack, OutKey::Set, ring, InKey::Set(0)),
            Err(ConnectError::Cycle)
        );
        // A number into a triple is fine; replacing a link keeps one per input.
        let array = g.add(NodeKind::Op(Step::new_array()), [0.0; 2]);
        connect(
            &mut g,
            &[],
            None,
            v,
            OutKey::Number,
            array,
            InKey::Field(StepField::ArrayOffset),
        )
        .unwrap();
        connect(&mut g, &[], None, stack, OutKey::Set, array, InKey::Set(0)).unwrap();
        connect(&mut g, &[], None, ring, OutKey::Set, array, InKey::Set(0)).unwrap();
        assert_eq!(g.links.iter().filter(|l| l.to == array).count(), 2);
        assert!(find_cycle(&g).is_none());
        g.validate(&[], None).unwrap();
    }

    #[test]
    fn removing_a_step_keeps_the_chain() {
        let mut g = chain();
        remove(&mut g, &[], None, &[super::super::chain_step_id(1)].into());
        let (_, steps) = g.as_chain().unwrap();
        assert_eq!(steps, vec![Step::new_ring(), Step::new_bend()]);
        let out = g.output().unwrap();
        remove(&mut g, &[], None, &[out].into());
        assert!(g.output().is_some(), "the output node stays");
    }

    #[test]
    fn duplicating_copies_inner_links() {
        let mut g = chain();
        let ids: BTreeSet<NodeId> = [
            super::super::chain_step_id(0),
            super::super::chain_step_id(1),
        ]
        .into();
        let copies = duplicate(&mut g, &ids, [0.0, 100.0]);
        assert_eq!(copies.len(), 2);
        assert!(
            g.links
                .iter()
                .any(|l| l.from == copies[0] && l.to == copies[1])
        );
        g.validate(&[], None).unwrap();
    }
}
