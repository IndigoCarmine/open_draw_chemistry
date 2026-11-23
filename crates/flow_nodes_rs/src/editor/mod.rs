//! A node editor widget for egui (feature `egui`): nodes as boxes with sockets, wires between
//! them, values typed or dragged in boxes on the canvas, an add menu with a search field, and
//! Blender's keys.
//!
//! The editor never changes the graph. It asks the application what to draw through
//! [`Viewer`] and reports what the user did as [`Action`]s, which the application applies
//! (so it keeps its own undo history, and can refuse an edit). Only what is the editor's own —
//! the selection, the custom nodes entered, the camera, a drag in progress — lives in
//! [`EditorState`].
//!
//! With the pointer over the editor: drag a node's header to move it (the selection moves
//! with it), drag from a socket to wire it (from a wired input to move or take off its wire),
//! drag on the background to select a box of nodes, right- or middle-drag to pan and turn the
//! wheel to zoom. Right-click or Shift+A opens the add menu; dropping a wire on the background
//! opens it for nodes that can take the wire. The keys are [`KeyMap`]'s: by default Delete/X
//! deletes, Ctrl+D duplicates, M bypasses, Ctrl+G groups and Ctrl+Alt+G ungroups, Tab enters
//! the selected custom node (and leaves it), Home frames everything, Esc lets go, Ctrl+A
//! selects all.

mod canvas;
mod draw;
#[cfg(test)]
mod tests;
mod value;

use std::collections::{BTreeSet, HashMap};

use egui::{Color32, KeyboardShortcut, Modifiers, Pos2, Vec2, vec2};

use crate::id::NodeId;
use crate::schema::{Key, ValueHint};

pub use value::{Scrub, Value, number_text, parse_value, value_text};

/// What the editor needs from the application: the graph to draw, and how.
///
/// `In` and `Out` name a node's sockets (as the graph's keys do), `Socket` is the type of data
/// a socket carries, and `Template` what an entry of the add menu makes.
pub trait Viewer {
    type In: Key;
    type Out: Key;
    type Socket: Copy + PartialEq + std::fmt::Debug;
    type Template: Clone;

    /// Every node of the graph shown, laid out: drawn in this order (the selection on top).
    fn nodes(&self) -> Vec<NodeView<Self::In, Self::Out, Self::Socket>>;

    /// Every wire of the graph shown.
    fn links(&self) -> Vec<WireView<Self::In, Self::Out>>;

    /// The colour and shape of a socket of type `ty` (wires take their output's colour).
    fn socket_style(&self, ty: Self::Socket) -> SocketStyle;

    /// Whether an output of type `from` may be wired to an input of type `to`. Sockets that
    /// cannot take a wire being drawn are not offered, and a wire dropped on one is ignored.
    fn can_connect(&self, from: Self::Socket, to: Self::Socket) -> bool;

    /// What the add menu offers, in order (entries of one group together). Asked only while
    /// the menu is open. With a wire dropped on the background, only the entries
    /// [`Viewer::auto_wire`] can wire to it are shown.
    fn add_menu(&self) -> Vec<MenuEntry<Self::Template>>;

    /// How a new node made from `template` is wired to the loose end of a wire: which of its
    /// inputs takes the wire's output, or which of its outputs feeds the wire's input. `None`
    /// when it cannot take the wire.
    fn auto_wire(
        &self,
        template: &Self::Template,
        end: &LooseEnd<Self::In, Self::Out, Self::Socket>,
    ) -> Option<AutoWire<Self::In, Self::Out>>;

    /// A short note on a wire (how many objects, the number it carries).
    fn wire_badge(&self, wire: &WireView<Self::In, Self::Out>) -> Option<String> {
        let _ = wire;
        None
    }

    /// Whether a wire is drawn faint (its data is not used, or not there).
    fn wire_dimmed(&self, wire: &WireView<Self::In, Self::Out>) -> bool {
        let _ = wire;
        false
    }

    /// Whether node `id` holds a graph of its own that the user can go into (a custom node).
    fn enterable(&self, id: NodeId) -> bool {
        let _ = id;
        false
    }

    /// A number typed into a value box (by default Rust's own float syntax).
    fn parse_number(&self, text: &str) -> Option<f64> {
        text.trim().parse().ok()
    }

    /// What to say on an empty canvas.
    fn empty_hint(&self) -> Option<String> {
        None
    }
}

