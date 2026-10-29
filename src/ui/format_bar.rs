//! The toolbar's format section (書式), right of クリーンアップ: font, size, bold/italic,
//! sub/superscript and Symbol for texts, then text and line colours, ring fill and shadow.
//!
//! It shows only with the drawing, always, its items greyed out (with the reason on hover)
//! where the selection has nothing they apply to. Each shows the selection's value, or
//! 混在 (mixed) where the selection differs. The text controls are [`super::text_format`]'s and
//! act through the editor as before: with nothing selected they set the format of new texts.
//! The colour, fill and shadow buttons open the items of the 書式 menu ([`super::appearance`]),
//! so they change exactly what the menu does.
//!
//! What does not fit the toolbar's width goes, whole groups from the right, into a "…" menu.
//! Each group's width is measured when it is drawn in the toolbar, so the fit follows the
//! language and the fonts.

use chembuider_rs::ChemStructEditor;
use chembuider_rs::widget::text_format::FormatChange;
use egui::{Color32, PopupCloseBehavior, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::Command;
use super::appearance::{self, Appearance, Colors, Selection};
use super::icons::{self, Frame, Icon};
use super::text_format::{self, TextFormatBar, TextSummary};

/// One value over the selection: the same throughout it, or differing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value<T> {
    Same(T),
    Mixed,
}

impl<T: PartialEq> Value<T> {
    /// The value `values` share (`None` when there are none).
    pub fn of(values: impl IntoIterator<Item = T>) -> Option<Self> {
        let mut values = values.into_iter();
        let first = values.next()?;
        Some(if values.all(|v| v == first) {
            Value::Same(first)
        } else {
            Value::Mixed
        })
    }
}

/// What the section shows of the active editor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormatState {
    pub text: TextSummary,
    pub colors: Colors,
    pub selection: Selection,
}

impl FormatState {
    pub fn of(editor: &ChemStructEditor) -> Self {
        Self {
            text: TextSummary::of(editor),
            colors: Colors::of(editor),
            selection: Selection::of(editor),
        }
    }
}

/// The section's groups, in toolbar order; the last ones go into "…" first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Font,
    Size,
    Style,
    Colors,
    Structure,
}

const GROUPS: [Group; 5] = [
    Group::Font,
    Group::Size,
    Group::Style,
    Group::Colors,
    Group::Structure,
];

impl Group {
    /// A width to go by until the group has been drawn.
    fn estimate(self) -> f32 {
        match self {
            Group::Font => 190.0,
            Group::Size => 96.0,
            Group::Style => 150.0,
            Group::Colors => 84.0,
            Group::Structure => 84.0,
        }
    }
}

/// The side of the "…" button.
const MORE: f32 = 32.0;
/// A colour, fill or shadow button.
const BUTTON: egui::Vec2 = vec2(34.0, 32.0);

/// How many of the groups, `widths` wide in order with `spacing` between them, fit in
/// `available`: all, or as many as leave room for the "…" button.
fn fitting(widths: &[f32], available: f32, spacing: f32) -> usize {
    let total: f32 = widths.iter().map(|w| w + spacing).sum();
    if total <= available {
        return widths.len();
    }
    let room = available - MORE - spacing;
    let mut used = 0.0;
    widths
        .iter()
        .take_while(|w| {
            used += *w + spacing;
            used <= room
        })
        .count()
}

/// The section, for the active editor's `state` (`None` without a document, everything greyed
/// out). Text format changes become [`Command::TextFormat`]; the rest are the 書式 menu's.
/// Returns where it is in the toolbar, the "…" button included (not its popups).
pub fn show(
    ui: &mut Ui,
    state: Option<&FormatState>,
    bar: &mut TextFormatBar,
    appearance: &mut Appearance,
    out: &mut Vec<Command>,
) -> Rect {
    let enabled = state.is_some();
    let blank;
    let state = match state {
        Some(state) => state,
        None => {
            blank = FormatState::of(&ChemStructEditor::default());
            &blank
        }
    };
    let id = egui::Id::unique("odc_format_bar_widths");
    let mut widths: [f32; GROUPS.len()] = ui
        .data(|d| d.get_temp(id))
        .unwrap_or(GROUPS.map(Group::estimate));
    let shown = fitting(&widths, ui.available_width(), ui.spacing().item_spacing.x);
    let mut changes = Vec::new();
    let rect = ui
        .add_enabled_ui(enabled, |ui| {
            for (k, &group) in GROUPS[..shown].iter().enumerate() {
                let response = ui
                    .horizontal(|ui| {
                        group_ui(ui, group, false, state, bar, appearance, &mut changes, out)
                    })
                    .response;
                widths[k] = response.rect.width();
            }
            if shown < GROUPS.len() {
                let more = icons::icon_button(
                    ui,
                    Icon::More,
                    vec2(MORE, MORE),
                    Frame::Flat,
                    false,
                    true,
                    t!("format_bar.more"),
                );
                let label = t!("format_bar.more");
                more.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
                egui::Popup::menu(&more)
                    .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                        for &group in &GROUPS[shown..] {
                            ui.horizontal(|ui| {
                                group_ui(ui, group, true, state, bar, appearance, &mut changes, out)
                            });
                        }
                    });
            }
        })
        .response
        .on_disabled_hover_text(t!("format_bar.no_document"))
        .rect;
    ui.data_mut(|d| d.insert_temp(id, widths));
    out.extend(changes.into_iter().map(Command::TextFormat));
    rect
}

