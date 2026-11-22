//! The node editor: the assembly's graph drawn as boxes and wires under the viewer.
//!
//! The canvas is the `flow_nodes_rs` crate's editor widget ([`flow_nodes_rs::editor`]): here
//! is what it draws ([`AssemblyViewer`], the crate's `Viewer`) and how what the user does is
//! applied. Drag a node's header to move it, drag from a socket to wire it (from a wired input
//! to move its wire), drag on the background to select a box of nodes, right- or middle-drag to
//! pan and turn the wheel to zoom. Right-click or Shift+A adds a node; dropping a wire on the
//! background adds one wired to it. With the pointer over the editor: Delete/X deletes, Ctrl+D
//! duplicates, M bypasses, Ctrl+G gathers the selection into a custom node and Ctrl+Alt+G takes
//! one apart, Tab goes into the selected custom node (and back out), Home frames everything,
//! Esc lets go.
//!
//! The editor only draws and reports what the user did; it is applied to the design as
//! [`GraphEdit`]s through [`access::edit_at`] after drawing, each one undo step (moving nodes
//! joins the history without rebuilding the assembly).

use std::cell::OnceCell;
use std::collections::BTreeSet;

use egui::{Color32, RichText, Sense, Ui, Vec2, pos2, vec2};
use flow_nodes_rs::ValueHint;
use flow_nodes_rs::editor::{
    Action, AutoWire, EditorState, EditorStyle, Inline, KeyMap, Labels, LooseEnd, MenuEntry,
    NodeEditor, NodeView, SocketRow, SocketShape, SocketStyle, Value, ValueTarget, Viewer,
    WireView,
};
use serde::{Deserialize, Serialize};

use crate::assembly::AssemblyState;
use crate::assembly::graph::defs::{self, NodeDef, Port};
use crate::assembly::graph::expr::eval_const;
use crate::assembly::graph::kinds::{Category, GroupRef, NodeKind, PortValue, SocketType};
use crate::assembly::graph::{
    Evaluation, Graph, InKey, Node, NodeId, OutKey, Shown, access, library, ops,
};
use crate::assembly::model::Design;
use crate::theme::dark;

pub(super) use flow_nodes_rs::editor::number_text;

/// A socket offered for wiring that is not there yet: a merge's next input, a custom node's
/// next input or output.
const NEW_PORT: u32 = u32::MAX;

/// The editor's state shared by every document. Whether it is open and its height are kept
/// between runs.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct NodeEditorView {
    pub open: bool,
    pub height: f32,
    /// Fills the central area instead of the viewer.
    pub maximized: bool,
    #[serde(skip)]
    pub(super) rt: Runtime,
}

impl Default for NodeEditorView {
    fn default() -> Self {
        Self {
            open: false,
            height: 260.0,
            maximized: false,
            rt: Runtime::default(),
        }
    }
}

/// What the editor holds while it runs.
#[derive(Default)]
pub(super) struct Runtime {
    /// The state and lineage the selection and scope belong to.
    key: Option<(u64, u64)>,
    /// The canvas's own: the selection, the custom nodes entered, the cameras.
    pub editor: EditorState<InKey, OutKey, SocketType>,
    /// The user's custom nodes, read when first needed.
    pub(super) library: OnceCell<Vec<library::Entry>>,
    /// The last thing the editor did or could not do, and whether it is a problem.
    pub report: Option<(String, bool)>,
}

impl Runtime {
    /// Forget the selection and scope when another design is shown.
    fn follow(&mut self, state: &AssemblyState) {
        let key = (state.id(), state.lineage());
        if self.key != Some(key) {
            self.key = Some(key);
            self.editor.reset();
        }
        if access::scope_def(&state.design, &self.editor.scope).is_none() {
            self.editor.scope.clear();
            self.editor.selected.clear();
        }
    }

    /// The node selected, when exactly one is.
    pub fn single(&self) -> Option<NodeId> {
        self.editor.single()
    }

    /// The library's custom nodes (read once; [`Runtime::reload_library`] reads again).
    pub fn library(&self) -> &[library::Entry] {
        self.library
            .get_or_init(|| library::NodeLibrary::default().list())
    }

    pub fn reload_library(&mut self) {
        self.library = OnceCell::new();
    }
}

/// An edit of the graph being shown.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum GraphEdit {
    Add {
        kind: Box<NodeKind>,
        pos: [f32; 2],
        wire: Option<(NodeId, Wire)>,
    },
    /// Move nodes (only where they are drawn).
    Move(Vec<NodeId>, Vec2),
    Connect(End2),
    /// Take the wire off an input and wire `End2` instead: one undo step.
    Reconnect(NodeId, InKey, End2),
    Disconnect(NodeId, InKey),
    Remove(BTreeSet<NodeId>),
    Duplicate(BTreeSet<NodeId>),
    Bypass(BTreeSet<NodeId>),
    Group(BTreeSet<NodeId>),
    Ungroup(NodeId),
    /// Set the value of an unwired input (or of a number or triple node, `InKey::Value`).
    SetValue(NodeId, InKey, PortValue),
    /// The same while it is dragged: the drag is one undo step when the button is let go.
    DragValue(NodeId, InKey, PortValue),
    /// Add custom node `def` (with the custom nodes it uses) from the library to the design,
    /// and a node using it at `pos`.
    AddFromLibrary(Box<library::Entry>, [f32; 2]),
}

/// How a new node is wired to an existing node's socket.
pub(super) type Wire = AutoWire<InKey, OutKey>;