/// A node as the editor draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeView<I, O, S> {
    pub id: NodeId,
    /// The top-left corner on the canvas.
    pub pos: Pos2,
    pub title: String,
    /// A line under the title (a summary of the settings), if any.
    pub subtitle: Option<String>,
    pub header: Color32,
    /// Switched off: drawn grey with a pause mark.
    pub bypass: bool,
    /// Drawn faint (it feeds nothing that is used).
    pub dim: bool,
    pub error: Option<String>,
    pub warning: Option<String>,
    /// The node's own value (a number or vector node), in a box on the subtitle's line.
    pub own: Option<Inline>,
    /// Its outputs, drawn first.
    pub outputs: Vec<SocketRow<O, S>>,
    /// Its inputs, drawn after the outputs.
    pub inputs: Vec<SocketRow<I, S>>,
}

impl<I, O, S> NodeView<I, O, S> {
    /// A node with a title, no sockets and the default look.
    pub fn new(id: NodeId, pos: Pos2, title: impl Into<String>, header: Color32) -> Self {
        Self {
            id,
            pos,
            title: title.into(),
            subtitle: None,
            header,
            bypass: false,
            dim: false,
            error: None,
            warning: None,
            own: None,
            outputs: Vec::new(),
            inputs: Vec::new(),
        }
    }
}

/// One socket of a node as the editor draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct SocketRow<K, S> {
    pub key: K,
    pub ty: S,
    pub label: String,
    /// The value of an input no wire feeds: drawn, and typed or dragged in a box.
    pub inline: Option<Inline>,
    /// A socket that is not there yet (a merge's next input, a custom node's next port):
    /// drawn hollow with a `+`.
    pub ghost: bool,
    /// Takes a wire of any type (a custom node's next port, typed by what is wired to it).
    pub any_type: bool,
}

impl<K, S> SocketRow<K, S> {
    pub fn new(key: K, ty: S, label: impl Into<String>) -> Self {
        Self {
            key,
            ty,
            label: label.into(),
            inline: None,
            ghost: false,
            any_type: false,
        }
    }
}

/// A value shown in a box: what it is, its unit, and how a drag changes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inline {
    pub value: Value,
    pub unit: &'static str,
    pub scrub: Scrub,
}

impl Inline {
    /// `value` with the unit and drag speed a [`ValueHint`] suggests (`°` for angles, `length`
    /// for lengths).
    pub fn hinted(value: Value, hint: ValueHint, length: &'static str) -> Self {
        let (unit, scrub) = match hint {
            ValueHint::Plain => ("", Scrub::PLAIN),
            ValueHint::Count => ("", Scrub::COUNT),
            ValueHint::Angle => ("°", Scrub::ANGLE),
            ValueHint::Length => (length, Scrub::LENGTH),
        };
        Self { value, unit, scrub }
    }
}

/// A wire as the editor draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WireView<I, O> {
    pub from: NodeId,
    pub out: O,
    pub to: NodeId,
    pub input: I,
}

/// The end of a wire being drawn that is not on a socket yet: the socket it starts from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LooseEnd<I, O, S> {
    /// From an output: it will feed an input.
    Out(NodeId, O, S),
    /// From an input: an output will feed it.
    In(NodeId, I, S),
}

impl<I, O, S> LooseEnd<I, O, S> {
    pub fn node(&self) -> NodeId {
        match self {
            LooseEnd::Out(n, ..) | LooseEnd::In(n, ..) => *n,
        }
    }
}

/// How a new node is wired to the loose end of a wire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutoWire<I, O> {
    /// Output `out` of the existing node feeds input `new_in` of the new one.
    From { out: O, new_in: I },
    /// Output `new_out` of the new node feeds input `input` of the existing one.
    To { input: I, new_out: O },
}

/// One entry of the add menu.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuEntry<T> {
    pub label: String,
    /// The heading it is listed under.
    pub group: String,
    pub template: T,
}

/// How a socket is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SocketStyle {
    pub color: Color32,
    pub shape: SocketShape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketShape {
    Circle,
    /// A little smaller: a single number.
    SmallCircle,
    /// Several numbers.
    Square,
}

/// Where the value of a box goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueTarget<I> {
    /// The node's own value ([`NodeView::own`]).
    Own,
    /// An input no wire feeds.
    Input(I),
}

