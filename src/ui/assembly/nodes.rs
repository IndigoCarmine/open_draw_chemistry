//! The node editor: the assembly's graph drawn as boxes and wires under the viewer.
//!
//! Drag a node's header to move it (the selection moves with it), drag from a socket to wire
//! it (from a wired input to take the wire off), drag on the background to select a box of
//! nodes, right- or middle-drag to pan and turn the wheel to zoom. Right-click or Shift+A
//! adds a node; dropping a wire on the background adds one wired to it. With the pointer over
//! the editor: Delete/X deletes, Ctrl+D duplicates, M bypasses, Ctrl+G gathers the selection
//! into a custom node and Ctrl+Alt+G takes one apart, Tab goes into the selected custom node
//! (and back out), Home frames everything, Esc lets go.
//!
//! The editor only draws and collects [`GraphEdit`]s; they are applied to the design through
//! [`access::edit_at`] after drawing, each one undo step (moving nodes joins the history
//! without rebuilding the assembly).

use std::collections::{BTreeSet, HashMap};

use egui::epaint::CubicBezierShape;
use egui::{
    Align2, Color32, CornerRadius, FontId, Key, Modifiers, PointerButton, Pos2, Rect, RichText,
    Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
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

/// Width of a node on the canvas at zoom 1.
const NODE_W: f32 = 176.0;
const HEADER_H: f32 = 22.0;
const ROW_H: f32 = 19.0;
const SOCKET_R: f32 = 4.5;
/// How close to a socket (screen px) a wire may be dropped.
const SOCKET_HIT: f32 = 11.0;

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
            open: true,
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
    /// The custom nodes entered, outermost first.
    pub scope: Vec<NodeId>,
    pub selected: BTreeSet<NodeId>,
    /// Where each graph is looked at, by the design's lineage and the scope.
    cameras: HashMap<(u64, Vec<NodeId>), Camera>,
    drag: Option<Drag>,
    menu: Option<AddMenu>,
    /// The user's custom nodes, read when first needed.
    pub library: Option<Vec<library::Entry>>,
    /// The last thing the editor did or could not do, and whether it is a problem.
    pub report: Option<(String, bool)>,
}

impl Runtime {
    /// Forget the selection and scope when another design is shown.
    fn follow(&mut self, state: &AssemblyState) {
        let key = (state.id(), state.lineage());
        if self.key != Some(key) {
            self.key = Some(key);
            self.scope.clear();
            self.selected.clear();
            self.drag = None;
            self.menu = None;
        }
        if access::scope_def(&state.design, &self.scope).is_none() {
            self.scope.clear();
            self.selected.clear();
        }
    }

    /// The node selected, when exactly one is.
    pub fn single(&self) -> Option<NodeId> {
        (self.selected.len() == 1).then(|| *self.selected.iter().next().expect("one"))
    }

    /// The library's custom nodes (read once; [`Runtime::reload_library`] reads again).
    pub fn library(&mut self) -> &[library::Entry] {
        self.library
            .get_or_insert_with(|| library::NodeLibrary::default().list())
    }