/// Both ends of a wire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct End2 {
    pub from: NodeId,
    pub out: OutKey,
    pub to: NodeId,
    pub input: InKey,
}

impl From<WireView<InKey, OutKey>> for End2 {
    fn from(w: WireView<InKey, OutKey>) -> Self {
        End2 {
            from: w.from,
            out: w.out,
            to: w.to,
            input: w.input,
        }
    }
}

fn to_value(v: PortValue) -> Value {
    match v {
        PortValue::Number(x) => Value::Number(x),
        PortValue::Vec3(v) => Value::Vec3(v),
    }
}

fn from_value(v: Value) -> PortValue {
    match v {
        Value::Number(x) => PortValue::Number(x),
        Value::Vec3(v) => PortValue::Vec3(v),
    }
}

fn value_text(v: PortValue, unit: &str) -> String {
    flow_nodes_rs::editor::value_text(to_value(v), unit)
}

/// The line under a node's title: a step's parameters in short, an expression, a name.
pub(super) fn subtitle(kind: &NodeKind, design: &Design) -> Option<String> {
    Some(match kind {
        NodeKind::Monomer { component } => design
            .component_index(*component)
            .and_then(|i| design.components.get(i))
            .map_or_else(|| "?".to_owned(), |c| c.name.clone()),
        NodeKind::Op(step) => super::pipeline::summary(step),
        NodeKind::Select(s) => s.expr.clone(),
        NodeKind::Repeat(r) => {
            let parts: Vec<String> = r
                .offset
                .iter()
                .chain(&r.euler_deg)
                .filter(|e| e.trim() != "0")
                .cloned()
                .collect();
            parts.join(" · ")
        }
        NodeKind::SetAttr(a) => format!("{} = {}", a.name, a.expr),
        NodeKind::ParamCurve(c) => format!(
            "{}: {}, {}, {}",
            match c.coords {
                crate::assembly::graph::kinds::Coords::Cartesian => "x y z",
                crate::assembly::graph::kinds::Coords::Polar => "r θ z",
            },
            c.exprs[0],
            c.exprs[1],
            c.exprs[2]
        ),
        NodeKind::Expression(e) => e.expr.clone(),
        NodeKind::Value { value } => number_text(*value),
        NodeKind::Vector { value } => value_text(PortValue::Vec3(*value), ""),
        _ => return None,
    })
    .filter(|s| !s.is_empty())
}

fn header_color(c: Category) -> Color32 {
    match c {
        Category::Input => Color32::from_rgb(0x2E, 0x66, 0x6A),
        Category::Copy => Color32::from_rgb(0x8A, 0x4B, 0x2A),
        Category::Move => Color32::from_rgb(0x3A, 0x58, 0x86),
        Category::Combine => Color32::from_rgb(0x66, 0x4A, 0x88),
        Category::Math => Color32::from_rgb(0x4C, 0x66, 0x46),
        Category::Geometry => Color32::from_rgb(0x2F, 0x6E, 0x55),
        Category::Group => Color32::from_rgb(0x7A, 0x66, 0x26),
        Category::Output => Color32::from_rgb(0x55, 0x4C, 0x46),
    }
}

fn socket_color(ty: SocketType) -> Color32 {
    match ty {
        SocketType::Set => crate::theme::ORANGE_ON_DARK,
        SocketType::Number => Color32::from_rgb(0xA8, 0xA2, 0x9B),
        SocketType::Vec3 => Color32::from_rgb(0x8C, 0x7B, 0xD9),
        SocketType::Curve => Color32::from_rgb(0x4F, 0xC2, 0x8E),
        SocketType::Points => Color32::from_rgb(0x4A, 0xB8, 0xD8),
    }
}

const ERROR: Color32 = Color32::from_rgb(0xF2, 0x6D, 0x5E);

/// The editor in the app's dark theme.
fn style() -> EditorStyle {
    EditorStyle {
        background: dark::CANVAS_BACK,
        node: dark::RAISED,
        border: dark::BORDER,
        text: dark::TEXT,
        muted: dark::MUTED,
        accent: crate::theme::ORANGE_ON_DARK,
        error: ERROR,
        warning: Color32::from_rgb(0xE8, 0xB3, 0x3C),
        bypassed: Color32::from_rgb(0x4A, 0x45, 0x40),
        field_hover: dark::RAISED.gamma_multiply(1.4),
        ..EditorStyle::default()
    }
}

/// The editor's own words, in the UI language.
fn labels() -> Labels {
    Labels {
        value_tip: t!("assembly.editor.value_tip").to_owned(),
        vector_tip: t!("assembly.editor.vector_tip").to_owned(),
        search: t!("assembly.editor.search").to_owned(),
        no_match: t!("assembly.editor.no_match").to_owned(),
    }
}

/// What the add menu makes.
#[derive(Debug, Clone)]
pub(super) enum Make {
    Kind(NodeKind),
    Library(Box<library::Entry>),
}

type OutRow = SocketRow<OutKey, SocketType>;
type InRow = SocketRow<InKey, SocketType>;

/// A socket offered for a new input of a merge (`any_type` false: molecules only) or a new
/// port of a custom node (`any_type`: typed by what is wired to it).
fn ghost<K>(key: K, any_type: bool) -> SocketRow<K, SocketType> {
    let mut row = SocketRow::new(key, SocketType::Set, "");
    row.ghost = true;
    row.any_type = any_type;
    row
}

/// The graph at the editor's scope, as the crate's editor sees it.
pub(super) struct AssemblyViewer<'a> {
    design: &'a Design,
    graph: &'a Graph,
    owner: Option<&'a NodeDef>,
    ev: &'a Evaluation,
    scope: &'a [NodeId],
    library: &'a OnceCell<Vec<library::Entry>>,
}

