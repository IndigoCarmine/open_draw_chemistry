//! The edits the node editor makes, as plain functions on a graph (and the custom nodes), so
//! each is tested without a window.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::defs::{self, DefId, NodeDef, Port};
use super::kinds::{GroupRef, NodeKind, SocketType};
use super::{Graph, InKey, Link, Node, NodeId, OutKey};

/// Why two sockets cannot be linked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectError {
    /// A node or socket that is not there.
    Missing,
    /// The output's type cannot feed the input's.
    Type,
    /// The link would loop back.
    Cycle,
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ConnectError::Missing => t!("assembly.editor.error.missing_socket"),
            ConnectError::Type => t!("assembly.editor.error.type"),
            ConnectError::Cycle => t!("assembly.editor.error.cycle"),
        })
    }
}

/// A node on a loop of links, if there is one.
pub fn find_cycle(graph: &Graph) -> Option<NodeId> {
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

/// Whether `to` feeds `from`, directly or through others (so `from → to` would loop).
pub fn reaches(graph: &Graph, from: NodeId, to: NodeId) -> bool {
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

/// Link output `out` of `from` to input `input` of `to`, replacing what fed that input.
pub fn connect(
    graph: &mut Graph,
    defs: &[NodeDef],
    owner: Option<&NodeDef>,
    from: NodeId,
    out: OutKey,
    to: NodeId,
    input: InKey,
) -> Result<(), ConnectError> {
    let (Some(a), Some(b)) = (graph.node(from), graph.node(to)) else {
        return Err(ConnectError::Missing);
    };
    let out_ty = a
        .kind
        .outputs(defs, owner)
        .into_iter()
        .find(|o| o.key == out)
        .ok_or(ConnectError::Missing)?
        .ty;
    let in_ty = b
        .kind
        .inputs(defs, owner)
        .into_iter()
        .find(|i| i.key == input)
        .ok_or(ConnectError::Missing)?
        .ty;
    if !out_ty.feeds(in_ty) {
        return Err(ConnectError::Type);
    }
    if from == to || reaches(graph, to, from) {
        return Err(ConnectError::Cycle);
    }
    graph.links.retain(|l| !(l.to == to && l.input == input));
    graph.links.push(Link {
        from,
        out,
        to,
        input,
    });
    Ok(())
}

/// Remove the link into input `input` of `to`. Returns it.
pub fn disconnect(graph: &mut Graph, to: NodeId, input: InKey) -> Option<Link> {
    let k = graph
        .links
        .iter()
        .position(|l| l.to == to && l.input == input)?;
    Some(graph.links.remove(k))
}

/// Whether the user may delete the node (not the output node).
pub fn removable(node: &Node) -> bool {
    !matches!(node.kind, NodeKind::Output)
}

/// Remove nodes and their links. A node removed between a molecule input and a molecule output
/// joins them, so taking a step out of a chain keeps the chain.
pub fn remove(graph: &mut Graph, ids: &BTreeSet<NodeId>) {
    let ids: BTreeSet<NodeId> = ids
        .iter()
        .copied()
        .filter(|id| graph.node(*id).is_some_and(removable))
        .collect();
    // Splice: for each removed node fed on Set(0) and feeding on Set, reconnect around it.
    let mut splices = Vec::new();
    for &id in &ids {
        let Some(mut feed) = graph.link_into(id, InKey::Set(0)).copied() else {
            continue;
        };
        // Through a run of removed nodes.
        let mut guard = 0;
        while ids.contains(&feed.from) && guard < ids.len() {
            match graph.link_into(feed.from, InKey::Set(0)) {
                Some(l) => feed = *l,
                None => break,
            }
            guard += 1;
        }
        if ids.contains(&feed.from) {
            continue;
        }
        for l in graph
            .links
            .iter()
            .filter(|l| l.from == id && matches!(l.out, OutKey::Set | OutKey::Selected))
        {
            if !ids.contains(&l.to) {
                splices.push(Link {
                    from: feed.from,
                    out: feed.out,
                    to: l.to,
                    input: l.input,
                });
            }
        }
    }
    graph
        .links
        .retain(|l| !ids.contains(&l.from) && !ids.contains(&l.to));
    graph.nodes.retain(|n| !ids.contains(&n.id));
    for s in splices {
        if graph.link_into(s.to, s.input).is_none() {
            graph.links.push(s);
        }
    }
}

/// Copies of nodes `ids` (and the links between them), moved by `offset`. Returns the copies'
/// ids, in the order of `ids`. Output nodes are not copied.
pub fn duplicate(graph: &mut Graph, ids: &BTreeSet<NodeId>, offset: [f32; 2]) -> Vec<NodeId> {
    let mut map = HashMap::new();
    let mut out = Vec::new();
    let originals: Vec<Node> = graph
        .nodes
        .iter()
        .filter(|n| ids.contains(&n.id) && removable(n))
        .cloned()
        .collect();
    for mut node in originals {
        let id = graph.alloc_id();
        map.insert(node.id, id);
        node.id = id;
        node.pos = [node.pos[0] + offset[0], node.pos[1] + offset[1]];
        graph.nodes.push(node);
        out.push(id);
    }
    let inner: Vec<Link> = graph
        .links
        .iter()
        .filter_map(|l| {
            Some(Link {
                from: *map.get(&l.from)?,
                out: l.out,
                to: *map.get(&l.to)?,
                input: l.input,
            })
        })
        .collect();
    graph.links.extend(inner);
    out
}

/// Why nodes cannot be gathered into a custom node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupError {
    /// Nothing that can be grouped is selected.
    Empty,
    /// The custom node would use itself.
    Recursion,
}

impl std::fmt::Display for GroupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            GroupError::Empty => t!("assembly.editor.error.group_empty"),
            GroupError::Recursion => t!("assembly.editor.error.recursion"),
        })
    }
}

