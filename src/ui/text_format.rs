//! The text format bar (書式): font, size, bold/italic, sub/superscript and ChemDraw's Symbol
//! font for page texts (reaction conditions, captions).
//!
//! It floats over the top-left corner of the canvas while it has something to act on: the
//! テキスト tool is active, a text box is open, or page texts are selected. Floating rather than
//! taking a row keeps the drawing where it is when the bar comes and goes, and keeps it next
//! to the text being edited; the right panel stays 解析. What a change applies to is the
//! editor's business (`chembuider_rs::widget::text_format`): the selected characters of the
//! open text box (or its whole text), else the selected texts, and in any case new texts.

use chembuider_rs::widget::fonts::{self, FontName};
use chembuider_rs::widget::text_format::{FormatChange, FormatTarget};
use chembuider_rs::{ChemStructEditor, Script, Tool};
use serde::{Deserialize, Serialize};

/// Sizes offered, in points (ChemDraw's text is 10 pt).
const SIZES: [f32; 10] = [8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 24.0, 36.0];
/// How many recently used fonts are remembered.
const RECENT_MAX: usize = 8;

/// The bar's state: the recently used fonts (saved with the app) and the font list's search.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextFormatBar {
    /// Newest first.
    pub recent: Vec<String>,
    #[serde(skip)]
    search: String,
}

impl TextFormatBar {
    /// Put `name` first among the recent fonts.
    pub fn remember(&mut self, name: &str) {
        self.recent.retain(|n| !n.eq_ignore_ascii_case(name));
        self.recent.insert(0, name.to_owned());
        self.recent.truncate(RECENT_MAX);
    }
}

/// Whether the bar has anything to act on.
pub fn wanted(editor: &ChemStructEditor) -> bool {
    editor.tool == Tool::Text
        || editor.text_edit.is_some()
        || editor
            .molecule
            .objects
            .iter()
            .any(|o| o.as_text().is_some() && editor.selected_objects.contains(&o.id()))
}

fn target_label(target: FormatTarget) -> String {
    match target {
        FormatTarget::NewText => "新しいテキスト".to_owned(),
        FormatTarget::Editing { selection: true } => "選択した文字".to_owned(),
        FormatTarget::Editing { selection: false } => "テキスト全体".to_owned(),
        FormatTarget::Selected(n) => format!("選択中のテキスト {n} 件"),
    }
}

fn size_label(size: Option<f32>) -> String {
    match size {
        Some(s) => format!("{s} pt"),
        None => "標準".to_owned(),
    }
}

/// Show the bar over the canvas `canvas` when it is wanted, applying what is chosen to
/// `editor`. Returns whether the molecule changed (formatting selected texts).
pub fn show(
    ui: &egui::Ui,
    canvas: egui::Rect,
    editor: &mut ChemStructEditor,
    bar: &mut TextFormatBar,
) -> bool {
    if !wanted(editor) {
        return false;
    }
    let format = editor.text_format();
    let style = format.style;
    let mut changes: Vec<FormatChange> = Vec::new();
    let mut popup_rects: Vec<egui::Rect> = Vec::new();
    let area = egui::Area::new(egui::Id::unique("odc_text_format_bar"))
        .order(egui::Order::Foreground)
        .fixed_pos(canvas.left_top() + egui::vec2(8.0, 8.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("書式").strong());
                    ui.weak(target_label(format.target));
                    ui.separator();
                    font_menu(ui, style.font, bar, &mut changes, &mut popup_rects);
                    let size = egui::ComboBox::from_id_salt("odc_text_size")
                        .width(64.0)
                        .selected_text(size_label(style.size))
                        .show_ui(ui, |ui| {
                            popup_rects.push(ui.clip_rect());
                            for size in std::iter::once(None).chain(SIZES.iter().map(|s| Some(*s))) {
                                if ui.selectable_label(style.size == size, size_label(size)).clicked() {
                                    changes.push(FormatChange::Size(size));
                                }
                            }
                        });
                    size.response.on_hover_text("文字の大きさ（標準は 10 pt）");
                    ui.separator();
                    if ui
                        .selectable_label(style.bold, egui::RichText::new("B").strong())
                        .on_hover_text("太字")
                        .clicked()
                    {
                        changes.push(FormatChange::Bold(!style.bold));
                    }
                    if ui
                        .selectable_label(style.italic, egui::RichText::new("I").italics())
                        .on_hover_text("斜体")
                        .clicked()
                    {
                        changes.push(FormatChange::Italic(!style.italic));
                    }
                    let sub = style.script == Script::Sub;
                    if ui.selectable_label(sub, "x₂").on_hover_text("下付き").clicked() {
                        changes.push(FormatChange::Script(if sub { Script::Normal } else { Script::Sub }));
                    }
                    let sup = style.script == Script::Super;
                    if ui.selectable_label(sup, "x²").on_hover_text("上付き").clicked() {
                        changes.push(FormatChange::Script(if sup { Script::Normal } else { Script::Super }));
                    }
                    ui.separator();
                    let symbol_on = match format.target {
                        FormatTarget::Editing { selection: false } => format.symbol_typing,
                        _ => style.is_symbol(),
                    };
                    if ui
                        .selectable_label(symbol_on, "Σ Symbol")
                        .on_hover_text(
                            "Symbol フォント（ChemDraw のギリシャ文字）：p → π、D → Δ、m → μ\n\
                             文字を選択していれば選択に、なければこれから入力する文字に適用（Ctrl+G）",
                        )
                        .clicked()
                    {
                        changes.push(FormatChange::Symbol(!symbol_on));
                    }
                });
            });
        });
    editor.keep_text_edit_open_over(area.response.rect);
    for rect in popup_rects {
        editor.keep_text_edit_open_over(rect);
    }
    let mut modified = false;
    for change in changes {
        if let FormatChange::Font(Some(font)) = change {
            bar.remember(font.as_str());
        }
        modified |= editor.apply_text_format(change);
    }
    modified
}

