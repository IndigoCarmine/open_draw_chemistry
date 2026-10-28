//! Vector icons for the toolbar and palette, and the brand mark.
//!
//! Each icon is the design's inline SVG redrawn with the egui painter in the same 20 × 20
//! coordinate space, so it stays sharp at any size and takes the theme's text colour.

use egui::{Color32, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, vec2};

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    New,
    Open,
    Save,
    Undo,
    Redo,
    Select,
    Eraser,
    Single,
    Double,
    Triple,
    Wedge,
    Hash,
    Wavy,
    Chain,
    Benzene,
    Cyclopentane,
    Cyclohexane,
    Text,
    Arrow,
    Equilibrium,
    Bracket,
    /// A small grid: the reaction (stoichiometry) table.
    Table,
    /// "»": collapse a panel towards the right edge.
    ChevronsRight,
    /// "«": expand a panel collapsed on the right edge.
    ChevronsLeft,
    /// "⠿": six dots, the handle a card is dragged by.
    DragHandle,
    /// Two overlapping squares: duplicate.
    Duplicate,
    /// "×": delete or close.
    Close,
    /// "…": more actions.
    More,
    /// "▾": expand.
    ChevronDown,
    /// "▴": collapse.
    ChevronUp,
    /// A ring with a dot: switched on.
    ToggleOn,
    /// An empty ring: switched off.
    ToggleOff,
    /// A globe (meridians and parallels): the UI language.
    Globe,
}

/// Maps the 20 × 20 design space into `rect`.
struct Canvas<'a> {
    painter: &'a egui::Painter,
    rect: Rect,
    stroke: Stroke,
}

impl Canvas<'_> {
    fn p(&self, x: f32, y: f32) -> Pos2 {
        self.rect.min + vec2(x, y) * (self.rect.width() / 20.0)
    }
    fn line(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.painter.add(Shape::line(pts, self.stroke));
    }
    fn closed(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.painter.add(Shape::closed_line(pts, self.stroke));
    }
    fn filled(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.painter
            .add(Shape::convex_polygon(pts, self.stroke.color, Stroke::NONE));
    }
    /// A filled dot of radius `r` about (`cx`, `cy`).
    fn dot(&self, cx: f32, cy: f32, r: f32) {
        let scale = self.rect.width() / 20.0;
        self.painter
            .circle_filled(self.p(cx, cy), r * scale, self.stroke.color);
    }
    /// A circle's outline of radius `r` about (`cx`, `cy`).
    fn ring(&self, cx: f32, cy: f32, r: f32) {
        let scale = self.rect.width() / 20.0;
        self.painter
            .circle_stroke(self.p(cx, cy), r * scale, self.stroke);
    }
    /// A half circle of radius `r` about (`cx`, `cy`), from the top to the bottom, bulging to
    /// the right when `right`.
    fn half_circle(&self, cx: f32, cy: f32, r: f32, right: bool) -> Vec<(f32, f32)> {
        let side = if right { 1.0 } else { -1.0 };
        (0..=12)
            .map(|i| {
                let a = std::f32::consts::PI * (i as f32 / 12.0 - 0.5);
                (cx + side * r * a.cos(), cy + r * a.sin())
            })
            .collect()
    }
}

pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let width = 1.6 * rect.width() / 20.0;
    let c = Canvas {
        painter,
        rect,
        stroke: Stroke::new(width, color),
    };
    match icon {
        Icon::New => {
            c.closed(&[
                (5.0, 2.5),
                (12.0, 2.5),
                (15.5, 6.0),
                (15.5, 17.5),
                (5.0, 17.5),
            ]);
            c.line(&[(12.0, 2.5), (12.0, 6.0), (15.5, 6.0)]);
        }
        Icon::Open => c.closed(&[
            (2.5, 5.5),
            (7.5, 5.5),
            (9.0, 7.5),
            (17.5, 7.5),
            (17.5, 16.0),
            (2.5, 16.0),
        ]),
        Icon::Save => {
            c.closed(&[
                (3.0, 3.0),
                (14.0, 3.0),
                (17.0, 6.0),
                (17.0, 17.0),
                (3.0, 17.0),
            ]);
            c.line(&[(6.0, 3.0), (6.0, 7.0), (13.0, 7.0), (13.0, 3.0)]);
            c.line(&[(6.0, 17.0), (6.0, 12.0), (14.0, 12.0), (14.0, 17.0)]);
        }
        Icon::Undo => {
            c.line(&[(7.0, 4.0), (3.0, 8.0), (7.0, 12.0)]);
            let mut path = vec![(3.0, 8.0)];
            path.extend(c.half_circle(12.0, 13.0, 5.0, true));
            path.push((9.0, 18.0));
            c.line(&path);
        }
        Icon::Redo => {
            c.line(&[(13.0, 4.0), (17.0, 8.0), (13.0, 12.0)]);
            let mut path = vec![(17.0, 8.0)];
            path.extend(c.half_circle(8.0, 13.0, 5.0, false));
            path.push((11.0, 18.0));
            c.line(&path);
        }
        Icon::Select => c.closed(&[(5.0, 3.0), (15.0, 9.5), (10.5, 10.7), (8.5, 15.5)]),
        Icon::Eraser => {
            c.closed(&[
                (3.5, 13.0),
                (11.0, 4.5),
                (16.5, 9.0),
                (10.0, 16.5),
                (6.0, 16.5),
            ]);
            c.line(&[(7.5, 8.5), (13.0, 13.0)]);
            c.line(&[(10.0, 16.5), (17.0, 16.5)]);
        }
        Icon::Single => c.line(&[(4.0, 16.0), (16.0, 4.0)]),
        Icon::Double => {
            c.line(&[(3.0, 14.0), (14.0, 3.0)]);
            c.line(&[(6.0, 17.0), (17.0, 6.0)]);
        }
        Icon::Triple => {
            c.line(&[(2.5, 12.5), (12.5, 2.5)]);
            c.line(&[(4.5, 15.5), (15.5, 4.5)]);
            c.line(&[(7.5, 17.5), (17.5, 7.5)]);
        }
        Icon::Wedge => c.filled(&[(4.0, 16.0), (14.5, 3.5), (17.0, 6.0)]),
        Icon::Hash => {
            for (a, b) in [
                ((4.5, 15.5), (5.1, 16.1)),
                ((6.6, 12.6), (8.4, 14.4)),
                ((8.6, 9.6), (11.6, 12.6)),
                ((10.7, 6.7), (14.8, 10.8)),
                ((12.8, 3.8), (18.0, 9.0)),
            ] {
                c.line(&[a, b]);
            }
        }
        Icon::Wavy => {
            // Three waves along the diagonal from (3, 16) to (17, 4).
            let (from, to) = (vec2(3.0, 16.0), vec2(17.0, 4.0));
            let along = to - from;
            let normal = vec2(along.y, -along.x).normalized();
            let pts: Vec<(f32, f32)> = (0..=36)
                .map(|i| {
                    let t = i as f32 / 36.0;
                    let q =
                        from + along * t + normal * (2.0 * (t * 6.0 * std::f32::consts::PI).sin());
                    (q.x, q.y)
                })
                .collect();
            c.line(&pts);
        }
        Icon::Chain => c.line(&[
            (2.0, 13.5),
            (6.0, 7.5),
            (10.0, 13.5),
            (14.0, 7.5),
            (18.0, 13.5),
        ]),
        Icon::Benzene => {
            c.closed(&HEXAGON);
            c.line(&[(10.0, 4.6), (14.7, 7.3)]);
            c.line(&[(14.7, 12.7), (10.0, 15.4)]);
            c.line(&[(5.3, 12.7), (5.3, 7.3)]);
        }
        Icon::Cyclopentane => c.closed(&[
            (10.0, 2.5),
            (17.6, 8.0),
            (14.7, 17.0),
            (5.3, 17.0),
            (2.4, 8.0),
        ]),
        Icon::Cyclohexane => c.closed(&HEXAGON),
        Icon::Text => {
            c.line(&[(4.0, 4.5), (16.0, 4.5)]);
            c.line(&[(10.0, 4.5), (10.0, 16.5)]);
        }
        Icon::Arrow => {
            c.line(&[(2.0, 10.0), (14.0, 10.0)]);
            c.filled(&[(18.5, 10.0), (13.0, 6.8), (13.0, 13.2)]);
        }
        Icon::Equilibrium => {
            c.line(&[(2.0, 8.0), (17.0, 8.0), (13.0, 4.5)]);
            c.line(&[(18.0, 12.0), (3.0, 12.0), (7.0, 15.5)]);
        }
        Icon::Bracket => {
            c.line(&[(7.0, 3.0), (4.0, 3.0), (4.0, 17.0), (7.0, 17.0)]);
            c.line(&[(13.0, 3.0), (16.0, 3.0), (16.0, 17.0), (13.0, 17.0)]);
        }
        Icon::Table => {
            c.line(&[
                (3.0, 4.0),
                (17.0, 4.0),
                (17.0, 16.0),
                (3.0, 16.0),
                (3.0, 4.0),
            ]);
            c.line(&[(8.0, 4.0), (8.0, 16.0)]);
            c.line(&[(3.0, 8.0), (17.0, 8.0)]);
            c.line(&[(3.0, 12.0), (17.0, 12.0)]);
        }
        Icon::ChevronsRight => {
            for chevron in CHEVRONS_RIGHT {
                c.line(&chevron);
            }
        }
        Icon::ChevronsLeft => {
            for chevron in CHEVRONS_RIGHT {
                c.line(&chevron.map(mirror_x));
            }
        }
        Icon::DragHandle => {
            for x in [7.5, 12.5] {
                for y in [4.5, 10.0, 15.5] {
                    c.dot(x, y, 1.5);
                }
            }
        }
        Icon::Duplicate => {
            c.line(&[
                (7.0, 7.0),
                (7.0, 3.0),
                (17.0, 3.0),
                (17.0, 13.0),
                (13.0, 13.0),
            ]);
            c.closed(&[(3.0, 7.0), (13.0, 7.0), (13.0, 17.0), (3.0, 17.0)]);
        }
        Icon::Close => {
            c.line(&[(5.0, 5.0), (15.0, 15.0)]);
            c.line(&[(15.0, 5.0), (5.0, 15.0)]);
        }
        Icon::More => {
            for x in [4.0, 10.0, 16.0] {
                c.dot(x, 10.0, 1.7);
            }
        }
        Icon::ChevronDown => c.line(&CHEVRON_DOWN),
        Icon::ChevronUp => c.line(&CHEVRON_DOWN.map(mirror_y)),
        Icon::ToggleOn => {
            c.ring(10.0, 10.0, 7.0);
            c.dot(10.0, 10.0, 3.6);
        }
        Icon::ToggleOff => c.ring(10.0, 10.0, 7.0),
        Icon::Globe => {
            c.ring(10.0, 10.0, 7.5);
            // The central meridian, and an ellipse for the meridians either side of it.
            c.line(&[(10.0, 2.5), (10.0, 17.5)]);
            let ellipse: Vec<(f32, f32)> = (0..=24)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / 24.0;
                    (10.0 + 3.6 * a.cos(), 10.0 + 7.5 * a.sin())
                })
                .collect();
            c.closed(&ellipse);
            // The equator and two parallels.
            c.line(&[(2.5, 10.0), (17.5, 10.0)]);
            c.line(&[(3.9, 6.0), (16.1, 6.0)]);
            c.line(&[(3.9, 14.0), (16.1, 14.0)]);
        }
    }
}

