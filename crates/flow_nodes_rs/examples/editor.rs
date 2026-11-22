//! The node editor on the calculator's nodes, in a window: add nodes (right-click or Shift+A),
//! wire them, type or drag values, group with Ctrl+G and go inside with Tab. Ctrl+Z undoes.
//!
//! `cargo run -p flow_nodes_rs --example editor --features egui`

#[path = "common/calc.rs"]
mod calc;

use calc::{Calc, In, NoEnv, Num, Out};
use eframe::egui;
use egui::{Color32, pos2};
use flow_nodes_rs::editor::{
    Action, AutoWire, EditorState, EditorStyle, Inline, KeyMap, Labels, LooseEnd, MenuEntry,
    NodeEditor, NodeView, SocketRow, SocketShape, SocketStyle, Value, ValueTarget, Viewer,
    WireView,
};
use flow_nodes_rs::{
    Graph, GroupRef, NodeDef, NodeId, NodeKind, ValueHint, eval, ops, schema, scope,
};

/// The graph shown (the top level, or a custom node's inside), as the editor sees it.
struct Shown<'a> {
    graph: &'a Graph<Calc>,
    defs: &'a [NodeDef<Calc>],
    owner: Option<&'a NodeDef<Calc>>,
    /// What each node gave (the top level only).
    values: &'a std::collections::HashMap<NodeId, f64>,
}

impl Viewer for Shown<'_> {
    type In = In;
    type Out = Out;
    type Socket = Num;
    type Template = Calc;

    fn nodes(&self) -> Vec<NodeView<In, Out, Num>> {
        self.graph
            .nodes
            .iter()
            .map(|n| {
                let title = match &n.kind {
                    Calc::Group(g) => flow_nodes_rs::def::find(self.defs, &g.def)
                        .map_or("?".into(), |d| d.name.clone()),
                    kind => kind.title().into(),
                };
                let header = match n.kind.role() {
                    flow_nodes_rs::Role::Plain => Color32::from_rgb(0x3A, 0x58, 0x86),
                    _ => Color32::from_rgb(0x55, 0x4C, 0x46),
                };
                let mut view = NodeView::new(n.id, pos2(n.pos[0], n.pos[1]), title, header);
                view.bypass = n.bypass;
                view.subtitle = self.values.get(&n.id).map(|v| format!("= {v}"));
                if let Calc::Number(v) = n.kind {
                    view.own = Some(Inline::hinted(Value::Number(v), ValueHint::Plain, ""));
                }
                view.outputs = schema::outputs(&n.kind, self.defs, self.owner)
                    .into_iter()
                    .map(|o| SocketRow::new(o.key, o.ty, o.label))
                    .collect();
                view.inputs = schema::inputs(&n.kind, self.defs, self.owner)
                    .into_iter()
                    .map(|i| {
                        let mut row = SocketRow::new(i.key, i.ty, i.label);
                        if self.graph.link_into(n.id, i.key).is_none() {
                            row.inline = n
                                .kind
                                .inline(i.key)
                                .map(|v| Inline::hinted(Value::Number(v), ValueHint::Plain, ""));
                        }
                        row
                    })
                    .collect();
                view
            })
            .collect()
    }

    fn links(&self) -> Vec<WireView<In, Out>> {
        self.graph
            .links
            .iter()
            .map(|l| WireView {
                from: l.from,
                out: l.out,
                to: l.to,
                input: l.input,
            })
            .collect()
    }

    fn socket_style(&self, _: Num) -> SocketStyle {
        SocketStyle {
            color: Color32::from_rgb(0xA8, 0xA2, 0x9B),
            shape: SocketShape::SmallCircle,
        }
    }

    fn can_connect(&self, _: Num, _: Num) -> bool {
        true
    }

    fn add_menu(&self) -> Vec<MenuEntry<Calc>> {
        let entry = |label: &str, kind| MenuEntry {
            label: label.into(),
            group: "Math".into(),
            template: kind,
        };
        let mut entries = vec![
            entry("Number", Calc::Number(1.0)),
            entry("Add", Calc::Add { a: 0.0, b: 0.0 }),
            entry("Multiply", Calc::Multiply { a: 1.0, b: 1.0 }),
        ];
        for d in self.defs {
            if self
                .owner
                .is_none_or(|o| !flow_nodes_rs::def::depends_on(self.defs, &d.id, &o.id))
            {
                entries.push(MenuEntry {
                    label: d.name.clone(),
                    group: "Custom".into(),
                    template: Calc::Group(GroupRef::new(d.id.clone())),
                });
            }
        }
        entries
    }

    fn auto_wire(&self, kind: &Calc, end: &LooseEnd<In, Out, Num>) -> Option<AutoWire<In, Out>> {
        match *end {
            LooseEnd::Out(_, out, _) => schema::inputs(kind, self.defs, None)
                .first()
                .map(|i| AutoWire::From { out, new_in: i.key }),
            LooseEnd::In(_, input, _) => {
                schema::outputs(kind, self.defs, None)
                    .first()
                    .map(|o| AutoWire::To {
                        input,
                        new_out: o.key,
                    })
            }
        }
    }

    fn enterable(&self, id: NodeId) -> bool {
        matches!(self.graph.node(id).map(|n| &n.kind), Some(Calc::Group(_)))
    }

    fn empty_hint(&self) -> Option<String> {
        Some("Right-click to add a node".into())
    }
}