/// One group, in the toolbar or (`in_menu`) in the "…" menu.
#[allow(clippy::too_many_arguments)]
fn group_ui(
    ui: &mut Ui,
    group: Group,
    in_menu: bool,
    state: &FormatState,
    bar: &mut TextFormatBar,
    appearance: &mut Appearance,
    changes: &mut Vec<FormatChange>,
    out: &mut Vec<Command>,
) {
    let separate = |ui: &mut Ui| {
        if !in_menu {
            ui.separator();
        }
    };
    match group {
        Group::Font => {
            separate(ui);
            text_format::font_menu(ui, in_menu, &state.text, bar, changes);
        }
        Group::Size => text_format::size_menu(ui, in_menu, &state.text, bar, changes),
        Group::Style => {
            ui.spacing_mut().item_spacing.x = 2.0;
            text_format::style_toggles(ui, &state.text, changes);
        }
        Group::Colors => {
            separate(ui);
            let colors = state.colors;
            dropdown(
                ui,
                in_menu,
                t!("format.text_color"),
                colors.text,
                (colors.text.is_none()).then(|| t!("format_bar.text_color_needs")),
                Glyph::Text,
                |ui| appearance::text_color_items(ui, appearance, out),
            );
            dropdown(
                ui,
                in_menu,
                t!("format.line_color"),
                colors.line,
                (colors.line.is_none()).then(|| t!("format_bar.line_color_needs")),
                Glyph::Line,
                |ui| appearance::line_color_items(ui, appearance, out),
            );
        }
        Group::Structure => {
            let selection = state.selection;
            dropdown(
                ui,
                in_menu,
                t!("format.fill_ring"),
                None,
                (!selection.structure).then(|| t!("format.ring_needs_selection")),
                Glyph::Ring,
                |ui| {
                    appearance::fill_ring_items(ui, appearance, out);
                    appearance::clear_fill_item(ui, out);
                },
            );
            dropdown(
                ui,
                in_menu,
                t!("format.shadow"),
                None,
                None,
                Glyph::Shadow,
                |ui| appearance::shadow_items(ui, appearance, Some(selection), out),
            );
        }
    }
}

/// What a colour, fill or shadow button shows.
#[derive(Clone, Copy)]
enum Glyph {
    Text,
    Line,
    Ring,
    Shadow,
}

/// A button named `label` opening `content`: in the toolbar a glyph with the selection's
/// colour (`color`) under it, in the "…" menu a submenu. Greyed out with `disabled` as the
/// reason.
fn dropdown(
    ui: &mut Ui,
    in_menu: bool,
    label: &str,
    color: Option<Value<Option<[u8; 4]>>>,
    disabled: Option<&str>,
    glyph: Glyph,
    content: impl FnOnce(&mut Ui),
) {
    let enabled = disabled.is_none() && ui.is_enabled();
    if in_menu {
        let response = ui
            .add_enabled_ui(enabled, |ui| appearance::submenu(ui, label, content))
            .response;
        if let Some(reason) = disabled {
            response.on_disabled_hover_text(reason);
        }
        return;
    }
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(BUTTON, sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let ink = visuals.widgets.inactive.fg_stroke.color;
        let ink = if enabled {
            ink
        } else {
            ink.gamma_multiply(0.35)
        };
        let painter = ui.painter();
        if response.hovered() && enabled {
            let w = &visuals.widgets.hovered;
            painter.rect(
                rect,
                6.0,
                w.weak_bg_fill,
                Stroke::new(1.0, w.bg_stroke.color),
                egui::StrokeKind::Inside,
            );
        }
        paint_glyph(painter, rect, glyph, ink);
        if let Some(value) = color {
            paint_color_bar(painter, rect, value, ink);
        }
    }
    let shown = match color {
        Some(Value::Same(Some(rgba))) => format!("{label} — {}", appearance::hex(rgba)),
        Some(Value::Same(None)) => format!("{label} — {}", t!("format.default_color")),
        Some(Value::Mixed) => format!("{label} — {}", t!("format_bar.mixed")),
        None => label.to_owned(),
    };
    let response = match disabled {
        Some(reason) => response.on_disabled_hover_text(reason),
        None => response.on_hover_text(shown),
    };
    egui::Popup::menu(&response)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .show(content);
}