/// Whether a node can go into a custom node (monomers and the input/output nodes cannot).
pub fn groupable(node: &Node) -> bool {
    !matches!(
        node.kind,
        NodeKind::Output | NodeKind::GroupInput | NodeKind::GroupOutput | NodeKind::Monomer { .. }
    )
}

/// Gather nodes `ids` of `graph` (the graph of custom node `owner`, or the top level) into a
/// new custom node named `name`, added to `defs`, and put one node using it in their place.
/// Each output outside the selection feeding nodes inside becomes an input of the custom node,
/// each output inside feeding nodes outside one of its outputs. Returns the new definition's
/// id and the node using it. What the graph makes does not change.
pub fn group(
    graph: &mut Graph,
    defs: &mut Vec<NodeDef>,
    owner: Option<&DefId>,
    ids: &BTreeSet<NodeId>,
    name: &str,
) -> Result<(DefId, NodeId), GroupError> {
    let owner_def = owner.and_then(|o| defs::find(defs, o)).cloned();
    let chosen: BTreeSet<NodeId> = ids
        .iter()
        .copied()
        .filter(|id| graph.node(*id).is_some_and(groupable))
        .collect();
    if chosen.is_empty() {
        return Err(GroupError::Empty);
    }
    if let Some(owner) = owner {
        // A custom node using the owner, moved into a new node the owner uses, would loop.
        let uses_owner = chosen
            .iter()
            .any(|id| match &graph.node(*id).unwrap().kind {
                NodeKind::Group(g) => defs::depends_on(defs, &g.def, owner),
                _ => false,
            });
        if uses_owner {
            return Err(GroupError::Recursion);
        }
    }
    let type_of_out = |graph: &Graph, from: NodeId, out: OutKey| -> SocketType {
        graph
            .node(from)
            .and_then(|n| {
                n.kind
                    .outputs(defs, owner_def.as_ref())
                    .into_iter()
                    .find(|o| o.key == out)
            })
            .map_or(SocketType::Set, |o| o.ty)
    };
    let label_of_in =
        |graph: &Graph, to: NodeId, input: InKey| -> (String, Option<super::PortValue>) {
            graph
                .node(to)
                .map(|n| {
                    let label = n
                        .kind
                        .inputs(defs, owner_def.as_ref())
                        .into_iter()
                        .find(|i| i.key == input)
                        .map_or_else(String::new, |i| i.label);
                    (label, n.kind.inline(input))
                })
                .unwrap_or_default()
        };

    let mut def = NodeDef::new(name);
    def.inputs.clear();
    def.outputs.clear();
    def.graph = Graph::default();
    // Inputs: each distinct outside source feeding inside.
    let mut in_ports: Vec<((NodeId, OutKey), u32)> = Vec::new();
    let mut out_ports: Vec<((NodeId, OutKey), u32)> = Vec::new();
    let mut key = 0u32;
    for l in graph
        .links
        .iter()
        .filter(|l| !chosen.contains(&l.from) && chosen.contains(&l.to))
    {
        if in_ports.iter().any(|(s, _)| *s == (l.from, l.out)) {
            continue;
        }
        let ty = type_of_out(graph, l.from, l.out);
        let (label, inline) = label_of_in(graph, l.to, l.input);
        def.inputs.push(Port {
            key,
            name: if label.is_empty() {
                format!("in {key}")
            } else {
                label
            },
            ty,
            default: if ty == SocketType::Set { None } else { inline },
        });
        in_ports.push(((l.from, l.out), key));
        key += 1;
    }
    for l in graph
        .links
        .iter()
        .filter(|l| chosen.contains(&l.from) && !chosen.contains(&l.to))
    {
        if out_ports.iter().any(|(s, _)| *s == (l.from, l.out)) {
            continue;
        }
        let ty = type_of_out(graph, l.from, l.out);
        let label = graph
            .node(l.from)
            .and_then(|n| {
                n.kind
                    .outputs(defs, owner_def.as_ref())
                    .into_iter()
                    .find(|o| o.key == l.out)
            })
            .map_or_else(String::new, |o| o.label);
        def.outputs.push(Port {
            key,
            name: if label.is_empty() {
                format!("out {key}")
            } else {
                label
            },
            ty,
            default: None,
        });
        out_ports.push(((l.from, l.out), key));
        key += 1;
    }

    // Lay out: the inside keeps its arrangement; group input left, output right.
    let inside: Vec<Node> = graph
        .nodes
        .iter()
        .filter(|n| chosen.contains(&n.id))
        .cloned()
        .collect();
    let min_x = inside
        .iter()
        .map(|n| n.pos[0])
        .fold(f32::INFINITY, f32::min);
    let max_x = inside
        .iter()
        .map(|n| n.pos[0])
        .fold(f32::NEG_INFINITY, f32::max);
    let mid_y = inside.iter().map(|n| n.pos[1]).sum::<f32>() / inside.len() as f32;
    let centre = [
        inside.iter().map(|n| n.pos[0]).sum::<f32>() / inside.len() as f32,
        mid_y,
    ];

    let mut map = HashMap::new();
    for mut node in inside {
        let id = def.graph.alloc_id();
        map.insert(node.id, id);
        node.id = id;
        def.graph.nodes.push(node);
    }
    let gin = def
        .graph
        .add(NodeKind::GroupInput, [min_x - super::CHAIN_SPACING, mid_y]);
    let gout = def
        .graph
        .add(NodeKind::GroupOutput, [max_x + super::CHAIN_SPACING, mid_y]);
    for l in &graph.links {
        let (fi, ti) = (chosen.contains(&l.from), chosen.contains(&l.to));
        match (fi, ti) {
            (true, true) => def.graph.links.push(Link {
                from: map[&l.from],
                out: l.out,
                to: map[&l.to],
                input: l.input,
            }),
            (false, true) => {
                let port = in_ports
                    .iter()
                    .find(|(s, _)| *s == (l.from, l.out))
                    .map(|(_, k)| *k)
                    .unwrap();
                def.graph.links.push(Link {
                    from: gin,
                    out: OutKey::Port(port),
                    to: map[&l.to],
                    input: l.input,
                });
            }
            (true, false) => {
                let port = out_ports
                    .iter()
                    .find(|(s, _)| *s == (l.from, l.out))
                    .map(|(_, k)| *k)
                    .unwrap();
                if def.graph.link_into(gout, InKey::Port(port)).is_none() {
                    def.graph.links.push(Link {
                        from: map[&l.from],
                        out: l.out,
                        to: gout,
                        input: InKey::Port(port),
                    });
                }
            }
            (false, false) => {}
        }
    }

    // Replace the nodes by one using the definition.
    let def_id = def.id.clone();
    let inline: BTreeMap<u32, super::PortValue> = def
        .inputs
        .iter()
        .filter_map(|p| Some((p.key, p.default?)))
        .collect();
    let new_links: Vec<Link> = graph
        .links
        .iter()
        .filter_map(|l| {
            let (fi, ti) = (chosen.contains(&l.from), chosen.contains(&l.to));
            match (fi, ti) {
                (false, true) => None, // re-made below, once per port
                (true, false) => Some(Link {
                    from: NodeId(u64::MAX), // patched below
                    out: OutKey::Port(
                        out_ports
                            .iter()
                            .find(|(s, _)| *s == (l.from, l.out))
                            .map(|(_, k)| *k)
                            .unwrap(),
                    ),
                    to: l.to,
                    input: l.input,
                }),
                (true, true) => None,
                (false, false) => Some(*l),
            }
        })
        .collect();
    graph.nodes.retain(|n| !chosen.contains(&n.id));
    let node = graph.add(
        NodeKind::Group(GroupRef {
            def: def_id.clone(),
            inputs: inline,
        }),
        centre,
    );
    graph.links = new_links
        .into_iter()
        .map(|mut l| {
            if l.from == NodeId(u64::MAX) {
                l.from = node;
            }
            l
        })
        .collect();
    for ((from, out), port) in in_ports {
        graph.links.push(Link {
            from,
            out,
            to: node,
            input: InKey::Port(port),
        });
    }
    defs.push(def);
    Ok((def_id, node))
}

