//! The brand palette (brand sheet 5a) and the egui visuals built from it.

use egui::{Color32, CornerRadius, Stroke, Theme, Visuals};

/// Brand Orange: the mark, accents and strokes.
pub const ORANGE: Color32 = Color32::from_rgb(0xE4, 0x48, 0x2A);
/// Orange on dark surfaces.
pub const ORANGE_ON_DARK: Color32 = Color32::from_rgb(0xF0, 0x62, 0x3F);
/// Fill of buttons that carry white text: brand orange darkened so the text reaches 4.5:1
/// (white on [`ORANGE`] is only 4:1).
pub const ORANGE_DEEP: Color32 = Color32::from_rgb(0xC9, 0x3A, 0x1E);
/// Orange text on [`TINT`].
pub const ORANGE_TEXT: Color32 = Color32::from_rgb(0xB8, 0x37, 0x1C);
/// Background of a selected tool or item.
pub const TINT: Color32 = Color32::from_rgb(0xFC, 0xE4, 0xDC);
pub const INK: Color32 = Color32::from_rgb(0x17, 0x13, 0x0F);
pub const DARK_SURFACE: Color32 = Color32::from_rgb(0x1C, 0x19, 0x16);

/// Warm neutrals of the light theme.
pub mod light {
    use egui::Color32;
    pub const BACKDROP: Color32 = Color32::from_rgb(0xEE, 0xEB, 0xE7);
    pub const PANEL: Color32 = Color32::from_rgb(0xFA, 0xF8, 0xF6);
    pub const CANVAS_BACK: Color32 = Color32::from_rgb(0xE4, 0xE0, 0xDA);
    pub const BORDER: Color32 = Color32::from_rgb(0xE3, 0xDE, 0xD8);
    pub const BORDER_STRONG: Color32 = Color32::from_rgb(0xD3, 0xCC, 0xC4);
    pub const HOVER: Color32 = Color32::from_rgb(0xF2, 0xEE, 0xE9);
    pub const TEXT: Color32 = Color32::from_rgb(0x3D, 0x36, 0x31);
    pub const MUTED: Color32 = Color32::from_rgb(0x6B, 0x62, 0x5B);
}

/// The dark theme's surfaces.
pub mod dark {
    use egui::Color32;
    pub const BACKDROP: Color32 = Color32::from_rgb(0x14, 0x12, 0x10);
    pub const RAISED: Color32 = Color32::from_rgb(0x26, 0x21, 0x1D);
    pub const CANVAS_BACK: Color32 = Color32::from_rgb(0x12, 0x0F, 0x0D);
    pub const BORDER: Color32 = Color32::from_rgb(0x3B, 0x34, 0x2E);
    pub const HOVER: Color32 = Color32::from_rgb(0x34, 0x2D, 0x28);
    pub const TINT: Color32 = Color32::from_rgb(0x4A, 0x24, 0x19);
    pub const TEXT: Color32 = Color32::from_rgb(0xF4, 0xF1, 0xEE);
    pub const MUTED: Color32 = Color32::from_rgb(0xB3, 0xAA, 0xA2);
}

/// The accent for the current theme: [`ORANGE`] on light, [`ORANGE_ON_DARK`] on dark.
pub fn accent(dark_mode: bool) -> Color32 {
    if dark_mode { ORANGE_ON_DARK } else { ORANGE }
}

/// Text and icons of lower emphasis.
pub fn muted(dark_mode: bool) -> Color32 {
    if dark_mode { dark::MUTED } else { light::MUTED }
}

/// Behind the drawing page (the page itself is always white, as in ChemDraw).
pub fn canvas_back(dark_mode: bool) -> Color32 {
    if dark_mode {
        dark::CANVAS_BACK
    } else {
        light::CANVAS_BACK
    }
}

/// Install the brand visuals for both themes; egui switches between them with the OS.
pub fn apply(ctx: &egui::Context) {
    ctx.set_visuals_of(Theme::Light, light_visuals());
    ctx.set_visuals_of(Theme::Dark, dark_visuals());
}

