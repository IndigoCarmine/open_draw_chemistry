//! The graph being edited, found by its scope: the design's own graph at the top level, or a
//! custom node's graph inside the custom nodes the scope enters.

use std::borrow::Cow;

use super::defs::{self, DefId, NodeDef};
use super::{Graph, NodeId, NodeKind};
use crate::assembly::model::Design;

/// The custom node whose graph `scope` is in (`Some(None)` at the top level), or `None` when
/// the scope does not lead anywhere (a node on the way was removed, or is not a custom node).
pub fn scope_def(design: &Design, scope: &[NodeId]) -> Option<Option<DefId>> {
    let top = design.effective_graph();
    let mut graph: &Graph = &top;
    let mut def: Option<&NodeDef> = None;
    for id in scope {
        let NodeKind::Group(g) = &graph.node(*id)?.kind else {
            return None;
        };
        let d = defs::find(&design.defs, &g.def)?;
        def = Some(d);
        graph = &d.graph;
    }
    Some(def.map(|d| d.id.clone()))
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
        f(design.graph_mut());
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
        Some(id) => {
            let k = design.defs.iter().position(|d| d.id == id)?;
            let mut graph = std::mem::take(&mut design.defs[k].graph);
            let r = f(&mut graph, &mut design.defs, Some(&id));
            // `f` may have added custom nodes, never removed one.
            if let Some(def) = defs::find_mut(&mut design.defs, &id) {
                def.graph = graph;
            }
            Some(r)
        }
        None if design.graph.is_some() => {
            let mut graph = design.graph.take().expect("checked");
            let r = f(&mut graph, &mut design.defs, None);
            design.graph = Some(graph);
            Some(r)
        }
        None => {
            let mut graph = design.effective_graph().into_owned();
            let r = f(&mut graph, &mut design.defs, None);
            // Where the nodes of a chain are drawn is not kept: it is laid out again.
            let linear = graph
                .as_chain()
                .and_then(|(monomer, steps)| Some((design.component_index(monomer)?, steps)));
            match linear {
                Some((base, steps)) => {
                    design.base = base;
                    design.steps = steps;
                }
                None => {
                    design.steps.clear();
                    design.graph = Some(graph);
                }
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
    fn a_parameter_change_keeps_the_steps_a_move_makes_a_graph() {
        let mut d = design();
        edit_at(&mut d, &[], |g, _, _| {
            let n = g.node_mut(chain_step_id(0)).unwrap();
            n.bypass = true;
        })
        .unwrap();
        assert!(d.is_linear());
        assert!(!d.steps[0].enabled());
        edit_at(&mut d, &[], |g, _, _| {
            ops::remove(g, &[chain_step_id(0)].into());
        })
        .unwrap();
        assert!(d.is_linear());
        assert_eq!(d.steps, vec![Step::new_stack()]);
        edit_layout_at(&mut d, &[], |g| {
            g.node_mut(chain_step_id(0)).unwrap().pos[1] += 40.0;
        });
        assert!(!d.is_linear());
        assert!(d.steps.is_empty());
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
            .find(|n| matches!(n.kind, NodeKind::Op(_)))
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