    pub fn reload_library(&mut self) {
        self.library = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Camera {
    /// Where the canvas origin is, from the editor's top-left corner (screen px).
    pan: Vec2,
    zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            pan: vec2(24.0, 24.0),
            zoom: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum End {
    Out(NodeId, OutKey, SocketType),
    In(NodeId, InKey, SocketType),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// Moving the selected nodes.
    Nodes,
    /// A wire from a socket, its loose end under the pointer.
    Wire(End),
    /// Selecting the nodes in a box (screen corners).
    Box(Pos2),
}

#[derive(Debug, Clone, PartialEq)]
struct AddMenu {
    /// Where it was opened (screen) and where the node goes (canvas).
    screen: Pos2,
    canvas: Pos2,
    query: String,
    /// A wire dropped on the background: the new node is wired to it.
    from: Option<End>,
    focus: bool,
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Wire {
    /// Output `out` of the existing node feeds input `new_in` of the new one.
    From { out: OutKey, new_in: InKey },
    /// Output `new_out` of the new node feeds input `input` of the existing one.
    To { input: InKey, new_out: OutKey },
}

/// Both ends of a wire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct End2 {
    pub from: NodeId,
    pub out: OutKey,
    pub to: NodeId,
    pub input: InKey,
}

/// One socket row of a node as laid out.
#[derive(Debug, Clone)]
struct Row {
    side: Side,
    ty: SocketType,
    label: String,
    /// The value of an unwired input, as text and as typed into its box.
    value: Option<String>,
    edit: Option<PortValue>,
    unit: &'static str,
    /// How a drag changes the value.
    scrub: Scrub,
    /// Canvas y of the row's middle, from the node's top.
    y: f32,
    /// A socket offered for a new port or merge input.
    new: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Side {
    Out(OutKey),
    In(InKey),
}

/// A node as drawn: its box on the canvas and its rows.
#[derive(Debug, Clone)]
struct Laid {
    id: NodeId,
    rect: Rect,
    title: String,
    subtitle: Option<String>,
    /// The value of a number or triple node, typed in or dragged on its subtitle's line.
    own: Option<PortValue>,
    rows: Vec<Row>,
    category: Category,
    bypass: bool,
    reached: bool,
    error: Option<String>,
    warning: Option<String>,
}

impl Laid {
    fn socket(&self, side: Side) -> Option<(Pos2, SocketType)> {
        self.rows.iter().find(|r| r.side == side).map(|r| {
            let x = match side {
                Side::Out(_) => self.rect.right(),
                Side::In(_) => self.rect.left(),
            };
            (pos2(x, self.rect.top() + r.y), r.ty)
        })
    }
}

/// The value of a number, for a node: up to 3 decimals, trailing zeros dropped.
pub(super) fn number_text(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn value_text(v: PortValue, unit: &str) -> String {
    match v {
        PortValue::Number(x) => format!("{}{unit}", number_text(x)),
        PortValue::Vec3([x, y, z]) => format!(
            "{}, {}, {}{unit}",
            number_text(x),
            number_text(y),
            number_text(z)
        ),
    }
}

/// The line under a node's title: a step's parameters in short, an expression, a name.
pub(super) fn subtitle(kind: &NodeKind, design: &Design) -> Option<String> {
    Some(match kind {
        NodeKind::Monomer { component } => design
            .component_index(*component)
            .and_then(|i| design.components.get(i))
            .map_or_else(|| "?".to_owned(), |c| c.name.clone()),
        NodeKind::Op(step) => super::labels::summary(step),
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
        NodeKind::Expression(e) => e.expr.clone(),
        NodeKind::Value { value } => number_text(*value),
        NodeKind::Vector { value } => value_text(PortValue::Vec3(*value), ""),
        _ => return None,
    })
    .filter(|s| !s.is_empty())
}

/// Lay node `node` out on the canvas.
fn lay(
    node: &Node,
    graph: &Graph,
    design: &Design,
    owner: Option<&NodeDef>,
    ev: &Evaluation,
    scope: &[NodeId],
) -> Laid {
    let defs = &design.defs;
    let subtitle = subtitle(&node.kind, design);
    let mut rows = Vec::new();
    let mut y = HEADER_H + if subtitle.is_some() { ROW_H } else { 0.0 } + ROW_H / 2.0 + 2.0;
    for o in node.kind.outputs(defs, owner) {
        rows.push(Row {
            side: Side::Out(o.key),
            ty: o.ty,
            label: o.label,
            value: None,
            edit: None,
            unit: "",
            scrub: Scrub::PLAIN,
            y,
            new: false,
        });
        y += ROW_H;
    }
    if matches!(node.kind, NodeKind::GroupInput) && owner.is_some() {
        rows.push(Row {
            side: Side::Out(OutKey::Port(NEW_PORT)),
            ty: SocketType::Set,
            label: String::new(),
            value: None,
            edit: None,
            unit: "",
            scrub: Scrub::PLAIN,
            y,
            new: true,
        });
        y += ROW_H;
    }
    for i in node.kind.inputs(defs, owner) {
        let wired = graph.link_into(node.id, i.key).is_some();
        let unit = if i.angle {
            "°"
        } else if i.length {
            " Å"
        } else {
            ""
        };
        let edit = (!wired && matches!(i.ty, SocketType::Number | SocketType::Vec3))
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
        rows.push(Row {
            side: Side::In(i.key),
            ty: i.ty,
            label: i.label,
            value: edit.map(|v| value_text(v, unit)),
            edit,
            unit,
            scrub: if matches!(i.key, InKey::Count)
                || matches!(i.key, InKey::Field(f) if f.is_count())
            {
                Scrub::COUNT
            } else if i.angle {
                Scrub::ANGLE
            } else if i.length {
                Scrub::LENGTH
            } else {
                Scrub::PLAIN
            },
            y,
            new: false,
        });
        y += ROW_H;
    }
    let extra_in = match &node.kind {
        NodeKind::Merge { inputs } => Some(InKey::Set(*inputs)),
        NodeKind::GroupOutput if owner.is_some() => Some(InKey::Port(NEW_PORT)),
        _ => None,
    };
    if let Some(key) = extra_in {
        rows.push(Row {
            side: Side::In(key),
            ty: SocketType::Set,
            label: String::new(),
            value: None,
            edit: None,
            unit: "",
            scrub: Scrub::PLAIN,
            y,
            new: true,
        });
        y += ROW_H;
    }
    let height = y - ROW_H / 2.0 + 6.0;
    let issue = |list: &[crate::assembly::graph::Issue]| {
        list.iter()
            .find(|i| i.node == node.id && i.scope == scope)
            .map(|i| i.message.clone())
    };
    Laid {
        id: node.id,
        rect: Rect::from_min_size(pos2(node.pos[0], node.pos[1]), vec2(NODE_W, height)),
        title: if node.label.is_empty() {
            node.kind.title(defs)
        } else {
            node.label.clone()
        },
        own: match node.kind {
            NodeKind::Value { value } => Some(PortValue::Number(value)),
            NodeKind::Vector { value } => Some(PortValue::Vec3(value)),
            _ => None,
        },
        subtitle,
        rows,
        category: node.kind.category(),
        bypass: node.bypass,
        reached: ev.reached(scope, node.id),
        error: issue(&ev.errors),
        warning: issue(&ev.warnings),
    }
}

/// The zoom from which unwired values are drawn as boxes to type into.
const BOX_ZOOM: f32 = 0.6;

/// Where the box of an unwired input's value goes, the node being at `r` on the screen.
fn value_box(r: Rect, row: &Row, z: f32) -> Rect {
    let width = if row.ty == SocketType::Vec3 {
        96.0
    } else {
        58.0
    };
    let unit = if row.unit.is_empty() { 0.0 } else { 12.0 };
    let right = r.right() - (6.0 + unit) * z;
    Rect::from_min_max(
        pos2(right - width * z, r.top() + (row.y - ROW_H / 2.0 + 1.5) * z),
        pos2(right, r.top() + (row.y + ROW_H / 2.0 - 1.5) * z),
    )
}

/// Where the box of a number or triple node's value goes (the subtitle's line).
fn own_box(r: Rect, z: f32) -> Rect {
    let top = r.top() + (HEADER_H + 1.5) * z;
    Rect::from_min_max(
        pos2(r.left() + 7.0 * z, top),
        pos2(r.right() - 7.0 * z, top + (ROW_H - 3.0) * z),
    )
}

/// A value typed in: a number or, for a triple, three numbers separated by commas (or one
/// for all three). Each may be an expression without names, such as `360/7`.
fn parse_value(text: &str, ty: SocketType) -> Option<PortValue> {
    let number = |s: &str| eval_const(s).filter(|x| x.is_finite());
    if ty != SocketType::Vec3 {
        return number(text).map(PortValue::Number);
    }
    // Commas inside brackets (`max(1, 2)`) belong to a function.
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    match parts.as_slice() {
        [one] => number(one).map(|x| PortValue::Vec3([x; 3])),
        [x, y, z] => Some(PortValue::Vec3([number(x)?, number(y)?, number(z)?])),
        _ => None,
    }
}

/// How dragging a value box changes the value: by `speed` a point, in steps of `grid`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Scrub {
    speed: f64,
    grid: f64,
}

impl Scrub {
    const COUNT: Scrub = Scrub {
        speed: 0.1,
        grid: 1.0,
    };
    const ANGLE: Scrub = Scrub {
        speed: 0.5,
        grid: 0.5,
    };
    const LENGTH: Scrub = Scrub {
        speed: 0.05,
        grid: 0.05,
    };
    const PLAIN: Scrub = Scrub {
        speed: 0.01,
        grid: 0.01,
    };

    /// The change for `points` of drag (finer with Ctrl).
    fn by(self, points: f32, fine: bool) -> f64 {
        let (speed, grid) = if fine && self.grid < 1.0 {
            (self.speed / 10.0, self.grid / 10.0)
        } else {
            (self.speed, self.grid)
        };
        (f64::from(points) * speed / grid).round() * grid
    }
}

/// `start` moved by a drag of `drag` (screen points, y down): a number by its x; a triple's
/// x by the drag's x and y by its upward y, or z instead of y with Shift.
fn scrubbed(start: PortValue, drag: Vec2, scrub: Scrub, shift: bool, fine: bool) -> PortValue {
    match start {
        PortValue::Number(v) => PortValue::Number(v + scrub.by(drag.x, fine)),
        PortValue::Vec3([x, y, z]) => {
            let up = scrub.by(-drag.y, fine);
            let x = x + scrub.by(drag.x, fine);
            if shift {
                PortValue::Vec3([x, y, z + up])
            } else {
                PortValue::Vec3([x, y + up, z])
            }
        }
    }
}

/// What a value box did this frame.
#[derive(Debug, Default)]
struct BoxResult {
    /// A value typed in (Enter, a click elsewhere).
    set: Option<PortValue>,
    /// A value being dragged (joins the history when the button is let go).
    drag: Option<PortValue>,
    /// The box was taken hold of (to type in or to drag).
    touched: bool,
}

/// The box of a value on the canvas. Dragging it changes the value (a triple: x left and
/// right, y up and down, z up and down with Shift; finer with Ctrl); a click turns it into a
/// field to type in, kept while it has the focus and set when it lets go of it (Enter, a
/// click elsewhere); Esc puts the value back.
fn value_field(
    ui: &mut Ui,
    id: egui::Id,
    rect: Rect,
    value: PortValue,
    scrub: Scrub,
    z: f32,
) -> BoxResult {
    let ty = match value {
        PortValue::Number(_) => SocketType::Number,
        PortValue::Vec3(_) => SocketType::Vec3,
    };
    let tip = match ty {
        SocketType::Vec3 => t!("assembly.editor.vector_tip"),
        _ => t!("assembly.editor.value_tip"),
    };
    let font = FontId::monospace(10.5 * z);
    let typing_id = id.with("typing");
    let typing =
        ui.memory(|m| m.has_focus(id)) || ui.data(|d| d.get_temp(typing_id).unwrap_or(false));
    let mut out = BoxResult::default();
    if !typing {
        let response = ui
            .interact(rect, id.with("drag"), Sense::click_and_drag())
            .on_hover_text(tip);
        let start_id = id.with("start");
        if response.drag_started_by(PointerButton::Primary) {
            ui.data_mut(|d| d.insert_temp(start_id, value));
            out.touched = true;
        }
        if response.dragged_by(PointerButton::Primary)
            && let Some(start) = ui.data(|d| d.get_temp::<PortValue>(start_id))
            && let Some((from, now)) =
                ui.input(|i| Some((i.pointer.press_origin()?, i.pointer.interact_pos()?)))
        {
            let (shift, fine) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
            let next = scrubbed(start, now - from, scrub, shift, fine);
            if next != value {
                out.drag = Some(next);
            }
        }
        if response.drag_stopped() {
            ui.data_mut(|d| d.remove::<PortValue>(start_id));
        }
        if response.clicked() {
            ui.data_mut(|d| d.insert_temp(typing_id, true));
            ui.memory_mut(|m| m.request_focus(id));
            out.touched = true;
        }
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(match ty {
                SocketType::Vec3 => egui::CursorIcon::AllScroll,
                _ => egui::CursorIcon::ResizeHorizontal,
            });
        }
        let active = response.hovered() || response.dragged();
        let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
        let radius = CornerRadius::same((3.0 * z).round().clamp(1.0, 4.0) as u8);
        let back = if active {
            dark::RAISED.gamma_multiply(1.4)
        } else {
            dark::CANVAS_BACK
        };
        painter.rect(
            rect,
            radius,
            back,
            Stroke::new(1.0, if active { dark::MUTED } else { dark::BORDER }),
            StrokeKind::Inside,
        );
        painter.text(
            rect.right_center() - vec2(3.0 * z, 0.0),
            Align2::RIGHT_CENTER,
            value_text(value, ""),
            font,
            dark::TEXT,
        );
        return out;
    }
    let shown = value_text(value, "");
    let mut text = if ui.memory(|m| m.has_focus(id)) {
        ui.data_mut(|d| d.get_temp::<String>(id))
            .unwrap_or_else(|| shown.clone())
    } else {
        shown
    };
    let color = if parse_value(&text, ty).is_some() {
        dark::TEXT
    } else {
        ERROR
    };
    let response = ui
        .put(
            rect,
            egui::TextEdit::singleline(&mut text)
                .id(id)
                .font(font)
                .text_color(color)
                .background_color(dark::CANVAS_BACK)
                .align(Align2::RIGHT_CENTER)
                .margin(egui::Margin::symmetric((3.0 * z).round() as i8, 0))
                .min_size(rect.size())
                .desired_width(rect.width()),
        )
        .on_hover_text(tip);
    if response.has_focus() {
        // Focused now: the flag that asked for it has done its work.
        ui.data_mut(|d| {
            d.remove::<bool>(typing_id);
            d.insert_temp(id, text);
        });
    } else if response.lost_focus() {
        ui.data_mut(|d| d.remove::<String>(id));
        let cancel = ui.input(|i| i.key_pressed(Key::Escape));
        if !cancel
            && let Some(v) = parse_value(&text, ty)
            && v != value
        {
            out.set = Some(v);
        }
    } else if !ui.memory(|m| m.has_focus(id))
        && ui.data(|d| d.get_temp::<bool>(typing_id).is_some())
        && ui.input(|i| i.pointer.any_pressed())
    {
        // Asked for the focus but a click went elsewhere first.
        ui.data_mut(|d| d.remove::<bool>(typing_id));
    }
    out
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

const WARN: Color32 = Color32::from_rgb(0xE8, 0xB3, 0x3C);
const ERROR: Color32 = Color32::from_rgb(0xF2, 0x6D, 0x5E);

/// The screen transform of a camera over `area`.
#[derive(Clone, Copy)]
struct View {
    origin: Pos2,
    zoom: f32,
}

impl View {
    fn of(area: Rect, cam: Camera) -> View {
        View {
            origin: area.min + cam.pan,
            zoom: cam.zoom,
        }
    }

    fn screen(&self, p: Pos2) -> Pos2 {
        self.origin + p.to_vec2() * self.zoom
    }

    fn rect(&self, r: Rect) -> Rect {
        Rect::from_min_max(self.screen(r.min), self.screen(r.max))
    }

    fn canvas(&self, p: Pos2) -> Pos2 {
        ((p - self.origin) / self.zoom).to_pos2()
    }
}

/// The editor: a small bar (where it is, add, group, frame, maximise, close) over the canvas.
/// Returns the edits made.
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
        if crumb(
            ui,
            t!("assembly.editor.top").into(),
            view.rt.scope.is_empty(),
        )
        .clicked()
        {
            view.rt.scope.clear();
            view.rt.selected.clear();
        }
        let design = &state.design;
        let mut back_to = None;
        let mut graph = design.effective_graph().into_owned();
        for (k, id) in view.rt.scope.iter().enumerate() {
            let Some(NodeKind::Group(g)) = graph.node(*id).map(|n| &n.kind) else {
                break;
            };
            let Some(def) = defs::find(&design.defs, &g.def) else {
                break;
            };
            ui.label(RichText::new("›").size(12.0).color(muted));
            if crumb(ui, def.name.clone(), k + 1 == view.rt.scope.len()).clicked() {
                back_to = Some(k + 1);
            }
            graph = def.graph.clone();
        }
        if let Some(k) = back_to {
            view.rt.scope.truncate(k);
            view.rt.selected.clear();
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
            if ui
                .small_button(t!("assembly.editor.frame"))
                .on_hover_text(t!("assembly.editor.frame_tip"))
                .clicked()
            {
                let key = (state.lineage(), view.rt.scope.clone());
                view.rt.cameras.remove(&key);
            }
            let groupable = !view.rt.selected.is_empty();
            if ui
                .add_enabled(
                    groupable,
                    egui::Button::new(t!("assembly.editor.group")).small(),
                )
                .on_hover_text(t!("assembly.editor.group_tip"))
                .clicked()
            {
                edits.push(GraphEdit::Group(view.rt.selected.clone()));
            }
            if ui
                .small_button(t!("assembly.editor.add"))
                .on_hover_text(t!("assembly.editor.add_tip"))
                .clicked()
            {
                view.rt.menu = Some(AddMenu {
                    screen: ui.max_rect().left_bottom() + vec2(0.0, 4.0),
                    canvas: pos2(40.0, 40.0),
                    query: String::new(),
                    from: None,
                    focus: true,
                });
            }
            if let Some((text, problem)) = &view.rt.report {
                let color = if *problem { ERROR } else { muted };
                ui.add(egui::Label::new(RichText::new(text).size(11.0).color(color)).truncate());
            }
        });
    });
}

