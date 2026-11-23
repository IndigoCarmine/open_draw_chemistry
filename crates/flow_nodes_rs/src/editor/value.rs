//! Values in boxes on the canvas: dragged to change them, clicked to type them.

use egui::{Align2, CornerRadius, FontId, Key, PointerButton, Rect, Sense, Stroke, StrokeKind, Ui};
use egui::{Vec2, vec2};

use super::{EditorStyle, Labels};

/// A value an input can have without a wire: a number or three.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Number(f64),
    Vec3([f64; 3]),
}

/// How dragging a value box changes the value: by `speed` a point, in steps of `grid`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scrub {
    pub speed: f64,
    pub grid: f64,
}

impl Scrub {
    /// Whole numbers.
    pub const COUNT: Scrub = Scrub {
        speed: 0.1,
        grid: 1.0,
    };
    /// Degrees.
    pub const ANGLE: Scrub = Scrub {
        speed: 0.5,
        grid: 0.5,
    };
    /// Distances.
    pub const LENGTH: Scrub = Scrub {
        speed: 0.05,
        grid: 0.05,
    };
    pub const PLAIN: Scrub = Scrub {
        speed: 0.01,
        grid: 0.01,
    };

    /// The change for `points` of drag (finer with Ctrl, unless it counts).
    fn by(self, points: f32, fine: bool) -> f64 {
        let (speed, grid) = if fine && self.grid < 1.0 {
            (self.speed / 10.0, self.grid / 10.0)
        } else {
            (self.speed, self.grid)
        };
        (f64::from(points) * speed / grid).round() * grid
    }
}

/// A number for a node: up to 3 decimals, trailing zeros dropped.
pub fn number_text(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// A value for a node, with its unit.
pub fn value_text(v: Value, unit: &str) -> String {
    match v {
        Value::Number(x) => format!("{}{unit}", number_text(x)),
        Value::Vec3([x, y, z]) => format!(
            "{}, {}, {}{unit}",
            number_text(x),
            number_text(y),
            number_text(z)
        ),
    }
}

/// A value typed in: a number or, for a vector, three numbers separated by commas (or one for
/// all three), each read by `number`. Commas inside brackets (`max(1, 2)`) belong to a
/// function.
pub fn parse_value(
    text: &str,
    vector: bool,
    number: &dyn Fn(&str) -> Option<f64>,
) -> Option<Value> {
    let number = |s: &str| number(s).filter(|x| x.is_finite());
    if !vector {
        return number(text).map(Value::Number);
    }
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
        [one] => number(one).map(|x| Value::Vec3([x; 3])),
        [x, y, z] => Some(Value::Vec3([number(x)?, number(y)?, number(z)?])),
        _ => None,
    }
}

/// `start` moved by a drag of `drag` (screen points, y down): a number by its x; a vector's x
/// by the drag's x and y by its upward y, or z instead of y with Shift.
pub(super) fn scrubbed(start: Value, drag: Vec2, scrub: Scrub, shift: bool, fine: bool) -> Value {
    match start {
        Value::Number(v) => Value::Number(v + scrub.by(drag.x, fine)),
        Value::Vec3([x, y, z]) => {
            let up = scrub.by(-drag.y, fine);
            let x = x + scrub.by(drag.x, fine);
            if shift {
                Value::Vec3([x, y, z + up])
            } else {
                Value::Vec3([x, y + up, z])
            }
        }
    }
}

/// What a value box did this frame.
#[derive(Debug, Default)]
pub(super) struct BoxResult {
    /// A value typed in (Enter, a click elsewhere).
    pub set: Option<Value>,
    /// A value being dragged.
    pub drag: Option<Value>,
    /// The box was taken hold of (to type in or to drag).
    pub touched: bool,
}

