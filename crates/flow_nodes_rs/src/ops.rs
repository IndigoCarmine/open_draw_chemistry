//! The edits a node editor makes, as plain functions on a graph (and the custom nodes), so
//! each is tested without a window. Errors are plain values: the application words them.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::SPACING;
use crate::def::{self, DefId, GroupRef, NodeDef, Port};
use crate::graph::{Graph, Link, LinkOf, Node};
pub use crate::graph::{find_cycle, reaches};
use crate::id::NodeId;
use crate::schema::{self, NodeKind, Role, SocketType};

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
            ConnectError::Missing => "no such socket",
            ConnectError::Type => "these sockets do not fit",
            ConnectError::Cycle => "the link would loop back",
        })
    }
}

impl std::error::Error for ConnectError {}

/// The type of output `out` of node `from`.
fn out_type<K: NodeKind>(
    graph: &Graph<K>,
    defs: &[NodeDef<K>],
    owner: Option<&NodeDef<K>>,
    from: NodeId,
    out: K::Out,
) -> Option<K::Socket> {
    let n = graph.node(from)?;
    schema::outputs(&n.kind, defs, owner)
        .into_iter()
        .find(|o| o.key == out)
        .map(|o| o.ty)
}

/// The type of input `input` of node `to`.
fn in_type<K: NodeKind>(
    graph: &Graph<K>,
    defs: &[NodeDef<K>],
    owner: Option<&NodeDef<K>>,
    to: NodeId,
    input: K::In,
) -> Option<K::Socket> {
    let n = graph.node(to)?;
    schema::inputs(&n.kind, defs, owner)
        .into_iter()
        .find(|i| i.key == input)
        .map(|i| i.ty)
}

/// Link output `out` of `from` to input `input` of `to`, replacing what fed that input.
pub fn connect<K: NodeKind>(
    graph: &mut Graph<K>,
    defs: &[NodeDef<K>],
    owner: Option<&NodeDef<K>>,
    from: NodeId,
    out: K::Out,
    to: NodeId,
    input: K::In,
) -> Result<(), ConnectError> {
    if graph.node(from).is_none() || graph.node(to).is_none() {
        return Err(ConnectError::Missing);
    }
    let out_ty = out_type(graph, defs, owner, from, out).ok_or(ConnectError::Missing)?;
    let in_ty = in_type(graph, defs, owner, to, input).ok_or(ConnectError::Missing)?;
    if !out_ty.feeds(in_ty) {
        return Err(ConnectError::Type);
    }
    if from == to || reaches(graph, to, from) {
        return Err(ConnectError::Cycle);
    }
    graph.links.retain(|l| !(l.to == to && l.input == input));
    graph.link(from, out, to, input);
    Ok(())
}

/// Remove the link into input `input` of `to`. Returns it.
pub fn disconnect<K: NodeKind>(
    graph: &mut Graph<K>,
    to: NodeId,
    input: K::In,
) -> Option<LinkOf<K>> {
    let k = graph
        .links
        .iter()
        .position(|l| l.to == to && l.input == input)?;
    Some(graph.links.remove(k))
}

/// Whether the user may delete the node (not the output node).
pub fn removable<K: NodeKind>(node: &Node<K>) -> bool {
    !matches!(node.kind.role(), Role::Output)
}

/// The socket a node passes its primary data on through, both ends (see [`schema::through`]).
fn through_of<K: NodeKind>(
    graph: &Graph<K>,
    defs: &[NodeDef<K>],
    owner: Option<&NodeDef<K>>,
    id: NodeId,
) -> Option<(K::In, K::Out)> {
    let n = graph.node(id)?;
    schema::through(
        &schema::inputs(&n.kind, defs, owner),
        &schema::outputs(&n.kind, defs, owner),
    )
}