/// The canvas: nodes and wires, with the pointer and the keys.
fn canvas(
    ui: &mut Ui,
    state: &mut AssemblyState,
    view: &mut NodeEditorView,
    edits: &mut Vec<GraphEdit>,
) {
    let area = ui.available_rect_before_wrap();
    let bg = ui.allocate_rect(area, Sense::click_and_drag());
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 0.0, dark::CANVAS_BACK);

    let design = &state.design;
    if design.components.is_empty() {
        painter.text(
            area.center(),
            Align2::CENTER_CENTER,
            t!("assembly.editor.empty_hint"),
            FontId::proportional(12.0),
            dark::MUTED,
        );
        return;
    }
    let ev = design.evaluation();
    let Some((graph, owner)) = access::graph_at(design, &view.rt.scope) else {
        return;
    };
    let scope = view.rt.scope.clone();
    let laid: Vec<Laid> = graph
        .nodes
        .iter()
        .map(|n| lay(n, &graph, design, owner, &ev, &scope))
        .collect();

    let cam_key = (state.lineage(), scope.clone());
    let cam = *view
        .rt
        .cameras
        .entry(cam_key.clone())
        .or_insert_with(|| frame_all(&laid, area));
    let mut cam_next = cam;
    let v = View::of(area, cam);
    let hovered = ui.rect_contains_pointer(area);
    let pointer = ui.input(|i| i.pointer.interact_pos());

    grid(&painter, area, v);

    // Wires.
    let find = |id: NodeId| laid.iter().find(|l| l.id == id);
    for link in &graph.links {
        let (Some(a), Some(b)) = (find(link.from), find(link.to)) else {
            continue;
        };
        let (Some((p0, ty)), Some((p1, _))) = (
            a.socket(Side::Out(link.out)),
            b.socket(Side::In(link.input)),
        ) else {
            continue;
        };
        let dim = !a.reached || a.bypass && matches!(link.out, OutKey::Rest);
        let color = if dim {
            socket_color(ty).gamma_multiply(0.35)
        } else {
            socket_color(ty)
        };
        let (s0, s1) = (v.screen(p0), v.screen(p1));
        wire(&painter, s0, s1, color, v.zoom);
        let shown = ev.shown.get(&(scope.clone(), link.from, link.out));
        let text = match shown {
            Some(Shown::Count(n)) if matches!(ty, SocketType::Set | SocketType::Points) => {
                Some(format!("{n}"))
            }
            Some(Shown::Field) => Some("ƒ".to_owned()),
            Some(Shown::Number(x)) => Some(number_text(*x)),
            _ => None,
        };
        if let Some(text) = text
            && v.zoom > 0.45
        {
            let mid = s0 + (s1 - s0) * 0.5;
            let galley =
                painter.layout_no_wrap(text, FontId::monospace(10.0 * v.zoom.min(1.2)), dark::TEXT);
            let r = Rect::from_center_size(mid, galley.size() + vec2(8.0, 2.0));
            painter.rect_filled(r, 6.0, dark::RAISED);
            painter.galley(r.center() - galley.size() / 2.0, galley, dark::TEXT);
        }
    }

    // Nodes: unselected first, so the selection is drawn on top.
    let mut order: Vec<&Laid> = laid.iter().collect();
    order.sort_by_key(|l| view.rt.selected.contains(&l.id));
    let mut node_hits = Vec::new();
    for l in &order {
        let selected = view.rt.selected.contains(&l.id);
        draw_node(&painter, l, v, selected, v.zoom >= BOX_ZOOM);
        let r = ui.interact(
            v.rect(l.rect),
            ui.scope_id().with(("node", l.id)),
            Sense::click_and_drag(),
        );
        node_hits.push((l.id, r));
    }
    let mut socket_hits = Vec::new();
    for l in &order {
        for row in &l.rows {
            let Some((p, _)) = l.socket(row.side) else {
                continue;
            };
            let r = ui.interact(
                Rect::from_center_size(v.screen(p), Vec2::splat(SOCKET_HIT * 1.4)),
                ui.scope_id()
                    .with(("socket", l.id, format!("{:?}", row.side))),
                Sense::drag(),
            );
            socket_hits.push((l.id, row.side, row.ty, r));
        }
    }

    // Unwired values: boxes to type into.
    if v.zoom >= BOX_ZOOM {
        let mut boxes = ui.new_child(egui::UiBuilder::new().max_rect(area));
        boxes.set_clip_rect(area.intersect(ui.clip_rect()));
        // The theme keeps a text field at least a button high: the boxes fit their rows.
        boxes.spacing_mut().interact_size.y = 0.0;
        let base = ui.scope_id().with(("value", &scope));
        for l in &order {
            let r = v.rect(l.rect);
            let mut fields: Vec<(InKey, Rect, PortValue, Scrub)> = Vec::new();
            if let Some(own) = l.own {
                fields.push((InKey::Value, own_box(r, v.zoom), own, Scrub::PLAIN));
            }
            for row in &l.rows {
                if let (Some(value), Side::In(key)) = (row.edit, row.side) {
                    fields.push((key, value_box(r, row, v.zoom), value, row.scrub));
                }
            }
            for (key, rect, value, scrub) in fields {
                let id = base.with((l.id, format!("{key:?}")));
                let done = value_field(&mut boxes, id, rect, value, scrub, v.zoom);
                if let Some(new) = done.set {
                    edits.push(GraphEdit::SetValue(l.id, key, new));
                }
                if let Some(new) = done.drag {
                    edits.push(GraphEdit::DragValue(l.id, key, new));
                }
                if done.touched && !view.rt.selected.contains(&l.id) {
                    view.rt.selected = [l.id].into();
                    view.rt.menu = None;
                }
            }
        }
    }

    let shift = ui.input(|i| i.modifiers.shift);
    // Nodes: click to select, drag to move, double-click a custom node to go in.
    for (id, r) in &node_hits {
        if r.clicked() {
            if shift {
                if !view.rt.selected.remove(id) {
                    view.rt.selected.insert(*id);
                }
            } else {
                view.rt.selected = [*id].into();
            }
            view.rt.menu = None;
        }
        if r.double_clicked()
            && let Some(n) = graph.node(*id)
            && matches!(n.kind, NodeKind::Group(_))
        {
            view.rt.scope.push(*id);
            view.rt.selected.clear();
        }
        if r.drag_started_by(PointerButton::Primary) {
            if !view.rt.selected.contains(id) {
                if !shift {
                    view.rt.selected.clear();
                }
                view.rt.selected.insert(*id);
            }
            view.rt.drag = Some(Drag::Nodes);
        }
        if r.dragged_by(PointerButton::Primary) && view.rt.drag == Some(Drag::Nodes) {
            let delta = r.drag_delta() / v.zoom;
            if delta != Vec2::ZERO {
                edits.push(GraphEdit::Move(
                    view.rt.selected.iter().copied().collect(),
                    delta,
                ));
            }
        }
        if r.drag_stopped() && view.rt.drag == Some(Drag::Nodes) {
            view.rt.drag = None;
        }
    }
    // Sockets: drag to wire; dragging a wired input takes its wire off.
    for (id, side, ty, r) in &socket_hits {
        if r.drag_started() {
            view.rt.drag = Some(Drag::Wire(match *side {
                Side::Out(out) => End::Out(*id, out, *ty),
                Side::In(input) => match graph.link_into(*id, input) {
                    Some(link) => {
                        let from_ty = find(link.from)
                            .and_then(|l| l.socket(Side::Out(link.out)))
                            .map_or(*ty, |(_, t)| t);
                        edits.push(GraphEdit::Disconnect(*id, input));
                        End::Out(link.from, link.out, from_ty)
                    }
                    None => End::In(*id, input, *ty),
                },
            }));
        }
    }
    // A wire being drawn: follow the pointer, and on release wire it to the socket under it
    // (or offer the nodes that could take it).
    if let Some(Drag::Wire(end)) = view.rt.drag {
        let target = pointer.unwrap_or(area.center());
        let (start, ty) = match end {
            End::Out(n, k, ty) => (
                find(n).and_then(|l| l.socket(Side::Out(k))).map(|(p, _)| p),
                ty,
            ),
            End::In(n, k, ty) => (
                find(n).and_then(|l| l.socket(Side::In(k))).map(|(p, _)| p),
                ty,
            ),
        };
        if let Some(start) = start {
            let s = v.screen(start);
            match end {
                End::Out(..) => wire(&painter, s, target, socket_color(ty), v.zoom),
                End::In(..) => wire(&painter, target, s, socket_color(ty), v.zoom),
            }
        }
        if !ui.input(|i| i.pointer.any_down()) {
            view.rt.drag = None;
            let hit = laid.iter().find_map(|l| {
                l.rows.iter().find_map(|row| {
                    let (p, rty) = l.socket(row.side)?;
                    (v.screen(p).distance(target) <= SOCKET_HIT).then_some((l.id, row.side, rty))
                })
            });
            match (end, hit) {
                (End::Out(from, out, _), Some((to, Side::In(input), _))) => {
                    edits.push(GraphEdit::Connect(End2 {
                        from,
                        out,
                        to,
                        input,
                    }));
                }
                (End::In(to, input, _), Some((from, Side::Out(out), _))) => {
                    edits.push(GraphEdit::Connect(End2 {
                        from,
                        out,
                        to,
                        input,
                    }));
                }
                (_, None) if area.contains(target) => {
                    view.rt.menu = Some(AddMenu {
                        screen: target,
                        canvas: v.canvas(target),
                        query: String::new(),
                        from: Some(end),
                        focus: true,
                    });
                }
                _ => {}
            }
        } else {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }

    // The background: click to let go, drag a box to select, right- or middle-drag to pan,
    // right-click to add, the wheel to zoom.
    if bg.clicked() {
        view.rt.selected.clear();
        view.rt.menu = None;
    }
    if bg.drag_started_by(PointerButton::Primary)
        && let Some(p) = pointer
    {
        view.rt.drag = Some(Drag::Box(p));
    }
    if bg.dragged_by(PointerButton::Secondary) || bg.dragged_by(PointerButton::Middle) {
        cam_next.pan += bg.drag_delta();
    }
    if bg.secondary_clicked()
        && let Some(p) = pointer
    {
        view.rt.menu = Some(AddMenu {
            screen: p,
            canvas: v.canvas(p),
            query: String::new(),
            from: None,
            focus: true,
        });
    }
    if let Some(Drag::Box(start)) = view.rt.drag {
        let now = pointer.unwrap_or(start);
        let r = Rect::from_two_pos(start, now);
        painter.rect(
            r,
            2.0,
            crate::theme::ORANGE_ON_DARK.gamma_multiply(0.08),
            Stroke::new(1.0, crate::theme::ORANGE_ON_DARK),
            StrokeKind::Inside,
        );
        if !ui.input(|i| i.pointer.any_down()) {
            view.rt.drag = None;
            if !shift {
                view.rt.selected.clear();
            }
            for l in &laid {
                if r.intersects(v.rect(l.rect)) {
                    view.rt.selected.insert(l.id);
                }
            }
        }
    }
    if hovered {
        let (scroll, zoom_delta) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        let factor = zoom_delta * (scroll.y / 400.0).exp();
        if (factor - 1.0).abs() > 1e-4
            && let Some(p) = pointer
        {
            let zoom = (cam.zoom * factor).clamp(0.25, 2.5);
            let at = v.canvas(p);
            cam_next.zoom = zoom;
            cam_next.pan = p - area.min - at.to_vec2() * zoom;
        }
    }

    // Keys, with the pointer over the editor (taken before the viewer's own).
    // Tab is also egui's key for moving the focus: only a text field keeps the keys.
    if hovered && !ui.ctx().text_edit_focused() && view.rt.menu.is_none() {
        keys(ui, state, view, &graph, &laid, area, pointer, edits);
    }
    if view.rt.cameras.get(&cam_key) == Some(&cam) {
        view.rt.cameras.insert(cam_key, cam_next);
    }
    if view.rt.selected.iter().any(|id| graph.node(*id).is_none()) {
        view.rt.selected.retain(|id| graph.node(*id).is_some());
    }

    if let Some(menu) = view.rt.menu.take() {
        view.rt.menu = add_menu(
            ui,
            state,
            &mut view.rt,
            menu,
            &graph,
            owner.is_some(),
            edits,
        );
    }

    if graph.nodes.is_empty() {
        painter.text(
            area.center(),
            Align2::CENTER_CENTER,
            t!("assembly.editor.empty_hint"),
            FontId::proportional(12.0),
            dark::MUTED,
        );
    }
}

