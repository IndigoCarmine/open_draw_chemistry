//! The tool palette on the left: drawing tools and elements, as in the design.

use chembuider_rs::molecule::bracket::crossing_bonds;
use chembuider_rs::molecule::{ArrowKind, BracketShape, BracketUsage, PageObject};
use chembuider_rs::{BondOrder, BondStereo, ChemStructEditor, Tool};
use egui::vec2;

use super::icons::{self, Frame, Icon};
use super::{Command, periodic_table};

/// Elements offered as buttons; anything else is picked from the 周期表… popup.
pub const ELEMENTS: [&str; 10] = ["C", "H", "N", "O", "S", "P", "F", "Cl", "Br", "I"];

/// What a palette button does to the editor.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
    Tool(PaletteTool),
    Bond(BondKind),
    /// Not a tool: asks the app to run this command (which reports in the status bar).
    Command(PaletteCommand),
    /// Opens the bracket chooser ([`bracket_menu`]): what the brackets put around the selected
    /// atoms mean, and their shape.
    BracketMenu,
    /// Shown for the design's later tools (rings, chains, brackets) but not available yet: the
    /// editor widget has no mouse tool for them.
    Planned,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PaletteTool {
    Select,
    Eraser,
    Text,
    Arrow(ArrowKind),
}

