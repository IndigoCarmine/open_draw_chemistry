//! The 構築 tab's pipeline: the base monomer, then each step as a card on a vertical line with
//! the running count of molecules between them, and the box that adds a step. A card is
//! reordered by dragging its handle; the expanded card (one at most) shows its parameters.

use egui::{Margin, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, vec2};

use super::style::{self, Look};
use super::{AssemblyView, Change, Ink};
use crate::assembly::model::{Array, Ring, Stack};
use crate::assembly::{AssemblyState, Step, symop};
use crate::ui::icons::{self, Icon};

/// Width left of the cards, where the line and its nodes are.
const GUTTER: f32 = 22.0;
/// The line's distance from the panel's left edge.
const LINE_X: f32 = 7.0;
const NODE_RADIUS: f32 = 4.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Node {
    /// The monomer, and the counts between steps: a hollow circle.
    Hollow,
    /// The final count: a filled accent circle.
    Last,
}

/// What one step's card asked for this frame.
#[derive(Debug)]
struct CardOutcome {
    /// The card as drawn (the gutter left out).
    rect: Rect,
    /// A parameter changed in place.
    changed: bool,
    toggle_expanded: bool,
    drag_started: bool,
    drag_stopped: bool,
}

/// The monomer node, the step cards and the counts between them. Does nothing without a base
/// monomer.
pub(super) fn pipeline(
    ui: &mut Ui,
    state: &mut AssemblyState,
    view: &mut AssemblyView,
    look: &Look,
    ink: &Ink,
    changes: &mut Vec<Change>,
) {
    let Some(base) = state.design.components.get(state.design.base) else {
        return;
    };
    let first = format!("{} × 1", super::formula(base));
    let counts = state.design.running_counts();
    let len = state.design.steps.len();
    if view.expanded.is_some_and(|e| e >= len) {
        view.expanded = None;
    }

    let saved = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 2.0;
    // The line goes behind the nodes: its place in the paint order is taken now.
    let line = ui.painter().add(Shape::Noop);
    let first_kind = if len == 0 { Node::Last } else { Node::Hollow };
    let mut nodes = vec![node(ui, look, first_kind, look.mono(first, look.muted))];

    let drag_id = ui.scope_id().with("assembly_step_drag");
    let dragging = ui
        .data(|d| d.get_temp::<usize>(drag_id))
        .filter(|&from| from < len);
    let mut cards = Vec::with_capacity(len);
    let mut released = false;
    let mut changed = false;
    let mut before = 1;
    for (i, step) in state.design.steps.iter_mut().enumerate() {
        let expanded = view.expanded == Some(i);
        let outcome = ui
            .push_id(("step", i), |ui| {
                step_card(ui, look, ink, i, step, expanded, changes)
            })
            .inner;
        changed |= outcome.changed;
        if outcome.toggle_expanded {
            view.expanded = if expanded { None } else { Some(i) };
        }
        if outcome.drag_started {
            ui.data_mut(|d| d.insert_temp(drag_id, i));
        }
        released |= outcome.drag_stopped;
        if dragging == Some(i) {
            ui.painter().rect_filled(
                outcome.rect,
                f32::from(style::CARD_RADIUS),
                look.accent.gamma_multiply(0.10),
            );
        }
        cards.push(outcome.rect);

        let n = counts.get(i).copied().unwrap_or(0);
        let (kind, color) = if i + 1 == len {
            (Node::Last, look.accent)
        } else {
            (Node::Hollow, look.node)
        };
        nodes.push(node(
            ui,
            look,
            kind,
            look.mono(count_text(step, before, n), color),
        ));
        before = n;
    }
    ui.spacing_mut().item_spacing.y = saved;
    if changed {
        state.touch();
    }
    if let (Some(&top), Some(&bottom)) = (nodes.first(), nodes.last())
        && nodes.len() > 1
    {
        ui.painter().set(
            line,
            Shape::line_segment([top, bottom], Stroke::new(1.5, look.line)),
        );
    }

    // A card being dragged: where it would land, and the move once it is dropped.
    if let Some(from) = dragging {
        let gap = ui
            .input(|i| i.pointer.latest_pos())
            .map(|p| gap_at(&cards, p.y));
        let target = gap.and_then(|gap| drop_target(from, gap).map(|to| (gap, to)));
        if released || !ui.input(|i| i.pointer.any_down()) {
            ui.data_mut(|d| d.remove::<usize>(drag_id));
            if released && let Some((_, to)) = target {
                changes.push(Change::MoveStep(from, to));
            }
        } else {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some((gap, _)) = target
                && let Some(y) = gap_y(&cards, gap)
            {
                let x = cards[0].x_range();
                ui.painter().hline(x, y, Stroke::new(2.0, look.accent));
                ui.painter()
                    .circle_filled(egui::pos2(x.min, y), 3.0, look.accent);
            }
        }
    }
}