/// What the user did. The application applies it to its graph (or refuses it).
#[derive(Debug, Clone, PartialEq)]
pub enum Action<I, O, T> {
    /// Add a node made from `template` at `pos` (canvas), wired to `wire`'s node if any.
    Add {
        template: T,
        pos: [f32; 2],
        wire: Option<(NodeId, AutoWire<I, O>)>,
    },
    /// Move nodes by `delta` (canvas units): only where they are drawn. Sent every frame of a
    /// drag.
    Move(Vec<NodeId>, Vec2),
    /// Wire output `out` of `from` to input `input` of `to` (replacing what fed it).
    Connect(WireView<I, O>),
    /// Take the wire off input `old.1` of node `old.0` and wire `new` instead (a wire dragged
    /// from one input to another): one edit.
    Reconnect {
        old: (NodeId, I),
        new: WireView<I, O>,
    },
    /// Take the wire off input `I` of the node.
    Disconnect(NodeId, I),
    Remove(BTreeSet<NodeId>),
    Duplicate(BTreeSet<NodeId>),
    /// Switch the nodes off (or back on, when all are off).
    Bypass(BTreeSet<NodeId>),
    /// Gather the nodes into a custom node.
    Group(BTreeSet<NodeId>),
    /// Put a custom node's nodes back in its place.
    Ungroup(NodeId),
    /// A value typed in or dragged. `dragging` while the drag goes on: the application may
    /// join a drag into one undo step when the button is let go.
    SetValue {
        node: NodeId,
        target: ValueTarget<I>,
        value: Value,
        dragging: bool,
    },
}

/// The editor's look: colours and sizes (canvas units at zoom 1).
#[derive(Debug, Clone, PartialEq)]
pub struct EditorStyle {
    pub background: Color32,
    /// A node's body.
    pub node: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    /// The selection, the box being dragged.
    pub accent: Color32,
    pub error: Color32,
    pub warning: Color32,
    /// A bypassed node's header.
    pub bypassed: Color32,
    /// A box being hovered or dragged.
    pub field_hover: Color32,
    pub node_width: f32,
    pub header_height: f32,
    pub row_height: f32,
    pub socket_radius: f32,
    /// How close to a socket (screen px) a wire may be dropped.
    pub socket_hit: f32,
    /// The zoom from which values are drawn as boxes to type into.
    pub box_zoom: f32,
    pub min_zoom: f32,
    pub max_zoom: f32,
    /// The grid's spacing (none below 8 screen px).
    pub grid: f32,
}

impl Default for EditorStyle {
    /// A dark look.
    fn default() -> Self {
        Self {
            background: Color32::from_rgb(0x12, 0x0F, 0x0D),
            node: Color32::from_rgb(0x26, 0x21, 0x1D),
            border: Color32::from_rgb(0x3B, 0x34, 0x2E),
            text: Color32::from_rgb(0xF4, 0xF1, 0xEE),
            muted: Color32::from_rgb(0xB3, 0xAA, 0xA2),
            accent: Color32::from_rgb(0xF0, 0x62, 0x3F),
            error: Color32::from_rgb(0xF2, 0x6D, 0x5E),
            warning: Color32::from_rgb(0xE8, 0xB3, 0x3C),
            bypassed: Color32::from_rgb(0x4A, 0x45, 0x40),
            field_hover: Color32::from_rgb(0x35, 0x2E, 0x28),
            node_width: 176.0,
            header_height: 22.0,
            row_height: 19.0,
            socket_radius: 4.5,
            socket_hit: 11.0,
            box_zoom: 0.6,
            min_zoom: 0.25,
            max_zoom: 2.5,
            grid: 40.0,
        }
    }
}

/// The words the editor shows itself (the rest come from the [`Viewer`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Labels {
    /// The tooltip of a number's box.
    pub value_tip: String,
    /// The tooltip of a vector's box.
    pub vector_tip: String,
    /// The add menu's search field.
    pub search: String,
    /// The add menu with nothing matching.
    pub no_match: String,
}

impl Default for Labels {
    fn default() -> Self {
        Self {
            value_tip: "Drag to change; click to type (an expression such as 360/7 works)".into(),
            vector_tip: "Drag: left and right for x, up and down for y (z with Shift); click \
                         to type x, y, z"
                .into(),
            search: "Search…".into(),
            no_match: "Nothing matches".into(),
        }
    }
}

/// The editor's keys, taken while the pointer is over it.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyMap {
    pub delete: Vec<KeyboardShortcut>,
    pub duplicate: Vec<KeyboardShortcut>,
    pub bypass: Vec<KeyboardShortcut>,
    pub group: Vec<KeyboardShortcut>,
    pub ungroup: Vec<KeyboardShortcut>,
    /// Into the selected custom node, or back out.
    pub enter_exit: Vec<KeyboardShortcut>,
    pub frame_all: Vec<KeyboardShortcut>,
    /// Let go of a drag, else the selection, else leave the custom node.
    pub cancel: Vec<KeyboardShortcut>,
    pub add: Vec<KeyboardShortcut>,
    pub select_all: Vec<KeyboardShortcut>,
}