/// "▾" as a stroke; "▴" is the same mirrored by [`mirror_y`].
const CHEVRON_DOWN: [(f32, f32); 3] = [(5.0, 7.5), (10.0, 12.5), (15.0, 7.5)];

/// Mirrors a point of the 20 × 20 design space about its horizontal centre line.
fn mirror_y((x, y): (f32, f32)) -> (f32, f32) {
    (x, 20.0 - y)
}

/// The two strokes of "»"; "«" is the same mirrored by [`mirror_x`].
const CHEVRONS_RIGHT: [[(f32, f32); 3]; 2] = [
    [(4.5, 5.0), (9.5, 10.0), (4.5, 15.0)],
    [(10.5, 5.0), (15.5, 10.0), (10.5, 15.0)],
];

/// Mirrors a point of the 20 × 20 design space about its vertical centre line.
fn mirror_x((x, y): (f32, f32)) -> (f32, f32) {
    (20.0 - x, y)
}

const HEXAGON: [(f32, f32); 6] = [
    (10.0, 2.0),
    (17.0, 6.0),
    (17.0, 14.0),
    (10.0, 18.0),
    (3.0, 14.0),
    (3.0, 6.0),
];

/// How an [`icon_button`] is framed when it is not selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// A white tile with a border: the palette.
    Tile,
    /// No background until hovered: the toolbar.
    Flat,
}

/// A square button showing `icon`, with `tooltip` on hover.
pub fn icon_button(
    ui: &mut Ui,
    icon: Icon,
    size: Vec2,
    frame: Frame,
    selected: bool,
    enabled: bool,
    tooltip: &str,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let dark = visuals.dark_mode;
        let (fill, border, color) = if selected {
            let fill = visuals.selection.bg_fill;
            (fill, theme::accent(dark), visuals.selection.stroke.color)
        } else if response.hovered() && enabled {
            let w = &visuals.widgets.hovered;
            (w.weak_bg_fill, w.bg_stroke.color, w.fg_stroke.color)
        } else {
            let w = &visuals.widgets.inactive;
            let color = if enabled {
                w.fg_stroke.color
            } else {
                w.fg_stroke.color.gamma_multiply(0.35)
            };
            match frame {
                Frame::Tile => (w.weak_bg_fill, w.bg_stroke.color, color),
                Frame::Flat => (Color32::TRANSPARENT, Color32::TRANSPARENT, color),
            }
        };
        let painter = ui.painter();
        painter.rect(
            rect,
            6.0,
            fill,
            Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
        let side = (size.x.min(size.y) * 0.46).round();
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(side));
        paint(painter, icon_rect, icon, color);
    }
    response.on_hover_text(tooltip)
}

