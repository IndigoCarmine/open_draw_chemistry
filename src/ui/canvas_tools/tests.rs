//! The ring and chain tools driven through real frames of the editor widget, as
//! [`crate::document::Document::show`] runs them.

use super::*;

const SCREEN: egui::Rect =
    egui::Rect::from_min_max(egui::pos2(100.0, 100.0), egui::pos2(900.0, 700.0));

struct Harness {
    ctx: egui::Context,
    editor: ChemStructEditor,
    tools: CanvasTools,
    canvas: egui::Rect,
    time: f64,
}

impl Harness {
    fn new(tool: CanvasTool) -> Self {
        let mut h = Harness {
            ctx: egui::Context::default(),
            editor: ChemStructEditor::default(),
            tools: CanvasTools::default(),
            canvas: SCREEN,
            time: 0.0,
        };
        h.tools.select(&mut h.editor, Some(tool));
        h.frame(Vec::new());
        h
    }

    /// One frame with `events`, 0.2 s after the last: a press and release in consecutive frames
    /// are a click, and two clicks a few frames apart are not a double-click.
    fn frame(&mut self, events: Vec<egui::Event>) {
        self.time += 0.2;
        self.frame_at(events);
    }

    fn frame_at(&mut self, events: Vec<egui::Event>) {
        let raw = egui::RawInput {
            screen_rect: Some(SCREEN),
            events,
            focused: true,
            time: Some(self.time),
            ..Default::default()
        };
        let (editor, tools, canvas) = (&mut self.editor, &mut self.tools, &mut self.canvas);
        let mut output = self.ctx.run_ui(raw, |ui| {
            tools.before_show(editor, ui.ctx());
            let mut response = editor.show(ui);
            tools.after_show(editor, ui, &mut response);
            *canvas = response.response.rect;
        });
        output.textures_delta.clear();
    }

    /// The screen point of molecule point `p`.
    fn at(&self, p: [f32; 2]) -> egui::Pos2 {
        self.editor.mol_to_screen(p, self.canvas.center())
    }

    fn point(&mut self, at: egui::Pos2) {
        self.frame(vec![egui::Event::PointerMoved(at)]);
        self.frame(vec![egui::Event::PointerMoved(at)]);
    }

    fn click(&mut self, at: egui::Pos2) {
        self.point(at);
        self.frame(vec![button(at, true)]);
        self.frame(vec![button(at, false)]);
        // The frame a ring clicked on an atom or bond is drawn in.
        self.frame(Vec::new());
    }

    fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
        self.point(from);
        self.frame(vec![button(from, true)]);
        for k in 1..=6 {
            let t = k as f32 / 6.0;
            self.frame(vec![egui::Event::PointerMoved(from + (to - from) * t)]);
        }
        self.frame(vec![button(to, false)]);
        self.frame(Vec::new());
    }

    fn count_order(&self, order: BondOrder) -> usize {
        self.editor
            .molecule
            .bonds
            .iter()
            .filter(|b| b.order == order)
            .count()
    }
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    }
}

fn distance(p: [f32; 2], q: [f32; 2]) -> f32 {
    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()
}

fn step() -> f32 {
    BOND_LENGTH * (std::f32::consts::PI / 6.0).cos()
}

/// Two carbons bonded, the first at the origin.
fn two_carbons(h: &mut Harness) -> (u32, u32, u32) {
    let a = h.editor.molecule.add_atom("C".into(), [0.0, 0.0], 0);
    let b = h
        .editor
        .molecule
        .add_atom("C".into(), [BOND_LENGTH, 0.0], 0);
    let bond = h.editor.molecule.add_bond(a, b, BondOrder::Single);
    (a, b, bond)
}

const RINGS: [(RingKind, usize, usize); 3] = [
    (RingKind::Benzene, 6, 3),
    (RingKind::Cyclopentane, 5, 0),
    (RingKind::Cyclohexane, 6, 0),
];

/// A ring tool on empty canvas puts a ring centred on the click, flat side down, in one undo
/// step, and leaves the selection alone.
#[test]
fn a_ring_on_empty_canvas() {
    for (kind, n, doubles) in RINGS {
        let mut h = Harness::new(CanvasTool::Ring(kind));
        let at = h.at([0.0, 0.0]);
        h.click(at);
        assert_eq!(h.editor.molecule.atoms.len(), n, "{kind:?}");
        assert_eq!(h.editor.molecule.bonds.len(), n, "{kind:?}");
        assert_eq!(h.count_order(BondOrder::Double), doubles, "{kind:?}");
        let c = h.editor.molecule.atoms.iter().fold([0.0, 0.0], |c, a| {
            [c[0] + a.pos[0] / n as f32, c[1] + a.pos[1] / n as f32]
        });
        assert!(distance(c, [0.0, 0.0]) < 1e-2, "{kind:?}: centred, {c:?}");
        let mut ys: Vec<f32> = h.editor.molecule.atoms.iter().map(|a| a.pos[1]).collect();
        ys.sort_by(|a, b| b.partial_cmp(a).unwrap());
        assert!((ys[0] - ys[1]).abs() < 1e-2, "{kind:?}: flat side down");
        assert_eq!(h.editor.undo_stack.len(), 1, "{kind:?}: one undo step");
        assert!(h.editor.selected_atoms.is_empty(), "{kind:?}");
        assert_eq!(h.editor.tool, Tool::Select);
        assert_eq!(
            h.tools.active(),
            Some(CanvasTool::Ring(kind)),
            "{kind:?}: still on"
        );
        assert!(h.editor.undo());
        assert!(h.editor.molecule.atoms.is_empty(), "{kind:?}: undone");
    }
}

