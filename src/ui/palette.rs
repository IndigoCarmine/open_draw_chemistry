//! The tool palette on the left: drawing tools and elements, as in the design.

use chembuider_rs::molecule::bracket::crossing_bonds;
use chembuider_rs::molecule::{ArrowKind, BracketShape, BracketUsage, PageObject};
use chembuider_rs::{BondOrder, BondStereo, ChemStructEditor, Tool};
use egui::vec2;

use super::icons::{self, Frame, Icon};
use super::{Command, periodic_table};

/// Shared scale for the drawing palette; popup contents retain their normal size.
pub const PALETTE_SCALE: f32 = 2.0 / 3.0;
const BASE_WIDTH: f32 = 112.0;
const BASE_CELL_HEIGHT: f32 = 44.0;
const BASE_BUTTON_HEIGHT: f32 = 34.0;
const BASE_GAP: f32 = 4.0;
const BASE_MARGIN_X: f32 = 8.0;
const BASE_MARGIN_Y: f32 = 10.0;
// The original 44-point cell used a 20-point icon (44 * 0.46, rounded).
const BASE_ICON_SIZE: f32 = 20.0;
const BASE_ELEMENT_FONT: f32 = 14.0;
const BASE_LABEL_FONT: f32 = 13.0;
const BASE_HEADING_FONT: f32 = 11.0;
const BASE_HEADING_SPACE: f32 = 6.0;
pub const PALETTE_WIDTH: f32 = BASE_WIDTH * PALETTE_SCALE;
pub const PALETTE_MARGIN: egui::Margin = egui::Margin::symmetric(
    (BASE_MARGIN_X * PALETTE_SCALE).round() as i8,
    (BASE_MARGIN_Y * PALETTE_SCALE).round() as i8,
);
const CELL_HEIGHT: f32 = BASE_CELL_HEIGHT * PALETTE_SCALE;
const BUTTON_HEIGHT: f32 = BASE_BUTTON_HEIGHT * PALETTE_SCALE;
const GAP: f32 = BASE_GAP * PALETTE_SCALE;

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

const BUTTONS: [(Icon, Action); 17] = [
    (Icon::Select, Action::Tool(PaletteTool::Select)),
    (Icon::Eraser, Action::Tool(PaletteTool::Eraser)),
    (Icon::Single, Action::Bond(BondKind::Single)),
    (Icon::Double, Action::Bond(BondKind::Double)),
    (Icon::Triple, Action::Bond(BondKind::Triple)),
    (Icon::Wedge, Action::Bond(BondKind::Wedge)),
    (Icon::Hash, Action::Bond(BondKind::Hash)),
    (Icon::Wavy, Action::Bond(BondKind::Wavy)),
    (Icon::Chain, Action::Planned),
    (Icon::Benzene, Action::Planned),
    (Icon::Cyclopentane, Action::Planned),
    (Icon::Cyclohexane, Action::Planned),
    (Icon::Text, Action::Tool(PaletteTool::Text)),
    (
        Icon::Arrow,
        Action::Tool(PaletteTool::Arrow(ArrowKind::Reaction)),
    ),
    (
        Icon::Equilibrium,
        Action::Tool(PaletteTool::Arrow(ArrowKind::Equilibrium)),
    ),
    (Icon::Bracket, Action::BracketMenu),
    (
        Icon::Table,
        Action::Command(PaletteCommand::InsertReactionTable),
    ),
];

/// The name of the palette button with `icon` (its tooltip, and the tool in the status bar).
fn button_name(icon: Icon) -> &'static str {
    match icon {
        Icon::Select => t!("palette.tool.select"),
        Icon::Eraser => t!("palette.tool.eraser"),
        Icon::Single => t!("palette.tool.single_bond"),
        Icon::Double => t!("palette.tool.double_bond"),
        Icon::Triple => t!("palette.tool.triple_bond"),
        Icon::Wedge => t!("palette.tool.wedge_bond"),
        Icon::Hash => t!("palette.tool.hashed_wedge"),
        Icon::Wavy => t!("palette.tool.wavy_bond"),
        Icon::Chain => t!("palette.tool.chain"),
        Icon::Benzene => t!("palette.tool.benzene"),
        Icon::Cyclopentane => t!("palette.tool.cyclopentane"),
        Icon::Cyclohexane => t!("palette.tool.cyclohexane"),
        Icon::Text => t!("palette.tool.text"),
        Icon::Arrow => t!("palette.tool.reaction_arrow"),
        Icon::Equilibrium => t!("palette.tool.equilibrium_arrow"),
        Icon::Bracket => t!("palette.tool.bracket"),
        Icon::Table => t!("palette.tool.insert_reaction_table"),
        _ => "",
    }
}

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