/// Remove nodes and their links. A node removed between what feeds its primary input and what
/// its primary output feeds ([`schema::through`]) is joined around, so taking a step out of a
/// chain keeps the chain. `owner` is the custom node the graph belongs to.
pub fn remove<K: NodeKind>(
    graph: &mut Graph<K>,
    defs: &[NodeDef<K>],
    owner: Option<&NodeDef<K>>,
    ids: &BTreeSet<NodeId>,
) {
    let ids: BTreeSet<NodeId> = ids
        .iter()
        .copied()
        .filter(|id| graph.node(*id).is_some_and(removable))
        .collect();
    let mut splices = Vec::new();
    for &id in &ids {
        let Some((input, output)) = through_of(graph, defs, owner, id) else {
            continue;
        };
        let Some(mut feed) = graph.link_into(id, input).copied() else {
            continue;
        };
        // Through a run of removed nodes.
        let mut guard = 0;
        while ids.contains(&feed.from) && guard < ids.len() {
            let next = through_of(graph, defs, owner, feed.from)
                .and_then(|(i, _)| graph.link_into(feed.from, i).copied());
            match next {
                Some(l) => feed = l,
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
            .filter(|l| l.from == id && l.out == output)
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
/// ids, in the order the nodes are in the graph. Output nodes are not copied.
pub fn duplicate<K: NodeKind>(
    graph: &mut Graph<K>,
    ids: &BTreeSet<NodeId>,
    offset: [f32; 2],
) -> Vec<NodeId> {
    let mut map = HashMap::new();
    let mut out = Vec::new();
    let originals: Vec<Node<K>> = graph
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
    let inner: Vec<LinkOf<K>> = graph
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
            GroupError::Empty => "nothing that can go into a custom node is selected",
            GroupError::Recursion => "the custom node would use itself",
        })
    }
}

impl std::error::Error for GroupError {}

/// Whether a node can go into a custom node: custom nodes and plain nodes that allow it
/// ([`NodeKind::groupable`]), not the output nor the group input and output nodes.
pub fn groupable<K: NodeKind>(node: &Node<K>) -> bool {
    match node.kind.role() {
        Role::Plain => node.kind.groupable(),
        Role::Group(_) => true,
        Role::Output | Role::GroupInput | Role::GroupOutput => false,
    }
}

/// Gather nodes `ids` of `graph` (the graph of custom node `owner`, or the top level) into a
/// new custom node named `name`, added to `defs`, and put one node using it in their place.
/// Each output outside the selection feeding nodes inside becomes an input of the custom node,
/// each output inside feeding nodes outside one of its outputs. Returns the new definition's
/// id and the node using it. What the graph makes does not change.
pub fn group<K: NodeKind>(
    graph: &mut Graph<K>,
    defs: &mut Vec<NodeDef<K>>,
    owner: Option<&DefId>,
    ids: &BTreeSet<NodeId>,
    name: &str,
) -> Result<(DefId, NodeId), GroupError> {
    let owner_def = owner.and_then(|o| def::find(defs, o)).cloned();
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
            .any(|id| match graph.node(*id).unwrap().kind.role() {
                Role::Group(g) => def::depends_on(defs, &g.def, owner),
                _ => false,
            });
        if uses_owner {
            return Err(GroupError::Recursion);
        }
    }
    let owner_ref = owner_def.as_ref();
    let out_decl = |graph: &Graph<K>, from: NodeId, out: K::Out| {
        graph.node(from).and_then(|n| {
            schema::outputs(&n.kind, defs, owner_ref)
                .into_iter()
                .find(|o| o.key == out)
        })
    };
    let in_label = |graph: &Graph<K>, to: NodeId, input: K::In| -> (String, Option<K::Value>) {
        graph
            .node(to)
            .map(|n| {
                let label = schema::inputs(&n.kind, defs, owner_ref)
                    .into_iter()
                    .find(|i| i.key == input)
                    .map_or_else(String::new, |i| i.label);
                (label, n.kind.inline(input))
            })
            .unwrap_or_default()
    };

    let mut def: NodeDef<K> = NodeDef::empty(name);
    // Inputs: each distinct outside source feeding inside.
    let mut in_ports: Vec<((NodeId, K::Out), u32)> = Vec::new();
    let mut out_ports: Vec<((NodeId, K::Out), u32)> = Vec::new();
    let mut key = 0u32;
    for l in graph
        .links
        .iter()
        .filter(|l| !chosen.contains(&l.from) && chosen.contains(&l.to))
    {
        if in_ports.iter().any(|(s, _)| *s == (l.from, l.out)) {
            continue;
        }
        let Some(decl) = out_decl(graph, l.from, l.out) else {
            continue;
        };
        let (label, inline) = in_label(graph, l.to, l.input);
        def.inputs.push(Port {
            key,
            name: if label.is_empty() {
                format!("in {key}")
            } else {
                label
            },
            ty: decl.ty,
            default: if decl.ty.has_value() { inline } else { None },
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
        let Some(decl) = out_decl(graph, l.from, l.out) else {
            continue;
        };
        def.outputs.push(Port {
            key,
            name: if decl.label.is_empty() {
                format!("out {key}")
            } else {
                decl.label
            },
            ty: decl.ty,
            default: None,
        });
        out_ports.push(((l.from, l.out), key));
        key += 1;
    }
    let in_port = |s: (NodeId, K::Out)| in_ports.iter().find(|(p, _)| *p == s).map(|(_, k)| *k);
    let out_port = |s: (NodeId, K::Out)| out_ports.iter().find(|(p, _)| *p == s).map(|(_, k)| *k);

    // Lay out: the inside keeps its arrangement; group input left, output right.
    let inside: Vec<Node<K>> = graph
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
    let gin = def.graph.add(K::group_input(), [min_x - SPACING, mid_y]);
    let gout = def.graph.add(K::group_output(), [max_x + SPACING, mid_y]);
    for l in &graph.links {
        match (chosen.contains(&l.from), chosen.contains(&l.to)) {
            (true, true) => def.graph.link(map[&l.from], l.out, map[&l.to], l.input),
            (false, true) => {
                if let Some(port) = in_port((l.from, l.out)) {
                    def.graph.link(gin, K::port_out(port), map[&l.to], l.input);
                }
            }
            (true, false) => {
                if let Some(port) = out_port((l.from, l.out))
                    && def.graph.link_into(gout, K::port_in(port)).is_none()
                {
                    def.graph.link(map[&l.from], l.out, gout, K::port_in(port));
                }
            }
            (false, false) => {}
        }
    }

    // Replace the nodes by one using the definition.
    let def_id = def.id.clone();
    let inline: BTreeMap<u32, K::Value> = def
        .inputs
        .iter()
        .filter_map(|p| Some((p.key, p.default.clone()?)))
        .collect();
    // The links staying outside, and those out of the selection, re-made from the new node
    // (a placeholder until its id is known).
    let placeholder = NodeId(u64::MAX);
    let new_links: Vec<LinkOf<K>> = graph
        .links
        .iter()
        .filter_map(
            |l| match (chosen.contains(&l.from), chosen.contains(&l.to)) {
                (true, false) => out_port((l.from, l.out)).map(|port| Link {
                    from: placeholder,
                    out: K::port_out(port),
                    to: l.to,
                    input: l.input,
                }),
                (false, false) => Some(*l),
                _ => None,
            },
        )
        .collect();
    graph.nodes.retain(|n| !chosen.contains(&n.id));
    let node = graph.add(
        K::group(GroupRef {
            def: def_id.clone(),
            inputs: inline,
        }),
        centre,
    );
    graph.links = new_links
        .into_iter()
        .map(|mut l| {
            if l.from == placeholder {
                l.from = node;
            }
            l
        })
        .collect();
    for ((from, out), port) in in_ports {
        graph.link(from, out, node, K::port_in(port));
    }
    defs.push(def);
    Ok((def_id, node))
}

/// Put the nodes of custom node `node` (a node of `graph`) back in its place: the inverse of
/// [`group`]. Returns the ids the inner nodes got.
pub fn ungroup<K: NodeKind>(
    graph: &mut Graph<K>,
    defs: &[NodeDef<K>],
    node: NodeId,
) -> Option<Vec<NodeId>> {
    let Role::Group(g) = graph.node(node)?.kind.role() else {
        return None;
    };
    let inline = g.inputs.clone();
    let def = def::find(defs, &g.def)?.clone();
    let at = graph.node(node)?.pos;
    let gin: HashSet<NodeId> = def
        .graph
        .nodes
        .iter()
        .filter(|n| matches!(n.kind.role(), Role::GroupInput))
        .map(|n| n.id)
        .collect();
    let gout = def.graph.group_output();
    let inner: Vec<&Node<K>> = def
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
    let feeds: HashMap<u32, (NodeId, K::Out)> = graph
        .links
        .iter()
        .filter(|l| l.to == node)
        .filter_map(|l| Some((K::as_port_in(l.input)?, (l.from, l.out))))
        .collect();
    let fed: Vec<(u32, NodeId, K::In)> = graph
        .links
        .iter()
        .filter(|l| l.from == node)
        .filter_map(|l| Some((K::as_port_out(l.out)?, l.to, l.input)))
        .collect();
    graph.links.retain(|l| l.from != node && l.to != node);
    graph.nodes.retain(|n| n.id != node);
    for l in &def.graph.links {
        let to_gout = Some(l.to) == gout;
        match (gin.contains(&l.from), to_gout) {
            (false, false) => {
                if let (Some(&f), Some(&t)) = (map.get(&l.from), map.get(&l.to)) {
                    graph.link(f, l.out, t, l.input);
                }
            }
            (true, false) => {
                let Some(p) = K::as_port_out(l.out) else {
                    continue;
                };
                let Some(&t) = map.get(&l.to) else { continue };
                match feeds.get(&p) {
                    Some(&(from, out)) => graph.link(from, out, t, l.input),
                    None => {
                        // Unfed: the port's value becomes the input's.
                        let value = inline
                            .get(&p)
                            .cloned()
                            .or_else(|| def.inputs.iter().find(|q| q.key == p)?.default.clone());
                        if let (Some(v), Some(n)) = (value, graph.node_mut(t)) {
                            n.kind.set_inline(l.input, v);
                        }
                    }
                }
            }
            (false, true) => {
                let Some(p) = K::as_port_in(l.input) else {
                    continue;
                };
                let Some(&f) = map.get(&l.from) else { continue };
                for &(q, to, input) in &fed {
                    if q == p {
                        graph.link(f, l.out, to, input);
                    }
                }
            }
            (true, true) => {
                // An input wired straight to an output.
                let (Some(p), Some(q)) = (K::as_port_out(l.out), K::as_port_in(l.input)) else {
                    continue;
                };
                if let Some(&(from, out)) = feeds.get(&p) {
                    for &(r, to, input) in &fed {
                        if r == q {
                            graph.link(from, out, to, input);
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
pub fn expose<K: NodeKind>(
    def: &mut NodeDef<K>,
    defs: &[NodeDef<K>],
    node: NodeId,
    input: K::In,
) -> Option<u32> {
    let owner = def.clone();
    let n = def.graph.node(node)?;
    let decl = schema::inputs(&n.kind, defs, Some(&owner))
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
        default: if decl.ty.has_value() { value } else { None },
    });
    let gin = match def.graph.group_input() {
        Some(id) => id,
        None => def.graph.add(K::group_input(), [pos[0] - SPACING, pos[1]]),
    };
    def.graph.link(gin, K::port_out(key), node, input);
    Some(key)
}

/// Drop the links of `graph` that no longer fit the sockets (after a custom node's ports
/// changed). Returns how many were dropped.
pub fn prune_links<K: NodeKind>(
    graph: &mut Graph<K>,
    defs: &[NodeDef<K>],
    owner: Option<&NodeDef<K>>,
) -> usize {
    let before = graph.links.len();
    let keep: Vec<bool> = graph
        .links
        .iter()
        .map(|l| {
            let out = out_type(graph, defs, owner, l.from, l.out);
            let input = in_type(graph, defs, owner, l.to, l.input);
            matches!((out, input), (Some(o), Some(i)) if o.feeds(i))
        })
        .collect();
    let mut k = 0;
    graph.links.retain(|_| {
        k += 1;
        keep[k - 1]
    });
    before - graph.links.len()
}

/// [`prune_links`] over the top-level graph and every custom node. Returns how many links went.
pub fn prune_all<K: NodeKind>(graph: &mut Graph<K>, defs: &mut [NodeDef<K>]) -> usize {
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
pub fn layout<K: NodeKind>(graph: &mut Graph<K>) {
    let mut depth: HashMap<NodeId, usize> = graph.nodes.iter().map(|n| (n.id, 0)).collect();
    for _ in 0..graph.nodes.len() {
        let mut changed = false;
        for l in &graph.links {
            let (Some(&from), Some(&to)) = (depth.get(&l.from), depth.get(&l.to)) else {
                continue;
            };
            if to < from + 1 {
                depth.insert(l.to, from + 1);
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
        n.pos = [d as f32 * SPACING, *row];
        *row += 160.0;
    }
}
