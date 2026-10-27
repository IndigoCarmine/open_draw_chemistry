//! Hydrogen bonds and π-stacking ([`crate::three_d::interactions`]) shown over a 3D viewer,
//! as the clashes are: dashed lines drawn in screen space over the rendered image, the nearest
//! few with their distance. Shared by the 3D view and the assembly view, with the 表示
//! toggles and the list for the panel.
//!
//! A hydrogen bond is a light-blue dashed line from the H to the acceptor; a π-stack an amber
//! dashed line between the two ring centroids, each marked with a small circle. In a periodic
//! box the far end may be a periodic image, and the line goes to where that image is.

use egui::{Color32, Pos2, RichText, Stroke, vec2};
use serde::{Deserialize, Serialize};

use crate::three_d::geom::Vec3;
use crate::three_d::interactions::{HBond, Interactions, PiStack};

/// Line and label colours on the dark viewer.
pub const HBOND_STROKE: Color32 = Color32::from_rgb(0x7D, 0xD3, 0xFC);
pub const PI_STROKE: Color32 = Color32::from_rgb(0xFC, 0xD3, 0x4D);
/// The same roles as text on the light theme's panel.
const HBOND_ON_LIGHT: Color32 = Color32::from_rgb(0x03, 0x69, 0xA1);
const PI_ON_LIGHT: Color32 = Color32::from_rgb(0x92, 0x40, 0x0E);
const LABEL_BACK: Color32 = Color32::from_rgba_premultiplied(0x14, 0x12, 0x10, 0xD8);

/// How many of each kind are drawn, how many of those (the nearest) get a label, and how many
/// the panel lists.
const DRAWN: usize = 400;
const LABELLED: usize = 8;
const LISTED: usize = 6;

pub const HBOND_TIP: &str = "N・O・F に付いた H と、別の N・O・F（受容体）の組を水色の点線で表示。\n\
    H···A ≤ 2.5 Å、D···A ≤ 3.5 Å、角 D–H···A ≥ 120°。同じ分子内では 3 結合以内の組を除きます";
pub const PI_TIP: &str = "芳香環（5・6 員環、平面、4n+2 電子）どうしの重なりを、環の重心を結ぶ黄色の点線で表示。\n\
    平行：重心間 ≤ 4.4 Å、面のなす角 ≤ 30°、横のずれ ≤ 2.0 Å。\n\
    T 字：重心間 ≤ 5.5 Å、角 60–90°、ずれ ≤ 2.0 Å（Marcou & Rognan 2007、PLIP に準拠）。\n\
    縮環した環どうし・結合でつながった環どうしは数えません";

/// Which contacts the viewer shows (remembered with the view's other options).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShowInteractions {
    pub hbonds: bool,
    pub pi_stacking: bool,
    /// Also contacts inside one molecule (the assembly view; the 3D view always has them).
    pub intramolecular: bool,
}

impl Default for ShowInteractions {
    fn default() -> Self {
        Self {
            hbonds: true,
            pi_stacking: true,
            intramolecular: true,
        }
    }
}

impl ShowInteractions {
    pub fn any(self) -> bool {
        self.hbonds || self.pi_stacking
    }
}

/// The two checkboxes of the 表示 section.
pub fn toggles(ui: &mut egui::Ui, show: &mut ShowInteractions) {
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut show.hbonds, "水素結合")
            .on_hover_text(HBOND_TIP);
        ui.checkbox(&mut show.pi_stacking, "π スタッキング")
            .on_hover_text(PI_TIP);
    });
}

/// A label on a dark plate, so it reads over any atom colour.
fn tag(painter: &egui::Painter, pos: Pos2, text: String, color: Color32) {
    let galley = painter.layout_no_wrap(text, egui::FontId::proportional(11.0), color);
    let r = egui::Rect::from_min_size(pos, galley.size()).expand2(vec2(4.0, 1.0));
    painter.rect_filled(r, 3.0, LABEL_BACK);
    painter.galley(pos, galley, color);
}