/// Name of the editor's current tool, for the status bar: the palette button that
/// is selected, or the bare tool when the bond style is one the palette does not offer.
pub fn tool_name(editor: &ChemStructEditor) -> &'static str {
    if let Some((icon, _)) = BUTTONS.iter().find(|(_, a)| is_selected(editor, *a)) {
        return button_name(*icon);
    }
    match editor.tool {
        Tool::Select => t!("palette.tool.select"),
        Tool::Bond => t!("palette.tool.bond"),
        Tool::Eraser => t!("palette.tool.eraser"),
        Tool::Text => t!("palette.tool.text"),
        Tool::Arrow(ArrowKind::Reaction) => t!("palette.tool.reaction_arrow"),
        Tool::Arrow(ArrowKind::Equilibrium) => t!("palette.tool.equilibrium_arrow"),
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
            t!("palette.reaction_table.inserted")
        }
        None => t!("palette.reaction_table.nothing_to_tabulate"),
    }
}

/// The bracket usages the chooser offers (labels from [`usage_name`]).
pub const BRACKET_USAGES: [BracketUsage; 8] = [
    BracketUsage::Sru,
    BracketUsage::MultipleGroup,
    BracketUsage::Copolymer,
    BracketUsage::CopolymerAlternating,
    BracketUsage::CopolymerRandom,
    BracketUsage::CopolymerBlock,
    BracketUsage::Generic,
    BracketUsage::MixtureUnordered,
];

/// The bracket shapes the chooser offers (labels from [`shape_name`]).
pub const BRACKET_SHAPES: [BracketShape; 3] = [
    BracketShape::Square,
    BracketShape::Round,
    BracketShape::Curly,
];

/// The bracket chooser (the palette's ブラケット button, the 構造 menu): one item per usage, then
/// the shapes. Returns whether an item was chosen.
pub fn bracket_menu(ui: &mut egui::Ui, out: &mut Vec<Command>) -> bool {
    let mut chosen = false;
    ui.label(egui::RichText::new(t!("palette.bracket.enclose_heading")).weak());
    for usage in BRACKET_USAGES {
        if ui.button(usage_name(usage)).clicked() {
            out.push(Command::Bracket(usage));
            chosen = true;
        }
    }
    ui.separator();
    ui.label(egui::RichText::new(t!("palette.bracket.shape_heading")).weak());
    for shape in BRACKET_SHAPES {
        if ui.button(shape_name(shape)).clicked() {
            out.push(Command::BracketShape(shape));
            chosen = true;
        }
    }
    chosen
}

/// The menu label of a bracket usage.
fn usage_name(usage: BracketUsage) -> &'static str {
    match usage {
        BracketUsage::Sru => t!("palette.bracket.usage.sru"),
        BracketUsage::MultipleGroup => t!("palette.bracket.usage.multiple_group"),
        BracketUsage::Copolymer => t!("palette.bracket.usage.copolymer"),
        BracketUsage::CopolymerAlternating => t!("palette.bracket.usage.alternating"),
        BracketUsage::CopolymerRandom => t!("palette.bracket.usage.random"),
        BracketUsage::CopolymerBlock => t!("palette.bracket.usage.block"),
        BracketUsage::Generic => t!("palette.bracket.usage.generic"),
        BracketUsage::MixtureUnordered => t!("palette.bracket.usage.mixture"),
        _ => t!("palette.bracket.usage.unknown"),
    }
}