impl AssemblyViewer<'_> {
    /// The socket rows of `node`, with the spare sockets offered for new ports and inputs.
    fn rows(&self, node: &Node) -> (Vec<OutRow>, Vec<InRow>) {
        let defs = &self.design.defs;
        let mut outs: Vec<_> = node
            .kind
            .outputs(defs, self.owner)
            .into_iter()
            .map(|o| SocketRow::new(o.key, o.ty, o.label))
            .collect();
        if matches!(node.kind, NodeKind::GroupInput) && self.owner.is_some() {
            outs.push(ghost(OutKey::Port(NEW_PORT), true));
        }
        let mut ins: Vec<_> = node
            .kind
            .inputs(defs, self.owner)
            .into_iter()
            .map(|i| {
                let wired = self.graph.link_into(node.id, i.key).is_some();
                let value = (!wired && matches!(i.ty, SocketType::Number | SocketType::Vec3))
                    .then(|| {
                        node.kind
                            .inline(i.key)
                            .or_else(|| match (&node.kind, i.key) {
                                (NodeKind::Group(g), InKey::Port(p)) => {
                                    defs::find(defs, &g.def)?
                                        .inputs
                                        .iter()
                                        .find(|q| q.key == p)?
                                        .default
                                }
                                _ => None,
                            })
                    })
                    .flatten();
                let mut row = SocketRow::new(i.key, i.ty, i.label);
                row.inline = value.map(|v| Inline::hinted(to_value(v), i.hint, " Å"));
                row
            })
            .collect();
        match &node.kind {
            NodeKind::Merge { inputs } => ins.push(ghost(InKey::Set(*inputs), false)),
            NodeKind::GroupOutput if self.owner.is_some() => {
                ins.push(ghost(InKey::Port(NEW_PORT), true));
            }
            _ => {}
        }
        (outs, ins)
    }

    /// The type of output `out` of node `id`.
    fn out_type(&self, id: NodeId, out: OutKey) -> Option<SocketType> {
        self.graph
            .node(id)?
            .kind
            .outputs(&self.design.defs, self.owner)
            .into_iter()
            .find(|o| o.key == out)
            .map(|o| o.ty)
    }
}

impl Viewer for AssemblyViewer<'_> {
    type In = InKey;
    type Out = OutKey;
    type Socket = SocketType;
    type Template = Make;

    fn nodes(&self) -> Vec<NodeView<InKey, OutKey, SocketType>> {
        let defs = &self.design.defs;
        self.graph
            .nodes
            .iter()
            .map(|node| {
                let title = if node.label.is_empty() {
                    node.kind.title(defs)
                } else {
                    node.label.clone()
                };
                let mut view = NodeView::new(
                    node.id,
                    pos2(node.pos[0], node.pos[1]),
                    title,
                    header_color(node.kind.category()),
                );
                view.subtitle = subtitle(&node.kind, self.design);
                view.bypass = node.bypass;
                view.dim = !self.ev.reached(self.scope, node.id);
                let issue = |list: &[crate::assembly::graph::Issue]| {
                    list.iter()
                        .find(|i| i.node == node.id && i.scope == self.scope)
                        .map(|i| i.message.clone())
                };
                view.error = issue(&self.ev.errors);
                view.warning = issue(&self.ev.warnings);
                view.own = match node.kind {
                    NodeKind::Value { value } => {
                        Some(Inline::hinted(Value::Number(value), ValueHint::Plain, ""))
                    }
                    NodeKind::Vector { value } => {
                        Some(Inline::hinted(Value::Vec3(value), ValueHint::Plain, ""))
                    }
                    _ => None,
                };
                (view.outputs, view.inputs) = self.rows(node);
                view
            })
            .collect()
    }

    fn links(&self) -> Vec<WireView<InKey, OutKey>> {
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

    fn socket_style(&self, ty: SocketType) -> SocketStyle {
        SocketStyle {
            color: socket_color(ty),
            shape: match ty {
                SocketType::Set | SocketType::Curve | SocketType::Points => SocketShape::Circle,
                SocketType::Number => SocketShape::SmallCircle,
                SocketType::Vec3 => SocketShape::Square,
            },
        }
    }

    fn can_connect(&self, from: SocketType, to: SocketType) -> bool {
        from.feeds(to)
    }

    fn add_menu(&self) -> Vec<MenuEntry<Make>> {
        let design = self.design;
        let mut entries = Vec::new();
        if self.owner.is_none() {
            for (i, c) in design.components.iter().enumerate() {
                entries.push(MenuEntry {
                    label: format!("{} · {}", t!("assembly.node.monomer"), c.name),
                    group: t!("assembly.editor.menu.input").to_owned(),
                    template: Make::Kind(NodeKind::Monomer {
                        component: design.component_id(i),
                    }),
                });
            }
        }
        for kind in NodeKind::menu() {
            let group = match kind.category() {
                Category::Copy => t!("assembly.editor.menu.copy"),
                Category::Move => t!("assembly.editor.menu.move"),
                Category::Combine => t!("assembly.editor.menu.combine"),
                Category::Geometry => t!("assembly.editor.menu.geometry"),
                Category::Input => t!("assembly.editor.menu.input"),
                _ => t!("assembly.editor.menu.math"),
            };
            entries.push(MenuEntry {
                label: kind.title(&design.defs),
                group: group.to_owned(),
                template: Make::Kind(kind),
            });
        }
        let current = self.owner.map(|d| &d.id);
        for def in &design.defs {
            // Not inside itself, nor inside one it uses.
            if current.is_some_and(|c| defs::depends_on(&design.defs, &def.id, c)) {
                continue;
            }
            entries.push(MenuEntry {
                label: def.name.clone(),
                group: t!("assembly.editor.menu.custom").to_owned(),
                template: Make::Kind(NodeKind::Group(GroupRef::new(def.id.clone()))),
            });
        }
        let library = self
            .library
            .get_or_init(|| library::NodeLibrary::default().list());
        for entry in library {
            if defs::find(&design.defs, &entry.def.id).is_some() {
                continue;
            }
            entries.push(MenuEntry {
                label: entry.def.name.clone(),
                group: t!("assembly.editor.menu.library").to_owned(),
                template: Make::Library(Box::new(entry.clone())),
            });
        }
        entries
    }

    fn auto_wire(
        &self,
        template: &Make,
        end: &LooseEnd<InKey, OutKey, SocketType>,
    ) -> Option<Wire> {
        match template {
            Make::Kind(kind) => wire_for(kind, &self.design.defs, end),
            Make::Library(entry) => {
                let kind = NodeKind::Group(GroupRef::new(entry.def.id.clone()));
                let mut all = entry.deps.clone();
                all.push(entry.def.clone());
                wire_for(&kind, &all, end)
            }
        }
    }

    fn wire_badge(&self, w: &WireView<InKey, OutKey>) -> Option<String> {
        let shown = self.ev.shown.get(&(self.scope.to_vec(), w.from, w.out))?;
        match shown {
            Shown::Count(n)
                if matches!(
                    self.out_type(w.from, w.out),
                    Some(SocketType::Set | SocketType::Points)
                ) =>
            {
                Some(format!("{n}"))
            }
            Shown::Field => Some("ƒ".to_owned()),
            Shown::Number(x) => Some(number_text(*x)),
            _ => None,
        }
    }

    /// A switched-off selection's other molecules: nothing goes that way.
    fn wire_dimmed(&self, w: &WireView<InKey, OutKey>) -> bool {
        w.out == OutKey::Rest && self.graph.node(w.from).is_some_and(|n| n.bypass)
    }

    fn enterable(&self, id: NodeId) -> bool {
        self.graph
            .node(id)
            .is_some_and(|n| matches!(n.kind, NodeKind::Group(_)))
    }

    /// A number, or an expression without names such as `360/7`.
    fn parse_number(&self, text: &str) -> Option<f64> {
        eval_const(text)
    }

    fn empty_hint(&self) -> Option<String> {
        Some(t!("assembly.editor.empty_hint").to_owned())
    }
}