/// The second click of a double-click on empty canvas does not stack a second ring.
#[test]
fn a_double_click_on_empty_canvas_draws_one_ring() {
    let mut h = Harness::new(CanvasTool::Ring(RingKind::Cyclohexane));
    let at = h.at([0.0, 0.0]);
    h.point(at);
    h.frame(vec![button(at, true)]);
    h.frame_at(vec![button(at, false)]);
    h.time += 0.05;
    h.frame_at(vec![button(at, true)]);
    h.time += 0.05;
    h.frame_at(vec![button(at, false)]);
    h.frame(Vec::new());
    assert_eq!(h.editor.molecule.atoms.len(), 6);
}

/// A ring tool on a bond fuses its ring onto it, sharing the bond, as the ring keys do, in one
/// undo step and with the user's own bindings back afterwards.
#[test]
fn a_ring_fused_onto_a_bond() {
    for (kind, n, doubles) in RINGS {
        let mut h = Harness::new(CanvasTool::Ring(kind));
        let (a, b, bond) = two_carbons(&mut h);
        let shortcuts = (
            format!("{:?}", h.editor.config.atom_shortcuts),
            format!("{:?}", h.editor.config.bond_shortcuts),
        );
        let at = h.at([BOND_LENGTH / 2.0, 0.0]);
        h.click(at);
        assert_eq!(h.editor.molecule.atoms.len(), n, "{kind:?}");
        assert_eq!(h.editor.molecule.bonds.len(), n, "{kind:?}");
        assert_eq!(h.count_order(BondOrder::Double), doubles, "{kind:?}");
        assert!(
            h.editor.molecule.bond_by_id(bond).is_some(),
            "{kind:?}: shared"
        );
        let ring = h
            .editor
            .molecule
            .smallest_ring_containing(a)
            .expect("a ring");
        assert!(ring.contains(&b) && ring.len() == n, "{kind:?}: {ring:?}");
        assert_eq!(h.editor.undo_stack.len(), 1, "{kind:?}: one undo step");
        assert!(
            h.editor.selected_bonds.is_empty(),
            "{kind:?}: nothing selected"
        );
        assert_eq!(
            (
                format!("{:?}", h.editor.config.atom_shortcuts),
                format!("{:?}", h.editor.config.bond_shortcuts),
            ),
            shortcuts,
            "{kind:?}: the user's key bindings are back"
        );
    }
}

/// A ring tool on an atom grows its ring from the atom.
#[test]
fn a_ring_grown_from_an_atom() {
    let mut h = Harness::new(CanvasTool::Ring(RingKind::Cyclohexane));
    let (a, _, _) = two_carbons(&mut h);
    let at = h.at([0.0, 0.0]);
    h.click(at);
    assert_eq!(h.editor.molecule.atoms.len(), 7);
    assert_eq!(h.editor.molecule.bonds.len(), 7);
    assert_eq!(
        h.editor
            .molecule
            .smallest_ring_containing(a)
            .map(|r| r.len()),
        Some(6)
    );
    assert_eq!(h.editor.undo_stack.len(), 1);
    assert!(
        h.editor.selected_atoms.is_empty(),
        "the click selected nothing"
    );
}

/// A drag with the chain tool from an atom draws a zigzag along it: one carbon per step,
/// bonds of the usual length at 120°, in one undo step; the atom it starts from stays put.
#[test]
fn a_chain_dragged_from_an_atom() {
    let mut h = Harness::new(CanvasTool::Chain);
    let a = h.editor.molecule.add_atom("C".into(), [0.0, 0.0], 0);
    let (from, to) = (h.at([0.0, 0.0]), h.at([4.0 * step(), 0.0]));
    h.drag(from, to);
    assert_eq!(h.editor.molecule.atoms.len(), 5, "four new carbons");
    assert_eq!(h.editor.molecule.bonds.len(), 4);
    assert_eq!(h.editor.undo_stack.len(), 1);
    assert_eq!(h.editor.molecule.atom_by_id(a).unwrap().pos, [0.0, 0.0]);
    assert!(h.tools.chain_preview(&h.editor).is_none());

    let mut path = vec![a];
    while let Some(&next) = h
        .editor
        .molecule
        .neighbor_atom_ids(*path.last().unwrap())
        .iter()
        .find(|id| !path.contains(id))
    {
        path.push(next);
    }
    assert_eq!(path.len(), 5, "unbranched");
    let pos: Vec<[f32; 2]> = path
        .iter()
        .map(|&id| h.editor.molecule.atom_by_id(id).unwrap().pos)
        .collect();
    for w in pos.windows(2) {
        assert!((distance(w[0], w[1]) - BOND_LENGTH).abs() < 1e-3);
    }
    for w in pos.windows(3) {
        let u = [w[0][0] - w[1][0], w[0][1] - w[1][1]];
        let v = [w[2][0] - w[1][0], w[2][1] - w[1][1]];
        let cos = (u[0] * v[0] + u[1] * v[1]) / (BOND_LENGTH * BOND_LENGTH);
        assert!((cos + 0.5).abs() < 1e-3, "120°, cos {cos}");
    }
    assert!(pos[1][1] < 0.0, "a drag to the right starts upwards");
    assert!((pos[4][0] - 4.0 * step()).abs() < 1e-3 && pos[4][1].abs() < 1e-3);
}