impl Default for KeyMap {
    /// Blender's.
    fn default() -> Self {
        use egui::Key;
        let key = |m: Modifiers, k: Key| vec![KeyboardShortcut::new(m, k)];
        let none = Modifiers::NONE;
        let cmd = Modifiers::COMMAND;
        Self {
            delete: vec![
                KeyboardShortcut::new(none, Key::Delete),
                KeyboardShortcut::new(none, Key::X),
                KeyboardShortcut::new(none, Key::Backspace),
            ],
            duplicate: key(cmd, Key::D),
            bypass: key(none, Key::M),
            group: key(cmd, Key::G),
            ungroup: key(cmd | Modifiers::ALT, Key::G),
            enter_exit: key(none, Key::Tab),
            frame_all: key(none, Key::Home),
            cancel: key(none, Key::Escape),
            add: key(Modifiers::SHIFT, Key::A),
            select_all: key(cmd, Key::A),
        }
    }
}

/// Where a graph is looked at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Where the canvas origin is, from the editor's top-left corner (screen px).
    pub pan: Vec2,
    pub zoom: f32,
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
enum Drag<I, O, S> {
    /// Moving the selected nodes.
    Nodes,
    /// A wire from a socket, its loose end under the pointer; `detached` is the input it was
    /// taken from, still wired until it is dropped.
    Wire {
        end: LooseEnd<I, O, S>,
        detached: Option<(NodeId, I)>,
    },
    /// Selecting the nodes in a box (screen corner where it began).
    Box(Pos2),
}

#[derive(Debug, Clone, PartialEq)]
struct AddMenu<I, O, S> {
    /// Where it was opened (screen) and where the node goes (canvas).
    screen: Pos2,
    canvas: Pos2,
    query: String,
    /// A wire dropped on the background: the new node is wired to it.
    from: Option<LooseEnd<I, O, S>>,
    focus: bool,
}

/// What the editor keeps between frames: the selection, the custom nodes entered, where each
/// graph is looked at, and what is going on.
#[derive(Debug, Clone)]
pub struct EditorState<I, O, S> {
    /// The custom nodes entered, outermost first: the application shows the graph inside
    /// the last one.
    pub scope: Vec<NodeId>,
    pub selected: BTreeSet<NodeId>,
    /// Where each graph is looked at, by the application's key (which document) and scope.
    cameras: HashMap<(u64, Vec<NodeId>), Camera>,
    drag: Option<Drag<I, O, S>>,
    menu: Option<AddMenu<I, O, S>>,
    /// Frame every node on the next frame.
    frame: bool,
}

impl<I, O, S> Default for EditorState<I, O, S> {
    fn default() -> Self {
        Self {
            scope: Vec::new(),
            selected: BTreeSet::new(),
            cameras: HashMap::new(),
            drag: None,
            menu: None,
            frame: false,
        }
    }
}

impl<I, O, S> EditorState<I, O, S> {
    /// The node selected, when exactly one is.
    pub fn single(&self) -> Option<NodeId> {
        (self.selected.len() == 1).then(|| *self.selected.iter().next().expect("one"))
    }

    /// Forget the selection, the scope and what is going on (another graph is shown). The
    /// cameras stay.
    pub fn reset(&mut self) {
        self.scope.clear();
        self.selected.clear();
        self.drag = None;
        self.menu = None;
    }

    /// Go into custom node `id` (a node of the graph shown).
    pub fn enter(&mut self, id: NodeId) {
        self.scope.push(id);
        self.selected.clear();
    }

    /// Go back to the graph `depth` custom nodes deep (0: the top level).
    pub fn leave_to(&mut self, depth: usize) {
        if depth < self.scope.len() {
            self.scope.truncate(depth);
            self.selected.clear();
        }
    }

    /// Frame every node on the next frame.
    pub fn frame_all(&mut self) {
        self.frame = true;
    }

    /// Open the add menu at `screen`, adding at canvas point `canvas`.
    pub fn open_add_menu(&mut self, screen: Pos2, canvas: Pos2) {
        self.menu = Some(AddMenu {
            screen,
            canvas,
            query: String::new(),
            from: None,
            focus: true,
        });
    }

    /// Whether the add menu is open.
    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }
}

/// The node editor: draw it with [`NodeEditor::show`] each frame.
#[derive(Debug, Clone, Copy)]
pub struct NodeEditor<'a> {
    pub style: &'a EditorStyle,
    pub labels: &'a Labels,
    pub keys: &'a KeyMap,
    /// Which graph this is to the application (a document): each keeps its own cameras.
    pub context: u64,
}
