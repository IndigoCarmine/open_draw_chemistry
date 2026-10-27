//! The strip of open documents above the canvas: the active tab is a white card, as in the design.
//!
//! Each tab carries its close control inside it, after the title, as in VS Code: `[無題 3  ×]`.
//! While the document has unsaved changes the same slot shows `●` instead, which turns back
//! into `×` under the pointer so a dirty tab can still be closed with one click.

use egui::{FontId, RichText, Sense, Stroke, StrokeKind, pos2, vec2};

use super::Command;
use crate::document::Document;

/// Height of a tab.
const TAB_HEIGHT: f32 = 30.0;
/// Space left and right of a tab's contents.
const PAD_X: f32 = 10.0;
/// Space between the title and the close slot.
const GAP: f32 = 6.0;
/// Side of the square close slot.
const CLOSE_SIDE: f32 = 18.0;

/// What a tab's close slot shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseGlyph {
    /// The close cross.
    Cross,
    /// The unsaved-changes dot.
    Dot,
}

/// The glyph in the close slot: the dot marks unsaved changes, but pointing at it reveals the cross.
fn close_glyph(dirty: bool, hovered: bool) -> CloseGlyph {
    if dirty && !hovered {
        CloseGlyph::Dot
    } else {
        CloseGlyph::Cross
    }
}

pub fn tab_strip(ui: &mut egui::Ui, docs: &[Document], active: usize, out: &mut Vec<Command>) {
    egui::ScrollArea::horizontal().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (i, doc) in docs.iter().enumerate() {
                tab(ui, doc, i, i == active, out);
                ui.add_space(4.0);
            }
            if ui
                .add(egui::Button::new(RichText::new("＋").size(14.0)).frame(false))
                .on_hover_text("新しい文書")
                .clicked()
            {
                out.push(Command::New);
            }
        });
    });
}

/// One tab: the title and, inside the same card, the close slot.
fn tab(ui: &mut egui::Ui, doc: &Document, i: usize, is_active: bool, out: &mut Vec<Command>) {
    let visuals = ui.visuals().clone();
    let muted = crate::theme::muted(visuals.dark_mode);
    let title_color = if is_active {
        visuals.widgets.noninteractive.fg_stroke.color
    } else {
        muted
    };
    let galley = ui
        .painter()
        .layout_no_wrap(doc.title(), FontId::proportional(12.5), title_color);

    let size = vec2(
        PAD_X + galley.size().x + GAP + CLOSE_SIDE + PAD_X * 0.5,
        TAB_HEIGHT,
    );
    let (rect, body) = ui.allocate_exact_size(size, Sense::click());
    // Registered after the body so it sits on top and takes the pointer first.
    let close_rect = egui::Rect::from_center_size(
        pos2(
            rect.right() - PAD_X * 0.5 - CLOSE_SIDE / 2.0,
            rect.center().y,
        ),
        vec2(CLOSE_SIDE, CLOSE_SIDE),
    );
    let close = ui.interact(close_rect, body.id.with("close"), Sense::click());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = visuals.widgets.noninteractive.corner_radius;
        if is_active {
            painter.rect(
                rect,
                radius,
                visuals.extreme_bg_color,
                visuals.widgets.noninteractive.bg_stroke,
                StrokeKind::Inside,
            );
        }
        let text_pos = pos2(rect.left() + PAD_X, rect.center().y - galley.size().y / 2.0);
        painter.galley(text_pos, galley, title_color);

        if close.hovered() {
            painter.rect_filled(close_rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let glyph_color = if close.hovered() {
            visuals.widgets.hovered.fg_stroke.color
        } else {
            muted
        };
        let c = close_rect.center();
        match close_glyph(doc.is_dirty(), close.hovered()) {
            CloseGlyph::Dot => {
                painter.circle_filled(c, 3.5, glyph_color);
            }
            CloseGlyph::Cross => {
                let r = 3.5;
                let stroke = Stroke::new(1.3, glyph_color);
                painter.line_segment([c + vec2(-r, -r), c + vec2(r, r)], stroke);
                painter.line_segment([c + vec2(-r, r), c + vec2(r, -r)], stroke);
            }
        }
    }

    if close.clicked() || body.middle_clicked() {
        out.push(Command::CloseTab(i));
    } else if body.clicked() {
        out.push(Command::SelectTab(i));
    }
    let close_tip = if doc.is_dirty() {
        "閉じる（未保存の変更あり）"
    } else {
        "閉じる"
    };
    close.on_hover_text(close_tip);
    if let Some(path) = &doc.path {
        body.on_hover_text(path.display().to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_tab_shows_cross() {
        assert_eq!(close_glyph(false, false), CloseGlyph::Cross);
        assert_eq!(close_glyph(false, true), CloseGlyph::Cross);
    }

    #[test]
    fn dirty_tab_shows_dot_until_hovered() {
        assert_eq!(close_glyph(true, false), CloseGlyph::Dot);
        assert_eq!(close_glyph(true, true), CloseGlyph::Cross);
    }
}