/// The box of a value on the canvas. Dragging it changes the value (a vector: x left and
/// right, y up and down, z up and down with Shift; finer with Ctrl); a click turns it into a
/// field to type in, kept while it has the focus and set when it lets go of it (Enter, a
/// click elsewhere); Esc puts the value back.
#[allow(clippy::too_many_arguments)]
pub(super) fn value_field(
    ui: &mut Ui,
    id: egui::Id,
    rect: Rect,
    value: Value,
    scrub: Scrub,
    z: f32,
    style: &EditorStyle,
    labels: &Labels,
    number: &dyn Fn(&str) -> Option<f64>,
) -> BoxResult {
    let vector = matches!(value, Value::Vec3(_));
    let tip = if vector {
        &labels.vector_tip
    } else {
        &labels.value_tip
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
            ui.data_mut(|d| d.insert_temp(start_id, StartValue(value)));
            out.touched = true;
        }
        if response.dragged_by(PointerButton::Primary)
            && let Some(StartValue(start)) = ui.data(|d| d.get_temp::<StartValue>(start_id))
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
            ui.data_mut(|d| d.remove::<StartValue>(start_id));
        }
        if response.clicked() {
            ui.data_mut(|d| d.insert_temp(typing_id, true));
            ui.memory_mut(|m| m.request_focus(id));
            out.touched = true;
        }
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(if vector {
                egui::CursorIcon::AllScroll
            } else {
                egui::CursorIcon::ResizeHorizontal
            });
        }
        let active = response.hovered() || response.dragged();
        let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
        let radius = CornerRadius::same((3.0 * z).round().clamp(1.0, 4.0) as u8);
        let back = if active {
            style.field_hover
        } else {
            style.background
        };
        painter.rect(
            rect,
            radius,
            back,
            Stroke::new(1.0, if active { style.muted } else { style.border }),
            StrokeKind::Inside,
        );
        painter.text(
            rect.right_center() - vec2(3.0 * z, 0.0),
            Align2::RIGHT_CENTER,
            value_text(value, ""),
            font,
            style.text,
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
    let color = if parse_value(&text, vector, number).is_some() {
        style.text
    } else {
        style.error
    };
    let response = ui
        .put(
            rect,
            egui::TextEdit::singleline(&mut text)
                .id(id)
                .font(font)
                .text_color(color)
                .background_color(style.background)
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
            && let Some(v) = parse_value(&text, vector, number)
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

/// The value a drag started from (kept in egui's memory while it goes on).
#[derive(Debug, Clone, Copy, PartialEq)]
struct StartValue(Value);

#[cfg(test)]
mod tests {
    use super::*;

    fn std_number(s: &str) -> Option<f64> {
        s.trim().parse().ok()
    }

    #[test]
    fn numbers_and_vectors_are_read_and_written() {
        assert_eq!(number_text(1.5), "1.5");
        assert_eq!(number_text(-0.0001), "0");
        assert_eq!(number_text(2.0), "2");
        assert_eq!(
            value_text(Value::Vec3([1.0, 2.5, 0.0]), " Å"),
            "1, 2.5, 0 Å"
        );
        assert_eq!(
            parse_value("3", false, &std_number),
            Some(Value::Number(3.0))
        );
        assert_eq!(
            parse_value("2", true, &std_number),
            Some(Value::Vec3([2.0; 3]))
        );
        assert_eq!(
            parse_value("1, 2, 3", true, &std_number),
            Some(Value::Vec3([1.0, 2.0, 3.0]))
        );
        assert_eq!(parse_value("1, 2", true, &std_number), None);
        assert_eq!(parse_value("x", false, &std_number), None);
    }

    #[test]
    fn a_drag_changes_by_steps() {
        let v = scrubbed(
            Value::Number(1.0),
            vec2(10.0, 0.0),
            Scrub::COUNT,
            false,
            false,
        );
        assert_eq!(v, Value::Number(2.0));
        let v = scrubbed(
            Value::Vec3([0.0; 3]),
            vec2(0.0, -20.0),
            Scrub::ANGLE,
            true,
            false,
        );
        assert_eq!(v, Value::Vec3([0.0, 0.0, 10.0]));
    }
}