/// The menu label of a bracket shape.
fn shape_name(shape: BracketShape) -> &'static str {
    match shape {
        BracketShape::Square => t!("palette.bracket.shape.square"),
        BracketShape::Round => t!("palette.bracket.shape.round"),
        BracketShape::Curly => t!("palette.bracket.shape.curly"),
    }
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
            t!("palette.bracket.usage_changed", name = name)
        } else {
            t!("palette.bracket.usage_unchanged", name = name)
        };
    }
    let Some(id) = editor.bracket_selection(usage) else {
        return t!("palette.bracket.select_atoms").to_owned();
    };
    apply(editor, Action::Tool(PaletteTool::Select));
    let crossings = match editor.molecule.object_by_id(id) {
        Some(PageObject::Bracket(b)) => crossing_bonds(&editor.molecule, b).len(),
        _ => 0,
    };
    let mut message = match usage {
        BracketUsage::Sru => t!("palette.bracket.added_sru").to_owned(),
        BracketUsage::MultipleGroup => t!("palette.bracket.added_multiple_group").to_owned(),
        _ => t!("palette.bracket.added", name = name),
    };
    if usage.is_polymer_unit() && crossings != 2 {
        message.push_str(&tn!("palette.bracket.crossing_note", crossings));
    }
    message
}

/// Give the selected brackets `shape`. The status bar message.
pub fn bracket_shape(editor: &mut ChemStructEditor, shape: BracketShape) -> String {
    let brackets = editor.selected_brackets();
    if brackets.is_empty() {
        return t!("palette.bracket.select_brackets").to_owned();
    }
    let changed = brackets
        .iter()
        .filter(|&&id| editor.set_bracket_shape(id, shape))
        .count();
    let label = shape_name(shape);
    if changed > 0 {
        t!("palette.bracket.shape_changed", shape = label)
    } else {
        t!("palette.bracket.shape_unchanged", shape = label)
    }
}

