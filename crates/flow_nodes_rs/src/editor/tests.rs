//! The editor driven headless over the crate's test kinds.

use egui::{Color32, Event, Key, Modifiers, PointerButton, RawInput, Rect, pos2, vec2};

use super::*;
use crate::graph::Graph;
use crate::schema;
use crate::tests::{In, Kind, Out, Ty};

struct Calc<'a>(&'a Graph<Kind>);

impl Viewer for Calc<'_> {
    type In = In;
    type Out = Out;
    type Socket = Ty;
    type Template = Kind;

    fn nodes(&self) -> Vec<NodeView<In, Out, Ty>> {
        self.0
            .nodes
            .iter()
            .map(|n| {
                let mut v = NodeView::new(n.id, pos2(n.pos[0], n.pos[1]), "n", Color32::GRAY);
                v.outputs = schema::outputs(&n.kind, &[], None)
                    .into_iter()
                    .map(|o| SocketRow::new(o.key, o.ty, o.label))
                    .collect();
                v.inputs = schema::inputs(&n.kind, &[], None)
                    .into_iter()
                    .map(|i| SocketRow::new(i.key, i.ty, i.label))
                    .collect();
                v
            })
            .collect()
    }

    fn links(&self) -> Vec<WireView<In, Out>> {
        self.0
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

    fn socket_style(&self, _: Ty) -> SocketStyle {
        SocketStyle {
            color: Color32::WHITE,
            shape: SocketShape::Circle,
        }
    }

    fn can_connect(&self, from: Ty, to: Ty) -> bool {
        crate::SocketType::feeds(from, to)
    }

    fn add_menu(&self) -> Vec<MenuEntry<Kind>> {
        vec![MenuEntry {
            label: "Sum".into(),
            group: "Math".into(),
            template: Kind::Sum,
        }]
    }

    fn auto_wire(&self, _: &Kind, _: &LooseEnd<In, Out, Ty>) -> Option<AutoWire<In, Out>> {
        None
    }
}

type Actions = Vec<Action<In, Out, Kind>>;

/// One frame of the editor over `graph` filling a 800×600 screen, with `events`.
fn frame(
    ctx: &egui::Context,
    graph: &Graph<Kind>,
    state: &mut EditorState<In, Out, Ty>,
    mut events: Vec<Event>,
    modifiers: Modifiers,
) -> Actions {
    // The modifiers held during the frame.
    events.insert(0, Event::ModifiersChanged(modifiers));
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
        events,
        ..Default::default()
    };
    let (style, labels, keys) = (EditorStyle::default(), Labels::default(), KeyMap::default());
    let editor = NodeEditor {
        style: &style,
        labels: &labels,
        keys: &keys,
        context: 1,
    };
    let mut actions = Vec::new();
    let mut output = ctx.run_ui(input, |ui| {
        actions = editor.show(ui, &Calc(graph), state);
    });
    output.textures_delta.clear();
    actions
}

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn graph() -> Graph<Kind> {
    let mut g = Graph::default();
    let n = g.add(Kind::Num(1.0), [0.0, 0.0]);
    let sum = g.add(Kind::Sum, [300.0, 0.0]);
    let _ = (n, sum);
    g
}

#[test]
fn ctrl_alt_g_ungroups_and_ctrl_g_groups() {
    let ctx = egui::Context::default();
    let g = graph();
    let mut state = EditorState::default();
    let hover = Event::PointerMoved(pos2(400.0, 300.0));
    frame(&ctx, &g, &mut state, vec![hover.clone()], Modifiers::NONE);
    state.selected = [g.nodes[0].id].into();
    let both = Modifiers::COMMAND | Modifiers::ALT;
    let actions = frame(
        &ctx,
        &g,
        &mut state,
        vec![hover.clone(), key(Key::G, both)],
        both,
    );
    assert_eq!(actions, vec![Action::Ungroup(g.nodes[0].id)]);
    let actions = frame(
        &ctx,
        &g,
        &mut state,
        vec![hover, key(Key::G, Modifiers::COMMAND)],
        Modifiers::COMMAND,
    );
    assert_eq!(actions, vec![Action::Group([g.nodes[0].id].into())]);
}

#[test]
fn a_wire_dropped_on_a_socket_of_another_type_is_ignored() {
    let ctx = egui::Context::default();
    let mut g = graph();
    // A list input (Scale's) and a number input (Add's): a number fits both, a list only the
    // first. Range's list output into Add's number input is refused.
    let range = g.add(Kind::Range { n: 3.0 }, [0.0, 200.0]);
    let add = g.add(Kind::Add { a: 0.0, b: 0.0 }, [300.0, 200.0]);
    let scale = g.add(Kind::Scale { k: 2.0 }, [300.0, 400.0]);
    let mut state = EditorState::default();
    let style = EditorStyle::default();
    frame(&ctx, &g, &mut state, vec![], Modifiers::NONE);
    // Where the sockets are on the screen, with the camera the first frame chose.
    let cam = state.cameras[&(1, vec![])];
    let v = super::draw::View::of(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)), cam);
    let laid = |id| {
        super::draw::Laid::of(
            Calc(&g).nodes().into_iter().find(|n| n.id == id).unwrap(),
            &style,
        )
    };
    let from = laid(range)
        .socket(super::draw::Side::Out(Out::List))
        .unwrap()
        .0;
    let to = laid(add).socket(super::draw::Side::In(In::A)).unwrap().0;
    let fits = laid(scale)
        .socket(super::draw::Side::In(In::List))
        .unwrap()
        .0;
    let (from, to, fits) = (v.screen(from), v.screen(to), v.screen(fits));
    let press = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    let drag = |state: &mut EditorState<In, Out, Ty>, to| {
        let mut actions = Vec::new();
        for events in [
            vec![Event::PointerMoved(from), press(from, true)],
            vec![Event::PointerMoved(from + vec2(20.0, 0.0))],
            vec![Event::PointerMoved(to)],
            vec![press(to, false)],
            vec![],
        ] {
            actions.extend(frame(&ctx, &g, state, events, Modifiers::NONE));
        }
        actions
    };
    let actions = drag(&mut state, to);
    assert!(actions.is_empty(), "{actions:?}");
    assert!(
        !state.menu_open(),
        "dropped on a socket: no add menu either"
    );
    // The same drag onto a list input wires it.
    let actions = drag(&mut state, fits);
    assert_eq!(
        actions,
        vec![Action::Connect(WireView {
            from: range,
            out: Out::List,
            to: scale,
            input: In::List,
        })]
    );
}
