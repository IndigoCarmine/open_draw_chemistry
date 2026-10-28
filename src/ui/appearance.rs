//! 書式 of the structures: ring fills, line and text colours and drop shadows.
//!
//! The same items are in the menu bar's 書式 menu and the canvas's right-click menu. Each
//! colour is picked inside its submenu (a colour picker with opacity, and a row of common
//! colours) and applied with a button, so trying colours out costs no undo steps; the editor
//! records one per change it makes (`ChemStructEditor::fill_selected_ring` and its siblings).
//! What each change applies to is the editor's business: the selected ring atoms or bonds, the
//! selected bonds, the selected labels and texts, and for a shadow the structures the
//! selection touches (every structure when nothing is selected). A ring is filled or cleared
//! only through a selection, never through the atom last drawn or hovered (the editor's
//! hotspot), so both menus offer it only while atoms or bonds are selected. Line and text
//! colours are opaque: CDX and CDXML keep no opacity for them, so a translucent one would not
//! survive saving.

use chembuider_rs::ChemStructEditor;
use chembuider_rs::molecule::Shadow;
use egui::containers::menu::{MenuConfig, SubMenuButton};
use egui::ecolor::Hsva;
use egui::widgets::color_picker::{self, Alpha};
use egui::{Color32, PopupCloseBehavior, vec2};
use serde::{Deserialize, Serialize};

use super::Command;

/// The colours offered next to the picker (straight RGBA).
const SWATCHES: [[u8; 4]; 8] = [
    [0, 0, 0, 255],
    [128, 128, 128, 255],
    [220, 38, 38, 255],
    [234, 88, 12, 255],
    [22, 163, 74, 255],
    [37, 99, 235, 255],
    [147, 51, 234, 255],
    [255, 255, 255, 255],
];

/// A colour being chosen in a picker, as straight (un-premultiplied) RGBA.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "[u8; 4]", into = "[u8; 4]")]
pub struct ColorChoice {
    pub rgba: [u8; 4],
    /// The picker's own value, which keeps a hue the colour cannot (a grey's), so dragging
    /// the hue of black and then its saturation gives the hue that was dragged.
    hsva: Option<Hsva>,
}

impl From<[u8; 4]> for ColorChoice {
    fn from(rgba: [u8; 4]) -> Self {
        Self { rgba, hsva: None }
    }
}

impl From<ColorChoice> for [u8; 4] {
    fn from(choice: ColorChoice) -> Self {
        choice.rgba
    }
}

impl ColorChoice {
    /// The common colours and the picker. Returns whether the colour changed.
    fn ui(&mut self, ui: &mut egui::Ui, alpha: Alpha) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for rgba in SWATCHES {
                let [r, g, b, _] = rgba;
                let swatch = egui::Button::new("")
                    .fill(Color32::from_rgb(r, g, b))
                    .min_size(vec2(18.0, 18.0));
                let name = hex(rgba);
                let response = ui.add(swatch);
                // Named for screen readers (and the tests) by its colour.
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
                if response.on_hover_text(&name).clicked() {
                    // A swatch keeps the opacity chosen.
                    self.rgba = [r, g, b, self.rgba[3]];
                    self.hsva = None;
                    changed = true;
                }
            }
        });
        if alpha == Alpha::Opaque {
            // A swatch keeps the opacity, and a colour remembered from before may have one.
            self.rgba[3] = 255;
        }
        let mut hsva = match self.hsva {
            Some(h) if h.to_srgba_unmultiplied() == self.rgba => h,
            _ => Hsva::from_srgba_unmultiplied(self.rgba),
        };
        if color_picker::color_picker_hsva_2d(ui, &mut hsva, alpha) {
            self.rgba = hsva.to_srgba_unmultiplied();
            changed = true;
        }
        self.hsva = Some(hsva);
        changed
    }
}