pub fn palette(
    ui: &mut egui::Ui,
    editor: &mut ChemStructEditor,
    custom_element: &mut String,
    out: &mut Vec<Command>,
) {
    let popup_style = ui.style().clone();
    ui.scope(|ui| {
        let spacing = ui.spacing_mut();
        spacing.button_padding *= PALETTE_SCALE;
        spacing.interact_size *= PALETTE_SCALE;
        spacing.item_spacing *= PALETTE_SCALE;
        egui::ScrollArea::vertical().show(ui, |ui| {
            let gap = GAP;
            let cell = vec2((ui.available_width() - gap) / 2.0, CELL_HEIGHT);

            heading(ui, t!("palette.heading.draw"));
            egui::Grid::new("palette_tools")
                .num_columns(2)
                .spacing([gap, gap])
                .show(ui, |ui| {
                    for (i, (icon, action)) in BUTTONS.into_iter().enumerate() {
                        let planned = action == Action::Planned;
                        let name = button_name(icon);
                        let tip = if planned {
                            t!("palette.tool.planned", name = name)
                        } else {
                            name.to_owned()
                        };
                        let selected = is_selected(editor, action);
                        let response = icons::icon_button_with_glyph(
                            ui,
                            icon,
                            (cell, (BASE_ICON_SIZE * PALETTE_SCALE).round()),
                            Frame::Tile,
                            selected,
                            !planned,
                            &tip,
                        );
                        if action == Action::BracketMenu {
                            egui::Popup::menu(&response)
                                .align(egui::RectAlign::RIGHT_START)
                                .gap(6.0)
                                .show(|ui| {
                                    ui.set_style(popup_style.clone());
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

            heading(ui, t!("palette.heading.elements"));
            egui::Grid::new("palette_elements")
                .num_columns(2)
                .spacing([gap, gap])
                .show(ui, |ui| {
                    for (i, symbol) in ELEMENTS.into_iter().enumerate() {
                        let selected = editor.current_element == symbol;
                        let text =
                            egui::RichText::new(symbol).size(BASE_ELEMENT_FONT * PALETTE_SCALE);
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
            periodic_table_button(ui, editor, custom_element, &popup_style);
        });
    });
}

/// 周期表…: a periodic table to pick any element from, opening to the right over the canvas,
/// with a field below it for typing a symbol. Escape or a click outside closes it.
fn periodic_table_button(
    ui: &mut egui::Ui,
    editor: &mut ChemStructEditor,
    custom: &mut String,
    popup_style: &std::sync::Arc<egui::Style>,
) {
    let button = egui::Button::new(
        egui::RichText::new(t!("palette.periodic_table.button"))
            .size(BASE_LABEL_FONT * PALETTE_SCALE),
    )
    .min_size(vec2(ui.available_width(), BUTTON_HEIGHT));
    let response = ui.add(button);
    egui::Popup::menu(&response)
        .align(egui::RectAlign::RIGHT_START)
        .gap(6.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_style(popup_style.clone());
            ui.spacing_mut().item_spacing.y = 6.0;
            if let Some(symbol) = periodic_table::table(ui, &editor.current_element) {
                set_element(editor, symbol);
                ui.close();
            }
            ui.horizontal(|ui| {
                ui.label(t!("palette.periodic_table.enter_symbol"));
                let field = ui.add(egui::TextEdit::singleline(custom).desired_width(60.0));
                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let valid = is_element_symbol(custom);
                if ui
                    .add_enabled(
                        valid,
                        egui::Button::new(t!("palette.periodic_table.set_symbol")),
                    )
                    .clicked()
                    || (entered && valid)
                {
                    set_element(editor, custom.trim());
                    ui.close();
                }
            });
        });
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(BASE_HEADING_SPACE * PALETTE_SCALE);
    let color = crate::theme::muted(ui.visuals().dark_mode);
    ui.label(
        egui::RichText::new(text)
            .size(BASE_HEADING_FONT * PALETTE_SCALE)
            .color(color),
    );
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
    fn palette_content_fits_the_narrow_panel() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.debug.show_unaligned = false);
        let mut editor = ChemStructEditor::default();
        let mut panel = egui::Rect::NOTHING;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(800.0, 1200.0),
                )),
                ..Default::default()
            },
            |ui| {
                panel = egui::Panel::left("test_palette")
                    .exact_size(PALETTE_WIDTH)
                    .frame(egui::Frame::new().inner_margin(PALETTE_MARGIN))
                    .show(ui, |ui| {
                        palette(ui, &mut editor, &mut String::new(), &mut Vec::new())
                    })
                    .response
                    .rect;
            },
        );
        output.textures_delta.clear();
        assert!((panel.width() - PALETTE_WIDTH).abs() < 1.0);
        let mut cells = Vec::new();
        let mut targets = Vec::new();
        for clipped in &output.shapes {
            let bounds = clipped.shape.visual_bounding_rect();
            if let egui::epaint::Shape::Rect(rect) = &clipped.shape
                && (rect.rect.height() - CELL_HEIGHT).abs() < 1.0
            {
                cells.push(rect.rect);
                targets.push(rect.rect);
            }
            if let egui::epaint::Shape::Rect(rect) = &clipped.shape
                && (rect.rect.height() - BUTTON_HEIGHT).abs() < 1.0
            {
                targets.push(rect.rect);
            }
            if bounds.is_positive() {
                assert!(
                    panel.expand(1.0).contains_rect(bounds),
                    "shape outside panel: {bounds:?}"
                );
            }
        }
        assert_eq!(cells.len(), BUTTONS.len() + ELEMENTS.len());
        assert_eq!(targets.len(), cells.len() + 1);
        for cell in &cells {
            assert!(
                (cell.width() - (PALETTE_WIDTH - 2.0 * f32::from(PALETTE_MARGIN.left) - GAP) / 2.0)
                    .abs()
                    < 1.0
            );
        }
        // All glyphs and button labels fit their full-cell interaction targets.
        for clipped in &output.shapes {
            let bounds = clipped.shape.visual_bounding_rect();
            if targets.iter().any(|cell| cell.intersects(bounds)) {
                assert!(
                    targets
                        .iter()
                        .any(|cell| cell.expand(0.5).contains_rect(bounds)),
                    "content outside cell: {bounds:?}"
                );
            }
        }
    }

    #[test]
    fn palette_dimensions_are_two_thirds() {
        assert!((PALETTE_WIDTH - 112.0 * 2.0 / 3.0).abs() < 0.001);
        assert!((CELL_HEIGHT - 44.0 * 2.0 / 3.0).abs() < 0.001);
        assert!((BUTTON_HEIGHT - 34.0 * 2.0 / 3.0).abs() < 0.001);
    }

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
        assert_eq!(tool_name(&editor), "Triple bond");

        apply(&mut editor, Action::Bond(BondKind::Hash));
        assert_eq!(editor.current_bond_order, BondOrder::Single);
        assert_eq!(editor.current_bond_stereo, BondStereo::WedgeDown);
        assert_eq!(tool_name(&editor), "Hashed wedge");
    }

    #[test]
    fn the_arrow_and_text_buttons_select_their_tools() {
        let mut editor = ChemStructEditor::default();
        select_arrow_tool(&mut editor, ArrowKind::Equilibrium);
        assert_eq!(editor.tool, Tool::Arrow(ArrowKind::Equilibrium));
        assert_eq!(tool_name(&editor), "Equilibrium arrow");
        select_arrow_tool(&mut editor, ArrowKind::Reaction);
        assert_eq!(tool_name(&editor), "Reaction arrow");
        select_text_tool(&mut editor);
        assert_eq!(editor.tool, Tool::Text);
        assert_eq!(tool_name(&editor), "Text");
        let planned: Vec<&str> = BUTTONS
            .iter()
            .filter(|(_, a)| *a == Action::Planned)
            .map(|(icon, _)| button_name(*icon))
            .collect();
        assert_eq!(
            planned,
            ["Chain", "Benzene ring", "Cyclopentane", "Cyclohexane"]
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
        assert!(bracket_selection(&mut editor, BracketUsage::Sru).starts_with("Select the atoms"));
        polyethylene(&mut editor);
        let message = bracket_selection(&mut editor, BracketUsage::Sru);
        assert!(
            message.starts_with("Added repeat-unit brackets"),
            "{message}"
        );
        assert_eq!(editor.tool, Tool::Select);
        assert_eq!(editor.molecule.brackets().count(), 1);
        let message = bracket_selection(&mut editor, BracketUsage::MultipleGroup);
        assert!(message.contains("Multiple group"), "{message}");
        let b = editor.molecule.brackets().next().unwrap();
        assert_eq!(
            (b.usage, b.label.as_str()),
            (BracketUsage::MultipleGroup, "2")
        );
        assert_eq!(editor.molecule.brackets().count(), 1, "changed, not added");
        assert!(bracket_shape(&mut editor, BracketShape::Round).contains("Parentheses"));
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
        assert!(message.contains("1 bond crosses"), "{message}");
    }

    /// 「反応表を挿入」 puts a table under the reaction and leaves the select tool on, so a
    /// double-click edits its cells; with nothing drawn it says why nothing happened.
    #[test]
    fn inserting_a_reaction_table() {
        let mut editor = ChemStructEditor::default();
        assert!(
            insert_reaction_table(&mut editor).starts_with("No structures for a reaction table")
        );
        assert!(editor.molecule.objects.is_empty());
        let m = &mut editor.molecule;
        let a = m.add_atom("C".into(), [0.0, 0.0], 0);
        let b = m.add_atom("O".into(), [1.3, 0.0], 0);
        m.add_bond(a, b, BondOrder::Single);
        m.add_arrow([2.5, 0.0], [5.5, 0.0], ArrowKind::Reaction);
        m.add_atom("C".into(), [6.5, 0.0], 0);
        editor.tool = Tool::Bond;
        assert!(insert_reaction_table(&mut editor).starts_with("Inserted a reaction table"));
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
        for (icon, action) in BUTTONS {
            if matches!(action, Action::Planned | Action::BracketMenu) {
                continue;
            }
            let mut editor = ChemStructEditor::default();
            apply(&mut editor, action);
            let selected = BUTTONS
                .iter()
                .filter(|(_, a)| is_selected(&editor, *a))
                .count();
            assert_eq!(selected, 1, "{}", button_name(icon));
        }
    }
}