/// The document: the top-level graph and the custom nodes, with an undo history.
#[derive(Clone)]
struct Doc {
    graph: Graph<Calc>,
    defs: Vec<NodeDef<Calc>>,
}

struct App {
    doc: Doc,
    undo: Vec<Doc>,
    editor: EditorState<In, Out, Num>,
    style: EditorStyle,
    labels: Labels,
    keys: KeyMap,
    /// A drag of a value or of nodes, already in the history.
    dragging: bool,
}

impl App {
    fn new() -> Self {
        let mut graph = Graph::default();
        let a = graph.add(Calc::Number(2.0), [0.0, 0.0]);
        let add = graph.add(Calc::Add { a: 0.0, b: 3.0 }, [220.0, 0.0]);
        let out = graph.add(Calc::Result, [440.0, 0.0]);
        ops::connect(&mut graph, &[], None, a, Out::Value, add, In::A).unwrap();
        ops::connect(&mut graph, &[], None, add, Out::Value, out, In::A).unwrap();
        Self {
            doc: Doc {
                graph,
                defs: Vec::new(),
            },
            undo: Vec::new(),
            editor: EditorState::default(),
            style: EditorStyle::default(),
            labels: Labels::default(),
            keys: KeyMap::default(),
            dragging: false,
        }
    }