/// One row of the pipeline: a node on the line and its text. Returns the node's centre.
fn node(ui: &mut Ui, look: &Look, kind: Node, text: RichText) -> Pos2 {
    ui.horizontal(|ui| {
        let width = GUTTER - ui.spacing().item_spacing.x;
        let (rect, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
        let centre = egui::pos2(rect.left() + LINE_X, rect.center().y);
        let painter = ui.painter();
        match kind {
            Node::Hollow => {
                painter.circle_filled(centre, NODE_RADIUS, look.back);
                painter.circle_stroke(centre, NODE_RADIUS, Stroke::new(1.5, look.line));
            }
            Node::Last => {
                painter.circle_filled(centre, NODE_RADIUS + 0.5, look.accent);
            }
        }
        ui.label(text);
        centre
    })
    .inner
}

/// A step's card: the handle, its name and summary, its buttons, and when `expanded` its
/// parameters.
fn step_card(
    ui: &mut Ui,
    look: &Look,
    ink: &Ink,
    i: usize,
    step: &mut Step,
    expanded: bool,
    changes: &mut Vec<Change>,
) -> CardOutcome {
    let mut outcome = CardOutcome {
        rect: Rect::NOTHING,
        changed: false,
        toggle_expanded: false,
        drag_started: false,
        drag_stopped: false,
    };
    let enabled = step.enabled();
    let frame = style::card(ui, look, expanded, GUTTER, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (handle_rect, handle) = ui.allocate_exact_size(vec2(12.0, 30.0), Sense::drag());
            let handle = handle
                .on_hover_cursor(egui::CursorIcon::Grab)
                .on_hover_text("ドラッグで並べ替え");
            let color = if handle.hovered() || handle.dragged() {
                look.ink
            } else {
                look.muted
            };
            icons::paint(
                ui.painter(),
                Rect::from_center_size(handle_rect.center(), vec2(12.0, 12.0)),
                Icon::DragHandle,
                color,
            );
            outcome.drag_started = handle.drag_started();
            outcome.drag_stopped = handle.drag_stopped();

            let title = if enabled { look.ink } else { look.muted };
            let text = ui
                .vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(step.label()).size(13.0).strong().color(title));
                    ui.label(look.mono(summary(step), look.muted));
                })
                .response
                .rect;

            let buttons = ui
                .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 1.0;
                    let (chevron, tip) = if expanded {
                        (Icon::ChevronUp, "パラメータを閉じる")
                    } else {
                        (Icon::ChevronDown, "パラメータを開く")
                    };
                    if style::tool(ui, look, chevron, tip, true, false, false).clicked() {
                        outcome.toggle_expanded = true;
                    }
                    if style::tool(ui, look, Icon::Close, "この手順を削除", true, true, false)
                        .clicked()
                    {
                        changes.push(Change::RemoveStep(i));
                    }
                    let lattice = matches!(step, Step::Lattice(_));
                    let tip = if lattice {
                        "結晶の手順は 1 つだけです"
                    } else {
                        "この手順を複製（すぐ後ろに入れます）"
                    };
                    if style::tool(ui, look, Icon::Duplicate, tip, !lattice, false, false).clicked()
                    {
                        changes.push(Change::DuplicateStep(i));
                    }
                    let (toggle, tip) = if enabled {
                        (Icon::ToggleOn, "使っています（クリックで飛ばす）")
                    } else {
                        (Icon::ToggleOff, "飛ばしています（クリックで使う）")
                    };
                    if style::tool(ui, look, toggle, tip, true, false, enabled).clicked() {
                        changes.push(Change::ToggleStep(i));
                    }
                })
                .response
                .rect;

            // The rest of the header opens and closes the card.
            let head = Rect::from_min_max(
                egui::pos2(text.left(), handle_rect.top()),
                egui::pos2(
                    (buttons.left() - 2.0).max(text.left()),
                    handle_rect.bottom(),
                ),
            );
            if ui
                .interact(head, ui.scope_id().with("head"), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                outcome.toggle_expanded = true;
            }
        });
        if expanded {
            ui.add_space(2.0);
            let y = ui.cursor().top();
            ui.painter()
                .hline(ui.max_rect().x_range(), y, Stroke::new(1.0, look.border));
            ui.add_space(6.0);
            ui.add_enabled_ui(enabled, |ui| {
                outcome.changed |= super::step_params(ui, step, ink);
            });
        }
    });
    let mut rect = frame.response.rect;
    rect.min.x += GUTTER;
    outcome.rect = rect;
    outcome
}

