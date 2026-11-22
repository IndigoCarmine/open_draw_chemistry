//! The canvas: nodes and wires, with the pointer and the keys.

use egui::{Align2, FontId, KeyboardShortcut, PointerButton, Pos2, Rect, RichText, Sense, Stroke};
use egui::{StrokeKind, Ui, Vec2};

use super::draw::{self, Laid, Side, View};
use super::value::value_field;
use super::{
    Action, AddMenu, Drag, EditorState, LooseEnd, NodeEditor, ValueTarget, Viewer, WireView,
};
use crate::id::NodeId;

type Edits<V> = Vec<Action<<V as Viewer>::In, <V as Viewer>::Out, <V as Viewer>::Template>>;
type State<V> = EditorState<<V as Viewer>::In, <V as Viewer>::Out, <V as Viewer>::Socket>;
type End<V> = LooseEnd<<V as Viewer>::In, <V as Viewer>::Out, <V as Viewer>::Socket>;
type LaidOf<V> = Laid<<V as Viewer>::In, <V as Viewer>::Out, <V as Viewer>::Socket>;

impl NodeEditor<'_> {
    /// Draw the graph `viewer` shows in the rest of `ui`, and handle the pointer and the keys.
    /// Returns what the user did, for the application to apply.
    pub fn show<V: Viewer>(&self, ui: &mut Ui, viewer: &V, state: &mut State<V>) -> Edits<V> {
        let mut edits = Vec::new();
        let style = self.style;
        let area = ui.available_rect_before_wrap();
        let bg = ui.allocate_rect(area, Sense::click_and_drag());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, style.background);

        let laid: Vec<LaidOf<V>> = viewer
            .nodes()
            .into_iter()
            .map(|n| Laid::of(n, style))
            .collect();
        let links = viewer.links();
        let find = |id: NodeId| laid.iter().find(|l| l.id == id);

        let cam_key = (self.context, state.scope.clone());
        if std::mem::take(&mut state.frame) {
            state.cameras.remove(&cam_key);
        }
        let cam = *state
            .cameras
            .entry(cam_key.clone())
            .or_insert_with(|| draw::frame_all(laid.iter().map(|l| l.rect), area, style.min_zoom));
        let mut cam_next = cam;
        let v = View::of(area, cam);
        let hovered = ui.rect_contains_pointer(area);
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let socket_style = |ty: V::Socket| viewer.socket_style(ty);

        draw::grid(&painter, area, v, style);

        // The input a wire is being dragged off: not drawn while it is.
        let detached = match state.drag {
            Some(Drag::Wire { detached, .. }) => detached,
            _ => None,
        };
        for link in &links {
            if detached == Some((link.to, link.input)) {
                continue;
            }
            let (Some(a), Some(b)) = (find(link.from), find(link.to)) else {
                continue;
            };
            let (Some((p0, ty)), Some((p1, _))) = (
                a.socket(Side::Out(link.out)),
                b.socket(Side::In(link.input)),
            ) else {
                continue;
            };
            let color = viewer.socket_style(ty).color;
            let color = if a.dim || viewer.wire_dimmed(link) {
                color.gamma_multiply(0.35)
            } else {
                color
            };
            let (s0, s1) = (v.screen(p0), v.screen(p1));
            draw::wire(&painter, s0, s1, color, v.zoom);
            if let Some(text) = viewer.wire_badge(link)
                && v.zoom > 0.45
            {
                draw::badge(&painter, s0 + (s1 - s0) * 0.5, text, v.zoom, style);
            }
        }

        // Nodes: unselected first, so the selection is drawn on top.
        let mut order: Vec<&LaidOf<V>> = laid.iter().collect();
        order.sort_by_key(|l| state.selected.contains(&l.id));
        let boxes = v.zoom >= style.box_zoom;
        let mut node_hits = Vec::new();
        for l in &order {
            let selected = state.selected.contains(&l.id);
            draw::node(&painter, l, v, selected, boxes, style, &socket_style);
            let mut r = ui.interact(
                v.rect(l.rect),
                ui.scope_id().with(("node", l.id)),
                Sense::click_and_drag(),
            );
            if let Some(issue) = &l.issue {
                let head = Rect::from_min_size(
                    v.screen(l.rect.min),
                    egui::vec2(l.rect.width(), style.header_height) * v.zoom,
                );
                if ui.rect_contains_pointer(head) {
                    r = r.on_hover_text(issue);
                }
            }
            node_hits.push((l.id, r));
        }
        // A wire being drawn: which sockets could take it.
        let wire_end = match state.drag {
            Some(Drag::Wire { end, .. }) => Some(end),
            _ => None,
        };
        let takes = |row: &draw::Row<V::In, V::Out, V::Socket>| -> bool {
            match (wire_end, row.side) {
                (Some(LooseEnd::Out(_, _, from)), Side::In(_)) => {
                    row.any_type || viewer.can_connect(from, row.ty)
                }
                (Some(LooseEnd::In(_, _, to)), Side::Out(_)) => {
                    row.any_type || viewer.can_connect(row.ty, to)
                }
                _ => false,
            }
        };
        let mut socket_hits = Vec::new();
        for l in &order {
            for row in &l.rows {
                let Some((p, _)) = l.socket(row.side) else {
                    continue;
                };
                if wire_end.is_some() && takes(row) {
                    painter.circle_stroke(
                        v.screen(p),
                        (style.socket_radius + 2.5) * v.zoom,
                        Stroke::new(1.2, style.text.gamma_multiply(0.6)),
                    );
                }
                let r = ui.interact(
                    Rect::from_center_size(v.screen(p), Vec2::splat(style.socket_hit * 1.4)),
                    ui.scope_id()
                        .with(("socket", l.id, format!("{:?}", row.side_key()))),
                    Sense::drag(),
                );
                socket_hits.push((l.id, row.side, row.ty, r));
            }
        }

        // Values no wire feeds: boxes to type into.
        if boxes {
            let mut ui_boxes = ui.new_child(egui::UiBuilder::new().max_rect(area));
            ui_boxes.set_clip_rect(area.intersect(ui.clip_rect()));
            // A theme may keep a text field at least a button high: the boxes fit their rows.
            ui_boxes.spacing_mut().interact_size.y = 0.0;
            let base = ui.scope_id().with(("value", &state.scope));
            let number = |s: &str| viewer.parse_number(s);
            for l in &order {
                let r = v.rect(l.rect);
                let mut fields = Vec::new();
                if let Some(own) = l.own {
                    fields.push((
                        ValueTarget::Own,
                        draw::own_box(r, v.zoom, style),
                        own,
                        "own".to_owned(),
                    ));
                }
                for row in &l.rows {
                    if let (Some(inline), Side::In(key)) = (row.inline, row.side) {
                        fields.push((
                            ValueTarget::Input(key),
                            draw::value_box(r, row.y, &inline, v.zoom, style),
                            inline,
                            format!("{key:?}"),
                        ));
                    }
                }
                for (target, rect, inline, name) in fields {
                    let id = base.with((l.id, name));
                    let done = value_field(
                        &mut ui_boxes,
                        id,
                        rect,
                        inline.value,
                        inline.scrub,
                        v.zoom,
                        style,
                        self.labels,
                        &number,
                    );
                    for (value, dragging) in [(done.set, false), (done.drag, true)] {
                        if let Some(value) = value {
                            edits.push(Action::SetValue {
                                node: l.id,
                                target,
                                value,
                                dragging,
                            });
                        }
                    }
                    if done.touched && !state.selected.contains(&l.id) {
                        state.selected = [l.id].into();
                        state.menu = None;
                    }
                }
            }
        }

        let shift = ui.input(|i| i.modifiers.shift);
        // Nodes: click to select, drag to move, double-click a custom node to go in.
        for (id, r) in &node_hits {
            if r.clicked() {
                if shift {
                    if !state.selected.remove(id) {
                        state.selected.insert(*id);
                    }
                } else {
                    state.selected = [*id].into();
                }
                state.menu = None;
            }
            if r.double_clicked() && viewer.enterable(*id) {
                state.enter(*id);
            }
            if r.drag_started_by(PointerButton::Primary) {
                if !state.selected.contains(id) {
                    if !shift {
                        state.selected.clear();
                    }
                    state.selected.insert(*id);
                }
                state.drag = Some(Drag::Nodes);
            }
            if r.dragged_by(PointerButton::Primary) && state.drag == Some(Drag::Nodes) {
                let delta = r.drag_delta() / v.zoom;
                if delta != Vec2::ZERO {
                    edits.push(Action::Move(
                        state.selected.iter().copied().collect(),
                        delta,
                    ));
                }
            }
            if r.drag_stopped() && state.drag == Some(Drag::Nodes) {
                state.drag = None;
            }
        }
        // Sockets: drag to wire; dragging a wired input takes its wire along.
        for (id, side, ty, r) in &socket_hits {
            if r.drag_started() {
                state.drag = Some(match *side {
                    Side::Out(out) => Drag::Wire {
                        end: LooseEnd::Out(*id, out, *ty),
                        detached: None,
                    },
                    Side::In(input) => {
                        match links.iter().find(|l| l.to == *id && l.input == input) {
                            Some(link) => {
                                let from_ty = find(link.from)
                                    .and_then(|l| l.socket(Side::Out(link.out)))
                                    .map_or(*ty, |(_, t)| t);
                                Drag::Wire {
                                    end: LooseEnd::Out(link.from, link.out, from_ty),
                                    detached: Some((*id, input)),
                                }
                            }
                            None => Drag::Wire {
                                end: LooseEnd::In(*id, input, *ty),
                                detached: None,
                            },
                        }
                    }
                });
            }
        }
        // A wire being drawn: follow the pointer, and on release wire it to the socket under
        // it (or offer the nodes that could take it).
        if let Some(Drag::Wire { end, detached }) = state.drag {
            let target = pointer.unwrap_or(area.center());
            let (start, ty) = match end {
                LooseEnd::Out(n, k, ty) => (
                    find(n).and_then(|l| l.socket(Side::Out(k))).map(|(p, _)| p),
                    ty,
                ),
                LooseEnd::In(n, k, ty) => (
                    find(n).and_then(|l| l.socket(Side::In(k))).map(|(p, _)| p),
                    ty,
                ),
            };
            if let Some(start) = start {
                let s = v.screen(start);
                let color = viewer.socket_style(ty).color;
                match end {
                    LooseEnd::Out(..) => draw::wire(&painter, s, target, color, v.zoom),
                    LooseEnd::In(..) => draw::wire(&painter, target, s, color, v.zoom),
                }
            }
            if !ui.input(|i| i.pointer.any_down()) {
                state.drag = None;
                let hit = laid.iter().find_map(|l| {
                    l.rows.iter().find_map(|row| {
                        let (p, _) = l.socket(row.side)?;
                        (v.screen(p).distance(target) <= style.socket_hit).then_some((l.id, row))
                    })
                });
                let wire = match (end, hit) {
                    (LooseEnd::Out(from, out, _), Some((to, row))) if takes(row) => {
                        let Side::In(input) = row.side else {
                            unreachable!("takes() only takes inputs here")
                        };
                        Some(WireView {
                            from,
                            out,
                            to,
                            input,
                        })
                    }
                    (LooseEnd::In(to, input, _), Some((from, row))) if takes(row) => {
                        let Side::Out(out) = row.side else {
                            unreachable!("takes() only takes outputs here")
                        };
                        Some(WireView {
                            from,
                            out,
                            to,
                            input,
                        })
                    }
                    _ => None,
                };
                match (wire, detached, hit) {
                    // Dropped back where it was.
                    (Some(w), Some(old), _) if (w.to, w.input) == old => {}
                    (Some(w), Some(old), _) => edits.push(Action::Reconnect { old, new: w }),
                    (Some(w), None, _) => edits.push(Action::Connect(w)),
                    // On a socket that cannot take it: nothing changes.
                    (None, _, Some(_)) => {}
                    (None, old, None) => {
                        if let Some((node, input)) = old {
                            edits.push(Action::Disconnect(node, input));
                        }
                        if area.contains(target) {
                            state.menu = Some(AddMenu {
                                screen: target,
                                canvas: v.canvas(target),
                                query: String::new(),
                                from: Some(end),
                                focus: true,
                            });
                        }
                    }
                }
            } else {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
        }

        // The background: click to let go, drag a box to select, right- or middle-drag to pan,
        // right-click to add, the wheel to zoom.
        if bg.clicked() {
            state.selected.clear();
            state.menu = None;
        }
        if bg.drag_started_by(PointerButton::Primary)
            && let Some(p) = pointer
        {
            state.drag = Some(Drag::Box(p));
        }
        if bg.dragged_by(PointerButton::Secondary) || bg.dragged_by(PointerButton::Middle) {
            cam_next.pan += bg.drag_delta();
        }
        if bg.secondary_clicked()
            && let Some(p) = pointer
        {
            state.open_add_menu(p, v.canvas(p));
        }
        if let Some(Drag::Box(start)) = state.drag {
            let now = pointer.unwrap_or(start);
            let r = Rect::from_two_pos(start, now);
            painter.rect(
                r,
                2.0,
                style.accent.gamma_multiply(0.08),
                Stroke::new(1.0, style.accent),
                StrokeKind::Inside,
            );
            if !ui.input(|i| i.pointer.any_down()) {
                state.drag = None;
                if !shift {
                    state.selected.clear();
                }
                for l in &laid {
                    if r.intersects(v.rect(l.rect)) {
                        state.selected.insert(l.id);
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
                let zoom = (cam.zoom * factor).clamp(style.min_zoom, style.max_zoom);
                let at = v.canvas(p);
                cam_next.zoom = zoom;
                cam_next.pan = p - area.min - at.to_vec2() * zoom;
            }
        }

        // Keys, with the pointer over the editor. Tab is also egui's key for moving the
        // focus: only a text field keeps the keys.
        if hovered && !ui.ctx().text_edit_focused() && state.menu.is_none() {
            self.keys(ui, viewer, state, &laid, area, v, pointer, &mut edits);
        }
        if state.cameras.get(&cam_key) == Some(&cam) {
            state.cameras.insert(cam_key, cam_next);
        }
        if state.selected.iter().any(|id| find(*id).is_none()) {
            state.selected.retain(|id| find(*id).is_some());
        }

        if let Some(menu) = state.menu.take() {
            state.menu = self.add_menu(ui, viewer, menu, &mut edits);
        }

        if laid.is_empty()
            && let Some(hint) = viewer.empty_hint()
        {
            painter.text(
                area.center(),
                Align2::CENTER_CENTER,
                hint,
                FontId::proportional(12.0),
                style.muted,
            );
        }
        edits
    }

    /// The keys the editor takes while the pointer is over it.
    #[allow(clippy::too_many_arguments)]
    fn keys<V: Viewer>(
        &self,
        ui: &Ui,
        viewer: &V,
        state: &mut State<V>,
        laid: &[LaidOf<V>],
        area: Rect,
        v: View,
        pointer: Option<Pos2>,
        edits: &mut Edits<V>,
    ) {
        let k = self.keys;
        // The shortcuts with more modifiers first: egui matches Ctrl+G when Ctrl+Alt+G is
        // pressed, so ungrouping must be asked before grouping.
        let mut all: Vec<(usize, KeyboardShortcut)> = [
            &k.delete,
            &k.duplicate,
            &k.bypass,
            &k.group,
            &k.ungroup,
            &k.enter_exit,
            &k.frame_all,
            &k.cancel,
            &k.add,
            &k.select_all,
        ]
        .into_iter()
        .enumerate()
        .flat_map(|(n, list)| list.iter().map(move |s| (n, *s)))
        .collect();
        let weight = |m: egui::Modifiers| {
            usize::from(m.alt) + usize::from(m.shift) + usize::from(m.command || m.ctrl)
        };
        all.sort_by_key(|(_, s)| std::cmp::Reverse(weight(s.modifiers)));
        let mut pressed = [false; 10];
        ui.input_mut(|i| {
            for (n, s) in &all {
                if !pressed.iter().any(|p| *p) && i.consume_shortcut(s) {
                    pressed[*n] = true;
                }
            }
        });
        let [
            delete,
            duplicate,
            bypass,
            group,
            ungroup,
            enter_exit,
            frame_all,
            cancel,
            add,
            select_all,
        ] = pressed;
        let selected = state.selected.clone();
        if delete && !selected.is_empty() {
            edits.push(Action::Remove(selected.clone()));
        }
        if duplicate && !selected.is_empty() {
            edits.push(Action::Duplicate(selected.clone()));
        }
        if bypass && !selected.is_empty() {
            edits.push(Action::Bypass(selected.clone()));
        }
        if ungroup && let Some(id) = state.single() {
            edits.push(Action::Ungroup(id));
        }
        if group && !selected.is_empty() {
            edits.push(Action::Group(selected));
        }
        if enter_exit {
            // egui moved the focus on Tab as the frame began: give it back.
            ui.ctx().memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            match state.single() {
                Some(id) if viewer.enterable(id) => state.enter(id),
                _ => {
                    if let Some(id) = state.scope.pop() {
                        state.selected = [id].into();
                    }
                }
            }
        }
        if frame_all {
            state.cameras.insert(
                (self.context, state.scope.clone()),
                draw::frame_all(laid.iter().map(|l| l.rect), area, self.style.min_zoom),
            );
        }
        if cancel {
            if state.drag.take().is_none() && state.selected.is_empty() {
                if let Some(id) = state.scope.pop() {
                    state.selected = [id].into();
                }
            } else {
                state.selected.clear();
            }
        }
        if add && let Some(p) = pointer {
            state.open_add_menu(p, v.canvas(p));
        }
        if select_all {
            state.selected = laid.iter().map(|l| l.id).collect();
        }
    }

    /// The add menu, at the pointer: a search field and the entries that match. Returns the
    /// menu while it stays open.
    fn add_menu<V: Viewer>(
        &self,
        ui: &mut Ui,
        viewer: &V,
        mut menu: AddMenu<V::In, V::Out, V::Socket>,
        edits: &mut Edits<V>,
    ) -> Option<AddMenu<V::In, V::Out, V::Socket>> {
        let style = self.style;
        let mut entries = viewer.add_menu();
        // With a wire: only what can take it.
        if let Some(end) = &menu.from {
            entries.retain(|e| viewer.auto_wire(&e.template, end).is_some());
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
                            .hint_text(&self.labels.search)
                            .desired_width(f32::INFINITY),
                    );
                    if std::mem::take(&mut menu.focus) {
                        field.request_focus();
                    }
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close = true;
                    }
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .show(ui, |ui| {
                            let mut last_group: Option<&str> = None;
                            for (k, e) in entries.iter().enumerate() {
                                if last_group != Some(e.group.as_str()) {
                                    last_group = Some(&e.group);
                                    ui.label(RichText::new(&e.group).size(10.5).color(style.muted));
                                }
                                if ui.selectable_label(false, &e.label).clicked()
                                    || (enter && k == 0)
                                {
                                    chosen = Some(k);
                                }
                            }
                            if entries.is_empty() {
                                ui.label(RichText::new(&self.labels.no_match).color(style.muted));
                            }
                        });
                });
            });
        if !opening && area.response.clicked_elsewhere() && chosen.is_none() {
            close = true;
        }
        if let Some(k) = chosen {
            let entry = entries.swap_remove(k);
            let wire = menu.from.as_ref().and_then(|end: &End<V>| {
                Some((end.node(), viewer.auto_wire(&entry.template, end)?))
            });
            edits.push(Action::Add {
                template: entry.template,
                pos: [menu.canvas.x, menu.canvas.y],
                wire,
            });
            return None;
        }
        (!close).then_some(menu)
    }
}

impl<I: std::fmt::Debug, O: std::fmt::Debug, S> draw::Row<I, O, S> {
    /// The socket's key, for telling widgets apart.
    fn side_key(&self) -> String
    where
        I: Copy,
        O: Copy,
    {
        match self.side {
            Side::Out(o) => format!("out {o:?}"),
            Side::In(i) => format!("in {i:?}"),
        }
    }
}