const RADIUS: CornerRadius = CornerRadius::same(6);

pub fn light_visuals() -> Visuals {
    let mut v = Visuals::light();
    v.panel_fill = light::PANEL;
    v.window_fill = Color32::WHITE;
    v.window_stroke = Stroke::new(1.0, light::BORDER);
    v.extreme_bg_color = Color32::WHITE;
    v.faint_bg_color = light::BACKDROP;
    v.code_bg_color = light::BACKDROP;
    v.hyperlink_color = ORANGE_TEXT;
    v.selection.bg_fill = TINT;
    v.selection.stroke = Stroke::new(1.0, ORANGE_TEXT);
    v.text_cursor.stroke = Stroke::new(2.0, ORANGE);

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, light::BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, INK);
    w.inactive.weak_bg_fill = Color32::WHITE;
    w.inactive.bg_fill = Color32::WHITE;
    w.inactive.bg_stroke = Stroke::new(1.0, light::BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, light::TEXT);
    w.hovered.weak_bg_fill = light::HOVER;
    w.hovered.bg_fill = light::HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, light::BORDER_STRONG);
    w.hovered.fg_stroke = Stroke::new(1.5, INK);
    w.active.weak_bg_fill = TINT;
    w.active.bg_fill = TINT;
    w.active.bg_stroke = Stroke::new(1.0, ORANGE);
    w.active.fg_stroke = Stroke::new(1.5, ORANGE_TEXT);
    w.open.weak_bg_fill = TINT;
    w.open.bg_stroke = Stroke::new(1.0, ORANGE);
    for wv in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        wv.corner_radius = RADIUS;
    }
    v
}

pub fn dark_visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.panel_fill = DARK_SURFACE;
    v.window_fill = dark::RAISED;
    v.window_stroke = Stroke::new(1.0, dark::BORDER);
    v.extreme_bg_color = dark::BACKDROP;
    v.faint_bg_color = dark::RAISED;
    v.code_bg_color = dark::RAISED;
    v.hyperlink_color = ORANGE_ON_DARK;
    v.selection.bg_fill = dark::TINT;
    v.selection.stroke = Stroke::new(1.0, ORANGE_ON_DARK);
    v.text_cursor.stroke = Stroke::new(2.0, ORANGE_ON_DARK);

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, dark::BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, dark::TEXT);
    w.inactive.weak_bg_fill = dark::RAISED;
    w.inactive.bg_fill = dark::RAISED;
    w.inactive.bg_stroke = Stroke::new(1.0, dark::BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, dark::TEXT);
    w.hovered.weak_bg_fill = dark::HOVER;
    w.hovered.bg_fill = dark::HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, dark::BORDER);
    w.hovered.fg_stroke = Stroke::new(1.5, dark::TEXT);
    w.active.weak_bg_fill = dark::TINT;
    w.active.bg_fill = dark::TINT;
    w.active.bg_stroke = Stroke::new(1.0, ORANGE_ON_DARK);
    w.active.fg_stroke = Stroke::new(1.5, ORANGE_ON_DARK);
    w.open.weak_bg_fill = dark::TINT;
    w.open.bg_stroke = Stroke::new(1.0, ORANGE_ON_DARK);
    for wv in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        wv.corner_radius = RADIUS;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance of an sRGB colour.
    fn luminance(c: Color32) -> f64 {
        let lin = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_colours_reach_aa_contrast() {
        assert!(contrast(Color32::WHITE, ORANGE_DEEP) >= 4.5);
        assert!(contrast(ORANGE_TEXT, TINT) >= 4.5);
        assert!(contrast(light::TEXT, light::PANEL) >= 4.5);
        assert!(contrast(light::MUTED, light::PANEL) >= 4.5);
        assert!(contrast(dark::TEXT, DARK_SURFACE) >= 4.5);
        assert!(contrast(dark::MUTED, DARK_SURFACE) >= 4.5);
        assert!(
            contrast(ORANGE_ON_DARK, dark::TINT) >= 3.0,
            "selected label on dark (large UI text)"
        );
    }
}