/// The font list: a search box, the standard font, the recent fonts, then every installed one.
fn font_menu(
    ui: &mut egui::Ui,
    current: Option<FontName>,
    bar: &mut TextFormatBar,
    changes: &mut Vec<FormatChange>,
    popup_rects: &mut Vec<egui::Rect>,
) {
    let shown = current.map_or_else(|| "標準フォント".to_owned(), |f| f.to_string());
    let missing = current.is_some_and(|f| !fonts::is_installed(f.as_str()));
    let combo = egui::ComboBox::from_id_salt("odc_text_font")
        .width(170.0)
        .height(360.0)
        .selected_text(if missing {
            format!("{shown}（未インストール）")
        } else {
            shown
        })
        .show_ui(ui, |ui| {
            popup_rects.push(ui.clip_rect());
            let search = ui.add(
                egui::TextEdit::singleline(&mut bar.search)
                    .hint_text("フォントを検索")
                    .desired_width(160.0),
            );
            if !search.has_focus() && !ui.ctx().memory(|m| m.focused().is_some()) {
                search.request_focus();
            }
            let query = bar.search.to_lowercase();
            let matches = |name: &str| query.is_empty() || name.to_lowercase().contains(&query);
            let mut pick = |ui: &mut egui::Ui, font: Option<&str>, label: &str| {
                let selected = current.map(|f| f.as_str()) == font
                    || current
                        .is_some_and(|c| font.is_some_and(|f| c.as_str().eq_ignore_ascii_case(f)));
                if ui.selectable_label(selected, label).clicked() {
                    changes.push(FormatChange::Font(font.map(FontName::new)));
                }
            };
            if query.is_empty() {
                pick(ui, None, "標準フォント");
            }
            let recent: Vec<String> = bar.recent.iter().filter(|n| matches(n)).cloned().collect();
            if !recent.is_empty() {
                ui.separator();
                ui.weak("最近使ったフォント");
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
                            "Symbol（ギリシャ文字）"
                        } else {
                            name
                        };
                        pick(ui, Some(name), label);
                    }
                    if !any {
                        ui.weak("一致するフォントはありません");
                    }
                });
        });
    combo
        .response
        .on_hover_text("フォント（インストールされていないフォント名も保存時はそのまま残ります）");
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
        // Saved with the app, the search box not.
        bar.search = "x".into();
        let back: TextFormatBar =
            serde_json::from_str(&serde_json::to_string(&bar).unwrap()).unwrap();
        assert_eq!(back.recent, bar.recent);
        assert!(back.search.is_empty());
    }

    #[test]
    fn the_bar_shows_for_the_text_tool_an_open_box_or_selected_texts() {
        let mut editor = ChemStructEditor::default();
        assert!(!wanted(&editor));
        editor.tool = Tool::Text;
        assert!(wanted(&editor));
        editor.tool = Tool::Select;
        let id = editor.molecule.add_text([0.0, 0.0], "THF");
        editor.selected_objects.insert(id);
        assert!(wanted(&editor));
    }

    /// The bar acts on the editor: a font chosen for the selected text is applied and
    /// remembered.
    #[test]
    fn choosing_a_font_formats_the_selected_text() {
        let mut editor = ChemStructEditor::default();
        let id = editor.molecule.add_text([0.0, 0.0], "THF");
        editor.selected_objects.insert(id);
        let mut bar = TextFormatBar::default();
        let font = FontName::new("Times New Roman");
        bar.remember(font.as_str());
        assert!(editor.apply_text_format(FormatChange::Font(Some(font))));
        let text = editor.molecule.objects[0].as_text().unwrap();
        assert_eq!(text.text, "{@Times New Roman:THF}");
        assert_eq!(bar.recent, ["Times New Roman"]);
    }
}