/// The keys the editor takes while the pointer is over it.
#[allow(clippy::too_many_arguments)]
fn keys(
    ui: &Ui,
    state: &AssemblyState,
    view: &mut NodeEditorView,
    graph: &Graph,
    laid: &[Laid],
    area: Rect,
    pointer: Option<Pos2>,
    edits: &mut Vec<GraphEdit>,
) {
    let cmd = Modifiers::COMMAND;
    let (delete, duplicate, bypass, group, ungroup, tab, home, escape, add, all) =
        ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Delete)
                    || i.consume_key(Modifiers::NONE, Key::X)
                    || i.consume_key(Modifiers::NONE, Key::Backspace),
                i.consume_key(cmd, Key::D),
                i.consume_key(Modifiers::NONE, Key::M),
                i.consume_key(cmd, Key::G),
                i.consume_key(cmd | Modifiers::ALT, Key::G),
                i.consume_key(Modifiers::NONE, Key::Tab),
                i.consume_key(Modifiers::NONE, Key::Home),
                i.consume_key(Modifiers::NONE, Key::Escape),
                i.consume_key(Modifiers::SHIFT, Key::A),
                i.consume_key(cmd, Key::A),
            )
        });
    let selected = view.rt.selected.clone();
    if delete && !selected.is_empty() {
        edits.push(GraphEdit::Remove(selected.clone()));
    }
    if duplicate && !selected.is_empty() {
        edits.push(GraphEdit::Duplicate(selected.clone()));
    }
    if bypass && !selected.is_empty() {
        edits.push(GraphEdit::Bypass(selected.clone()));
    }
    if ungroup {
        if let Some(id) = view.rt.single() {
            edits.push(GraphEdit::Ungroup(id));
        }
    } else if group && !selected.is_empty() {
        edits.push(GraphEdit::Group(selected.clone()));
    }
    if tab {
        // egui moved the focus on Tab as the frame began: give it back.
        ui.ctx().memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
        match view.rt.single().and_then(|id| graph.node(id)) {
            Some(n) if matches!(n.kind, NodeKind::Group(_)) => {
                view.rt.scope.push(n.id);
                view.rt.selected.clear();
            }
            _ => {
                if let Some(id) = view.rt.scope.pop() {
                    view.rt.selected = [id].into();
                }
            }
        }
    }
    if home {
        view.rt.cameras.insert(
            (state.lineage(), view.rt.scope.clone()),
            frame_all(laid, area),
        );
    }
    if escape {
        if view.rt.drag.take().is_none() && view.rt.selected.is_empty() {
            if let Some(id) = view.rt.scope.pop() {
                view.rt.selected = [id].into();
            }
        } else {
            view.rt.selected.clear();
        }
    }
    if add && let Some(p) = pointer {
        let cam = view
            .rt
            .cameras
            .get(&(state.lineage(), view.rt.scope.clone()))
            .copied()
            .unwrap_or_default();
        view.rt.menu = Some(AddMenu {
            screen: p,
            canvas: View::of(area, cam).canvas(p),
            query: String::new(),
            from: None,
            focus: true,
        });
    }
    if all {
        view.rt.selected = graph.nodes.iter().map(|n| n.id).collect();
    }
}

