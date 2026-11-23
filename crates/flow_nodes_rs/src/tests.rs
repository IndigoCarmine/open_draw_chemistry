//! The crate's machinery on a small kind of node that knows nothing of any application:
//! numbers and lists of numbers.

use std::cell::Cell;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::def::{self, GroupRef, NodeDef, Port};
use crate::eval::{self, EvalCx, Evaluate, Problem};
use crate::graph::{Graph, find_cycle};
use crate::id::NodeId;
use crate::ops::{self, ConnectError, GroupError};
use crate::schema::{self, InDecl, NodeKind, OutDecl, Role, SocketType, ValueHint};
use crate::scope;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Ty {
    Num,
    /// The primary data: what bypassing passes on.
    List,
}

impl SocketType for Ty {
    fn feeds(self, to: Ty) -> bool {
        self == to || (self == Ty::Num && to == Ty::List)
    }
    fn has_value(self) -> bool {
        self == Ty::Num
    }
    fn is_primary(self) -> bool {
        self == Ty::List
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum In {
    A,
    B,
    List,
    Port(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Out {
    Value,
    List,
    Port(u32),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Kind {
    Num(f64),
    Add {
        a: f64,
        b: f64,
    },
    /// `0, 1, …, n − 1`.
    Range {
        n: f64,
    },
    /// Each element times `k`.
    Scale {
        k: f64,
    },
    /// Always fails.
    Broken,
    Sum,
    Group(GroupRef<f64>),
    GroupInput,
    GroupOutput,
    Output,
}

impl NodeKind for Kind {
    type In = In;
    type Out = Out;
    type Socket = Ty;
    type Value = f64;

    fn role(&self) -> Role<'_, f64> {
        match self {
            Kind::Group(g) => Role::Group(g),
            Kind::GroupInput => Role::GroupInput,
            Kind::GroupOutput => Role::GroupOutput,
            Kind::Output => Role::Output,
            _ => Role::Plain,
        }
    }
    fn group(r: GroupRef<f64>) -> Self {
        Kind::Group(r)
    }
    fn group_input() -> Self {
        Kind::GroupInput
    }
    fn group_output() -> Self {
        Kind::GroupOutput
    }
    fn port_in(key: u32) -> In {
        In::Port(key)
    }
    fn as_port_in(key: In) -> Option<u32> {
        match key {
            In::Port(k) => Some(k),
            _ => None,
        }
    }
    fn port_out(key: u32) -> Out {
        Out::Port(key)
    }
    fn as_port_out(key: Out) -> Option<u32> {
        match key {
            Out::Port(k) => Some(k),
            _ => None,
        }
    }
    fn own_inputs(&self) -> Vec<InDecl<In, Ty>> {
        match self {
            Kind::Add { .. } => vec![
                InDecl::new(In::A, Ty::Num, "a"),
                InDecl::new(In::B, Ty::Num, "b"),
            ],
            Kind::Range { .. } => vec![InDecl::new(In::A, Ty::Num, "n").hint(ValueHint::Count)],
            Kind::Scale { .. } | Kind::Broken => vec![
                InDecl::new(In::List, Ty::List, "list"),
                InDecl::new(In::A, Ty::Num, "k"),
            ],
            Kind::Sum | Kind::Output => vec![InDecl::new(In::List, Ty::List, "list")],
            _ => Vec::new(),
        }
    }
    fn own_outputs(&self) -> Vec<OutDecl<Out, Ty>> {
        match self {
            Kind::Num(_) | Kind::Add { .. } | Kind::Sum => {
                vec![OutDecl::new(Out::Value, Ty::Num, "value")]
            }
            Kind::Range { .. } | Kind::Scale { .. } | Kind::Broken => {
                vec![OutDecl::new(Out::List, Ty::List, "list")]
            }
            _ => Vec::new(),
        }
    }
    fn inline(&self, key: In) -> Option<f64> {
        match (self, key) {
            (Kind::Add { a, .. }, In::A) => Some(*a),
            (Kind::Add { b, .. }, In::B) => Some(*b),
            (Kind::Range { n }, In::A) => Some(*n),
            (Kind::Scale { k }, In::A) => Some(*k),
            (Kind::Group(g), In::Port(p)) => g.inputs.get(&p).copied(),
            _ => None,
        }
    }
    fn set_inline(&mut self, key: In, value: f64) -> bool {
        let slot = match (self, key) {
            (Kind::Add { a, .. }, In::A) => a,
            (Kind::Add { b, .. }, In::B) => b,
            (Kind::Range { n }, In::A) => n,
            (Kind::Scale { k }, In::A) => k,
            (Kind::Group(g), In::Port(p)) => {
                g.inputs.insert(p, value);
                return true;
            }
            _ => return false,
        };
        *slot = value;
        true
    }
    fn top_level_only(&self) -> bool {
        matches!(self, Kind::Range { .. })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Data {
    Num(f64),
    List(Vec<f64>),
}

/// Counts the nodes computed.
#[derive(Default)]
pub struct Env {
    computed: Cell<usize>,
}

impl Evaluate<Env> for Kind {
    type Data = Data;
    type Error = String;

    fn compute(&self, cx: &mut EvalCx<'_, '_, Self, Env>) -> Result<Vec<(Out, Data)>, String> {
        cx.env().computed.set(cx.env().computed.get() + 1);
        let num = |cx: &mut EvalCx<'_, '_, Self, Env>, key: In| match cx.input(key) {
            Some(Data::Num(v)) => v,
            _ => self.inline(key).unwrap_or(0.0),
        };
        Ok(match self {
            Kind::Num(v) => vec![(Out::Value, Data::Num(*v))],
            Kind::Add { .. } => {
                let v = num(cx, In::A) + num(cx, In::B);
                vec![(Out::Value, Data::Num(v))]
            }
            Kind::Range { .. } => {
                let n = num(cx, In::A);
                if n > 1000.0 {
                    cx.warn("long".into());
                }
                vec![(
                    Out::List,
                    Data::List((0..n as usize).map(|i| i as f64).collect()),
                )]
            }
            Kind::Scale { .. } => {
                let k = num(cx, In::A);
                let list = match cx.input(In::List) {
                    Some(Data::List(l)) => l,
                    _ => Vec::new(),
                };
                vec![(Out::List, Data::List(list.iter().map(|v| v * k).collect()))]
            }
            Kind::Broken => return Err("broken".into()),
            Kind::Sum => {
                let list = match cx.input(In::List) {
                    Some(Data::List(l)) => l,
                    _ => Vec::new(),
                };
                vec![(Out::Value, Data::Num(list.iter().sum()))]
            }
            _ => Vec::new(),
        })
    }

    fn default_data(ty: Ty, value: Option<&f64>) -> Data {
        match ty {
            Ty::Num => Data::Num(value.copied().unwrap_or(0.0)),
            Ty::List => Data::List(Vec::new()),
        }
    }

    fn conform(data: Data, ty: Ty) -> Option<Data> {
        match (data, ty) {
            (d @ Data::Num(_), Ty::Num) | (d @ Data::List(_), Ty::List) => Some(d),
            (Data::Num(v), Ty::List) => Some(Data::List(vec![v])),
            _ => None,
        }
    }
}

type G = Graph<Kind>;

/// range(n = 4) → scale(k = 2) → output.
fn chain() -> (G, NodeId, NodeId, NodeId) {
    let mut g = G::default();
    let range = g.add(Kind::Range { n: 4.0 }, [0.0; 2]);
    let scale = g.add(Kind::Scale { k: 2.0 }, [200.0, 0.0]);
    let out = g.add(Kind::Output, [400.0, 0.0]);
    ops::connect(&mut g, &[], None, range, Out::List, scale, In::List).unwrap();
    ops::connect(&mut g, &[], None, scale, Out::List, out, In::List).unwrap();
    (g, range, scale, out)
}

fn result(g: &G, defs: &[NodeDef<Kind>]) -> Vec<f64> {
    match eval::evaluate(g, defs, &Env::default()).output.as_slice() {
        [(In::List, Data::List(l))] => l.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn connecting_checks_types_and_loops() {
    let (mut g, range, scale, _) = chain();
    let n = g.add(Kind::Num(3.0), [0.0; 2]);
    ops::connect(&mut g, &[], None, n, Out::Value, scale, In::A).unwrap();
    let sum = g.add(Kind::Sum, [0.0; 2]);
    assert_eq!(
        ops::connect(&mut g, &[], None, sum, Out::Value, scale, In::A),
        Ok(()),
        "replaces the link"
    );
    assert_eq!(g.links.iter().filter(|l| l.to == scale).count(), 2);
    assert_eq!(
        ops::connect(&mut g, &[], None, range, Out::List, n, In::A),
        Err(ConnectError::Missing)
    );
    assert_eq!(
        ops::connect(&mut g, &[], None, scale, Out::List, range, In::A),
        Err(ConnectError::Type)
    );
    assert_eq!(
        ops::connect(&mut g, &[], None, scale, Out::List, sum, In::List),
        Err(ConnectError::Cycle)
    );
    g.validate(&[], None).unwrap();
    assert!(find_cycle(&g).is_none());
}

#[test]
fn evaluation_pulls_once_and_passes_through() {
    let (mut g, range, scale, out) = chain();
    assert_eq!(result(&g, &[]), vec![0.0, 2.0, 4.0, 6.0]);
    // A node fed to two others is computed once.
    let sum = g.add(Kind::Sum, [0.0; 2]);
    ops::connect(&mut g, &[], None, range, Out::List, sum, In::List).unwrap();
    ops::connect(&mut g, &[], None, sum, Out::Value, scale, In::A).unwrap();
    let env = Env::default();
    let outcome = eval::evaluate(&g, &[], &env);
    assert_eq!(env.computed.get(), 3);
    assert!(outcome.reached.contains(&(vec![], out)));
    assert_eq!(
        outcome.values[&(vec![], sum)].as_slice(),
        &[(Out::Value, Data::Num(6.0))]
    );
    // Bypassed: the list goes through; nodes no longer needed are not reached.
    g.node_mut(scale).unwrap().bypass = true;
    let outcome = eval::evaluate(&g, &[], &Env::default());
    assert!(!outcome.reached.contains(&(vec![], sum)));
    assert_eq!(result(&g, &[]), vec![0.0, 1.0, 2.0, 3.0]);
    // A failing node is reported and passes through too.
    g.node_mut(scale).unwrap().kind = Kind::Broken;
    g.node_mut(scale).unwrap().bypass = false;
    let outcome = eval::evaluate(&g, &[], &Env::default());
    assert_eq!(outcome.issues.len(), 1);
    assert_eq!(outcome.issues[0].problem, Problem::Node("broken".into()));
    assert_eq!(result(&g, &[]), vec![0.0, 1.0, 2.0, 3.0]);
    // Warnings do not stop a node.
    g.node_mut(range).unwrap().kind = Kind::Range { n: 2000.0 };
    let outcome = eval::evaluate(&g, &[], &Env::default());
    assert!(outcome.issues.iter().any(|i| i.warning && i.node == range));
}

#[test]
fn a_loop_is_reported_where_it_closes() {
    let (mut g, _, scale, _) = chain();
    let a = g.add(Kind::Add { a: 1.0, b: 0.0 }, [0.0; 2]);
    let b = g.add(Kind::Add { a: 1.0, b: 0.0 }, [0.0; 2]);
    // Not through `connect`, which refuses it: a damaged graph.
    g.link(a, Out::Value, b, In::A);
    g.link(b, Out::Value, a, In::A);
    g.link(a, Out::Value, scale, In::A);
    assert!(g.validate(&[], None).is_err());
    let outcome = eval::evaluate(&g, &[], &Env::default());
    assert!(outcome.issues.iter().any(|i| i.problem == Problem::Cycle));
}

#[test]
fn removing_a_node_joins_around_it() {
    let (mut g, range, scale, out) = chain();
    ops::remove(&mut g, &[], None, &[scale].into());
    assert_eq!(g.link_into(out, In::List).unwrap().from, range);
    ops::remove(&mut g, &[], None, &[out].into());
    assert!(g.output().is_some(), "the output node stays");
}

#[test]
fn duplicating_copies_inner_links() {
    let (mut g, range, scale, _) = chain();
    let copies = ops::duplicate(&mut g, &[range, scale].into(), [0.0, 100.0]);
    assert_eq!(copies.len(), 2);
    assert!(
        g.links
            .iter()
            .any(|l| l.from == copies[0] && l.to == copies[1])
    );
    g.validate(&[], None).unwrap();
}

#[test]
fn grouping_and_ungrouping_keep_the_result() {
    let (mut g, _, scale, _) = chain();
    let before = result(&g, &[]);
    let mut defs = Vec::new();
    let (id, node) = ops::group(&mut g, &mut defs, None, &[scale].into(), "double").unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].inputs.len(), 1);
    assert_eq!(defs[0].outputs.len(), 1);
    def::validate(&defs).unwrap();
    g.validate(&defs, None).unwrap();
    assert_eq!(result(&g, &defs), before);
    // Expose the factor: a port with the value as its default.
    let inner_scale = defs[0]
        .graph
        .nodes
        .iter()
        .find(|n| matches!(n.kind, Kind::Scale { .. }))
        .unwrap()
        .id;
    let all = defs.clone();
    let port = ops::expose(&mut defs[0], &all, inner_scale, In::A).unwrap();
    assert_eq!(defs[0].inputs[1].default, Some(2.0));
    assert_eq!(result(&g, &defs), before);
    // The group node's own value wins over the default.
    g.node_mut(node)
        .unwrap()
        .kind
        .set_inline(In::Port(port), 10.0);
    assert_eq!(result(&g, &defs), vec![0.0, 10.0, 20.0, 30.0]);
    // Ungrouping puts the value back into the node.
    let added = ops::ungroup(&mut g, &defs, node).unwrap();
    assert_eq!(added.len(), 1);
    assert_eq!(result(&g, &defs), vec![0.0, 10.0, 20.0, 30.0]);
    assert!(def::find(&defs, &id).is_some());
    // Nothing groupable: the output node alone.
    let out = g.output().unwrap();
    assert_eq!(
        ops::group(&mut g, &mut defs, None, &[out].into(), "x"),
        Err(GroupError::Empty)
    );
}

#[test]
fn custom_nodes_cannot_hold_themselves() {
    let (mut g, _, scale, _) = chain();
    let mut defs = Vec::new();
    let (id, node) = ops::group(&mut g, &mut defs, None, &[scale].into(), "double").unwrap();
    // Grouping the custom node's own use inside itself would loop.
    let inner = defs[0].graph.add(
        Kind::Group(GroupRef {
            def: id.clone(),
            inputs: BTreeMap::new(),
        }),
        [0.0; 2],
    );
    assert!(def::validate(&defs).is_err());
    let outcome = eval::evaluate(&g, &defs, &Env::default());
    let _ = (node, inner);
    // The inner use is not wired to anything, so it is never reached.
    assert!(outcome.issues.is_empty());
    let gin = defs[0].graph.group_input().unwrap();
    defs[0].graph.link(gin, Out::Port(0), inner, In::Port(0));
    let gout = defs[0].graph.group_output().unwrap();
    defs[0].graph.links.retain(|l| l.to != gout);
    defs[0].graph.link(inner, Out::Port(1), gout, In::Port(1));
    let outcome = eval::evaluate(&g, &defs, &Env::default());
    assert!(
        outcome
            .issues
            .iter()
            .any(|i| matches!(i.problem, Problem::Recursion(_)))
    );
    // Grouping inside the custom node, a node using it, is refused.
    let mut defs2 = defs.clone();
    let r = scope::edit_def(&mut defs2, &id, |graph, defs, owner| {
        ops::group(graph, defs, owner, &[inner].into(), "loop")
    })
    .unwrap();
    assert_eq!(r, Err(GroupError::Recursion));
}

#[test]
fn scopes_lead_into_custom_nodes() {
    let (mut g, _, scale, _) = chain();
    let mut defs = Vec::new();
    let (id, node) = ops::group(&mut g, &mut defs, None, &[scale].into(), "double").unwrap();
    let (inner, owner) = scope::graph_at(&g, &defs, &[node]).unwrap();
    assert_eq!(owner.unwrap().id, id);
    assert!(inner.group_output().is_some());
    assert!(scope::graph_at(&g, &defs, &[NodeId(999)]).is_none());
    assert!(scope::resolve(&g, &defs, &[]).unwrap().is_none());
    // Bypass inside: the result follows.
    let scale_inside = inner
        .nodes
        .iter()
        .find(|n| matches!(n.kind, Kind::Scale { .. }))
        .unwrap()
        .id;
    scope::edit_def(&mut defs, &id, |graph, _, owner| {
        assert!(owner.is_some());
        graph.node_mut(scale_inside).unwrap().bypass = true;
    })
    .unwrap();
    assert_eq!(result(&g, &defs), vec![0.0, 1.0, 2.0, 3.0]);
    // A top-level-only node inside a custom node is refused.
    defs[0].graph.add(Kind::Range { n: 1.0 }, [0.0; 2]);
    assert!(def::validate(&defs).is_err());
}

#[test]
fn ports_conform_and_prune() {
    let def = NodeDef::<Kind>::passing("id", Ty::List, "list", crate::SPACING);
    assert_eq!(def.graph.nodes.len(), 2);
    def::validate(std::slice::from_ref(&def)).unwrap();
    let mut g = G::default();
    let n = g.add(Kind::Num(5.0), [0.0; 2]);
    let use_ = g.add(Kind::Group(GroupRef::new(def.id.clone())), [0.0; 2]);
    let out = g.add(Kind::Output, [0.0; 2]);
    let defs = vec![def.clone()];
    // A number into a list port becomes a list of one.
    ops::connect(&mut g, &defs, None, n, Out::Value, use_, In::Port(0)).unwrap();
    ops::connect(&mut g, &defs, None, use_, Out::Port(0), out, In::List).unwrap();
    assert_eq!(result(&g, &defs), vec![5.0]);
    // The port becomes a number: the link out of it to a list still fits, the one into the
    // output too; a port gone drops its links.
    let mut defs = defs;
    defs[0].outputs.clear();
    assert_eq!(
        ops::prune_all(&mut g, &mut defs),
        2,
        "the output port's link inside too"
    );
    assert_eq!(g.links.len(), 1);
}

#[test]
fn graphs_save_as_plain_data() {
    let (g, ..) = chain();
    let text = serde_json::to_string(&g).unwrap();
    assert!(text.contains(r#""kind":{"Range":{"n":4.0}}"#), "{text}");
    assert!(!text.contains("bypass"), "defaults are left out");
    let back: G = serde_json::from_str(&text).unwrap();
    assert_eq!(back, g);
    let ports = vec![Port {
        key: 0,
        name: "k".into(),
        ty: Ty::Num,
        default: Some(1.0),
    }];
    let text = serde_json::to_string(&ports).unwrap();
    assert_eq!(text, r#"[{"key":0,"name":"k","ty":"Num","default":1.0}]"#);
    assert_eq!(schema::inputs(&Kind::Sum, &[], None)[0].label, "list");
}

#[test]
fn layout_puts_nodes_in_columns() {
    let (mut g, range, scale, out) = chain();
    ops::layout(&mut g);
    let x = |id| g.node(id).unwrap().pos[0];
    assert!(x(range) < x(scale) && x(scale) < x(out));
}

#[cfg(feature = "library")]
mod library {
    use super::*;
    use crate::def::Status;
    use crate::library::{self, LibraryFormat, NodeLibrary};

    const FORMAT: LibraryFormat = LibraryFormat {
        tag: "test-node",
        version: 1,
    };

    #[test]
    fn the_hash_ignores_layout_and_names() {
        let a = NodeDef::<Kind>::passing("a", Ty::List, "list", 100.0);
        let mut b = a.clone();
        b.name = "renamed".into();
        b.graph.nodes[0].pos = [50.0, 50.0];
        b.revision = 3;
        assert_eq!(def::content_hash(&a), def::content_hash(&b));
        b.inputs[0].name = "in".into();
        assert_ne!(def::content_hash(&a), def::content_hash(&b));
    }

    #[test]
    fn saving_listing_and_statuses() {
        let dir = tempfile::tempdir().unwrap();
        let lib = NodeLibrary::at(dir.path(), FORMAT);
        assert!(lib.list::<Kind>().is_empty(), "no folder yet");
        let def = NodeDef::<Kind>::passing("Doubler (2)", Ty::List, "list", 100.0);
        let stamp = lib.save(&def, &[]).unwrap();
        assert_eq!(stamp.revision, 1);
        let listed = lib.list::<Kind>();
        assert_eq!(listed.len(), 1);
        let mut copies = Vec::new();
        library::adopt(&mut copies, &listed[0], false);
        let status = |copies: &[NodeDef<Kind>]| {
            def::status(&copies[0], lib.get::<Kind>(&def.id).map(|e| e.def).as_ref())
        };
        assert_eq!(status(&copies), Status::InSync);
        let mut changed = def.clone();
        changed.inputs[0].name = "in".into();
        lib.save(&changed, &[]).unwrap();
        assert_eq!(status(&copies), Status::UpdateAvailable);
        let now = lib.get::<Kind>(&def.id).unwrap();
        assert_eq!(now.def.revision, 2);
        library::adopt(&mut copies, &now, true);
        assert_eq!(status(&copies), Status::InSync);
        assert_eq!(lib.list::<Kind>().len(), 1, "saved over the same file");
        std::fs::write(dir.path().join("broken.json"), "{").unwrap();
        assert_eq!(lib.list::<Kind>().len(), 1, "a damaged file is skipped");
        lib.remove::<Kind>(&def.id).unwrap();
        assert!(lib.list::<Kind>().is_empty());
    }

    #[test]
    fn files_of_another_format_or_newer_are_refused() {
        let def = NodeDef::<Kind>::passing("x", Ty::List, "list", 100.0);
        let text = library::to_string(&FORMAT, &def, &[]);
        assert!(library::from_str::<Kind>(&FORMAT, &text).is_ok());
        let other = LibraryFormat {
            tag: "other",
            version: 1,
        };
        assert!(matches!(
            library::from_str::<Kind>(&other, &text),
            Err(library::LibraryError::Format(_))
        ));
        let old = LibraryFormat {
            tag: "test-node",
            version: 0,
        };
        assert!(matches!(
            library::from_str::<Kind>(&old, &text),
            Err(library::LibraryError::Newer { .. })
        ));
        let no_dir = NodeLibrary::new(None, FORMAT);
        assert_eq!(no_dir.save(&def, &[]), Err(library::LibraryError::NoDir));
    }
}
