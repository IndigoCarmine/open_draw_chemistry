//! The graph being edited, found by its scope: the design's own graph at the top level, or a
//! custom node's graph inside the custom nodes the scope enters. The walk is the
//! `flow_nodes_rs` crate's ([`flow_nodes_rs::scope`]); here the top level is the design's,
//! which may still be a chain of steps.

use std::borrow::Cow;

use super::defs::{self, DefId, NodeDef};
use super::{Graph, NodeId};
use crate::assembly::model::Design;

/// The custom node whose graph `scope` is in (`Some(None)` at the top level), or `None` when
/// the scope does not lead anywhere (a node on the way was removed, or is not a custom node).
pub fn scope_def(design: &Design, scope: &[NodeId]) -> Option<Option<DefId>> {
    let top = design.effective_graph();
    flow_nodes_rs::scope::resolve(&top, &design.defs, scope).map(|d| d.map(|d| d.id.clone()))
}

/// The graph at `scope`, with the custom node it belongs to.
pub fn graph_at<'a>(
    design: &'a Design,
    scope: &[NodeId],
) -> Option<(Cow<'a, Graph>, Option<&'a NodeDef>)> {
    match scope_def(design, scope)? {
        None => Some((design.effective_graph(), None)),
        Some(id) => {
            let def = defs::find(&design.defs, &id)?;
            Some((Cow::Borrowed(&def.graph), Some(def)))
        }
    }
}

/// Change where the nodes of the graph at `scope` are drawn with `f`. A design still made of
/// steps becomes a graph, which keeps them.
pub fn edit_layout_at(design: &mut Design, scope: &[NodeId], f: impl FnOnce(&mut Graph)) {
    if scope.is_empty() {
        match design.graph.as_mut() {
            Some(graph) => f(graph),
            // A chain stays steps: only where its nodes are drawn is kept.
            None => {
                let mut graph = design.effective_graph().into_owned();
                f(&mut graph);
                design.chain_layout = graph.nodes.iter().map(|n| n.pos).collect();
            }
        }
    } else {
        edit_at(design, scope, |g, _, _| f(g));
    }
}

/// Change the graph at `scope` with `f`, which is also given the design's custom nodes (to
/// add to, as grouping does) and the id of the one being edited. A design still made of steps
/// stays so when the result is again such a chain (a parameter changed, a step bypassed or
/// removed); otherwise it becomes a graph. `None` when the scope leads nowhere.
pub fn edit_at<R>(
    design: &mut Design,
    scope: &[NodeId],
    f: impl FnOnce(&mut Graph, &mut Vec<NodeDef>, Option<&DefId>) -> R,
) -> Option<R> {
    match scope_def(design, scope)? {
        // `f` may add custom nodes, never remove one.
        Some(id) => flow_nodes_rs::scope::edit_def(&mut design.defs, &id, f),
        // A graph that is (again) a chain of a monomer and steps is kept as steps, so the
        // panel shows it as such and the file opens in older releases.
        None => {
            let mut graph = design
                .graph
                .take()
                .unwrap_or_else(|| design.effective_graph().into_owned());
            let r = f(&mut graph, &mut design.defs, None);
            if !design.take_chain(&graph) {
                design.steps.clear();
                design.graph = Some(graph);
            }
            Some(r)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::graph::{chain_step_id, ops};
    use crate::assembly::model::{Component, Step};
    use crate::three_d::Atom3d;

    fn design() -> Design {
        Design {
            components: vec![Component {
                name: "x".into(),
                atoms: vec![Atom3d {
                    element: "C".into(),
                    pos: [0.0; 3],
                }],
                bonds: vec![],
                orient: None,
            }],
            steps: vec![Step::new_ring(), Step::new_stack()],
            ..Default::default()
        }
    }

    #[test]
    fn a_chain_stays_steps_until_it_branches() {
        let mut d = design();
        edit_at(&mut d, &[], |g, _, _| {
            let n = g.node_mut(chain_step_id(0)).unwrap();
            n.bypass = true;
        })
        .unwrap();
        assert!(d.is_linear());
        assert!(!d.steps[0].enabled());
        edit_at(&mut d, &[], |g, _, _| {
            ops::remove(g, &[], None, &[chain_step_id(0)].into());
        })
        .unwrap();
        assert!(d.is_linear());
        assert_eq!(d.steps, vec![Step::new_stack()]);
        // Moving a node keeps the steps, and where the node is drawn.
        edit_layout_at(&mut d, &[], |g| {
            g.node_mut(chain_step_id(0)).unwrap().pos[1] += 40.0;
        });
        assert!(d.is_linear());
        assert_eq!(
            d.effective_graph().node(chain_step_id(0)).unwrap().pos[1],
            40.0
        );
        // A loose node makes a graph; taking it away makes the steps again, drawn where they
        // were.
        let loose = edit_at(&mut d, &[], |g, _, _| {
            g.add(
                crate::assembly::graph::NodeKind::Value { value: 1.0 },
                [0.0; 2],
            )
        })
        .unwrap();
        assert!(!d.is_linear());
        assert!(d.steps.is_empty());
        edit_at(&mut d, &[], |g, _, _| {
            ops::remove(g, &[], None, &[loose].into())
        })
        .unwrap();
        assert!(d.is_linear());
        assert_eq!(d.steps, vec![Step::new_stack()]);
        assert_eq!(
            d.effective_graph().node(chain_step_id(0)).unwrap().pos[1],
            40.0
        );
    }

    #[test]
    fn the_inside_of_a_custom_node_is_edited_in_place() {
        let mut d = design();
        let node = edit_at(&mut d, &[], |g, defs, _| {
            ops::group(g, defs, None, &[chain_step_id(0)].into(), "r")
                .unwrap()
                .1
        })
        .unwrap();
        let (inner, owner) = graph_at(&d, &[node]).unwrap();
        assert!(owner.is_some());
        let ring = inner
            .nodes
            .iter()
            .find(|n| matches!(n.kind, crate::assembly::graph::NodeKind::Op(_)))
            .unwrap()
            .id;
        edit_at(&mut d, &[node], |g, _, owner| {
            assert!(owner.is_some());
            g.node_mut(ring).unwrap().bypass = true;
        })
        .unwrap();
        assert_eq!(d.instances().len(), 10, "the ring inside is bypassed");
        assert!(graph_at(&d, &[NodeId(999)]).is_none());
    }
}