impl PaletteTool {
    fn tool(self) -> Tool {
        match self {
            PaletteTool::Select => Tool::Select,
            PaletteTool::Eraser => Tool::Eraser,
            PaletteTool::Text => Tool::Text,
            PaletteTool::Arrow(kind) => Tool::Arrow(kind),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PaletteCommand {
    InsertReactionTable,
}

impl PaletteCommand {
    fn command(self) -> Command {
        match self {
            PaletteCommand::InsertReactionTable => Command::InsertReactionTable,
        }
    }
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

const BUTTONS: [(Icon, &str, Action); 17] = [
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
    (Icon::Text, "テキスト", Action::Tool(PaletteTool::Text)),
    (
        Icon::Arrow,
        "反応矢印",
        Action::Tool(PaletteTool::Arrow(ArrowKind::Reaction)),
    ),
    (
        Icon::Equilibrium,
        "平衡矢印",
        Action::Tool(PaletteTool::Arrow(ArrowKind::Equilibrium)),
    ),
    (Icon::Bracket, "ブラケット", Action::BracketMenu),
    (
        Icon::Table,
        "反応表を挿入",
        Action::Command(PaletteCommand::InsertReactionTable),
    ),
];

/// Whether `action` is the editor's current state.
fn is_selected(editor: &ChemStructEditor, action: Action) -> bool {
    match action {
        Action::Tool(tool) => editor.tool == tool.tool(),
        Action::Bond(kind) => {
            let (order, stereo) = kind.order_and_stereo();
            editor.tool == Tool::Bond
                && editor.current_bond_order == order
                && editor.current_bond_stereo == stereo
        }
        Action::Command(_) | Action::BracketMenu | Action::Planned => false,
    }
}

fn apply(editor: &mut ChemStructEditor, action: Action) {
    match action {
        Action::Tool(tool) => editor.tool = tool.tool(),
        Action::Bond(kind) => {
            let (order, stereo) = kind.order_and_stereo();
            editor.current_bond_order = order;
            editor.current_bond_stereo = stereo;
            editor.tool = Tool::Bond;
        }
        Action::Command(_) | Action::BracketMenu | Action::Planned => {}
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
        Tool::Text => "テキスト",
        Tool::Arrow(ArrowKind::Reaction) => "反応矢印",
        Tool::Arrow(ArrowKind::Equilibrium) => "平衡矢印",
    }
}

/// Switch the editor to the arrow tool of `kind` (the 反応 menu's 「反応矢印を追加」「平衡矢印を追加」).
pub fn select_arrow_tool(editor: &mut ChemStructEditor, kind: ArrowKind) {
    apply(editor, Action::Tool(PaletteTool::Arrow(kind)));
}

/// Switch the editor to the text tool (the 反応 menu's 「反応条件のテキスト」).
pub fn select_text_tool(editor: &mut ChemStructEditor) {
    apply(editor, Action::Tool(PaletteTool::Text));
}

/// 「反応表を挿入」: put a reaction table under the selected arrow's reaction (or the first
/// reaction on the page) and switch to the select tool, with which its cells are edited by a
/// double-click. The status bar message: what was done, or why nothing was.
pub fn insert_reaction_table(editor: &mut ChemStructEditor) -> &'static str {
    match editor.insert_reaction_table() {
        Some(_) => {
            apply(editor, Action::Tool(PaletteTool::Select));
            "反応表を挿入しました。セルをダブルクリックすると当量・質量などを入力できます（入力欄は Enter で確定）"
        }
        None => "反応表にする構造がありません。先に構造と反応矢印を描いてください",
    }
}

/// The bracket usages the chooser offers, with their menu labels.
pub const BRACKET_USAGES: [(BracketUsage, &str); 8] = [
    (BracketUsage::Sru, "繰り返し単位（n）"),
    (BracketUsage::MultipleGroup, "多重基（×n）"),
    (BracketUsage::Copolymer, "共重合体（co）"),
    (BracketUsage::CopolymerAlternating, "交互共重合体（alt）"),
    (BracketUsage::CopolymerRandom, "ランダム共重合体（ran）"),
    (BracketUsage::CopolymerBlock, "ブロック共重合体（blk）"),
    (BracketUsage::Generic, "ポリマー（一般, gen）"),
    (BracketUsage::MixtureUnordered, "混合物（mix）"),
];

/// The bracket shapes the chooser offers.
pub const BRACKET_SHAPES: [(BracketShape, &str); 3] = [
    (BracketShape::Square, "角括弧 [ ]"),
    (BracketShape::Round, "丸括弧 ( )"),
    (BracketShape::Curly, "波括弧 { }"),
];

/// The bracket chooser (the palette's ブラケット button, the 構造 menu): one item per usage, then
/// the shapes. Returns whether an item was chosen.
pub fn bracket_menu(ui: &mut egui::Ui, out: &mut Vec<Command>) -> bool {
    let mut chosen = false;
    ui.label(egui::RichText::new("選択した原子を括弧で囲む").weak());
    for (usage, label) in BRACKET_USAGES {
        if ui.button(label).clicked() {
            out.push(Command::Bracket(usage));
            chosen = true;
        }
    }
    ui.separator();
    ui.label(egui::RichText::new("選択した括弧の形").weak());
    for (shape, label) in BRACKET_SHAPES {
        if ui.button(label).clicked() {
            out.push(Command::BracketShape(shape));
            chosen = true;
        }
    }
    chosen
}

fn usage_name(usage: BracketUsage) -> &'static str {
    BRACKET_USAGES
        .iter()
        .find(|(u, _)| *u == usage)
        .map_or("括弧", |(_, label)| label)
}

/// 括弧で囲む: brackets of `usage` around the selected atoms — or, when the selection is
/// brackets (with at most the atoms they enclose), those brackets get `usage`. Switches to the
/// select tool, with which the label is edited by a double-click. The status bar message.
pub fn bracket_selection(editor: &mut ChemStructEditor, usage: BracketUsage) -> String {
    let existing = editor.selected_brackets();
    let enclosed: std::collections::HashSet<u32> = existing
        .iter()
        .filter_map(|id| match editor.molecule.object_by_id(*id) {
            Some(PageObject::Bracket(b)) => Some(b.atoms.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    let only_brackets = !existing.is_empty()
        && (editor.selected_atoms.iter().all(|a| enclosed.contains(a))
            || existing
                .iter()
                .all(|id| editor.selected_objects.contains(id)));
    let name = usage_name(usage);
    if only_brackets {
        let changed = existing
            .iter()
            .filter(|&&id| editor.set_bracket_usage(id, usage))
            .count();
        return if changed > 0 {
            format!("選択した括弧を「{name}」にしました")
        } else {
            format!("選択した括弧はすでに「{name}」です")
        };
    }
    let Some(id) = editor.bracket_selection(usage) else {
        return "括弧で囲む原子を選択してください（選択ツールで範囲をドラッグ）".to_owned();
    };
    apply(editor, Action::Tool(PaletteTool::Select));
    let crossings = match editor.molecule.object_by_id(id) {
        Some(PageObject::Bracket(b)) => crossing_bonds(&editor.molecule, b).len(),
        _ => 0,
    };
    let mut message = match usage {
        BracketUsage::Sru => {
            "繰り返し単位の括弧を付けました。ラベル n はダブルクリックで編集できます".to_owned()
        }
        BracketUsage::MultipleGroup => {
            "多重基の括弧を付けました。繰り返し数（2）はダブルクリックで編集できます".to_owned()
        }
        _ => format!("「{name}」の括弧を付けました"),
    };
    if usage.is_polymer_unit() && crossings != 2 {
        message.push_str(&format!(
            "（括弧を横切る結合が {crossings} 本です。繰り返し単位は通常、両側に 1 本ずつです）"
        ));
    }
    message
}

/// Give the selected brackets `shape`. The status bar message.
pub fn bracket_shape(editor: &mut ChemStructEditor, shape: BracketShape) -> String {
    let brackets = editor.selected_brackets();
    if brackets.is_empty() {
        return "形を変える括弧を選択してください".to_owned();
    }
    let changed = brackets
        .iter()
        .filter(|&&id| editor.set_bracket_shape(id, shape))
        .count();
    let label = BRACKET_SHAPES
        .iter()
        .find(|(s, _)| *s == shape)
        .map_or("", |(_, l)| l);
    if changed > 0 {
        format!("括弧を{label}にしました")
    } else {
        format!("括弧はすでに{label}です")
    }
}

pub fn palette(
    ui: &mut egui::Ui,
    editor: &mut ChemStructEditor,
    custom_element: &mut String,
    out: &mut Vec<Command>,
) {
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
                    let response =
                        icons::icon_button(ui, icon, cell, Frame::Tile, selected, !planned, &tip);
                    if action == Action::BracketMenu {
                        egui::Popup::menu(&response)
                            .align(egui::RectAlign::RIGHT_START)
                            .gap(6.0)
                            .show(|ui| {
                                if bracket_menu(ui, out) {
                                    ui.close();
                                }
                            });
                    } else if response.clicked() {
                        match action {
                            Action::Command(command) => out.push(command.command()),
                            _ => apply(editor, action),
                        }
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
    fn the_arrow_and_text_buttons_select_their_tools() {
        let mut editor = ChemStructEditor::default();
        select_arrow_tool(&mut editor, ArrowKind::Equilibrium);
        assert_eq!(editor.tool, Tool::Arrow(ArrowKind::Equilibrium));
        assert_eq!(tool_name(&editor), "平衡矢印");
        select_arrow_tool(&mut editor, ArrowKind::Reaction);
        assert_eq!(tool_name(&editor), "反応矢印");
        select_text_tool(&mut editor);
        assert_eq!(editor.tool, Tool::Text);
        assert_eq!(tool_name(&editor), "テキスト");
        let planned: Vec<&str> = BUTTONS
            .iter()
            .filter(|(_, _, a)| *a == Action::Planned)
            .map(|(_, name, _)| *name)
            .collect();
        assert_eq!(
            planned,
            ["鎖", "ベンゼン環", "シクロペンタン", "シクロヘキサン"]
        );
    }

    /// CH2–CH2 with a bond leaving each side (polyethylene's repeat unit), its middle selected.
    fn polyethylene(editor: &mut ChemStructEditor) -> Vec<u32> {
        let m = &mut editor.molecule;
        let ids: Vec<u32> = (0..4)
            .map(|k| {
                let y = if k % 2 == 0 { 0.0 } else { -0.75 };
                m.add_atom("C".into(), [k as f32 * 1.3, y], 0)
            })
            .collect();
        for w in ids.windows(2) {
            m.add_bond(w[0], w[1], BondOrder::Single);
        }
        editor.selected_atoms = [ids[1], ids[2]].into_iter().collect();
        ids
    }

    /// 括弧で囲む puts brackets round the selection (one undo step), a second choice changes
    /// what they mean instead of adding more, and nothing selected says what to do.
    #[test]
    fn bracketing_the_selection() {
        let mut editor = ChemStructEditor::default();
        assert!(bracket_selection(&mut editor, BracketUsage::Sru).contains("選択してください"));
        polyethylene(&mut editor);
        let message = bracket_selection(&mut editor, BracketUsage::Sru);
        assert!(
            message.starts_with("繰り返し単位の括弧を付けました"),
            "{message}"
        );
        assert_eq!(editor.tool, Tool::Select);
        assert_eq!(editor.molecule.brackets().count(), 1);
        let message = bracket_selection(&mut editor, BracketUsage::MultipleGroup);
        assert!(message.contains("多重基"), "{message}");
        let b = editor.molecule.brackets().next().unwrap();
        assert_eq!(
            (b.usage, b.label.as_str()),
            (BracketUsage::MultipleGroup, "2")
        );
        assert_eq!(editor.molecule.brackets().count(), 1, "changed, not added");
        assert!(bracket_shape(&mut editor, BracketShape::Round).contains("丸括弧"));
        assert_eq!(
            editor.molecule.brackets().next().unwrap().shape,
            BracketShape::Round
        );
        assert!(editor.undo() && editor.undo() && editor.undo());
        assert_eq!(editor.molecule.brackets().count(), 0);
    }

    /// A repeat unit with the wrong number of bonds leaving it is bracketed, with a note.
    #[test]
    fn a_repeat_unit_without_two_crossing_bonds_gets_a_note() {
        let mut editor = ChemStructEditor::default();
        let ids = polyethylene(&mut editor);
        editor.selected_atoms = [ids[2], ids[3]].into_iter().collect();
        let message = bracket_selection(&mut editor, BracketUsage::Sru);
        assert!(message.contains("1 本"), "{message}");
    }

    /// 「反応表を挿入」 puts a table under the reaction and leaves the select tool on, so a
    /// double-click edits its cells; with nothing drawn it says why nothing happened.
    #[test]
    fn inserting_a_reaction_table() {
        let mut editor = ChemStructEditor::default();
        assert!(insert_reaction_table(&mut editor).starts_with("反応表にする構造がありません"));
        assert!(editor.molecule.objects.is_empty());
        let m = &mut editor.molecule;
        let a = m.add_atom("C".into(), [0.0, 0.0], 0);
        let b = m.add_atom("O".into(), [1.3, 0.0], 0);
        m.add_bond(a, b, BondOrder::Single);
        m.add_arrow([2.5, 0.0], [5.5, 0.0], ArrowKind::Reaction);
        m.add_atom("C".into(), [6.5, 0.0], 0);
        editor.tool = Tool::Bond;
        assert!(insert_reaction_table(&mut editor).starts_with("反応表を挿入しました"));
        assert_eq!(editor.tool, Tool::Select);
        let table = editor
            .molecule
            .objects
            .iter()
            .find_map(|o| o.as_table())
            .expect("a table");
        assert_eq!((table.reactants.len(), table.products.len()), (1, 1));
        assert!(table.pos[1] > 0.0, "under the reaction");
        assert!(editor.undo());
        assert!(
            editor
                .molecule
                .objects
                .iter()
                .all(|o| o.as_table().is_none())
        );
    }

    #[test]
    fn exactly_one_button_is_selected_for_each_available_action() {
        for (_, name, action) in BUTTONS {
            if matches!(action, Action::Planned | Action::BracketMenu) {
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