    /// Apply what the user did to the graph at the editor's scope.
    fn apply(&mut self, action: Action<In, Out, Calc>) {
        let continuing = matches!(
            action,
            Action::Move(..) | Action::SetValue { dragging: true, .. }
        );
        if !(continuing && self.dragging) {
            self.undo.push(self.doc.clone());
        }
        self.dragging = continuing;
        let scope_ = self.editor.scope.clone();
        let Doc { graph, defs } = &mut self.doc;
        let owner = scope::resolve(graph, defs, &scope_)
            .flatten()
            .map(|d| d.id.clone());
        let edit = |g: &mut Graph<Calc>, defs: &mut Vec<NodeDef<Calc>>, owner: Option<&_>| {
            let owner_def = owner
                .and_then(|o| flow_nodes_rs::def::find(defs, o))
                .cloned();
            match action {
                Action::Add {
                    template,
                    pos,
                    wire,
                } => {
                    let id = g.add(template, pos);
                    if let Some((other, w)) = wire {
                        let (from, out, to, input) = match w {
                            AutoWire::From { out, new_in } => (other, out, id, new_in),
                            AutoWire::To { input, new_out } => (id, new_out, other, input),
                        };
                        let _ = ops::connect(g, defs, owner_def.as_ref(), from, out, to, input);
                    }
                }
                Action::Move(ids, d) => {
                    for id in ids {
                        if let Some(n) = g.node_mut(id) {
                            n.pos[0] += d.x;
                            n.pos[1] += d.y;
                        }
                    }
                }
                Action::Connect(w) => {
                    let _ = ops::connect(g, defs, owner_def.as_ref(), w.from, w.out, w.to, w.input);
                }
                Action::Reconnect { old, new } => {
                    ops::disconnect(g, old.0, old.1);
                    let _ = ops::connect(
                        g,
                        defs,
                        owner_def.as_ref(),
                        new.from,
                        new.out,
                        new.to,
                        new.input,
                    );
                }
                Action::Disconnect(id, input) => {
                    ops::disconnect(g, id, input);
                }
                Action::Remove(ids) => ops::remove(g, defs, owner_def.as_ref(), &ids),
                Action::Duplicate(ids) => {
                    ops::duplicate(g, &ids, [30.0, 30.0]);
                }
                Action::Bypass(ids) => {
                    for n in &mut g.nodes {
                        if ids.contains(&n.id) {
                            n.bypass = !n.bypass;
                        }
                    }
                }
                Action::Group(ids) => {
                    let name = format!("Group {}", defs.len() + 1);
                    let _ = ops::group(g, defs, owner, &ids, &name);
                }
                Action::Ungroup(id) => {
                    let snapshot = defs.clone();
                    ops::ungroup(g, &snapshot, id);
                }
                Action::SetValue {
                    node,
                    target,
                    value: Value::Number(v),
                    ..
                } => {
                    if let Some(n) = g.node_mut(node) {
                        match (target, &mut n.kind) {
                            (ValueTarget::Own, Calc::Number(x)) => *x = v,
                            (ValueTarget::Input(key), kind) => {
                                kind.set_inline(key, v);
                            }
                            _ => {}
                        }
                    }
                }
                Action::SetValue { .. } => {}
            }
        };
        match owner {
            None => edit(graph, defs, None),
            Some(id) => {
                scope::edit_def(defs, &id, edit);
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !ctx.input(|i| i.pointer.any_down()) {
            self.dragging = false;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z))
            && let Some(doc) = self.undo.pop()
        {
            self.doc = doc;
        }
        let outcome = eval::evaluate(&self.doc.graph, &self.doc.defs, &NoEnv);
        let values = outcome
            .values
            .iter()
            .filter(|((s, _), _)| s.is_empty())
            .filter_map(|((_, id), outs)| Some((*id, *outs.first().map(|(_, v)| v)?)))
            .collect();
        let result = outcome.output.first().map_or(0.0, |(_, v)| *v);
        egui::Panel::top("bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.link("Top").clicked() {
                    self.editor.leave_to(0);
                }
                for _ in &self.editor.scope {
                    ui.label("› custom node");
                }
                ui.separator();
                ui.label(format!("Result = {result}"));
            });
        });
        let actions = egui::CentralPanel::default()
            .show(ui, |ui| {
                let Doc { graph, defs } = &self.doc;
                let Some((shown, owner)) = scope::graph_at(graph, defs, &self.editor.scope) else {
                    self.editor.leave_to(0);
                    return Vec::new();
                };
                let viewer = Shown {
                    graph: shown,
                    defs,
                    owner,
                    values: if owner.is_none() {
                        &values
                    } else {
                        &Default::default()
                    },
                };
                let editor = NodeEditor {
                    style: &self.style,
                    labels: &self.labels,
                    keys: &self.keys,
                    context: 0,
                };
                editor.show(ui, &viewer, &mut self.editor)
            })
            .inner;
        for action in actions {
            self.apply(action);
        }
    }
}

fn main() -> eframe::Result {
    eframe::run_native(
        "flow_nodes_rs editor",
        eframe::NativeOptions::default(),
        Box::new(|_| Ok(Box::new(App::new()))),
    )
}