/// The step's parameters in a few characters: "7個 · z 3.20Å".
pub(super) fn summary(step: &Step) -> String {
    match step {
        Step::Array(Array {
            n,
            offset,
            euler_deg,
            ..
        }) => {
            let mut s = format!("{n}個 · {}", offset_text(offset));
            if euler_deg.iter().any(|&a| a != 0.0) {
                s.push_str(" · 回転");
            }
            s
        }
        Step::Ring(Ring {
            n,
            radius,
            tilt_deg,
            ..
        }) => {
            let mut s = format!("{n}個 · r {radius:.2}Å");
            if *tilt_deg != 0.0 {
                s.push_str(&format!(" · {tilt_deg:.1}°"));
            }
            s
        }
        Step::Stack(Stack {
            n,
            rise,
            twist_deg,
            slip,
            ..
        }) => {
            let mut s = format!("{n}個 · Δz {rise:.2}Å · {twist_deg:.1}°");
            if *slip != 0.0 {
                s.push_str(&format!(" · ずれ {slip:.2}Å"));
            }
            s
        }
        Step::Lattice(l) => {
            let group = symop::matching_preset(&l.symops)
                .map_or_else(|| format!("{} 操作", l.symops.len()), |p| p.name.to_owned());
            let [a, b, c] = l.reps.map(|r| r.max(1));
            format!("{group} · {a}×{b}×{c}")
        }
    }
}

/// An array's spacing: "z 3.20Å" along one axis, all three otherwise.
fn offset_text(offset: &[f64; 3]) -> String {
    let along: Vec<(&str, f64)> = ["x", "y", "z"]
        .into_iter()
        .zip(offset.iter().copied())
        .filter(|&(_, v)| v != 0.0)
        .collect();
    match along.as_slice() {
        [] => "間隔 0Å".to_owned(),
        [(axis, v)] => format!("{axis} {v:.2}Å"),
        _ => format!("({:.1}, {:.1}, {:.1})Å", offset[0], offset[1], offset[2]),
    }
}