/// A camera showing every node of `laid` in `area` (at most life size).
fn frame_all(laid: &[Laid], area: Rect) -> Camera {
    let Some(bounds) = laid.iter().map(|l| l.rect).reduce(|a, b| a.union(b)) else {
        return Camera::default();
    };
    let bounds = bounds.expand(24.0);
    let zoom = (area.width() / bounds.width())
        .min(area.height() / bounds.height())
        .clamp(0.25, 1.0);
    let centre = bounds.center().to_vec2() * zoom;
    Camera {
        pan: area.size() / 2.0 - centre,
        zoom,
    }
}

fn grid(painter: &egui::Painter, area: Rect, v: View) {
    let step = 40.0 * v.zoom;
    if step < 8.0 {
        return;
    }
    let color = dark::BORDER.gamma_multiply(0.5);
    let start = v.origin - area.min;
    let mut x = area.left() + start.x.rem_euclid(step);
    while x < area.right() {
        painter.vline(x, area.y_range(), Stroke::new(1.0, color));
        x += step;
    }
    let mut y = area.top() + start.y.rem_euclid(step);
    while y < area.bottom() {
        painter.hline(area.x_range(), y, Stroke::new(1.0, color));
        y += step;
    }
}

fn wire(painter: &egui::Painter, a: Pos2, b: Pos2, color: Color32, zoom: f32) {
    let d = ((b.x - a.x).abs() * 0.5).max(40.0 * zoom);
    let shape = CubicBezierShape::from_points_stroke(
        [a, a + vec2(d, 0.0), b - vec2(d, 0.0), b],
        false,
        Color32::TRANSPARENT,
        Stroke::new(2.0 * zoom.clamp(0.6, 1.4), color),
    );
    painter.add(shape);
}

