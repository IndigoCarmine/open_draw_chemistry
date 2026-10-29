//! The text format controls (書式): font, size, bold/italic, sub/superscript and ChemDraw's
//! Symbol font for page texts (reaction conditions, captions).
//!
//! They sit in the toolbar's format section ([`super::format_bar`]), which reports each change
//! as a [`Command::TextFormat`](super::Command::TextFormat) for [`apply`] to carry out. What a
//! change applies to is the editor's business (`chembuider_rs::widget::text_format`): the
//! selected characters of the open text box (or its whole text), else the selected texts, and
//! in any case new texts. The controls show the format of that text ([`TextSummary`]), mixed
//! where it differs.

use chembuider_rs::widget::fonts::{self, FontName};
use chembuider_rs::widget::text_format::{FormatChange, FormatTarget, text_range};
use chembuider_rs::{ChemStructEditor, RichText, Script, SpanStyle};
use serde::{Deserialize, Serialize};

use super::format_bar::Value;

/// Sizes offered, in points (ChemDraw's text is 10 pt).
const SIZES: [f32; 10] = [8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 24.0, 36.0];
/// How many recently used fonts are remembered.
const RECENT_MAX: usize = 8;
/// The sizes that can be typed, in points.
const SIZE_RANGE: std::ops::RangeInclusive<f32> = 1.0..=999.0;

/// The controls' state: the recently used fonts (saved with the app), the font list's search
/// and the size being typed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextFormatBar {
    /// Newest first.
    pub recent: Vec<String>,
    #[serde(skip)]
    search: String,
    #[serde(skip)]
    size_input: String,
}

impl TextFormatBar {
    /// Put `name` first among the recent fonts.
    pub fn remember(&mut self, name: &str) {
        self.recent.retain(|n| !n.eq_ignore_ascii_case(name));
        self.recent.insert(0, name.to_owned());
        self.recent.truncate(RECENT_MAX);
    }
}

/// The format of the text a change would apply to, each value over all of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextSummary {
    pub target: FormatTarget,
    /// The font of the text not in Symbol (Symbol only when all of it is).
    pub font: Value<Option<FontName>>,
    pub size: Value<Option<f32>>,
    pub bold: Value<bool>,
    pub italic: Value<bool>,
    /// In subscript, and in superscript: each on its own, so text partly in one is mixed for
    /// that one only.
    pub sub: Value<bool>,
    pub sup: Value<bool>,
    /// In Symbol; for the open text box with nothing selected, whether Symbol typing is on.
    pub symbol: Value<bool>,
}

impl TextSummary {
    pub fn of(editor: &ChemStructEditor) -> Self {
        let format = editor.text_format();
        let mut styles = styles(editor);
        if styles.is_empty() {
            styles.push(format.style);
        }
        let font = Value::of(styles.iter().filter(|s| !s.is_symbol()).map(|s| s.font))
            .unwrap_or(Value::Same(Some(FontName::symbol())));
        let symbol = match format.target {
            FormatTarget::Editing { selection: false } => Value::Same(format.symbol_typing),
            _ => over(&styles, |s| s.is_symbol()),
        };
        Self {
            target: format.target,
            font,
            size: over(&styles, |s| s.size),
            bold: over(&styles, |s| s.bold),
            italic: over(&styles, |s| s.italic),
            sub: over(&styles, |s| s.script == Script::Sub),
            sup: over(&styles, |s| s.script == Script::Super),
            symbol,
        }
    }
}

/// What `styles` (one at least) share of `f`.
fn over<T: PartialEq>(styles: &[SpanStyle], f: impl Fn(&SpanStyle) -> T) -> Value<T> {
    Value::of(styles.iter().map(f)).expect("one style at least")
}