/// `#RRGGBB`, with the opacity when it is not opaque.
fn hex([r, g, b, a]: [u8; 4]) -> String {
    if a == 255 {
        format!("#{r:02X}{g:02X}{b:02X}")
    } else {
        format!("#{r:02X}{g:02X}{b:02X} · {} %", (a as f32 / 2.55).round())
    }
}

/// The colours last chosen (remembered with the app) and the shadow being edited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub ring_fill: ColorChoice,
    pub line: ColorChoice,
    pub text: ColorChoice,
    #[serde(skip)]
    shadow: ShadowDraft,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            // A light blue the bonds still show through.
            ring_fill: [66, 133, 244, 90].into(),
            line: [220, 38, 38, 255].into(),
            text: [37, 99, 235, 255].into(),
            shadow: ShadowDraft::default(),
        }
    }
}

/// The shadow the 影 submenu edits: the selection's own until the user changes it, and the
/// selection's again once the selection's shadow changes (another selection, an undo, Apply).
#[derive(Debug, Clone, Default, PartialEq)]
struct ShadowDraft {
    shadow: Shadow,
    color: ColorChoice,
    /// The selection's shadow `shadow` was taken from.
    of: Option<Option<Shadow>>,
}

impl ShadowDraft {
    fn follow(&mut self, current: Option<Shadow>) {
        if self.of != Some(current) {
            self.shadow = current.unwrap_or_default();
            let [r, g, b, _] = self.shadow.color;
            self.color = [r, g, b, 255].into();
            self.of = Some(current);
        }
    }
}

/// What the items need to know about the active editor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Selection {
    /// The shadow of the structures a shadow would be given (`selected_shadow`).
    pub shadow: Option<Shadow>,
    /// Nothing is selected, so a shadow goes to every structure.
    pub everything: bool,
    /// Atoms or bonds are selected: there is a ring to fill or clear, and structures for a
    /// shadow.
    pub structure: bool,
}

impl Selection {
    pub fn of(editor: &ChemStructEditor) -> Self {
        Self {
            shadow: editor.selected_shadow(),
            everything: editor.selected_atoms.is_empty()
                && editor.selected_bonds.is_empty()
                && editor.selected_objects.is_empty(),
            structure: structure_selected(editor),
        }
    }
}

/// Whether atoms or bonds are selected.
fn structure_selected(editor: &ChemStructEditor) -> bool {
    !editor.selected_atoms.is_empty() || !editor.selected_bonds.is_empty()
}

/// Run `change` on the ring the selection picks out: false, doing nothing, when no atom or bond
/// is selected, and the hotspot (which only follows the cursor) set aside while it runs, so a
/// lone selected atom is not passed over for it.
fn on_selected_ring(
    editor: &mut ChemStructEditor,
    change: impl FnOnce(&mut ChemStructEditor) -> bool,
) -> bool {
    if !structure_selected(editor) {
        return false;
    }
    let hotspot = editor.hotspot_atom.take();
    let changed = change(editor);
    editor.hotspot_atom = hotspot;
    changed
}

/// A submenu that stays open while its picker is clicked and dragged.
fn submenu(ui: &mut egui::Ui, label: &str, content: impl FnOnce(&mut egui::Ui)) {
    let config = MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside);
    SubMenuButton::new(label).config(config).ui(ui, content);
}

/// A menu button that runs `command` and closes the menu.
fn action(ui: &mut egui::Ui, label: &str, command: Command, out: &mut Vec<Command>) {
    if ui.button(label).clicked() {
        out.push(command);
        ui.close();
    }
}