fn draw_node(painter: &egui::Painter, l: &Laid, v: View, selected: bool, boxes: bool) {
    let r = v.rect(l.rect);
    let z = v.zoom;
    let radius = CornerRadius::same((6.0 * z).round().clamp(1.0, 10.0) as u8);
    let alpha = if l.reached { 1.0 } else { 0.55 };
    let body = dark::RAISED.gamma_multiply(alpha);
    let border = if selected {
        Stroke::new(2.0, crate::theme::ORANGE_ON_DARK)
    } else if l.error.is_some() {
        Stroke::new(1.5, ERROR)
    } else {
        Stroke::new(1.0, dark::BORDER)
    };
    painter.rect(r, radius, body, Stroke::NONE, StrokeKind::Inside);
    let head = Rect::from_min_size(r.min, vec2(r.width(), HEADER_H * z));
    let mut head_color = header_color(l.category);
    if l.bypass {
        head_color = Color32::from_rgb(0x4A, 0x45, 0x40);
    }
    painter.rect_filled(
        head,
        CornerRadius {
            nw: radius.nw,
            ne: radius.ne,
            sw: 0,
            se: 0,
        },
        head_color.gamma_multiply(alpha),
    );
    painter.rect_stroke(r, radius, border, StrokeKind::Inside);
    if z < 0.35 {
        return;
    }
    let text = |pos: Pos2, align: Align2, s: &str, size: f32, color: Color32| {
        painter.text(pos, align, s, FontId::proportional(size * z), color);
    };
    let title_color = if l.bypass { dark::MUTED } else { dark::TEXT };
    let mut title = l.title.clone();
    if l.bypass {
        title = format!("{title} ⏸");
    }
    let marker = if l.error.is_some() {
        Some(("⚠", ERROR))
    } else if l.warning.is_some() {
        Some(("⚠", WARN))
    } else {
        None
    };
    painter.with_clip_rect(head.shrink(2.0)).text(
        head.left_center() + vec2(7.0 * z, 0.0),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(12.0 * z),
        title_color,
    );
    if let Some((m, c)) = marker {
        text(
            head.right_center() - vec2(7.0 * z, 0.0),
            Align2::RIGHT_CENTER,
            m,
            12.0,
            c,
        );
    }
    if let Some(sub) = l.subtitle.as_ref().filter(|_| !(boxes && l.own.is_some())) {
        painter.with_clip_rect(r.shrink(3.0)).text(
            pos2(r.left() + 7.0 * z, head.bottom() + ROW_H * z / 2.0 + 1.0),
            Align2::LEFT_CENTER,
            sub,
            FontId::monospace(10.5 * z),
            dark::MUTED,
        );
    }
    for row in &l.rows {
        let y = r.top() + row.y * z;
        let (sx, align, lx) = match row.side {
            Side::Out(_) => (r.right(), Align2::RIGHT_CENTER, r.right() - 10.0 * z),
            Side::In(_) => (r.left(), Align2::LEFT_CENTER, r.left() + 10.0 * z),
        };
        let c = socket_color(row.ty);
        let p = pos2(sx, y);
        if row.new {
            painter.circle_stroke(p, SOCKET_R * z, Stroke::new(1.2, c.gamma_multiply(0.7)));
            text(pos2(lx, y), align, "+", 11.0, dark::MUTED);
            continue;
        }
        match row.ty {
            SocketType::Set | SocketType::Curve | SocketType::Points => {
                painter.circle_filled(p, SOCKET_R * z, c);
            }
            SocketType::Number => {
                painter.circle_filled(p, (SOCKET_R - 0.5) * z, c);
            }
            SocketType::Vec3 => {
                painter.rect_filled(
                    Rect::from_center_size(p, Vec2::splat(2.0 * (SOCKET_R - 0.5) * z)),
                    1.0,
                    c,
                );
            }
        }
        let typed = boxes && row.edit.is_some();
        let mut clip = r.shrink(2.0);
        if typed {
            clip.max.x = value_box(r, row, z).left() - 3.0 * z;
        }
        painter.with_clip_rect(clip).text(
            pos2(lx, y),
            align,
            &row.label,
            FontId::proportional(11.0 * z),
            dark::TEXT.gamma_multiply(0.9),
        );
        let clip = painter.with_clip_rect(r.shrink(2.0));
        if typed {
            clip.text(
                pos2(r.right() - 5.0 * z, y),
                Align2::RIGHT_CENTER,
                row.unit.trim(),
                FontId::proportional(10.5 * z),
                dark::MUTED,
            );
        } else if let Some(value) = &row.value {
            clip.text(
                pos2(r.right() - 8.0 * z, y),
                Align2::RIGHT_CENTER,
                value,
                FontId::monospace(10.5 * z),
                dark::MUTED,
            );
        }
    }
}