/// The glyph of a button, in the top of `rect`.
fn paint_glyph(painter: &egui::Painter, rect: Rect, glyph: Glyph, ink: Color32) {
    let c = rect.center() - vec2(0.0, 3.0);
    let stroke = Stroke::new(1.6, ink);
    match glyph {
        Glyph::Text => {
            painter.text(
                c,
                egui::Align2::CENTER_CENTER,
                "A",
                egui::FontId::proportional(15.0),
                ink,
            );
        }
        Glyph::Line => {
            painter.line_segment([c + vec2(-7.0, 6.0), c + vec2(7.0, -6.0)], stroke);
        }
        Glyph::Ring | Glyph::Shadow => {
            let hexagon = |at: egui::Pos2| -> Vec<egui::Pos2> {
                (0..6)
                    .map(|k| {
                        let a =
                            std::f32::consts::FRAC_PI_6 + k as f32 * std::f32::consts::FRAC_PI_3;
                        at + 7.5 * vec2(a.cos(), a.sin())
                    })
                    .collect()
            };
            if let Glyph::Ring = glyph {
                painter.add(egui::Shape::convex_polygon(
                    hexagon(c),
                    ink.gamma_multiply(0.35),
                    stroke,
                ));
            } else {
                painter.add(egui::Shape::convex_polygon(
                    hexagon(c + vec2(2.5, 2.5)),
                    ink.gamma_multiply(0.3),
                    Stroke::NONE,
                ));
                painter.add(egui::Shape::closed_line(hexagon(c), stroke));
            }
        }
    }
}

/// The selection's colour as a bar under the glyph: the style's ink for none, three colours
/// for mixed.
fn paint_color_bar(
    painter: &egui::Painter,
    rect: Rect,
    value: Value<Option<[u8; 4]>>,
    ink: Color32,
) {
    let bar = Rect::from_min_max(
        pos2(rect.left() + 8.0, rect.bottom() - 8.0),
        pos2(rect.right() - 8.0, rect.bottom() - 5.0),
    );
    match value {
        Value::Same(rgba) => {
            let color = rgba.map_or(ink, |[r, g, b, _]| Color32::from_rgb(r, g, b));
            painter.rect_filled(bar, 1.0, color);
        }
        Value::Mixed => {
            let third = bar.width() / 3.0;
            for (k, color) in [Color32::RED, Color32::GREEN, Color32::BLUE]
                .into_iter()
                .enumerate()
            {
                let x = bar.left() + k as f32 * third;
                let part = Rect::from_min_max(pos2(x, bar.top()), pos2(x + third, bar.bottom()));
                painter.rect_filled(part, 0.0, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::appearance::tests::phenol;

    #[test]
    fn a_value_is_shared_or_mixed() {
        assert_eq!(Value::<u8>::of([]), None);
        assert_eq!(Value::of([1, 1]), Some(Value::Same(1)));
        assert_eq!(Value::of([1, 2, 1]), Some(Value::Mixed));
    }

    /// Groups that do not all fit leave room for the "…" button; the first ones stay.
    #[test]
    fn the_groups_that_fit_stay_in_the_toolbar() {
        let widths = [100.0, 50.0, 50.0];
        assert_eq!(fitting(&widths, 212.0, 4.0), 3);
        assert_eq!(fitting(&widths, 211.0, 4.0), 2);
        // 100 + 50 + 2 × 4 + the button (32 + 4) = 194.
        assert_eq!(fitting(&widths, 194.0, 4.0), 2);
        assert_eq!(fitting(&widths, 193.0, 4.0), 1);
        assert_eq!(fitting(&widths, 20.0, 4.0), 0);
    }

    /// The colours are of what would be coloured: bonds selected themselves or by both their
    /// atoms, the selected atoms' labels and texts; mixed where they differ.
    #[test]
    fn the_colours_are_the_selection_s() {
        let mut editor = ChemStructEditor::default();
        let (ring, o, text) = phenol(&mut editor);
        let none = Colors::of(&editor);
        assert_eq!((none.line, none.text), (None, None));

        editor.selected_atoms = [ring[0], ring[1]].into();
        let colors = Colors::of(&editor);
        assert_eq!(colors.line, Some(Value::Same(None)));
        assert_eq!(colors.text, Some(Value::Same(None)));

        let red = [255, 0, 0, 255];
        editor.molecule.bonds[0].color = Some(red);
        assert_eq!(Colors::of(&editor).line, Some(Value::Same(Some(red))));
        editor.selected_atoms.insert(ring[2]);
        assert_eq!(Colors::of(&editor).line, Some(Value::Mixed));

        editor.selected_atoms = [o].into();
        editor.selected_objects = [text].into();
        editor.molecule.atom_by_id_mut(o).unwrap().color = Some(red);
        let colors = Colors::of(&editor);
        assert_eq!(colors.line, None, "a lone atom has no bond selected");
        assert_eq!(colors.text, Some(Value::Mixed));
    }
}