/// The styles of all the text a change would apply to: the characters selected in the open
/// text box, or every run of the selected texts. Empty when that is the one style the editor
/// reports (the text box with nothing selected, or new texts).
fn styles(editor: &ChemStructEditor) -> Vec<SpanStyle> {
    if let Some(edit) = &editor.text_edit {
        let (rich, map) = RichText::parse_mapped(&edit.text);
        let Some(range) = edit
            .selection
            .clone()
            .map(|s| text_range(&map, s))
            .filter(|s| !s.is_empty())
        else {
            return Vec::new();
        };
        let mut styles = rich.styles_in(range);
        // A new text takes what it does not set itself from the style of new texts.
        if edit.id.is_none() {
            let base = &editor.new_text_style;
            for style in &mut styles {
                style.font = style.font.or(base.font);
                style.size = style.size.or(base.size);
                style.bold |= base.bold;
                style.italic |= base.italic;
            }
        }
        return styles;
    }
    editor
        .molecule
        .objects
        .iter()
        .filter(|o| editor.selected_objects.contains(&o.id()))
        .filter_map(|o| o.as_text())
        .flat_map(|t| t.lines())
        .flat_map(|line| line.spans.into_iter().map(|s| s.style))
        .collect()
}

/// Apply `change` to what the user is working on, remembering a chosen font. Returns whether
/// the molecule changed (formatting selected texts).
pub fn apply(editor: &mut ChemStructEditor, bar: &mut TextFormatBar, change: FormatChange) -> bool {
    if let FormatChange::Font(Some(font)) = change {
        bar.remember(font.as_str());
    }
    editor.apply_text_format(change)
}

pub fn target_label(target: FormatTarget) -> String {
    match target {
        FormatTarget::NewText => t!("text_format.target.new_text").to_owned(),
        FormatTarget::Editing { selection: true } => t!("text_format.target.selection").to_owned(),
        FormatTarget::Editing { selection: false } => {
            t!("text_format.target.whole_text").to_owned()
        }
        FormatTarget::Selected(n) => tn!("text_format.target.selected_texts", n),
    }
}

/// A control's tooltip, followed by what it applies to.
fn tip(text: &str, target: FormatTarget) -> String {
    let target = target_label(target);
    format!("{text}\n{}", t!("format_bar.applies_to", target = target))
}

fn size_label(size: Option<f32>) -> String {
    match size {
        Some(s) => format!("{s} pt"),
        None => t!("text_format.size_default").to_owned(),
    }
}

/// The size shown for `size`.
pub fn size_text(size: Value<Option<f32>>) -> String {
    match size {
        Value::Same(s) => size_label(s),
        Value::Mixed => t!("format_bar.mixed").to_owned(),
    }
}

/// A size typed in: points, with or without "pt", within [`SIZE_RANGE`], to a tenth.
pub fn parse_size(text: &str) -> Option<f32> {
    let text = text.trim();
    let number = text
        .strip_suffix("pt")
        .or_else(|| text.strip_suffix("PT"))
        .or_else(|| text.strip_suffix("Pt"))
        .unwrap_or(text)
        .trim()
        .replace(',', ".");
    let size: f32 = number.parse().ok()?;
    let size = (size * 10.0).round() / 10.0;
    (size.is_finite() && SIZE_RANGE.contains(&size)).then_some(size)
}

/// The font shown for `font`.
pub fn font_text(font: Value<Option<FontName>>) -> String {
    match font {
        Value::Same(None) => t!("text_format.font.default").to_owned(),
        Value::Same(Some(f)) if !fonts::is_installed(f.as_str()) => {
            t!("text_format.font.not_installed", font = f.to_string())
        }
        Value::Same(Some(f)) => f.to_string(),
        Value::Mixed => t!("format_bar.mixed").to_owned(),
    }
}