/// How a new node of `kind` would be wired to `end`: its first input that takes the output,
/// or its first output that feeds the input.
fn wire_for(
    kind: &NodeKind,
    defs: &[NodeDef],
    end: &LooseEnd<InKey, OutKey, SocketType>,
) -> Option<Wire> {
    match *end {
        LooseEnd::Out(_, out, ty) => kind
            .inputs(defs, None)
            .into_iter()
            .find(|i| ty.feeds(i.ty))
            .map(|i| Wire::From { out, new_in: i.key }),
        LooseEnd::In(_, input, ty) => kind
            .outputs(defs, None)
            .into_iter()
            .find(|o| o.ty.feeds(ty))
            .map(|o| Wire::To {
                input,
                new_out: o.key,
            }),
    }
}

/// What the user did on the canvas, as an edit of the design.
fn edit_of(action: Action<InKey, OutKey, Make>) -> GraphEdit {
    match action {
        Action::Add {
            template: Make::Kind(kind),
            pos,
            wire,
        } => GraphEdit::Add {
            kind: Box::new(kind),
            pos,
            wire,
        },
        Action::Add {
            template: Make::Library(entry),
            pos,
            ..
        } => GraphEdit::AddFromLibrary(entry, pos),
        Action::Move(ids, delta) => GraphEdit::Move(ids, delta),
        Action::Connect(w) => GraphEdit::Connect(w.into()),
        Action::Reconnect { old, new } => GraphEdit::Reconnect(old.0, old.1, new.into()),
        Action::Disconnect(node, input) => GraphEdit::Disconnect(node, input),
        Action::Remove(ids) => GraphEdit::Remove(ids),
        Action::Duplicate(ids) => GraphEdit::Duplicate(ids),
        Action::Bypass(ids) => GraphEdit::Bypass(ids),
        Action::Group(ids) => GraphEdit::Group(ids),
        Action::Ungroup(id) => GraphEdit::Ungroup(id),
        Action::SetValue {
            node,
            target,
            value,
            dragging,
        } => {
            let key = match target {
                ValueTarget::Own => InKey::Value,
                ValueTarget::Input(key) => key,
            };
            let value = from_value(value);
            if dragging {
                GraphEdit::DragValue(node, key, value)
            } else {
                GraphEdit::SetValue(node, key, value)
            }
        }
    }
}

/// The editor: a small bar (where it is, add, group, frame, maximise, close) over the canvas.
pub(super) fn editor(ui: &mut Ui, state: &mut AssemblyState, view: &mut NodeEditorView) {
    view.rt.follow(state);
    let mut edits = Vec::new();
    bar(ui, state, view, &mut edits);
    canvas(ui, state, view, &mut edits);
    for edit in edits {
        apply(state, &mut view.rt, edit);
    }
}