/// One entry of the add menu.
struct Entry {
    label: String,
    group: &'static str,
    make: Make,
}

enum Make {
    Kind(NodeKind),
    Library(Box<library::Entry>),
}

/// The add menu, at the pointer: a search field and the nodes that match. Returns the menu
/// while it stays open.
fn add_menu(
    ui: &mut Ui,
    state: &AssemblyState,
    rt: &mut Runtime,
    mut menu: AddMenu,
    graph: &Graph,
    inside: bool,
    edits: &mut Vec<GraphEdit>,
) -> Option<AddMenu> {
    let design = &state.design;
    let mut entries: Vec<Entry> = Vec::new();
    if !inside {
        for (i, c) in design.components.iter().enumerate() {
            entries.push(Entry {
                label: format!("{} · {}", t!("assembly.node.monomer"), c.name),
                group: t!("assembly.editor.menu.input"),
                make: Make::Kind(NodeKind::Monomer {
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
        entries.push(Entry {
            label: kind.title(&design.defs),
            group,
            make: Make::Kind(kind),
        });
    }
    let current = access::scope_def(design, &rt.scope).flatten();
    for def in &design.defs {
        // Not inside itself, nor inside one it uses.
        if current
            .as_ref()
            .is_some_and(|c| defs::depends_on(&design.defs, &def.id, c))
        {
            continue;
        }
        entries.push(Entry {
            label: def.name.clone(),
            group: t!("assembly.editor.menu.custom"),
            make: Make::Kind(NodeKind::Group(GroupRef {
                def: def.id.clone(),
                inputs: Default::default(),
            })),
        });
    }
    for entry in rt.library() {
        if defs::find(&design.defs, &entry.def.id).is_some() {
            continue;
        }
        entries.push(Entry {
            label: entry.def.name.clone(),
            group: t!("assembly.editor.menu.library"),
            make: Make::Library(Box::new(entry.clone())),
        });
    }
    // With a wire: only nodes that can take it.
    if let Some(end) = menu.from {
        entries.retain(|e| match &e.make {
            Make::Kind(kind) => wire_for(kind, &design.defs, end).is_some(),
            Make::Library(entry) => {
                let kind = NodeKind::Group(GroupRef {
                    def: entry.def.id.clone(),
                    inputs: Default::default(),
                });
                let mut all = entry.deps.clone();
                all.push(entry.def.clone());
                wire_for(&kind, &all, end).is_some()
            }
        });
    }
    let query = menu.query.to_lowercase();
    entries.retain(|e| query.is_empty() || e.label.to_lowercase().contains(&query));

    let mut close = false;
    let opening = menu.focus;
    let mut chosen = None;
    let area = egui::Area::new(ui.scope_id().with("node_add_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(menu.screen)
        .constrain(true)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(200.0);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut menu.query)
                        .hint_text(t!("assembly.editor.search"))
                        .desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut menu.focus) {
                    field.request_focus();
                }
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if ui.input(|i| i.key_pressed(Key::Escape)) {
                    close = true;
                }
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        let mut last_group = "";
                        for (k, e) in entries.iter().enumerate() {
                            if e.group != last_group {
                                last_group = e.group;
                                ui.label(RichText::new(e.group).size(10.5).color(dark::MUTED));
                            }
                            if ui.selectable_label(false, &e.label).clicked() || (enter && k == 0) {
                                chosen = Some(k);
                            }
                        }
                        if entries.is_empty() {
                            ui.label(
                                RichText::new(t!("assembly.editor.no_match")).color(dark::MUTED),
                            );
                        }
                    });
            });
        });
    if !opening && area.response.clicked_elsewhere() && chosen.is_none() {
        close = true;
    }
    if let Some(k) = chosen {
        let pos = [menu.canvas.x, menu.canvas.y];
        let entry = entries.swap_remove(k);
        match entry.make {
            Make::Kind(kind) => {
                let wire = menu.from.and_then(|end| {
                    let w = wire_for(&kind, &design.defs, end)?;
                    Some((end_node(end), w))
                });
                edits.push(GraphEdit::Add {
                    kind: Box::new(kind),
                    pos,
                    wire,
                });
            }
            Make::Library(entry) => edits.push(GraphEdit::AddFromLibrary(entry, pos)),
        }
        let _ = graph;
        return None;
    }
    (!close).then_some(menu)
}