/// The text after a step: what it multiplies by and the molecules so far ("× 6 → 42 分子"),
/// from `before` molecules to `after`.
pub(super) fn count_text(step: &Step, before: usize, after: usize) -> String {
    if !step.enabled() {
        return format!("飛ばす → {after} 分子");
    }
    match step {
        Step::Lattice(l) if l.check().is_err() => format!("作れません → {after} 分子"),
        // A crystal drops copies on special positions: its factor need not be whole.
        Step::Lattice(_) if before == 0 || !after.is_multiple_of(before) => {
            format!("→ {after} 分子")
        }
        Step::Lattice(_) => format!("× {} → {after} 分子", after / before),
        _ => format!("× {} → {after} 分子", step.copies()),
    }
}

/// The gap between cards (0 before the first, `cards.len()` after the last) that height `y`
/// falls in: past a card's middle is after it.
pub(super) fn gap_at(cards: &[Rect], y: f32) -> usize {
    cards.iter().filter(|r| r.center().y < y).count()
}

/// Where a card dragged from `from` and dropped in `gap` ends up, or `None` when it would not
/// move.
pub(super) fn drop_target(from: usize, gap: usize) -> Option<usize> {
    let to = if gap > from { gap - 1 } else { gap };
    (to != from).then_some(to)
}

/// Where the drop line for `gap` is drawn: halfway between the cards around it.
fn gap_y(cards: &[Rect], gap: usize) -> Option<f32> {
    let above = gap
        .checked_sub(1)
        .and_then(|k| cards.get(k))
        .map(|r| r.bottom());
    let below = cards.get(gap).map(|r| r.top());
    match (above, below) {
        (Some(a), Some(b)) => Some((a + b) / 2.0),
        (Some(a), None) => Some(a + 4.0),
        (None, Some(b)) => Some(b - 4.0),
        (None, None) => None,
    }
}

/// Where index `e` of a list is after the item at `from` moves to `to`.
pub(super) fn index_after_move(e: usize, from: usize, to: usize) -> usize {
    if e == from {
        to
    } else if from < e && e <= to {
        e - 1
    } else if to <= e && e < from {
        e + 1
    } else {
        e
    }
}

/// A chip of the add box: its name, tooltip, whether it can be used, and the step it adds.
type Chip = (&'static str, &'static str, bool, fn() -> Step);

/// The dashed box that adds a step: one chip per kind (a second crystal is not allowed).
pub(super) fn add_box(ui: &mut Ui, look: &Look, state: &AssemblyState, changes: &mut Vec<Change>) {
    let has_lattice = state
        .design
        .steps
        .iter()
        .any(|s| matches!(s, Step::Lattice(_)));
    let inner = egui::Frame::new()
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("＋ 手順を追加").size(12.5).color(look.muted));
            });
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                let chips: [Chip; 4] = [
                    (
                        "配列",
                        "それまでの全体を一定の間隔と回転で並べます",
                        true,
                        Step::new_array,
                    ),
                    (
                        "環状",
                        "分子を z 軸まわりの円に並べます（ロゼット）",
                        true,
                        Step::new_ring,
                    ),
                    (
                        "積層・らせん",
                        "それまでの全体を z 方向に積み重ねます。ねじれとずれでらせんになります",
                        true,
                        Step::new_stack,
                    ),
                    (
                        "結晶",
                        "それまでの全体を非対称単位として、格子定数と空間群の対称操作で単位胞を作り、繰り返します",
                        !has_lattice,
                        Step::new_lattice,
                    ),
                ];
                for (name, tip, enabled, make) in chips {
                    let chip = egui::Button::new(RichText::new(name).size(12.0))
                        .corner_radius(12.0)
                        .min_size(vec2(0.0, 24.0));
                    if ui
                        .add_enabled(enabled, chip)
                        .on_hover_text(tip)
                        .on_disabled_hover_text("結晶の手順はすでにあります")
                        .clicked()
                    {
                        changes.push(Change::AddStep(make()));
                    }
                }
            });
        });
    style::dashed_box(ui, inner.response.rect, look.line);
}