/// The bar over the canvas: the custom nodes entered (click one to go back to it), and the
/// buttons.
fn bar(ui: &mut Ui, state: &AssemblyState, view: &mut NodeEditorView, edits: &mut Vec<GraphEdit>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let muted = dark::MUTED;
        let crumb = |ui: &mut Ui, text: String, current: bool| {
            let color = if current { dark::TEXT } else { muted };
            ui.add(
                egui::Label::new(RichText::new(text).size(12.0).color(color))
                    .sense(Sense::click())
                    .selectable(false),
            )
        };
        let rt = &mut view.rt;
        if crumb(
            ui,
            t!("assembly.editor.top").into(),
            rt.editor.scope.is_empty(),
        )
        .clicked()
        {
            rt.editor.leave_to(0);
        }
        let design = &state.design;
        let mut back_to = None;
        let mut graph = design.effective_graph().into_owned();
        for (k, id) in rt.editor.scope.iter().enumerate() {
            let Some(NodeKind::Group(g)) = graph.node(*id).map(|n| &n.kind) else {
                break;
            };
            let Some(def) = defs::find(&design.defs, &g.def) else {
                break;
            };
            ui.label(RichText::new("›").size(12.0).color(muted));
            if crumb(ui, def.name.clone(), k + 1 == rt.editor.scope.len()).clicked() {
                back_to = Some(k + 1);
            }
            graph = def.graph.clone();
        }
        if let Some(k) = back_to {
            rt.editor.leave_to(k);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("✕")
                .on_hover_text(t!("assembly.editor.close"))
                .clicked()
            {
                view.open = false;
            }
            let (text, tip) = if view.maximized {
                (
                    t!("assembly.editor.restore_short"),
                    t!("assembly.editor.restore"),
                )
            } else {
                (
                    t!("assembly.editor.maximize_short"),
                    t!("assembly.editor.maximize"),
                )
            };
            if ui.small_button(text).on_hover_text(tip).clicked() {
                view.maximized = !view.maximized;
            }
            let rt = &mut view.rt;
            if ui
                .small_button(t!("assembly.editor.frame"))
                .on_hover_text(t!("assembly.editor.frame_tip"))
                .clicked()
            {
                rt.editor.frame_all();
            }
            let groupable = !rt.editor.selected.is_empty();
            if ui
                .add_enabled(
                    groupable,
                    egui::Button::new(t!("assembly.editor.group")).small(),
                )
                .on_hover_text(t!("assembly.editor.group_tip"))
                .clicked()
            {
                edits.push(GraphEdit::Group(rt.editor.selected.clone()));
            }
            if ui
                .small_button(t!("assembly.editor.add"))
                .on_hover_text(t!("assembly.editor.add_tip"))
                .clicked()
            {
                rt.editor.open_add_menu(
                    ui.max_rect().left_bottom() + vec2(0.0, 4.0),
                    pos2(40.0, 40.0),
                );
            }
            if let Some((text, problem)) = &rt.report {
                let color = if *problem { ERROR } else { muted };
                ui.add(egui::Label::new(RichText::new(text).size(11.0).color(color)).truncate());
            }
        });
    });
}

/// The canvas: the crate's editor over the graph at the scope.
fn canvas(
    ui: &mut Ui,
    state: &mut AssemblyState,
    view: &mut NodeEditorView,
    edits: &mut Vec<GraphEdit>,
) {
    let design = &state.design;
    if design.components.is_empty() {
        let area = ui.available_rect_before_wrap();
        ui.allocate_rect(area, Sense::hover());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, dark::CANVAS_BACK);
        painter.text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            t!("assembly.editor.empty_hint"),
            egui::FontId::proportional(12.0),
            dark::MUTED,
        );
        return;
    }
    let ev = design.evaluation();
    let rt = &mut view.rt;
    let scope = rt.editor.scope.clone();
    let Some((graph, owner)) = access::graph_at(design, &scope) else {
        return;
    };
    let viewer = AssemblyViewer {
        design,
        graph: &graph,
        owner,
        ev: &ev,
        scope: &scope,
        library: &rt.library,
    };
    let (style, labels, keys) = (style(), labels(), KeyMap::default());
    let editor = NodeEditor {
        style: &style,
        labels: &labels,
        keys: &keys,
        context: state.lineage(),
    };
    for action in editor.show(ui, &viewer, &mut rt.editor) {
        edits.push(edit_of(action));
    }
}

/// Apply `edit` to the graph the editor shows: moving nodes joins the undo history without
/// rebuilding; every other edit is one undo step.
pub(super) fn apply(state: &mut AssemblyState, rt: &mut Runtime, edit: GraphEdit) {
    let scope = rt.editor.scope.clone();
    if let GraphEdit::Move(ids, d) = &edit {
        access::edit_layout_at(&mut state.design, &scope, |g| {
            for id in ids {
                if let Some(n) = g.node_mut(*id) {
                    n.pos[0] += d.x;
                    n.pos[1] += d.y;
                }
            }
        });
        state.touch_layout();
        return;
    }
    if let GraphEdit::DragValue(id, key, value) = edit {
        access::edit_at(&mut state.design, &scope, |g, _, _| {
            set_value(g, id, key, value);
        });
        state.touch();
        return;
    }
    let result = state.edit(|design| {
        access::edit_at(design, &scope, |g, defs, owner| {
            apply_to(g, defs, owner, edit)
        })
        .unwrap_or(Ok(None))
    });
    match result {
        Ok(Some(selected)) => {
            rt.editor.selected = selected;
            rt.report = None;
        }
        Ok(None) => rt.report = None,
        Err(message) => rt.report = Some((message, true)),
    }
}

