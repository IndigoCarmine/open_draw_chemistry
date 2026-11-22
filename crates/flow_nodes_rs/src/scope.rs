//! Graphs inside custom nodes, found by their scope: the custom-node nodes entered to reach
//! them, outermost first (empty at the top level).

use crate::def::{self, DefId, NodeDef};
use crate::graph::Graph;
use crate::id::NodeId;
use crate::schema::{NodeKind, Role};

/// Where a node is: the custom nodes entered to reach it, outermost first (empty at the top).
pub type Scope = Vec<NodeId>;

/// The custom node whose graph `scope` leads into from `top` (`Some(None)` at the top level),
/// or `None` when it leads nowhere (a node on the way was removed, or is not a custom node).
pub fn resolve<'a, K: NodeKind>(
    top: &'a Graph<K>,
    defs: &'a [NodeDef<K>],
    scope: &[NodeId],
) -> Option<Option<&'a NodeDef<K>>> {
    let mut graph = top;
    let mut def: Option<&NodeDef<K>> = None;
    for id in scope {
        let Role::Group(g) = graph.node(*id)?.kind.role() else {
            return None;
        };
        let d = def::find(defs, &g.def)?;
        def = Some(d);
        graph = &d.graph;
    }
    Some(def)
}

/// The graph `scope` leads into from `top`, with the custom node it belongs to.
pub fn graph_at<'a, K: NodeKind>(
    top: &'a Graph<K>,
    defs: &'a [NodeDef<K>],
    scope: &[NodeId],
) -> Option<(&'a Graph<K>, Option<&'a NodeDef<K>>)> {
    Some(match resolve(top, defs, scope)? {
        None => (top, None),
        Some(def) => (&def.graph, Some(def)),
    })
}

/// Change the graph of custom node `id` with `f`, which is also given every custom node (to
/// add to, as grouping does) and `id`. The graph is taken out while `f` runs, so `f` may change
/// the other custom nodes freely; it must not remove the one it edits. `None` when there is no
/// such custom node.
pub fn edit_def<K: NodeKind, R>(
    defs: &mut Vec<NodeDef<K>>,
    id: &DefId,
    f: impl FnOnce(&mut Graph<K>, &mut Vec<NodeDef<K>>, Option<&DefId>) -> R,
) -> Option<R> {
    let k = defs.iter().position(|d| &d.id == id)?;
    let mut graph = std::mem::take(&mut defs[k].graph);
    let r = f(&mut graph, defs, Some(id));
    if let Some(def) = def::find_mut(defs, id) {
        def.graph = graph;
    }
    Some(r)
}