/// The size list: a box to type a size into (Enter applies it), then the usual sizes; in the
/// toolbar a drop-down, in a menu (`in_menu`) a submenu.
pub fn size_menu(
    ui: &mut egui::Ui,
    in_menu: bool,
    summary: &TextSummary,
    bar: &mut TextFormatBar,
    changes: &mut Vec<FormatChange>,
) {
    let list = |ui: &mut egui::Ui| size_list(ui, summary.size, bar, changes);
    let shown = size_text(summary.size);
    if in_menu {
        let label = t!("format_bar.size", size = shown);
        super::appearance::submenu(ui, &label, list);
        return;
    }
    let combo = egui::ComboBox::from_id_salt("odc_text_size")
        .width(64.0)
        // Open while its box is clicked into; a size picked closes it.
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .selected_text(shown)
        .show_ui(ui, list);
    combo
        .response
        .on_hover_text(tip(t!("text_format.size_tip"), summary.target));
}

fn size_list(
    ui: &mut egui::Ui,
    current: Value<Option<f32>>,
    bar: &mut TextFormatBar,
    changes: &mut Vec<FormatChange>,
) {
    let input = ui.add(
        egui::TextEdit::singleline(&mut bar.size_input)
            .hint_text(t!("format_bar.size_input"))
            .desired_width(80.0),
    );
    if input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        if let Some(size) = parse_size(&bar.size_input) {
            changes.push(FormatChange::Size(Some(size)));
            ui.close();
        }
        bar.size_input.clear();
    }
    ui.separator();
    for size in std::iter::once(None).chain(SIZES.iter().map(|s| Some(*s))) {
        if ui
            .selectable_label(current == Value::Same(size), size_label(size))
            .clicked()
        {
            changes.push(FormatChange::Size(size));
            ui.close();
        }
    }
}

/// A toggle for `value`: pressed when all of the text has it, half-lit when some has, and
/// named (for screen readers and the tests) `name`, with its state. Returns whether it was
/// clicked.
fn toggle(ui: &mut egui::Ui, value: Value<bool>, text: egui::RichText, name: &str) -> bool {
    let on = value == Value::Same(true);
    let mut button = egui::Button::selectable(on, text).min_size(egui::vec2(24.0, 0.0));
    if value == Value::Mixed {
        button = button.fill(ui.visuals().selection.bg_fill.gamma_multiply(0.4));
    }
    let response = ui.add(button);
    let enabled = ui.is_enabled();
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::new(egui::Role::CheckBox);
        info.enabled = enabled;
        info.label = Some(name.to_owned());
        // No state is the mixed one.
        info.selected = match value {
            Value::Same(b) => Some(b),
            Value::Mixed => None,
        };
        info
    });
    let hover = match value {
        Value::Mixed => format!("{name} — {}", t!("format_bar.mixed")),
        Value::Same(_) => name.to_owned(),
    };
    response.on_hover_text(hover).clicked()
}

/// Bold, italic, sub- and superscript, and Symbol.
pub fn style_toggles(ui: &mut egui::Ui, summary: &TextSummary, changes: &mut Vec<FormatChange>) {
    if toggle(
        ui,
        summary.bold,
        egui::RichText::new("B").strong(),
        t!("text_format.bold"),
    ) {
        changes.push(FormatChange::Bold(summary.bold != Value::Same(true)));
    }
    if toggle(
        ui,
        summary.italic,
        egui::RichText::new("I").italics(),
        t!("text_format.italic"),
    ) {
        changes.push(FormatChange::Italic(summary.italic != Value::Same(true)));
    }
    for (script, value, text, name) in [
        (Script::Sub, summary.sub, "x₂", t!("text_format.subscript")),
        (
            Script::Super,
            summary.sup,
            "x²",
            t!("text_format.superscript"),
        ),
    ] {
        if toggle(ui, value, egui::RichText::new(text), name) {
            let on = value == Value::Same(true);
            changes.push(FormatChange::Script(if on {
                Script::Normal
            } else {
                script
            }));
        }
    }
    if toggle(
        ui,
        summary.symbol,
        egui::RichText::new("Σ"),
        t!("text_format.symbol_tip"),
    ) {
        changes.push(FormatChange::Symbol(summary.symbol != Value::Same(true)));
    }
}