/// Set input `key` of node `id` to `value` (`InKey::Value`: a number or triple node's own).
fn set_value(graph: &mut Graph, id: NodeId, key: InKey, value: PortValue) {
    if let Some(n) = graph.node_mut(id) {
        match (&mut n.kind, key) {
            (NodeKind::Value { value: v }, InKey::Value) => *v = value.number(),
            (NodeKind::Vector { value: v }, InKey::Value) => *v = value.vec3(),
            (kind, key) => {
                kind.set_inline(key, value);
            }
        }
    }
}

/// `edit` on `graph` (the graph of custom node `owner`, or the top level), the design's custom
/// nodes being `defs`. Returns the nodes to select afterwards.
fn apply_to(
    graph: &mut Graph,
    defs: &mut Vec<NodeDef>,
    owner: Option<&defs::DefId>,
    edit: GraphEdit,
) -> Result<Option<BTreeSet<NodeId>>, String> {
    match edit {
        GraphEdit::Move(..) => Ok(None),
        GraphEdit::Add { kind, pos, wire } => {
            let id = graph.add(*kind, pos);
            if let Some((other, wire)) = wire {
                let end = match wire {
                    Wire::From { out, new_in } => End2 {
                        from: other,
                        out,
                        to: id,
                        input: new_in,
                    },
                    Wire::To { input, new_out } => End2 {
                        from: id,
                        out: new_out,
                        to: other,
                        input,
                    },
                };
                connect(graph, defs, owner, end)?;
            }
            Ok(Some([id].into()))
        }
        GraphEdit::Connect(end) => connect(graph, defs, owner, end).map(|()| None),
        GraphEdit::Reconnect(node, input, end) => {
            // Put the wire back if the new one cannot be made.
            let before = graph.links.clone();
            ops::disconnect(graph, node, input);
            connect(graph, defs, owner, end)
                .map(|()| None)
                .inspect_err(|_| graph.links = before)
        }
        GraphEdit::Disconnect(to, input) => {
            ops::disconnect(graph, to, input);
            Ok(None)
        }
        GraphEdit::Remove(ids) => {
            let owner_def = owner.and_then(|o| defs::find(defs, o)).cloned();
            ops::remove(graph, defs, owner_def.as_ref(), &ids);
            Ok(Some(BTreeSet::new()))
        }
        GraphEdit::Duplicate(ids) => {
            let copies = ops::duplicate(graph, &ids, [30.0, 30.0]);
            Ok(Some(copies.into_iter().collect()))
        }
        GraphEdit::Bypass(ids) => {
            let can = |n: &Node| {
                n.kind
                    .outputs(defs, None)
                    .iter()
                    .any(|o| o.ty == SocketType::Set)
            };
            let on = graph
                .nodes
                .iter()
                .filter(|n| ids.contains(&n.id) && can(n))
                .any(|n| !n.bypass);
            for n in graph.nodes.iter_mut() {
                if ids.contains(&n.id) && can(n) {
                    n.bypass = on;
                }
            }
            Ok(None)
        }
        GraphEdit::Group(ids) => {
            let name = new_group_name(defs);
            let (_, node) = ops::group(graph, defs, owner, &ids, &name)
                .map_err(|e| ops::group_message(e).to_owned())?;
            Ok(Some([node].into()))
        }
        GraphEdit::SetValue(id, key, value) | GraphEdit::DragValue(id, key, value) => {
            set_value(graph, id, key, value);
            Ok(None)
        }
        GraphEdit::Ungroup(node) => {
            let snapshot = defs.clone();
            match ops::ungroup(graph, &snapshot, node) {
                Some(added) => Ok(Some(added.into_iter().collect())),
                None => Ok(None),
            }
        }
        GraphEdit::AddFromLibrary(entry, pos) => {
            if let Some(owner) = owner
                && defs::depends_on(defs, &entry.def.id, owner)
            {
                return Err(t!("assembly.editor.error.recursion").to_owned());
            }
            library::adopt(defs, &entry, false);
            let id = graph.add(
                NodeKind::Group(GroupRef {
                    def: entry.def.id.clone(),
                    inputs: Default::default(),
                }),
                pos,
            );
            Ok(Some([id].into()))
        }
    }
}

/// "Group", "Group 2", …: the first name no custom node of the design has.
fn new_group_name(defs: &[NodeDef]) -> String {
    let base = t!("assembly.editor.new_group");
    let taken = |n: &str| defs.iter().any(|d| d.name == n);
    if !taken(base) {
        return base.to_owned();
    }
    (2..)
        .map(|k| format!("{base} {k}"))
        .find(|n| !taken(n))
        .expect("an unused name")
}

