//! The tool palette on the left: drawing tools and elements, as in the design.

use chembuider_rs::{BondOrder, BondStereo, ChemStructEditor, Tool};
use egui::vec2;

use super::icons::{self, Frame, Icon};
use super::periodic_table;

/// Elements offered as buttons; anything else is picked from the 周期表… popup.
pub const ELEMENTS: [&str; 10] = ["C", "H", "N", "O", "S", "P", "F", "Cl", "Br", "I"];

/// What a palette button does to the editor.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
    Tool(PaletteTool),
    Bond(BondKind),
    /// Shown for the design's later tools (rings, chains, text, arrows, brackets) but not
    /// available yet: the editor widget has no mouse tool for them.
    Planned,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PaletteTool {
    Select,
    Eraser,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BondKind {
    Single,
    Double,
    Triple,
    Wedge,
    Hash,
    Wavy,
}

impl BondKind {
    fn order_and_stereo(self) -> (BondOrder, BondStereo) {
        match self {
            BondKind::Single => (BondOrder::Single, BondStereo::None),
            BondKind::Double => (BondOrder::Double, BondStereo::None),
            BondKind::Triple => (BondOrder::Triple, BondStereo::None),
            BondKind::Wedge => (BondOrder::Single, BondStereo::WedgeUp),
            BondKind::Hash => (BondOrder::Single, BondStereo::WedgeDown),
            BondKind::Wavy => (BondOrder::Single, BondStereo::Wavy),
        }
    }
}

const BUTTONS: [(Icon, &str, Action); 16] = [
    (Icon::Select, "選択", Action::Tool(PaletteTool::Select)),
    (Icon::Eraser, "消しゴム", Action::Tool(PaletteTool::Eraser)),
    (Icon::Single, "単結合", Action::Bond(BondKind::Single)),
    (Icon::Double, "二重結合", Action::Bond(BondKind::Double)),
    (Icon::Triple, "三重結合", Action::Bond(BondKind::Triple)),
    (Icon::Wedge, "くさび結合", Action::Bond(BondKind::Wedge)),
    (Icon::Hash, "破線くさび", Action::Bond(BondKind::Hash)),
    (Icon::Wavy, "波線結合", Action::Bond(BondKind::Wavy)),
    (Icon::Chain, "鎖", Action::Planned),
    (Icon::Benzene, "ベンゼン環", Action::Planned),
    (Icon::Cyclopentane, "シクロペンタン", Action::Planned),
    (Icon::Cyclohexane, "シクロヘキサン", Action::Planned),
    (Icon::Text, "テキスト", Action::Planned),
    (Icon::Arrow, "反応矢印", Action::Planned),
    (Icon::Equilibrium, "平衡矢印", Action::Planned),
    (Icon::Bracket, "ブラケット", Action::Planned),
];

/// Whether `action` is the editor's current state.
fn is_selected(editor: &ChemStructEditor, action: Action) -> bool {
    match action {
        Action::Tool(PaletteTool::Select) => editor.tool == Tool::Select,
        Action::Tool(PaletteTool::Eraser) => editor.tool == Tool::Eraser,
        Action::Bond(kind) => {
            let (order, stereo) = kind.order_and_stereo();
            editor.tool == Tool::Bond
                && editor.current_bond_order == order
                && editor.current_bond_stereo == stereo
        }
        Action::Planned => false,
    }
}

fn apply(editor: &mut ChemStructEditor, action: Action) {
    match action {
        Action::Tool(PaletteTool::Select) => editor.tool = Tool::Select,
        Action::Tool(PaletteTool::Eraser) => editor.tool = Tool::Eraser,
        Action::Bond(kind) => {
            let (order, stereo) = kind.order_and_stereo();
            editor.current_bond_order = order;
            editor.current_bond_stereo = stereo;
            editor.tool = Tool::Bond;
        }
        Action::Planned => {}
    }
}

/// Japanese name of the editor's current tool, for the status bar: the palette button that
/// is selected, or the bare tool when the bond style is one the palette does not offer.
pub fn tool_name(editor: &ChemStructEditor) -> &'static str {
    if let Some((_, name, _)) = BUTTONS.iter().find(|(_, _, a)| is_selected(editor, *a)) {
        return name;
    }
    match editor.tool {
        Tool::Select => "選択",
        Tool::Bond => "結合",
        Tool::Eraser => "消しゴム",
    }
}

