//! How nodes, wires and the grid are drawn, and where on a node each socket and box is.

use egui::epaint::CubicBezierShape;
use egui::{Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind};
use egui::{Vec2, pos2, vec2};

use super::value::value_text;
use super::{Camera, EditorStyle, Inline, NodeView, SocketShape, SocketStyle};
use crate::id::NodeId;

/// Which side of a node a socket is on, and its key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Side<I, O> {
    Out(O),
    In(I),
}

/// One socket row as laid out.
#[derive(Debug, Clone)]
pub(super) struct Row<I, O, S> {
    pub side: Side<I, O>,
    pub ty: S,
    pub label: String,
    pub inline: Option<Inline>,
    pub ghost: bool,
    pub any_type: bool,
    /// Canvas y of the row's middle, from the node's top.
    pub y: f32,
}

/// A node as laid out on the canvas: its box and its rows.
#[derive(Debug, Clone)]
pub(super) struct Laid<I, O, S> {
    pub id: NodeId,
    pub rect: Rect,
    pub title: String,
    pub subtitle: Option<String>,
    pub header: Color32,
    pub own: Option<Inline>,
    pub rows: Vec<Row<I, O, S>>,
    pub bypass: bool,
    pub dim: bool,
    pub error: bool,
    pub warning: bool,
    /// What the ⚠ says when hovered.
    pub issue: Option<String>,
}

impl<I: Copy + PartialEq, O: Copy + PartialEq, S: Copy> Laid<I, O, S> {
    /// Lay `n` out.
    pub fn of(n: NodeView<I, O, S>, style: &EditorStyle) -> Self {
        let row_h = style.row_height;
        let mut rows = Vec::new();
        let mut y = style.header_height
            + if n.subtitle.is_some() || n.own.is_some() {
                row_h
            } else {
                0.0
            }
            + row_h / 2.0
            + 2.0;
        for o in n.outputs {
            rows.push(Row {
                side: Side::Out(o.key),
                ty: o.ty,
                label: o.label,
                inline: None,
                ghost: o.ghost,
                any_type: o.any_type,
                y,
            });
            y += row_h;
        }
        for i in n.inputs {
            rows.push(Row {
                side: Side::In(i.key),
                ty: i.ty,
                label: i.label,
                inline: i.inline,
                ghost: i.ghost,
                any_type: i.any_type,
                y,
            });
            y += row_h;
        }
        let height = y - row_h / 2.0 + 6.0;
        Laid {
            id: n.id,
            rect: Rect::from_min_size(n.pos, vec2(style.node_width, height)),
            title: n.title,
            subtitle: n.subtitle,
            header: n.header,
            own: n.own,
            rows,
            bypass: n.bypass,
            dim: n.dim,
            error: n.error.is_some(),
            warning: n.warning.is_some(),
            issue: n.error.or(n.warning),
        }
    }

    /// Where socket `side` is on the canvas, and its type.
    pub fn socket(&self, side: Side<I, O>) -> Option<(Pos2, S)> {
        self.rows.iter().find(|r| r.side == side).map(|r| {
            let x = match side {
                Side::Out(_) => self.rect.right(),
                Side::In(_) => self.rect.left(),
            };
            (pos2(x, self.rect.top() + r.y), r.ty)
        })
    }
}

/// The screen transform of a camera over `area`.
#[derive(Clone, Copy)]
pub(super) struct View {
    pub origin: Pos2,
    pub zoom: f32,
}

impl View {
    pub fn of(area: Rect, cam: Camera) -> View {
        View {
            origin: area.min + cam.pan,
            zoom: cam.zoom,
        }
    }

    pub fn screen(&self, p: Pos2) -> Pos2 {
        self.origin + p.to_vec2() * self.zoom
    }

    pub fn rect(&self, r: Rect) -> Rect {
        Rect::from_min_max(self.screen(r.min), self.screen(r.max))
    }