/// Wire `end`, making the socket it ends on first when it is one offered: the next input of a
/// merge, a new input of the custom node (from its group input node) or a new output (into its
/// group output node).
fn connect(
    graph: &mut Graph,
    defs: &mut [NodeDef],
    owner: Option<&defs::DefId>,
    mut end: End2,
) -> Result<(), String> {
    let missing = || ops::connect_message(ops::ConnectError::Missing).to_owned();
    // The spare input of a merge.
    if let (
        Some(Node {
            kind: NodeKind::Merge { inputs },
            ..
        }),
        InKey::Set(k),
    ) = (graph.node_mut(end.to), end.input)
        && k >= *inputs
    {
        *inputs = (*inputs + 1).min(32);
        end.input = InKey::Set(*inputs - 1);
    }
    let owner_def = owner.and_then(|o| defs::find(defs, o)).cloned();
    // A new output of the custom node: typed and named by what feeds it.
    if end.input == InKey::Port(NEW_PORT)
        && let Some(def_id) = owner
    {
        let (ty, name) = graph
            .node(end.from)
            .and_then(|n| {
                n.kind
                    .outputs(defs, owner_def.as_ref())
                    .into_iter()
                    .find(|o| o.key == end.out)
            })
            .map(|o| (o.ty, o.label))
            .ok_or_else(missing)?;
        let def = defs::find_mut(defs, def_id).ok_or_else(missing)?;
        let key = def.next_port_key();
        def.outputs.push(Port {
            key,
            name,
            ty,
            default: None,
        });
        end.input = InKey::Port(key);
    }
    // A new input of the custom node: typed, named and valued by what it feeds.
    if end.out == OutKey::Port(NEW_PORT)
        && let Some(def_id) = owner
    {
        let (decl, value) = graph
            .node(end.to)
            .and_then(|n| {
                let decl = n
                    .kind
                    .inputs(defs, owner_def.as_ref())
                    .into_iter()
                    .find(|i| i.key == end.input)?;
                Some((decl, n.kind.inline(end.input)))
            })
            .ok_or_else(missing)?;
        let def = defs::find_mut(defs, def_id).ok_or_else(missing)?;
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
        end.out = OutKey::Port(key);
    }
    let owner_def = owner.and_then(|o| defs::find(defs, o)).cloned();
    ops::connect(
        graph,
        defs,
        owner_def.as_ref(),
        end.from,
        end.out,
        end.to,
        end.input,
    )
    .map_err(|e| ops::connect_message(e).to_owned())
}