/// From empty canvas a chain starts with a new carbon; it joins an atom it ends on; the first
/// bond leans away from a bond its atom already has.
#[test]
fn a_chain_from_empty_canvas_and_onto_an_atom() {
    let mut h = Harness::new(CanvasTool::Chain);
    let (from, to) = (h.at([0.0, 0.0]), h.at([0.0, 3.0 * step()]));
    h.drag(from, to);
    assert_eq!(h.editor.molecule.atoms.len(), 4);
    assert_eq!(h.editor.molecule.bonds.len(), 3);
    assert!(h.editor.selected_atoms.is_empty(), "no lasso selection");

    let mut h = Harness::new(CanvasTool::Chain);
    h.editor.molecule.add_atom("C".into(), [0.0, 0.0], 0);
    let b = h
        .editor
        .molecule
        .add_atom("C".into(), [2.0 * step(), 0.0], 0);
    let (from, to) = (h.at([0.0, 0.0]), h.at([2.0 * step(), 0.0]));
    h.drag(from, to);
    assert_eq!(h.editor.molecule.atoms.len(), 3, "one carbon between them");
    assert_eq!(h.editor.molecule.neighbor_atom_ids(b).len(), 1, "joined");

    let mut h = Harness::new(CanvasTool::Chain);
    let a = h.editor.molecule.add_atom("C".into(), [0.0, 0.0], 0);
    let up = h.editor.molecule.add_atom("C".into(), [-0.75, -1.299], 0);
    h.editor.molecule.add_bond(a, up, BondOrder::Single);
    let (from, to) = (h.at([0.0, 0.0]), h.at([3.0 * step(), 0.0]));
    h.drag(from, to);
    let first = h
        .editor
        .molecule
        .neighbor_atom_ids(a)
        .into_iter()
        .find(|&id| id != up)
        .unwrap();
    assert!(
        h.editor.molecule.atom_by_id(first).unwrap().pos[1] > 0.0,
        "downwards: {:?}",
        h.editor
            .molecule
            .atoms
            .iter()
            .map(|a| (a.id, a.pos))
            .collect::<Vec<_>>()
    );
}

/// Escape during a chain drag draws nothing.
#[test]
fn escape_gives_a_chain_up() {
    let mut h = Harness::new(CanvasTool::Chain);
    let from = h.at([0.0, 0.0]);
    h.point(from);
    h.frame(vec![button(from, true)]);
    for k in 1..=4 {
        h.frame(vec![egui::Event::PointerMoved(
            from + egui::vec2(40.0 * k as f32, 0.0),
        )]);
    }
    assert!(
        h.tools
            .chain_preview(&h.editor)
            .is_some_and(|p| p.len() > 2)
    );
    h.frame(vec![egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    }]);
    h.frame(vec![button(from + egui::vec2(160.0, 0.0), false)]);
    assert!(h.editor.molecule.atoms.is_empty());
    assert!(h.editor.undo_stack.is_empty());
}

/// A click with the chain tool on an atom adds one carbon to it.
#[test]
fn a_chain_click_adds_a_carbon() {
    let mut h = Harness::new(CanvasTool::Chain);
    let a = h.editor.molecule.add_atom("C".into(), [0.0, 0.0], 0);
    let at = h.at([0.0, 0.0]);
    h.click(at);
    assert_eq!(h.editor.molecule.atoms.len(), 2);
    assert_eq!(h.editor.molecule.neighbor_atom_ids(a).len(), 1);
    assert_eq!(h.editor.undo_stack.len(), 1);
    assert!(h.editor.selected_atoms.is_empty());
}

/// Choosing another tool any other way turns these off.
#[test]
fn another_tool_turns_them_off() {
    let mut h = Harness::new(CanvasTool::Chain);
    h.editor.set_tool(Tool::Bond);
    h.frame(Vec::new());
    assert_eq!(h.tools.active(), None);

    let mut h = Harness::new(CanvasTool::Ring(RingKind::Benzene));
    h.frame(vec![egui::Event::Key {
        key: egui::Key::Num1,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::CTRL,
    }]);
    assert_eq!(h.tools.active(), None, "Ctrl+1 is the Select tool");
    assert_eq!(h.editor.tool, Tool::Select);
}