fn end_node(end: End) -> NodeId {
    match end {
        End::Out(n, ..) | End::In(n, ..) => n,
    }
}

/// How a new node of `kind` would be wired to `end`: its first input that takes the output,
/// or its first output that feeds the input.
fn wire_for(kind: &NodeKind, defs: &[NodeDef], end: End) -> Option<Wire> {
    match end {
        End::Out(_, out, ty) => kind
            .inputs(defs, None)
            .into_iter()
            .find(|i| ty.feeds(i.ty))
            .map(|i| Wire::From { out, new_in: i.key }),
        End::In(_, input, ty) => kind
            .outputs(defs, None)
            .into_iter()
            .find(|o| o.ty.feeds(ty))
            .map(|o| Wire::To {
                input,
                new_out: o.key,
            }),
    }
}

/// Apply `edit` to the graph the editor shows: moving nodes joins the undo history without
/// rebuilding; every other edit is one undo step.
pub(super) fn apply(state: &mut AssemblyState, rt: &mut Runtime, edit: GraphEdit) {
    let scope = rt.scope.clone();
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
            rt.selected = selected;
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
        GraphEdit::Disconnect(to, input) => {
            ops::disconnect(graph, to, input);
            Ok(None)
        }
        GraphEdit::Remove(ids) => {
            ops::remove(graph, &ids);
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
            let (_, node) =
                ops::group(graph, defs, owner, &ids, &name).map_err(|e| e.to_string())?;
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
    let missing = || ops::ConnectError::Missing.to_string();
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
    .map_err(|e| e.to_string())
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
        assert!(!s.design.is_linear());
        assert!(s.undo());
        assert!(s.design.is_linear(), "undo brings the steps back");
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
        rt.selected = [chain_step_id(0)].into();
        let chosen = rt.selected.clone();
        apply(&mut s, &mut rt, GraphEdit::Group(chosen));
        let group = rt.single().unwrap();
        rt.scope = vec![group];
        let (inner, owner) = access::graph_at(&s.design, &rt.scope).unwrap();
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
        let (_, owner) = access::graph_at(&s.design, &rt.scope).unwrap();
        assert_eq!(owner.unwrap().outputs.len(), outputs + 1);
        assert!(s.design.evaluation().errors.is_empty());
    }

    #[test]
    fn typed_values_read_numbers_expressions_and_triples() {
        let n = |s| parse_value(s, SocketType::Number);
        let v = |s| parse_value(s, SocketType::Vec3);
        assert_eq!(n(" 360/7 "), Some(PortValue::Number(360.0 / 7.0)));
        assert_eq!(n("1/0"), None);
        assert_eq!(n("i"), None);
        assert_eq!(
            v("1, 2*2, max(1, 3)"),
            Some(PortValue::Vec3([1.0, 4.0, 3.0]))
        );
        assert_eq!(v("2"), Some(PortValue::Vec3([2.0; 3])));
        assert_eq!(v("1, 2"), None);
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

    #[test]
    fn a_drag_moves_a_number_sideways_and_a_triple_both_ways() {
        let n = scrubbed(
            PortValue::Number(1.0),
            vec2(20.0, 50.0),
            Scrub::LENGTH,
            false,
            false,
        );
        assert_eq!(n, PortValue::Number(2.0));
        let v = PortValue::Vec3([0.0; 3]);
        let up_right = scrubbed(v, vec2(10.0, -20.0), Scrub::ANGLE, false, false);
        assert_eq!(up_right, PortValue::Vec3([5.0, 10.0, 0.0]));
        let shifted = scrubbed(v, vec2(0.0, -20.0), Scrub::ANGLE, true, false);
        assert_eq!(shifted, PortValue::Vec3([0.0, 0.0, 10.0]));
        let count = scrubbed(
            PortValue::Number(6.0),
            vec2(14.0, 0.0),
            Scrub::COUNT,
            false,
            false,
        );
        assert_eq!(count, PortValue::Number(7.0));
        let fine = scrubbed(
            PortValue::Number(0.0),
            vec2(10.0, 0.0),
            Scrub::LENGTH,
            false,
            true,
        );
        assert!(matches!(fine, PortValue::Number(x) if (x - 0.05).abs() < 1e-12));
    }

    /// The editor draws headless, inside a custom node too, with the add menu open.
    #[test]
    fn the_editor_draws_headless() {
        let ctx = egui::Context::default();
        let mut s = state();
        let mut view = NodeEditorView::default();
        view.rt.library = Some(Vec::new());
        for round in 0..4 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                editor(ui, &mut s, &mut view);
            });
            output.textures_delta.clear();
            match round {
                0 => {
                    let chosen: BTreeSet<NodeId> = [chain_step_id(0), chain_step_id(1)].into();
                    apply(&mut s, &mut view.rt, GraphEdit::Group(chosen));
                    view.rt.menu = Some(AddMenu {
                        screen: pos2(10.0, 10.0),
                        canvas: pos2(0.0, 0.0),
                        query: String::new(),
                        from: None,
                        focus: true,
                    });
                }
                1 => view.rt.scope = vec![view.rt.single().unwrap()],
                _ => {}
            }
        }
        assert_eq!(view.rt.scope.len(), 1);
    }
}
