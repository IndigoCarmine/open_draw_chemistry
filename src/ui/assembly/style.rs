//! The look of the 集合体 panel: its colours on the light and the dark theme, cards, captions,
//! tabs and the small icon buttons on the cards.

use egui::{
    Color32, CornerRadius, Margin, Pos2, Rect, Response, RichText, Sense, Stroke, Ui, vec2,
};

use crate::theme::{self, dark, light};
use crate::ui::icons::{self, Icon};

/// Corner radius of the cards.
pub(super) const CARD_RADIUS: u8 = 8;

/// The panel's colours for the current theme.
#[derive(Debug, Clone, Copy)]
pub(super) struct Look {
    /// Body text.
    pub ink: Color32,
    pub muted: Color32,
    /// The brand orange.
    pub accent: Color32,
    /// The running counts between the steps: between muted and the accent.
    pub node: Color32,
    pub card: Color32,
    pub border: Color32,
    /// The pipeline's line and hollow nodes.
    pub line: Color32,
    /// The panel's own background.
    pub back: Color32,
    /// An icon button under the pointer.
    pub hover: Color32,
    /// A delete button under the pointer.
    pub danger: Color32,
}

impl Look {
    pub fn of(ui: &Ui) -> Self {
        let v = ui.visuals();
        let dark_mode = v.dark_mode;
        let muted = theme::muted(dark_mode);
        let accent = theme::accent(dark_mode);
        let (card, border, line, danger) = if dark_mode {
            (
                dark::RAISED,
                dark::BORDER,
                Color32::from_rgb(0x5A, 0x50, 0x48),
                Color32::from_rgb(0xF2, 0x6D, 0x5E),
            )
        } else {
            (
                Color32::WHITE,
                light::BORDER,
                light::BORDER_STRONG,
                Color32::from_rgb(0xC6, 0x28, 0x28),
            )
        };
        Self {
            ink: v.widgets.noninteractive.fg_stroke.color,
            muted,
            accent,
            node: mix(
                muted,
                if dark_mode {
                    accent
                } else {
                    theme::ORANGE_TEXT
                },
                0.6,
            ),
            card,
            border,
            line,
            back: v.panel_fill,
            hover: v.widgets.hovered.weak_bg_fill,
            danger,
        }
    }

    pub fn muted(&self, text: impl Into<String>) -> RichText {
        RichText::new(text).size(11.5).color(self.muted)
    }

    pub fn mono(&self, text: impl Into<String>, color: Color32) -> RichText {
        RichText::new(text).monospace().size(11.5).color(color)
    }
}

/// `a` moved towards `b` by `t` (0 to 1), in gamma space.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// A small grey caption over a group ("単量体").
pub(super) fn caption(ui: &mut Ui, look: &Look, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).size(11.0).color(look.muted));
    ui.add_space(1.0);
}

/// A white card with a subtle border (the accent when `selected`), as wide as the panel.
pub(super) fn card<R>(
    ui: &mut Ui,
    look: &Look,
    selected: bool,
    left: f32,
    contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    let stroke = if selected {
        Stroke::new(1.5, look.accent)
    } else {
        Stroke::new(1.0, look.border)
    };
    egui::Frame::new()
        .fill(look.card)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::symmetric(8, 6))
        .outer_margin(Margin {
            left: left as i8,
            ..Margin::ZERO
        })
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            contents(ui)
        })
}

/// A dashed rounded outline around `rect`.
pub(super) fn dashed_box(ui: &Ui, rect: Rect, color: Color32) {
    let r = f32::from(CARD_RADIUS);
    let mut points: Vec<Pos2> = Vec::new();
    // Corners clockwise from the top left, each a quarter circle.
    let corners = [
        (rect.left_top() + vec2(r, r), 180.0f32),
        (rect.right_top() + vec2(-r, r), 270.0),
        (rect.right_bottom() + vec2(-r, -r), 0.0),
        (rect.left_bottom() + vec2(r, -r), 90.0),
    ];
    for (centre, start) in corners {
        for k in 0..=6 {
            let a = (start + 15.0 * k as f32).to_radians();
            points.push(centre + r * vec2(a.cos(), a.sin()));
        }
    }
    points.push(points[0]);
    ui.painter().extend(egui::Shape::dashed_line(
        &points,
        Stroke::new(1.0, color),
        4.0,
        3.0,
    ));
}

/// Two tabs side by side, the selected one underlined in the accent. Returns the one clicked.
pub(super) fn tabs<T: Copy + PartialEq>(
    ui: &mut Ui,
    look: &Look,
    current: T,
    all: &[(T, &str)],
) -> Option<T> {
    let mut clicked = None;
    let row = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        for &(tab, name) in all {
            let selected = tab == current;
            let color = if selected { look.ink } else { look.muted };
            let mut text = RichText::new(name).size(13.0).color(color);
            if selected {
                text = text.strong();
            }
            let response = ui
                .add(
                    egui::Label::new(text)
                        .sense(Sense::click())
                        .selectable(false),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if selected {
                let r = response.rect;
                ui.painter().line_segment(
                    [
                        egui::pos2(r.left(), r.bottom() + 4.0),
                        egui::pos2(r.right(), r.bottom() + 4.0),
                    ],
                    Stroke::new(2.0, look.accent),
                );
            } else if response.clicked() {
                clicked = Some(tab);
            }
        }
    });
    // A hairline under the whole row.
    let y = row.response.rect.bottom() + 5.0;
    let x = ui.max_rect().x_range();
    ui.painter().hline(x, y, Stroke::new(1.0, look.border));
    ui.add_space(8.0);
    clicked
}

/// A small flat button showing `icon`: muted, darker on hover (red for `danger`), the accent
/// when `on`.
pub(super) fn tool(
    ui: &mut Ui,
    look: &Look,
    icon: Icon,
    tooltip: &str,
    enabled: bool,
    danger: bool,
    on: bool,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(20.0, 20.0), sense);
    if ui.is_rect_visible(rect) {
        let hovered = enabled && response.hovered();
        if hovered {
            ui.painter().rect_filled(rect, 5.0, look.hover);
        }
        let color = if !enabled {
            look.muted.gamma_multiply(0.4)
        } else if hovered && danger {
            look.danger
        } else if on {
            look.accent
        } else if hovered {
            look.ink
        } else {
            look.muted
        };
        let icon_rect = Rect::from_center_size(rect.center(), vec2(13.0, 13.0));
        icons::paint(ui.painter(), icon_rect, icon, color);
    }
    response.on_hover_text(tooltip)
}