/// Put the nodes of custom node `node` (a node of `graph`) back in its place: the inverse of
/// [`group`]. Returns the ids the inner nodes got.
pub fn ungroup(graph: &mut Graph, defs: &[NodeDef], node: NodeId) -> Option<Vec<NodeId>> {
    let NodeKind::Group(g) = &graph.node(node)?.kind else {
        return None;
    };
    let inline = g.inputs.clone();
    let def = defs::find(defs, &g.def)?.clone();
    let at = graph.node(node)?.pos;
    let gin: HashSet<NodeId> = def
        .graph
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::GroupInput))
        .map(|n| n.id)
        .collect();
    let gout = def.graph.group_output();
    let inner: Vec<&Node> = def
        .graph
        .nodes
        .iter()
        .filter(|n| !gin.contains(&n.id) && Some(n.id) != gout)
        .collect();
    let cx = inner.iter().map(|n| n.pos[0]).sum::<f32>() / inner.len().max(1) as f32;
    let cy = inner.iter().map(|n| n.pos[1]).sum::<f32>() / inner.len().max(1) as f32;
    let mut map = HashMap::new();
    let mut added = Vec::new();
    for n in &inner {
        let id = graph.alloc_id();
        map.insert(n.id, id);
        let mut copy = (*n).clone();
        copy.id = id;
        copy.pos = [n.pos[0] - cx + at[0], n.pos[1] - cy + at[1]];
        graph.nodes.push(copy);
        added.push(id);
    }
    // What fed each input port, and the nodes the outputs fed.
    let feeds: HashMap<u32, (NodeId, OutKey)> = graph
        .links
        .iter()
        .filter(|l| l.to == node)
        .filter_map(|l| match l.input {
            InKey::Port(p) => Some((p, (l.from, l.out))),
            _ => None,
        })
        .collect();
    let fed: Vec<(u32, NodeId, InKey)> = graph
        .links
        .iter()
        .filter(|l| l.from == node)
        .filter_map(|l| match l.out {
            OutKey::Port(p) => Some((p, l.to, l.input)),
            _ => None,
        })
        .collect();
    graph.links.retain(|l| l.from != node && l.to != node);
    graph.nodes.retain(|n| n.id != node);
    for l in &def.graph.links {
        let to_gout = Some(l.to) == gout;
        match (gin.contains(&l.from), to_gout) {
            (false, false) => {
                if let (Some(&f), Some(&t)) = (map.get(&l.from), map.get(&l.to)) {
                    graph.links.push(Link {
                        from: f,
                        out: l.out,
                        to: t,
                        input: l.input,
                    });
                }
            }
            (true, false) => {
                let OutKey::Port(p) = l.out else { continue };
                let Some(&t) = map.get(&l.to) else { continue };
                match feeds.get(&p) {
                    Some(&(from, out)) => graph.links.push(Link {
                        from,
                        out,
                        to: t,
                        input: l.input,
                    }),
                    None => {
                        // Unfed: the port's value becomes the input's.
                        let value = inline
                            .get(&p)
                            .copied()
                            .or_else(|| def.inputs.iter().find(|q| q.key == p)?.default);
                        if let (Some(v), Some(n)) = (value, graph.node_mut(t)) {
                            n.kind.set_inline(l.input, v);
                        }
                    }
                }
            }
            (false, true) => {
                let InKey::Port(p) = l.input else { continue };
                let Some(&f) = map.get(&l.from) else { continue };
                for &(q, to, input) in &fed {
                    if q == p {
                        graph.links.push(Link {
                            from: f,
                            out: l.out,
                            to,
                            input,
                        });
                    }
                }
            }
            (true, true) => {
                // An input wired straight to an output.
                let (OutKey::Port(p), InKey::Port(q)) = (l.out, l.input) else {
                    continue;
                };
                if let Some(&(from, out)) = feeds.get(&p) {
                    for &(r, to, input) in &fed {
                        if r == q {
                            graph.links.push(Link {
                                from,
                                out,
                                to,
                                input,
                            });
                        }
                    }
                }
            }
        }
    }
    Some(added)
}