/// The font list: a search box, the standard font, the recent fonts, then every installed one;
/// in the toolbar a drop-down, in a menu (`in_menu`) a submenu.
pub fn font_menu(
    ui: &mut egui::Ui,
    in_menu: bool,
    summary: &TextSummary,
    bar: &mut TextFormatBar,
    changes: &mut Vec<FormatChange>,
) {
    let list = |ui: &mut egui::Ui| font_list(ui, summary.font, bar, changes);
    let shown = font_text(summary.font);
    if in_menu {
        let label = t!("format_bar.font", font = shown);
        super::appearance::submenu(ui, &label, list);
        return;
    }
    let combo = egui::ComboBox::from_id_salt("odc_text_font")
        .width(150.0)
        .height(360.0)
        // Open while its search box is clicked into or its list scrolled; a font picked
        // closes it.
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .selected_text(shown)
        .show_ui(ui, list);
    combo
        .response
        .on_hover_text(tip(t!("text_format.font.tip"), summary.target));
}

fn font_list(
    ui: &mut egui::Ui,
    font: Value<Option<FontName>>,
    bar: &mut TextFormatBar,
    changes: &mut Vec<FormatChange>,
) {
    let current = match font {
        Value::Same(f) => Some(f),
        Value::Mixed => None,
    };
    let before = changes.len();
    let search = ui.add(
        egui::TextEdit::singleline(&mut bar.search)
            .hint_text(t!("text_format.font.search"))
            .desired_width(160.0),
    );
    if !search.has_focus() && !ui.ctx().memory(|m| m.focused().is_some()) {
        search.request_focus();
    }
    let query = bar.search.to_lowercase();
    let matches = |name: &str| query.is_empty() || name.to_lowercase().contains(&query);
    let mut pick = |ui: &mut egui::Ui, font: Option<&str>, label: &str| {
        let selected = current.is_some_and(|c| match (c, font) {
            (None, None) => true,
            (Some(c), Some(f)) => c.as_str().eq_ignore_ascii_case(f),
            _ => false,
        });
        if ui.selectable_label(selected, label).clicked() {
            changes.push(FormatChange::Font(font.map(FontName::new)));
            ui.close();
        }
    };
    if query.is_empty() {
        pick(ui, None, t!("text_format.font.default"));
    }
    let recent: Vec<String> = bar.recent.iter().filter(|n| matches(n)).cloned().collect();
    if !recent.is_empty() {
        ui.separator();
        ui.weak(t!("text_format.font.recent"));
        for name in &recent {
            pick(ui, Some(name), name);
        }
    }
    ui.separator();
    egui::ScrollArea::vertical()
        .max_height(280.0)
        .show(ui, |ui| {
            let all = fonts::system_families();
            let mut any = false;
            for name in all.iter().filter(|n| matches(n)) {
                any = true;
                let label = if name.eq_ignore_ascii_case(fonts::SYMBOL) {
                    t!("text_format.font.symbol")
                } else {
                    name
                };
                pick(ui, Some(name), label);
            }
            if !any {
                ui.weak(t!("text_format.font.no_match"));
            }
        });
    // A font was picked: the next search starts afresh.
    if changes.len() > before {
        bar.search.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_fonts_are_newest_first_without_repeats_and_capped() {
        let mut bar = TextFormatBar::default();
        for name in ["Arial", "Georgia", "arial"] {
            bar.remember(name);
        }
        assert_eq!(bar.recent, ["arial", "Georgia"]);
        for k in 0..20 {
            bar.remember(&format!("Font {k}"));
        }
        assert_eq!(bar.recent.len(), RECENT_MAX);
        assert_eq!(bar.recent[0], "Font 19");
        // Saved with the app, the search box and the size being typed not.
        bar.search = "x".into();
        bar.size_input = "12".into();
        let back: TextFormatBar =
            serde_json::from_str(&serde_json::to_string(&bar).unwrap()).unwrap();
        assert_eq!(back.recent, bar.recent);
        assert!(back.search.is_empty());
        assert!(back.size_input.is_empty());
    }

    /// A font chosen for the selected text is applied and remembered.
    #[test]
    fn choosing_a_font_formats_the_selected_text() {
        let mut editor = ChemStructEditor::default();
        let id = editor.molecule.add_text([0.0, 0.0], "THF");
        editor.selected_objects.insert(id);
        let mut bar = TextFormatBar::default();
        let font = FontName::new("Times New Roman");
        assert!(apply(&mut editor, &mut bar, FormatChange::Font(Some(font))));
        let text = editor.molecule.objects[0].as_text().unwrap();
        assert_eq!(text.text, "{@Times New Roman:THF}");
        assert_eq!(bar.recent, ["Times New Roman"]);
    }

    #[test]
    fn a_typed_size_is_points_within_range() {
        assert_eq!(parse_size("12"), Some(12.0));
        assert_eq!(parse_size(" 13.5 pt "), Some(13.5));
        assert_eq!(parse_size("7,25pt"), Some(7.3));
        assert_eq!(parse_size("0"), None);
        assert_eq!(parse_size("1000"), None);
        assert_eq!(parse_size("big"), None);
        assert_eq!(parse_size("NaN"), None);
    }

    /// The summary is of the text a change applies to: new texts with nothing selected, every
    /// run of the selected texts (mixed where they differ), the characters selected in the
    /// open text box.
    #[test]
    fn the_summary_is_mixed_where_the_text_differs() {
        let mut editor = ChemStructEditor::default();
        editor.new_text_style.bold = true;
        let summary = TextSummary::of(&editor);
        assert_eq!(summary.target, FormatTarget::NewText);
        assert_eq!(summary.bold, Value::Same(true));
        assert_eq!(summary.size, Value::Same(None));

        let a = editor.molecule.add_text([0.0, 0.0], "**THF**");
        let b = editor
            .molecule
            .add_text([0.0, 2.0], "{@Arial,14:Et}{@Symbol:a}");
        editor.selected_objects = [a].into();
        let summary = TextSummary::of(&editor);
        assert_eq!(summary.target, FormatTarget::Selected(1));
        assert_eq!(summary.bold, Value::Same(true));
        assert_eq!(summary.font, Value::Same(None));
        assert_eq!(summary.symbol, Value::Same(false));

        editor.selected_objects = [a, b].into();
        let summary = TextSummary::of(&editor);
        assert_eq!(summary.bold, Value::Mixed);
        assert_eq!(summary.size, Value::Mixed);
        assert_eq!(summary.font, Value::Mixed);
        assert_eq!(summary.symbol, Value::Mixed);
        assert_eq!(summary.italic, Value::Same(false));

        // Symbol runs leave the font alone: b alone is in Arial.
        editor.selected_objects = [b].into();
        let summary = TextSummary::of(&editor);
        assert_eq!(summary.font, Value::Same(Some(FontName::new("Arial"))));

        // Sub- and superscript each on its own: H₂O is partly in subscript and in no
        // superscript, CO₃²⁻ partly in each.
        let water = editor.molecule.add_text([0.0, 4.0], "H_2O");
        editor.selected_objects = [water].into();
        let summary = TextSummary::of(&editor);
        assert_eq!(summary.sub, Value::Mixed);
        assert_eq!(summary.sup, Value::Same(false));
        let ion = editor.molecule.add_text([0.0, 6.0], "CO_3^{2-}");
        editor.selected_objects = [ion].into();
        let summary = TextSummary::of(&editor);
        assert_eq!(summary.sub, Value::Mixed);
        assert_eq!(summary.sup, Value::Mixed);
        let squared = editor.molecule.add_text([0.0, 8.0], "^2");
        editor.selected_objects = [squared].into();
        let summary = TextSummary::of(&editor);
        assert_eq!(summary.sub, Value::Same(false));
        assert_eq!(summary.sup, Value::Same(true));
    }
}