/// Replace node `node.id` of the graph at `scope` by `node` (an edit from the panel), keeping
/// a design made of steps so while it is still a chain.
pub(super) fn set_node(state: &mut AssemblyState, scope: &[NodeId], node: Node) {
    access::edit_at(&mut state.design, scope, |g, _, _| {
        if let Some(n) = g.node_mut(node.id) {
            *n = node;
        }
    });
    state.touch();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::graph::chain_step_id;
    use crate::assembly::model::{Component, Step};
    use crate::three_d::Atom3d;

    fn state() -> AssemblyState {
        let mut s = AssemblyState::default();
        s.edit(|d| {
            d.components.push(Component {
                name: "w".into(),
                atoms: vec![Atom3d {
                    element: "O".into(),
                    pos: [0.0; 3],
                }],
                bonds: vec![],
                orient: None,
            });
            d.steps = vec![Step::new_ring(), Step::new_stack()];
        });
        s
    }

    #[test]
    fn moving_a_node_is_undoable_and_rebuilds_nothing() {
        let mut s = state();
        let mut rt = Runtime::default();
        let built = s.built().unwrap();
        let revision = s.revision();
        apply(
            &mut s,
            &mut rt,
            GraphEdit::Move(vec![chain_step_id(0)], vec2(10.0, 5.0)),
        );
        s.settle();
        assert_eq!(s.revision(), revision);
        assert!(std::sync::Arc::ptr_eq(&built, &s.built().unwrap()));
        assert!(s.design.is_linear(), "a moved chain is still steps");
        let moved = |s: &AssemblyState| {
            s.design
                .effective_graph()
                .node(chain_step_id(0))
                .unwrap()
                .pos
        };
        let at = moved(&s);
        assert!(s.undo());
        assert_ne!(moved(&s), at, "undo puts the node back");
    }

    #[test]
    fn a_merge_grows_an_input_and_a_custom_node_a_port_when_wired() {
        let mut s = state();
        let mut rt = Runtime::default();
        let b = s.design.component_id(0);
        apply(
            &mut s,
            &mut rt,
            GraphEdit::Add {
                kind: Box::new(NodeKind::Merge { inputs: 1 }),
                pos: [0.0, 200.0],
                wire: None,
            },
        );
        let merge = rt.single().unwrap();
        apply(
            &mut s,
            &mut rt,
            GraphEdit::Add {
                kind: Box::new(NodeKind::Monomer { component: b }),
                pos: [0.0, 300.0],
                wire: Some((
                    merge,
                    Wire::To {
                        input: InKey::Set(0),
                        new_out: OutKey::Set,
                    },
                )),
            },
        );
        apply(
            &mut s,
            &mut rt,
            GraphEdit::Connect(End2 {
                from: chain_step_id(1),
                out: OutKey::Set,
                to: merge,
                input: InKey::Set(1),
            }),
        );
        assert_eq!(rt.report, None);
        let g = s.design.graph.as_ref().unwrap();
        assert!(matches!(
            g.node(merge).unwrap().kind,
            NodeKind::Merge { inputs: 2 }
        ));

        // Group the ring and go inside: wiring into the spare socket of the group output adds
        // an output port.
        rt.editor.selected = [chain_step_id(0)].into();
        let chosen = rt.editor.selected.clone();
        apply(&mut s, &mut rt, GraphEdit::Group(chosen));
        let group = rt.single().unwrap();
        rt.editor.scope = vec![group];
        let (inner, owner) = access::graph_at(&s.design, &rt.editor.scope).unwrap();
        let ring = inner
            .nodes
            .iter()
            .find(|n| matches!(n.kind, NodeKind::Op(_)))
            .unwrap()
            .id;
        let gout = inner.group_output().unwrap();
        let outputs = owner.unwrap().outputs.len();
        apply(
            &mut s,
            &mut rt,
            GraphEdit::Connect(End2 {
                from: ring,
                out: OutKey::Set,
                to: gout,
                input: InKey::Port(NEW_PORT),
            }),
        );
        assert_eq!(rt.report, None);
        let (_, owner) = access::graph_at(&s.design, &rt.editor.scope).unwrap();
        assert_eq!(owner.unwrap().outputs.len(), outputs + 1);
        assert!(s.design.evaluation().errors.is_empty());
    }

    #[test]
    fn typed_values_read_numbers_expressions_and_triples() {
        use flow_nodes_rs::editor::parse_value;
        let number = |s: &str| eval_const(s);
        let n = |s| parse_value(s, false, &number);
        let v = |s| parse_value(s, true, &number);
        assert_eq!(n(" 360/7 "), Some(Value::Number(360.0 / 7.0)));
        assert_eq!(n("1/0"), None);
        assert_eq!(n("i"), None);
        assert_eq!(v("1, 2*2, max(1, 3)"), Some(Value::Vec3([1.0, 4.0, 3.0])));
        assert_eq!(v("2"), Some(Value::Vec3([2.0; 3])));
        assert_eq!(v("1, 2"), None);
    }

    /// Dragging a wire from one input to another is one undo step; a wire that cannot be
    /// made leaves the old one.
    #[test]
    fn moving_a_wire_is_one_undo_step() {
        let mut s = state();
        let mut rt = Runtime::default();
        apply(
            &mut s,
            &mut rt,
            GraphEdit::Add {
                kind: Box::new(NodeKind::Merge { inputs: 2 }),
                pos: [0.0, 200.0],
                wire: None,
            },
        );
        let merge = rt.single().unwrap();
        let stack = chain_step_id(1);
        let out = s.design.effective_graph().output().unwrap();
        let fed_by = |s: &AssemblyState, to: NodeId, input: InKey| {
            s.design
                .effective_graph()
                .link_into(to, input)
                .map(|l| l.from)
        };
        assert_eq!(fed_by(&s, out, InKey::Set(0)), Some(stack));
        // From the output's input onto the merge's.
        apply(
            &mut s,
            &mut rt,
            GraphEdit::Reconnect(
                out,
                InKey::Set(0),
                End2 {
                    from: stack,
                    out: OutKey::Set,
                    to: merge,
                    input: InKey::Set(1),
                },
            ),
        );
        assert_eq!(rt.report, None);
        assert_eq!(fed_by(&s, out, InKey::Set(0)), None);
        assert_eq!(fed_by(&s, merge, InKey::Set(1)), Some(stack));
        assert!(s.undo());
        assert_eq!(fed_by(&s, out, InKey::Set(0)), Some(stack), "one step back");
        // Onto a socket of another type: refused, the wire stays.
        apply(
            &mut s,
            &mut rt,
            GraphEdit::Reconnect(
                out,
                InKey::Set(0),
                End2 {
                    from: stack,
                    out: OutKey::Set,
                    to: chain_step_id(0),
                    input: InKey::Field(crate::assembly::graph::StepField::RingRadius),
                },
            ),
        );
        assert!(rt.report.is_some());
        assert_eq!(fed_by(&s, out, InKey::Set(0)), Some(stack));
    }

    /// A value typed on the canvas sets the input, keeps a chain a chain, and is one undo step.
    #[test]
    fn setting_a_typed_value_is_one_undo_step() {
        use crate::assembly::graph::StepField;
        let mut s = state();
        let mut rt = Runtime::default();
        let count = |s: &AssemblyState| s.design.molecule_count();
        let before = count(&s);
        apply(
            &mut s,
            &mut rt,
            GraphEdit::SetValue(
                chain_step_id(0),
                InKey::Field(StepField::RingCount),
                PortValue::Number(8.0),
            ),
        );
        assert_eq!(rt.report, None);
        assert!(s.design.is_linear());
        assert_eq!(count(&s) * 6, before * 8, "the ring of 6 is a ring of 8");
        assert!(s.undo());
        assert_eq!(count(&s), before);
    }

    /// Dragging a value changes it every frame and is one undo step once let go.
    #[test]
    fn dragging_a_value_is_one_undo_step() {
        use crate::assembly::graph::StepField;
        let mut s = state();
        let mut rt = Runtime::default();
        let before = s.design.molecule_count();
        for n in [7.0, 8.0, 9.0] {
            apply(
                &mut s,
                &mut rt,
                GraphEdit::DragValue(
                    chain_step_id(0),
                    InKey::Field(StepField::RingCount),
                    PortValue::Number(n),
                ),
            );
        }
        assert_eq!(s.design.molecule_count() * 6, before * 9);
        s.settle();
        assert!(s.undo());
        assert_eq!(s.design.molecule_count(), before);
    }

    /// The editor draws headless, inside a custom node too, with the add menu open.
    #[test]
    fn the_editor_draws_headless() {
        let ctx = egui::Context::default();
        let mut s = state();
        let mut view = NodeEditorView::default();
        view.rt.library = OnceCell::from(Vec::new());
        for round in 0..4 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                editor(ui, &mut s, &mut view);
            });
            output.textures_delta.clear();
            match round {
                0 => {
                    let chosen: BTreeSet<NodeId> = [chain_step_id(0), chain_step_id(1)].into();
                    apply(&mut s, &mut view.rt, GraphEdit::Group(chosen));
                    view.rt
                        .editor
                        .open_add_menu(pos2(10.0, 10.0), pos2(0.0, 0.0));
                }
                1 => view.rt.editor.enter(view.rt.single().unwrap()),
                _ => {}
            }
        }
        assert_eq!(view.rt.editor.scope.len(), 1);
    }
}