    pub fn canvas(&self, p: Pos2) -> Pos2 {
        ((p - self.origin) / self.zoom).to_pos2()
    }
}

/// A camera showing every node in `rects` in `area` (at most life size).
pub(super) fn frame_all(rects: impl Iterator<Item = Rect>, area: Rect, min_zoom: f32) -> Camera {
    let Some(bounds) = rects.reduce(|a, b| a.union(b)) else {
        return Camera::default();
    };
    let bounds = bounds.expand(24.0);
    let zoom = (area.width() / bounds.width())
        .min(area.height() / bounds.height())
        .clamp(min_zoom, 1.0);
    let centre = bounds.center().to_vec2() * zoom;
    Camera {
        pan: area.size() / 2.0 - centre,
        zoom,
    }
}

pub(super) fn grid(painter: &Painter, area: Rect, v: View, style: &EditorStyle) {
    let step = style.grid * v.zoom;
    if step < 8.0 {
        return;
    }
    let color = style.border.gamma_multiply(0.5);
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

pub(super) fn wire(painter: &Painter, a: Pos2, b: Pos2, color: Color32, zoom: f32) {
    let d = ((b.x - a.x).abs() * 0.5).max(40.0 * zoom);
    let shape = CubicBezierShape::from_points_stroke(
        [a, a + vec2(d, 0.0), b - vec2(d, 0.0), b],
        false,
        Color32::TRANSPARENT,
        Stroke::new(2.0 * zoom.clamp(0.6, 1.4), color),
    );
    painter.add(shape);
}

/// A note on a wire, at its middle.
pub(super) fn badge(painter: &Painter, at: Pos2, text: String, zoom: f32, style: &EditorStyle) {
    let galley = painter.layout_no_wrap(text, FontId::monospace(10.0 * zoom.min(1.2)), style.text);
    let r = Rect::from_center_size(at, galley.size() + vec2(8.0, 2.0));
    painter.rect_filled(r, 6.0, style.node);
    painter.galley(r.center() - galley.size() / 2.0, galley, style.text);
}

/// Where the box of an unwired input's value goes, the node being at `r` on the screen.
pub(super) fn value_box(r: Rect, y: f32, inline: &Inline, z: f32, style: &EditorStyle) -> Rect {
    let width = if matches!(inline.value, super::Value::Vec3(_)) {
        96.0
    } else {
        58.0
    };
    let unit = if inline.unit.is_empty() { 0.0 } else { 12.0 };
    let right = r.right() - (6.0 + unit) * z;
    let half = style.row_height / 2.0 - 1.5;
    Rect::from_min_max(
        pos2(right - width * z, r.top() + (y - half) * z),
        pos2(right, r.top() + (y + half) * z),
    )
}

/// Where the box of a node's own value goes (the subtitle's line).
pub(super) fn own_box(r: Rect, z: f32, style: &EditorStyle) -> Rect {
    let top = r.top() + (style.header_height + 1.5) * z;
    Rect::from_min_max(
        pos2(r.left() + 7.0 * z, top),
        pos2(r.right() - 7.0 * z, top + (style.row_height - 3.0) * z),
    )
}

/// A socket at `p`, `r` across (the smaller shapes a little less).
pub(super) fn socket(painter: &Painter, p: Pos2, s: SocketStyle, r: f32) {
    let small = r * 8.0 / 9.0;
    match s.shape {
        SocketShape::Circle => {
            painter.circle_filled(p, r, s.color);
        }
        SocketShape::SmallCircle => {
            painter.circle_filled(p, small, s.color);
        }
        SocketShape::Square => {
            painter.rect_filled(
                Rect::from_center_size(p, Vec2::splat(2.0 * small)),
                1.0,
                s.color,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn node<I, O, S: Copy>(
    painter: &Painter,
    l: &Laid<I, O, S>,
    v: View,
    selected: bool,
    boxes: bool,
    style: &EditorStyle,
    socket_style: &dyn Fn(S) -> SocketStyle,
) {
    let r = v.rect(l.rect);
    let z = v.zoom;
    let radius = CornerRadius::same((6.0 * z).round().clamp(1.0, 10.0) as u8);
    let alpha = if l.dim { 0.55 } else { 1.0 };
    let body = style.node.gamma_multiply(alpha);
    let border = if selected {
        Stroke::new(2.0, style.accent)
    } else if l.error {
        Stroke::new(1.5, style.error)
    } else {
        Stroke::new(1.0, style.border)
    };
    painter.rect(r, radius, body, Stroke::NONE, StrokeKind::Inside);
    let head = Rect::from_min_size(r.min, vec2(r.width(), style.header_height * z));
    let head_color = if l.bypass { style.bypassed } else { l.header };
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
    let title_color = if l.bypass { style.muted } else { style.text };
    let title = if l.bypass {
        format!("{} ⏸", l.title)
    } else {
        l.title.clone()
    };
    painter.with_clip_rect(head.shrink(2.0)).text(
        head.left_center() + vec2(7.0 * z, 0.0),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(12.0 * z),
        title_color,
    );
    let marker = if l.error {
        Some(style.error)
    } else if l.warning {
        Some(style.warning)
    } else {
        None
    };
    if let Some(c) = marker {
        text(
            head.right_center() - vec2(7.0 * z, 0.0),
            Align2::RIGHT_CENTER,
            "⚠",
            12.0,
            c,
        );
    }
    let sub = match (&l.subtitle, l.own) {
        // The own value's box covers its line.
        (_, Some(_)) if boxes => None,
        (Some(s), _) => Some(s.clone()),
        (None, Some(own)) => Some(value_text(own.value, own.unit)),
        (None, None) => None,
    };
    if let Some(sub) = sub {
        painter.with_clip_rect(r.shrink(3.0)).text(
            pos2(
                r.left() + 7.0 * z,
                head.bottom() + style.row_height * z / 2.0 + 1.0,
            ),
            Align2::LEFT_CENTER,
            sub,
            FontId::monospace(10.5 * z),
            style.muted,
        );
    }
    for row in &l.rows {
        let y = r.top() + row.y * z;
        let (sx, align, lx) = match row.side {
            Side::Out(_) => (r.right(), Align2::RIGHT_CENTER, r.right() - 10.0 * z),
            Side::In(_) => (r.left(), Align2::LEFT_CENTER, r.left() + 10.0 * z),
        };
        let s = socket_style(row.ty);
        let p = pos2(sx, y);
        if row.ghost {
            painter.circle_stroke(
                p,
                style.socket_radius * z,
                Stroke::new(1.2, s.color.gamma_multiply(0.7)),
            );
            text(pos2(lx, y), align, "+", 11.0, style.muted);
            continue;
        }
        socket(painter, p, s, style.socket_radius * z);
        let typed = boxes && row.inline.is_some();
        let mut clip = r.shrink(2.0);
        if let (true, Some(inline)) = (typed, &row.inline) {
            clip.max.x = value_box(r, row.y, inline, z, style).left() - 3.0 * z;
        }
        painter.with_clip_rect(clip).text(
            pos2(lx, y),
            align,
            &row.label,
            FontId::proportional(11.0 * z),
            style.text.gamma_multiply(0.9),
        );
        let clip = painter.with_clip_rect(r.shrink(2.0));
        match (&row.inline, typed) {
            (Some(inline), true) => {
                clip.text(
                    pos2(r.right() - 5.0 * z, y),
                    Align2::RIGHT_CENTER,
                    inline.unit.trim(),
                    FontId::proportional(10.5 * z),
                    style.muted,
                );
            }
            (Some(inline), false) => {
                clip.text(
                    pos2(r.right() - 8.0 * z, y),
                    Align2::RIGHT_CENTER,
                    value_text(inline.value, inline.unit),
                    FontId::monospace(10.5 * z),
                    style.muted,
                );
            }
            (None, _) => {}
        }
    }
}