pub fn palette(ui: &mut egui::Ui, editor: &mut ChemStructEditor, custom_element: &mut String) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        let gap = 4.0;
        let cell = vec2((ui.available_width() - gap) / 2.0, 44.0);

        heading(ui, "描画");
        egui::Grid::new("palette_tools")
            .num_columns(2)
            .spacing([gap, gap])
            .show(ui, |ui| {
                for (i, (icon, name, action)) in BUTTONS.into_iter().enumerate() {
                    let planned = action == Action::Planned;
                    let tip = if planned {
                        format!("{name}（準備中）")
                    } else {
                        name.to_owned()
                    };
                    let selected = is_selected(editor, action);
                    if icons::icon_button(ui, icon, cell, Frame::Tile, selected, !planned, &tip)
                        .clicked()
                    {
                        apply(editor, action);
                    }
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });

        heading(ui, "元素");
        egui::Grid::new("palette_elements")
            .num_columns(2)
            .spacing([gap, gap])
            .show(ui, |ui| {
                for (i, symbol) in ELEMENTS.into_iter().enumerate() {
                    let selected = editor.current_element == symbol;
                    let text = egui::RichText::new(symbol).size(14.0);
                    if ui
                        .add_sized(cell, egui::Button::selectable(selected, text))
                        .clicked()
                    {
                        set_element(editor, symbol);
                    }
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        ui.add_space(gap);
        periodic_table_button(ui, editor, custom_element);
    });
}

/// 周期表…: a periodic table to pick any element from, opening to the right over the canvas,
/// with a field below it for typing a symbol. Escape or a click outside closes it.
fn periodic_table_button(ui: &mut egui::Ui, editor: &mut ChemStructEditor, custom: &mut String) {
    let button = egui::Button::new("周期表…").min_size(vec2(ui.available_width(), 34.0));
    let response = ui.add(button);
    egui::Popup::menu(&response)
        .align(egui::RectAlign::RIGHT_START)
        .gap(6.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            if let Some(symbol) = periodic_table::table(ui, &editor.current_element) {
                set_element(editor, symbol);
                ui.close();
            }
            ui.horizontal(|ui| {
                ui.label("元素記号を入力");
                let field = ui.add(egui::TextEdit::singleline(custom).desired_width(60.0));
                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let valid = is_element_symbol(custom);
                if ui.add_enabled(valid, egui::Button::new("設定")).clicked() || (entered && valid)
                {
                    set_element(editor, custom.trim());
                    ui.close();
                }
            });
        });
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    let color = crate::theme::muted(ui.visuals().dark_mode);
    ui.label(egui::RichText::new(text).size(11.0).color(color));
}

/// Atoms the bond tool places from now on are `symbol`.
fn set_element(editor: &mut ChemStructEditor, symbol: &str) {
    editor.current_element = symbol.to_owned();
    if editor.tool != Tool::Bond {
        editor.tool = Tool::Bond;
    }
}

/// A plausible element symbol: one capital letter, optionally followed by one or two lower-case
/// letters. (Whether the element exists is left to the editor's valence check.)
pub fn is_element_symbol(text: &str) -> bool {
    let text = text.trim();
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && text.len() <= 3
        && chars.all(|c| c.is_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_symbols_are_recognised() {
        for ok in ["C", "Cl", "Uue", " Na "] {
            assert!(is_element_symbol(ok), "{ok}");
        }
        for bad in ["", "c", "CL", "Abcd", "1"] {
            assert!(!is_element_symbol(bad), "{bad}");
        }
    }

    #[test]
    fn choosing_an_element_switches_to_the_bond_tool() {
        let mut editor = ChemStructEditor::default();
        editor.tool = Tool::Select;
        set_element(&mut editor, "N");
        assert_eq!(editor.current_element, "N");
        assert_eq!(editor.tool, Tool::Bond);
    }

    #[test]
    fn picking_from_the_periodic_table_sets_every_element() {
        for z in 1..=periodic_table::LAST_Z {
            let symbol = periodic_table::element(z).unwrap().symbol;
            let mut editor = ChemStructEditor::default();
            editor.tool = Tool::Eraser;
            set_element(&mut editor, symbol);
            assert_eq!(editor.current_element, symbol);
            assert_eq!(editor.tool, Tool::Bond);
            assert!(is_element_symbol(symbol), "{symbol}");
        }
    }

    #[test]
    fn a_bond_button_sets_order_and_stereo_and_shows_selected() {
        let mut editor = ChemStructEditor::default();
        apply(&mut editor, Action::Bond(BondKind::Triple));
        assert_eq!(editor.current_bond_order, BondOrder::Triple);
        assert_eq!(editor.current_bond_stereo, BondStereo::None);
        assert!(is_selected(&editor, Action::Bond(BondKind::Triple)));
        assert!(!is_selected(&editor, Action::Bond(BondKind::Single)));
        assert_eq!(tool_name(&editor), "三重結合");

        apply(&mut editor, Action::Bond(BondKind::Hash));
        assert_eq!(editor.current_bond_order, BondOrder::Single);
        assert_eq!(editor.current_bond_stereo, BondStereo::WedgeDown);
        assert_eq!(tool_name(&editor), "破線くさび");
    }

    #[test]
    fn exactly_one_button_is_selected_for_each_available_action() {
        for (_, name, action) in BUTTONS {
            if action == Action::Planned {
                continue;
            }
            let mut editor = ChemStructEditor::default();
            apply(&mut editor, action);
            let selected = BUTTONS
                .iter()
                .filter(|(_, _, a)| is_selected(&editor, *a))
                .count();
            assert_eq!(selected, 1, "{name}");
        }
    }
}