/// Make unlinked input `input` of node `node` (inside custom node `def`) an input of the
/// custom node: a new port with the input's value as its default, linked from a group input
/// node. Returns the port's key.
pub fn expose(def: &mut NodeDef, defs: &[NodeDef], node: NodeId, input: InKey) -> Option<u32> {
    let owner = def.clone();
    let n = def.graph.node(node)?;
    let decl = n
        .kind
        .inputs(defs, Some(&owner))
        .into_iter()
        .find(|i| i.key == input)?;
    if def.graph.link_into(node, input).is_some() {
        return None;
    }
    let value = n.kind.inline(input);
    let pos = n.pos;
    let key = def.next_port_key();
    def.inputs.push(Port {
        key,
        name: decl.label,
        ty: decl.ty,
        default: if decl.ty == SocketType::Set {
            None
        } else {
            value
        },
    });
    let gin = match def
        .graph
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::GroupInput))
    {
        Some(n) => n.id,
        None => def.graph.add(
            NodeKind::GroupInput,
            [pos[0] - super::CHAIN_SPACING, pos[1]],
        ),
    };
    def.graph.links.push(Link {
        from: gin,
        out: OutKey::Port(key),
        to: node,
        input,
    });
    Some(key)
}

/// Drop the links of `graph` that no longer fit the sockets (after a custom node's ports
/// changed). Returns how many were dropped.
pub fn prune_links(graph: &mut Graph, defs: &[NodeDef], owner: Option<&NodeDef>) -> usize {
    let before = graph.links.len();
    let fits = |l: &Link, graph: &Graph| {
        let (Some(a), Some(b)) = (graph.node(l.from), graph.node(l.to)) else {
            return false;
        };
        let out = a
            .kind
            .outputs(defs, owner)
            .into_iter()
            .find(|o| o.key == l.out);
        let input = b
            .kind
            .inputs(defs, owner)
            .into_iter()
            .find(|i| i.key == l.input);
        matches!((out, input), (Some(o), Some(i)) if o.ty.feeds(i.ty))
    };
    let keep: Vec<bool> = graph.links.iter().map(|l| fits(l, graph)).collect();
    let mut k = 0;
    graph.links.retain(|_| {
        k += 1;
        keep[k - 1]
    });
    before - graph.links.len()
}