/// Draw the contacts `show` asks for. `position` gives an atom's position (for the
/// hydrogens), and `project` puts a point of the scene on the screen (`None` behind the
/// camera).
pub fn draw(
    painter: &egui::Painter,
    project: impl Fn(Vec3) -> Option<Pos2>,
    position: impl Fn(usize) -> Option<Vec3>,
    found: &Interactions,
    show: ShowInteractions,
) {
    if show.hbonds {
        for (n, hb) in found.hbonds.iter().take(DRAWN).enumerate() {
            let Some(h) = position(hb.hydrogen) else {
                continue;
            };
            if let (Some(p), Some(q)) = (project(h), project(hb.acceptor_pos)) {
                painter.add(egui::Shape::dashed_line(
                    &[p, q],
                    Stroke::new(1.5, HBOND_STROKE),
                    3.0,
                    3.0,
                ));
                if n < LABELLED {
                    tag(
                        painter,
                        p.lerp(q, 0.5) + vec2(4.0, -8.0),
                        format!("{:.2} Å", hb.distance),
                        HBOND_STROKE,
                    );
                }
            }
        }
    }
    if show.pi_stacking {
        for (n, s) in found.stacks.iter().take(DRAWN).enumerate() {
            let Some(ring) = found.rings.get(s.a) else {
                continue;
            };
            if let (Some(p), Some(q)) = (project(ring.centroid), project(s.centroid_b)) {
                let stroke = Stroke::new(1.5, PI_STROKE);
                painter.add(egui::Shape::dashed_line(&[p, q], stroke, 5.0, 3.0));
                painter.circle_stroke(p, 3.5, stroke);
                painter.circle_stroke(q, 3.5, stroke);
                if n < LABELLED {
                    tag(
                        painter,
                        p.lerp(q, 0.5) + vec2(4.0, -8.0),
                        format!("{:.2} Å {}", s.distance, s.kind.label()),
                        PI_STROKE,
                    );
                }
            }
        }
    }
}

/// The text colours of the two kinds in the panel.
fn inks(ui: &egui::Ui) -> (Color32, Color32) {
    if ui.visuals().dark_mode {
        (HBOND_STROKE, PI_STROKE)
    } else {
        (HBOND_ON_LIGHT, PI_ON_LIGHT)
    }
}

/// One hydrogen bond as text, its atoms named by `label`.
pub fn hbond_text(hb: &HBond, label: &impl Fn(usize) -> String) -> String {
    format!(
        "{}–{}···{}：{:.2} Å（{:.0}°）",
        label(hb.donor),
        label(hb.hydrogen),
        label(hb.acceptor),
        hb.distance,
        hb.angle
    )
}

/// One π-stack as text, each ring named by its first atom.
pub fn stack_text(found: &Interactions, s: &PiStack, label: &impl Fn(usize) -> String) -> String {
    let ring = |r: usize| {
        found
            .rings
            .get(r)
            .and_then(|r| r.atoms.first())
            .map_or_else(String::new, |&a| label(a))
    };
    format!(
        "環 {} – 環 {}：{:.2} Å {}（{:.0}°）",
        ring(s.a),
        ring(s.b),
        s.distance,
        s.kind.label(),
        s.angle
    )
}

/// An entry of [`list`] that was clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clicked {
    /// Index into [`Interactions::hbonds`].
    HBond(usize),
    /// Index into [`Interactions::stacks`].
    Stack(usize),
}

/// Counts and the nearest few of each kind shown, for the panel. `pending` follows the counts
/// (an update still running); `tip` says what clicking an entry does.
pub fn list(
    ui: &mut egui::Ui,
    found: &Interactions,
    show: ShowInteractions,
    pending: &str,
    tip: &str,
    label: impl Fn(usize) -> String,
) -> Option<Clicked> {
    let (hbond_ink, pi_ink) = inks(ui);
    let more = if found.truncated { " 以上" } else { "" };
    let mut clicked = None;
    let mut entry = |ui: &mut egui::Ui, text: String, color: Color32, what: Clicked| {
        if ui
            .add(egui::Button::new(RichText::new(text).size(11.0).color(color)).frame(false))
            .on_hover_text(tip)
            .clicked()
        {
            clicked = Some(what);
        }
    };
    if show.hbonds {
        ui.label(
            RichText::new(format!(
                "水素結合：{} 本{more}{pending}",
                found.hbonds.len()
            ))
            .size(12.0)
            .color(hbond_ink),
        )
        .on_hover_text(HBOND_TIP);
        for (k, hb) in found.hbonds.iter().take(LISTED).enumerate() {
            entry(ui, hbond_text(hb, &label), hbond_ink, Clicked::HBond(k));
        }
        if found.hbonds.len() > LISTED {
            ui.label(
                RichText::new(format!("ほか {} 本", found.hbonds.len() - LISTED))
                    .size(11.0)
                    .color(hbond_ink),
            );
        }
    }
    if show.pi_stacking {
        ui.label(
            RichText::new(format!(
                "π スタッキング：{} 組{more}{pending}",
                found.stacks.len()
            ))
            .size(12.0)
            .color(pi_ink),
        )
        .on_hover_text(PI_TIP);
        for (k, s) in found.stacks.iter().take(LISTED).enumerate() {
            entry(ui, stack_text(found, s, &label), pi_ink, Clicked::Stack(k));
        }
        if found.stacks.len() > LISTED {
            ui.label(
                RichText::new(format!("ほか {} 組", found.stacks.len() - LISTED))
                    .size(11.0)
                    .color(pi_ink),
            );
        }
    }
    clicked
}