/// The 書式 items, for the menu bar's 書式 menu and the canvas's menu; greyed out without a
/// document (`selection` is `None`).
pub fn items(
    ui: &mut egui::Ui,
    appearance: &mut Appearance,
    selection: Option<Selection>,
    out: &mut Vec<Command>,
) {
    ui.add_enabled_ui(selection.is_some(), |ui| {
        let ring = selection.is_some_and(|s| s.structure);
        if selection.is_some() && !ring {
            ui.weak(t!("format.ring_needs_selection"));
        }
        ui.add_enabled_ui(ring, |ui| {
            submenu(ui, t!("format.fill_ring"), |ui| {
                ui.weak(t!("format.fill_ring_hint"));
                appearance.ring_fill.ui(ui, Alpha::OnlyBlend);
                action(
                    ui,
                    t!("format.fill"),
                    Command::FillRing(appearance.ring_fill.rgba),
                    out,
                );
            });
            action(ui, t!("format.clear_fill"), Command::ClearRingFill, out);
        });
        ui.separator();
        submenu(ui, t!("format.line_color"), |ui| {
            ui.weak(t!("format.line_color_hint"));
            appearance.line.ui(ui, Alpha::Opaque);
            action(
                ui,
                t!("format.apply_color"),
                Command::LineColor(Some(appearance.line.rgba)),
                out,
            );
            action(
                ui,
                t!("format.default_color"),
                Command::LineColor(None),
                out,
            );
        });
        submenu(ui, t!("format.text_color"), |ui| {
            ui.weak(t!("format.text_color_hint"));
            appearance.text.ui(ui, Alpha::Opaque);
            action(
                ui,
                t!("format.apply_color"),
                Command::TextColor(Some(appearance.text.rgba)),
                out,
            );
            action(
                ui,
                t!("format.default_color"),
                Command::TextColor(None),
                out,
            );
        });
        ui.separator();
        submenu(ui, t!("format.shadow"), |ui| {
            shadow_items(ui, &mut appearance.shadow, selection, out);
        });
    });
}

/// The 影 submenu: on/off for the structures the selection touches, and the shadow's colour,
/// opacity, offset and blur, which start as theirs.
fn shadow_items(
    ui: &mut egui::Ui,
    draft: &mut ShadowDraft,
    selection: Option<Selection>,
    out: &mut Vec<Command>,
) {
    let current = selection.and_then(|s| s.shadow);
    draft.follow(current);
    // Page objects alone (texts, arrows) belong to no structure, so a shadow would change
    // nothing.
    let structures = selection.is_some_and(|s| s.everything || s.structure);
    ui.weak(match selection {
        Some(s) if s.everything => t!("format.shadow_scope_all"),
        Some(s) if s.structure => t!("format.shadow_scope_selection"),
        _ => t!("format.shadow_scope_none"),
    });
    let mut on = current.is_some();
    let checkbox = ui.add_enabled(
        structures,
        egui::Checkbox::new(&mut on, t!("format.shadow_on")),
    );
    if checkbox.changed() {
        out.push(Command::Shadow(on.then_some(draft.shadow)));
    }
    ui.separator();
    if draft.color.ui(ui, Alpha::Opaque) {
        let [r, g, b, _] = draft.color.rgba;
        draft.shadow.color = [r, g, b, draft.shadow.color[3]];
    }
    egui::Grid::new("odc_shadow_grid")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label(t!("format.shadow_opacity"));
            let mut opacity = draft.shadow.color[3] as f32 / 2.55;
            if ui
                .add(egui::Slider::new(&mut opacity, 0.0..=100.0).suffix(" %"))
                .changed()
            {
                draft.shadow.color[3] = (opacity * 2.55).round() as u8;
            }
            ui.end_row();
            let length = |ui: &mut egui::Ui, value: &mut f32, range| {
                ui.add(
                    egui::DragValue::new(value)
                        .speed(0.01)
                        .range(range)
                        .fixed_decimals(2),
                )
                .on_hover_text(t!("format.shadow_length_tip"));
            };
            ui.label(t!("format.shadow_offset_x"));
            length(ui, &mut draft.shadow.offset[0], -3.0..=3.0);
            ui.end_row();
            ui.label(t!("format.shadow_offset_y"));
            length(ui, &mut draft.shadow.offset[1], -3.0..=3.0);
            ui.end_row();
            ui.label(t!("format.shadow_blur"));
            length(ui, &mut draft.shadow.blur, 0.0..=3.0);
            ui.end_row();
        });
    ui.add_enabled_ui(structures, |ui| {
        action(
            ui,
            t!("format.shadow_apply"),
            Command::Shadow(Some(draft.shadow)),
            out,
        );
    });
}