/// [`prune_links`] over the top-level graph and every custom node. Returns how many links went.
pub fn prune_all(graph: &mut Graph, defs: &mut [NodeDef]) -> usize {
    let snapshot = defs.to_vec();
    let mut n = prune_links(graph, &snapshot, None);
    for def in defs.iter_mut() {
        let owner = def.clone();
        n += prune_links(&mut def.graph, &snapshot, Some(&owner));
    }
    n
}

/// Arrange a graph left to right by how far each node is from the inputs (for graphs made
/// without positions).
pub fn layout(graph: &mut Graph) {
    let mut depth: HashMap<NodeId, usize> = graph.nodes.iter().map(|n| (n.id, 0)).collect();
    for _ in 0..graph.nodes.len() {
        let mut changed = false;
        for l in &graph.links {
            let d = depth[&l.from] + 1;
            if depth[&l.to] < d {
                depth.insert(l.to, d);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut rows: HashMap<usize, f32> = HashMap::new();
    for n in &mut graph.nodes {
        let d = depth[&n.id];
        let row = rows.entry(d).or_insert(0.0);
        n.pos = [d as f32 * super::CHAIN_SPACING, *row];
        *row += 160.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::graph::StepField;
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
        remove(&mut g, &[super::super::chain_step_id(1)].into());
        let (_, steps) = g.as_chain().unwrap();
        assert_eq!(steps, vec![Step::new_ring(), Step::new_bend()]);
        let out = g.output().unwrap();
        remove(&mut g, &[out].into());
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