/// The brand mark (brand sheet 5a, "Hex Nib"): a white hexagonal nib on an orange tile.
pub fn paint_logo(painter: &egui::Painter, rect: Rect) {
    let s = rect.width() / 100.0;
    let p = |x: f32, y: f32| rect.min + vec2(x, y) * s;
    painter.rect_filled(rect, 22.5 * s, theme::ORANGE);
    // The glyph is 66% of the tile, centred, in its own 100-unit space.
    let g = |x: f32, y: f32| p(17.0 + x * 0.66, 17.0 + y * 0.66);
    let nib: Vec<Pos2> = [
        (50.0, 16.0),
        (79.0, 33.0),
        (79.0, 63.0),
        (50.0, 86.0),
        (21.0, 63.0),
        (21.0, 33.0),
    ]
    .iter()
    .map(|&(x, y)| g(x, y))
    .collect();
    painter.add(Shape::convex_polygon(
        nib,
        Color32::WHITE,
        Stroke::new(4.0 * 0.66 * s, Color32::WHITE),
    ));
    painter.line_segment(
        [g(50.0, 56.0), g(50.0, 88.0)],
        Stroke::new(4.5 * 0.66 * s, theme::ORANGE),
    );
    painter.circle_filled(g(50.0, 52.0), 7.0 * 0.66 * s, theme::ORANGE);
}

/// The logo and the "Open DrawChemistry" wordmark, for the menu bar. egui's default font has
/// no bold face, so "Open" is set apart by full ink and "DrawChemistry" is a shade lighter.
///
/// On Windows the logo mark is omitted: the native title bar already shows the app icon
/// directly above the menu bar, so only the wordmark is drawn there.
pub fn brand(ui: &mut Ui) {
    if !cfg!(windows) {
        let (rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
        paint_logo(ui.painter(), rect);
        ui.add_space(2.0);
    }
    let ink = ui.visuals().widgets.noninteractive.fg_stroke.color;
    let mut job = egui::text::LayoutJob::default();
    let font = egui::FontId::proportional(14.5);
    job.append(
        "Open",
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: ink,
            ..Default::default()
        },
    );
    job.append(
        "DrawChemistry",
        4.0,
        egui::TextFormat {
            font_id: font,
            color: ink.gamma_multiply(0.8),
            ..Default::default()
        },
    );
    ui.label(job);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_expand_chevron_points_down_and_its_mirror_up() {
        assert!(CHEVRON_DOWN[1].1 > CHEVRON_DOWN[0].1);
        let up = CHEVRON_DOWN.map(mirror_y);
        assert!(up[1].1 < up[0].1);
        assert_eq!(up.map(mirror_y), CHEVRON_DOWN);
    }

    /// Every icon paints without panicking.
    #[test]
    fn the_panel_icons_paint() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for icon in [
                Icon::DragHandle,
                Icon::Duplicate,
                Icon::Close,
                Icon::More,
                Icon::ChevronDown,
                Icon::ChevronUp,
                Icon::ToggleOn,
                Icon::ToggleOff,
                Icon::Globe,
            ] {
                let rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(14.0));
                paint(ui.painter(), rect, icon, Color32::WHITE);
            }
        });
        output.textures_delta.clear();
    }

    #[test]
    fn chevrons_point_away_from_each_other() {
        for chevron in CHEVRONS_RIGHT {
            // "»": the tip is the rightmost point of each stroke.
            assert!(chevron[1].0 > chevron[0].0 && chevron[1].0 > chevron[2].0);
            // "«": mirrored, the tip is the leftmost, and mirroring twice is the identity.
            let left = chevron.map(mirror_x);
            assert!(left[1].0 < left[0].0 && left[1].0 < left[2].0);
            assert_eq!(left.map(mirror_x), chevron);
            // Both stay inside the design space.
            for (x, y) in chevron.into_iter().chain(left) {
                assert!((0.0..=20.0).contains(&x) && (0.0..=20.0).contains(&y));
            }
        }
    }
}