/// Carry out a 書式 command on `editor`. Returns the status-bar message, or `None` for a
/// command that is not a 書式 one.
pub fn run(editor: &mut ChemStructEditor, command: &Command) -> Option<&'static str> {
    let message = match *command {
        Command::FillRing(rgba) => {
            if on_selected_ring(editor, |e| e.fill_selected_ring(rgba)) {
                t!("format.status.ring_filled")
            } else {
                t!("format.status.no_ring")
            }
        }
        Command::ClearRingFill => {
            if on_selected_ring(editor, |e| e.clear_selected_ring_fill()) {
                t!("format.status.fill_cleared")
            } else {
                t!("format.status.no_fill")
            }
        }
        Command::LineColor(color) => {
            if editor.set_selected_bond_color(color) {
                t!("format.status.line_color_set")
            } else {
                t!("format.status.no_bonds")
            }
        }
        Command::TextColor(color) => {
            if editor.set_selected_text_color(color) {
                t!("format.status.text_color_set")
            } else {
                t!("format.status.no_texts")
            }
        }
        Command::Shadow(shadow) => match (editor.set_shadow(shadow), shadow) {
            (true, Some(_)) => t!("format.status.shadow_set"),
            (true, None) => t!("format.status.shadow_removed"),
            (false, _) => t!("format.status.shadow_unchanged"),
        },
        _ => return None,
    };
    Some(message)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chembuider_rs::BondOrder;

    /// Benzene with an OH on its first carbon, and a text.
    pub fn phenol(editor: &mut ChemStructEditor) -> (Vec<u32>, u32, u32) {
        let mol = &mut editor.molecule;
        let ring: Vec<u32> = (0..6)
            .map(|k| {
                let angle = k as f32 * std::f32::consts::TAU / 6.0;
                mol.add_atom("C".into(), [1.5 * angle.cos(), 1.5 * angle.sin()], 0)
            })
            .collect();
        for k in 0..6 {
            let order = if k % 2 == 0 {
                BondOrder::Double
            } else {
                BondOrder::Single
            };
            mol.add_bond(ring[k], ring[(k + 1) % 6], order);
        }
        let o = mol.add_atom("O".into(), [3.0, 0.0], 0);
        mol.add_bond(ring[0], o, BondOrder::Single);
        let text = mol.add_text([0.0, 4.0], "phenol");
        (ring, o, text)
    }

    /// Where the widget labelled `label` is in the last frame's accessibility tree.
    pub fn find(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
        let update = output.platform_output.accesskit_update.as_ref()?;
        update.nodes.iter().find_map(|(_, node)| {
            (node.label() == Some(label)).then(|| {
                let b = node.bounds()?;
                Some(egui::pos2(
                    ((b.x0 + b.x1) / 2.0) as f32,
                    ((b.y0 + b.y1) / 2.0) as f32,
                ))
            })?
        })
    }

    /// A headless egui context, clicked by the accessible labels of its widgets.
    pub struct Harness {
        pub ctx: egui::Context,
        pub last: Option<egui::FullOutput>,
    }

    impl Harness {
        pub fn new() -> Self {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            Self { ctx, last: None }
        }

        /// One frame with `events`, drawing `content`.
        pub fn frame(&mut self, events: Vec<egui::Event>, content: impl FnMut(&mut egui::Ui)) {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1200.0, 900.0));
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, content);
            output.textures_delta.clear();
            self.last = Some(output);
        }

        pub fn pos(&self, label: &str) -> egui::Pos2 {
            self.last
                .as_ref()
                .and_then(|o| find(o, label))
                .unwrap_or_else(|| panic!("no widget labelled {label:?} in {:?}", self.labels()))
        }

        /// The labels of the last frame's widgets, and the texts of its labels.
        pub fn labels(&self) -> Vec<String> {
            let nodes = self
                .last
                .as_ref()
                .and_then(|o| o.platform_output.accesskit_update.as_ref());
            nodes
                .into_iter()
                .flat_map(|u| u.nodes.iter())
                // A label's text is its value.
                .filter_map(|(_, n)| n.label().or(n.value()).map(str::to_owned))
                .collect()
        }

        /// Hover the widget labelled `label` (opening a submenu) for a few frames.
        pub fn hover(&mut self, label: &str, mut content: impl FnMut(&mut egui::Ui)) {
            let pos = self.pos(label);
            for _ in 0..3 {
                self.frame(vec![egui::Event::PointerMoved(pos)], &mut content);
            }
        }

        /// Click the widget labelled `label`.
        pub fn click(&mut self, label: &str, content: impl FnMut(&mut egui::Ui)) {
            let pos = self.pos(label);
            self.click_at(pos, egui::PointerButton::Primary, content);
        }

        /// Click `button` at `pos`.
        pub fn click_at(
            &mut self,
            pos: egui::Pos2,
            button: egui::PointerButton,
            mut content: impl FnMut(&mut egui::Ui),
        ) {
            let press = |pressed| egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(vec![egui::Event::PointerMoved(pos)], &mut content);
            self.frame(vec![press(true)], &mut content);
            self.frame(vec![press(false)], &mut content);
            self.frame(Vec::new(), &mut content);
        }
    }

    /// The button in the 環を塗りつぶす submenu fills with the colour chosen there: a click on
    /// a swatch leaves the submenu open, and the opacity chosen stays.
    #[test]
    fn the_fill_ring_submenu_fills_with_the_chosen_colour() {
        let mut appearance = Appearance::default();
        let mut out = Vec::new();
        let selection = Some(Selection {
            shadow: None,
            everything: false,
            structure: true,
        });
        let mut h = Harness::new();
        let mut show = |ui: &mut egui::Ui| {
            ui.menu_button("Format", |ui| {
                items(ui, &mut appearance, selection, &mut out)
            });
        };
        h.frame(Vec::new(), &mut show);
        h.click("Format", &mut show);
        h.hover(t!("format.fill_ring"), &mut show);
        h.click("#16A34A", &mut show);
        h.click(t!("format.fill"), &mut show);
        drop(show);
        assert_eq!(out, [Command::FillRing([22, 163, 74, 90])]);
        assert_eq!(appearance.ring_fill.rgba, [22, 163, 74, 90]);
    }

    /// The commands act on the editor as one undo step each, and say so in the status bar.
    #[test]
    fn the_commands_reach_the_editor() {
        let mut editor = ChemStructEditor::default();
        let (ring, o, text) = phenol(&mut editor);
        // Nothing selected: no ring to fill, no bonds or texts to colour.
        assert_eq!(
            run(&mut editor, &Command::FillRing([1, 2, 3, 4])),
            Some(t!("format.status.no_ring"))
        );
        assert_eq!(
            run(&mut editor, &Command::LineColor(Some([1, 2, 3, 255]))),
            Some(t!("format.status.no_bonds"))
        );
        assert!(editor.undo_stack.is_empty());

        editor.selected_atoms.extend(ring.iter().copied());
        assert_eq!(
            run(&mut editor, &Command::FillRing([1, 2, 3, 4])),
            Some(t!("format.status.ring_filled"))
        );
        assert_eq!(editor.molecule.ring_fills.len(), 1);
        assert_eq!(
            run(&mut editor, &Command::LineColor(Some([9, 9, 9, 255]))),
            Some(t!("format.status.line_color_set"))
        );
        assert_eq!(
            editor
                .molecule
                .bonds
                .iter()
                .filter(|b| b.color.is_some())
                .count(),
            6
        );
        editor.selected_atoms = [o].into();
        editor.selected_objects = [text].into();
        assert_eq!(
            run(&mut editor, &Command::TextColor(Some([0, 0, 255, 255]))),
            Some(t!("format.status.text_color_set"))
        );
        // The O's label is coloured, its bond is not.
        let atom = editor.molecule.atom_by_id(o).unwrap();
        assert_eq!(atom.color, Some([0, 0, 255, 255]));
        assert_eq!(
            editor
                .molecule
                .bonds
                .iter()
                .filter(|b| b.color.is_some())
                .count(),
            6
        );
        let shadow = Shadow::default();
        assert_eq!(
            run(&mut editor, &Command::Shadow(Some(shadow))),
            Some(t!("format.status.shadow_set"))
        );
        assert_eq!(editor.molecule.shadow_of(ring[3]), Some(shadow));
        assert_eq!(editor.undo_stack.len(), 4);
        assert_eq!(run(&mut editor, &Command::Undo), None);
    }

    /// A menu with the 書式 items as `selection` has them, and the commands a walk through it
    /// gives.
    fn walk(
        appearance: &mut Appearance,
        selection: Selection,
        steps: &[&str],
    ) -> (Vec<Command>, Vec<String>) {
        let mut out = Vec::new();
        let mut h = Harness::new();
        let mut show = |ui: &mut egui::Ui| {
            ui.menu_button("Format", |ui| {
                items(ui, appearance, Some(selection), &mut out)
            });
        };
        h.frame(Vec::new(), &mut show);
        h.click("Format", &mut show);
        let (last, through) = steps.split_last().unwrap();
        for step in through {
            h.hover(step, &mut show);
        }
        let labels = h.labels();
        h.click(last, &mut show);
        drop(show);
        (out, labels)
    }

    /// A ring is filled or cleared only through a selection: with none the items are greyed out
    /// with a hint, and the commands leave the ring under the hotspot (the atom last drawn or
    /// hovered) alone; a lone selected atom wins over the hotspot.
    #[test]
    fn the_ring_items_need_a_selection() {
        let nothing = Selection {
            shadow: None,
            everything: true,
            structure: false,
        };
        let mut appearance = Appearance::default();
        let (out, labels) = walk(&mut appearance, nothing, &[t!("format.clear_fill")]);
        assert!(out.is_empty(), "{out:?}");
        assert!(
            labels
                .iter()
                .any(|l| l == t!("format.ring_needs_selection"))
        );
        let (out, _) = walk(&mut appearance, nothing, &[t!("format.fill_ring")]);
        assert!(out.is_empty(), "{out:?}");
        let ring = Selection {
            structure: true,
            everything: false,
            ..nothing
        };
        let (out, labels) = walk(&mut appearance, ring, &[t!("format.clear_fill")]);
        assert_eq!(out, [Command::ClearRingFill]);
        assert!(
            !labels
                .iter()
                .any(|l| l == t!("format.ring_needs_selection"))
        );

        let mut editor = ChemStructEditor::default();
        let (ring, o, _) = phenol(&mut editor);
        editor.hotspot_atom = Some(ring[2]);
        assert_eq!(
            run(&mut editor, &Command::FillRing([1, 2, 3, 4])),
            Some(t!("format.status.no_ring"))
        );
        assert!(editor.molecule.ring_fills.is_empty());
        editor.molecule.set_ring_fill(ring.clone(), [1, 2, 3, 4]);
        assert_eq!(
            run(&mut editor, &Command::ClearRingFill),
            Some(t!("format.status.no_fill"))
        );
        assert_eq!(editor.molecule.ring_fills.len(), 1);
        assert!(editor.undo_stack.is_empty());
        assert_eq!(editor.hotspot_atom, Some(ring[2]));

        // The hotspot on the O (on no ring) does not hide the selected ring atom.
        editor.hotspot_atom = Some(o);
        editor.selected_atoms = [ring[0]].into();
        assert_eq!(
            run(&mut editor, &Command::ClearRingFill),
            Some(t!("format.status.fill_cleared"))
        );
        assert!(editor.molecule.ring_fills.is_empty());
        assert_eq!(editor.hotspot_atom, Some(o));
    }

    /// With only texts selected the shadow submenu says what it needs, and offers nothing that
    /// would change nothing.
    #[test]
    fn the_shadow_submenu_says_texts_cast_none() {
        let texts = Selection {
            shadow: None,
            everything: false,
            structure: false,
        };
        let mut appearance = Appearance::default();
        for item in [t!("format.shadow_on"), t!("format.shadow_apply")] {
            let (out, labels) = walk(&mut appearance, texts, &[t!("format.shadow"), item]);
            assert!(out.is_empty(), "{item}: {out:?}");
            assert!(labels.iter().any(|l| l == t!("format.shadow_scope_none")));
            assert!(
                !labels
                    .iter()
                    .any(|l| l == t!("format.shadow_scope_selection"))
            );
        }
        let atoms = Selection {
            structure: true,
            ..texts
        };
        let (out, labels) = walk(
            &mut appearance,
            atoms,
            &[t!("format.shadow"), t!("format.shadow_apply")],
        );
        assert_eq!(out, [Command::Shadow(Some(Shadow::default()))]);
        assert!(
            labels
                .iter()
                .any(|l| l == t!("format.shadow_scope_selection"))
        );
    }

    /// Line and text colours are applied opaque, even one remembered with an opacity.
    #[test]
    fn line_and_text_colours_are_opaque() {
        let selection = Selection {
            shadow: None,
            everything: false,
            structure: true,
        };
        let mut appearance = Appearance {
            line: [1, 2, 3, 100].into(),
            text: [4, 5, 6, 0].into(),
            ..Default::default()
        };
        let (line, _) = walk(
            &mut appearance,
            selection,
            &[t!("format.line_color"), t!("format.apply_color")],
        );
        assert_eq!(line, [Command::LineColor(Some([1, 2, 3, 255]))]);
        let (text, _) = walk(
            &mut appearance,
            selection,
            &[t!("format.text_color"), t!("format.apply_color")],
        );
        assert_eq!(text, [Command::TextColor(Some([4, 5, 6, 255]))]);
    }

    #[test]
    fn the_chosen_colours_are_remembered() {
        let appearance = Appearance {
            line: [1, 2, 3, 4].into(),
            ..Default::default()
        };
        let text = serde_json::to_string(&appearance).unwrap();
        let back: Appearance = serde_json::from_str(&text).unwrap();
        assert_eq!(back.line.rgba, [1, 2, 3, 4]);
        assert_eq!(back.ring_fill, Appearance::default().ring_fill);
        // An older save without them gets the defaults.
        let old: Appearance = serde_json::from_str("{}").unwrap();
        assert_eq!(old, Appearance::default());
    }

    /// The shadow controls start as the selection's shadow, and follow it when it changes.
    #[test]
    fn the_shadow_draft_follows_the_selection() {
        let mut draft = ShadowDraft::default();
        draft.follow(None);
        assert_eq!(draft.shadow, Shadow::default());
        draft.shadow.blur = 0.7;
        // The same selection keeps what the user is editing.
        draft.follow(None);
        assert_eq!(draft.shadow.blur, 0.7);
        let theirs = Shadow {
            color: [200, 0, 0, 50],
            offset: [0.5, -0.5],
            blur: 0.0,
        };
        draft.follow(Some(theirs));
        assert_eq!(draft.shadow, theirs);
        assert_eq!(draft.color.rgba, [200, 0, 0, 255]);
    }
}
