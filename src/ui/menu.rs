//! The menu bar and the toolbar under it.

use std::path::PathBuf;

use egui::vec2;

use super::icons::{self, Frame, Icon};
use super::{Command, ViewMode};
use crate::i18n::Lang;
use crate::io::Export;
use crate::theme;
use crate::three_d::Export3d;

/// What the menus need to know about the app to grey out or tick their items.
pub struct MenuState<'a> {
    pub has_document: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub cleaning: bool,
    pub show_properties: bool,
    pub view: ViewMode,
    pub recent: &'a [PathBuf],
    /// A 3D model can be generated now: there is a document, OpenBabel is built in and no 3D
    /// work is running.
    pub can_generate_3d: bool,
    /// There is a 3D model to optimise or export (optimising also needs no work running).
    pub can_optimize_3d: bool,
    pub can_export_3d: bool,
    /// There is a document whose assembly can be started or shown.
    pub can_create_assembly: bool,
    /// The UI language, ticked in the language menu.
    pub language: Lang,
}

/// The languages to choose from, each by its own name, the current one ticked: the 表示 menu's
/// submenu and the toolbar's globe button both show this list.
pub fn language_list(ui: &mut egui::Ui, current: Lang, out: &mut Vec<Command>) {
    for lang in Lang::ALL {
        if ui
            .radio(lang == current, lang.native_name())
            .on_hover_text(lang.code())
            .clicked()
        {
            out.push(Command::SetLanguage(lang));
            ui.close();
        }
    }
}

/// The toolbar's globe button, which opens [`language_list`]: an icon rather than a word, so
/// it can be found whatever language the UI is in.
pub fn language_button(ui: &mut egui::Ui, current: Lang, out: &mut Vec<Command>) {
    let tip = format!("{} — {}", t!("language.tooltip"), current.native_name());
    let response = icons::icon_button(
        ui,
        Icon::Globe,
        vec2(32.0, 32.0),
        Frame::Flat,
        false,
        true,
        &tip,
    );
    egui::Popup::menu(&response)
        .id(egui::Id::unique("odc_language_popup"))
        .show(|ui| language_list(ui, current, out));
}

pub fn menu_bar(ui: &mut egui::Ui, state: &MenuState<'_>, out: &mut Vec<Command>) {
    egui::MenuBar::new().ui(ui, |ui| {
        icons::brand(ui);
        ui.add_space(10.0);

        ui.menu_button("ファイル", |ui| {
            item(ui, "新規", "Ctrl+N", true, Command::New, out);
            item(ui, "開く…", "Ctrl+O", true, Command::Open, out);
            ui.add_enabled_ui(!state.recent.is_empty(), |ui| {
                ui.menu_button("最近使ったファイル", |ui| {
                    for path in state.recent {
                        if ui.button(path.display().to_string()).clicked() {
                            out.push(Command::OpenPath(path.clone()));
                            ui.close();
                        }
                    }
                });
            });
            ui.separator();
            item(ui, "保存", "Ctrl+S", state.has_document, Command::Save, out);
            item(
                ui,
                "名前を付けて保存…",
                "Ctrl+Shift+S",
                state.has_document,
                Command::SaveAs,
                out,
            );
            ui.add_enabled_ui(state.has_document, |ui| {
                ui.menu_button("エクスポート", |ui| {
                    for &export in Export::all() {
                        let label = format!("{} (.{})…", export.description(), export.extension());
                        if ui.button(label).clicked() {
                            out.push(Command::Export(export));
                            ui.close();
                        }
                    }
                });
            });
            ui.separator();
            item(
                ui,
                "閉じる",
                "Ctrl+W",
                state.has_document,
                Command::CloseActive,
                out,
            );
            // Quitting from a menu is desktop-only; on web there is no window to close.
            if !cfg!(target_arch = "wasm32") {
                item(ui, "終了", "", true, Command::Quit, out);
            }
        });

        ui.menu_button("編集", |ui| {
            item(ui, "元に戻す", "Ctrl+Z", state.can_undo, Command::Undo, out);
            item(ui, "やり直し", "Ctrl+Y", state.can_redo, Command::Redo, out);
            ui.separator();
            item(
                ui,
                "コピー",
                "Ctrl+C",
                state.has_document,
                Command::Copy,
                out,
            );
            item(
                ui,
                "貼り付け",
                "Ctrl+V",
                state.has_document,
                Command::Paste,
                out,
            );
        });

        ui.menu_button("表示", |ui| {
            item(
                ui,
                "全体表示",
                "Ctrl+0",
                state.has_document,
                Command::FitView,
                out,
            );
            ui.separator();
            for view in ViewMode::ALL {
                if ui.radio(state.view == view, view.label()).clicked() {
                    out.push(Command::SetView(view));
                    ui.close();
                }
            }
            ui.separator();
            let mut show = state.show_properties;
            if ui.checkbox(&mut show, "右パネル").clicked() {
                out.push(Command::ToggleProperties);
                ui.close();
            }
            ui.separator();
            egui::widgets::global_theme_preference_buttons(ui);
            ui.separator();
            ui.menu_button(t!("language.menu"), |ui| {
                language_list(ui, state.language, out);
            });
        });

        ui.menu_button("構造", |ui| {
            let label = if state.cleaning {
                "クリーンアップを停止"
            } else {
                "クリーンアップ"
            };
            item(
                ui,
                label,
                "",
                state.has_document,
                Command::ToggleCleanup,
                out,
            );
            ui.separator();
            item(ui, "IUPAC 名をコピー", "", state.has_document, Command::CopyIupacName, out);
            item(
                ui,
                "IUPAC 名をテキストとして挿入",
                "",
                state.has_document,
                Command::InsertIupacName,
                out,
            );
            ui.separator();
            item(
                ui,
                "括弧で囲む（繰り返し単位）",
                "",
                state.has_document,
                Command::Bracket(chembuider_rs::molecule::BracketUsage::Sru),
                out,
            );
            ui.add_enabled_ui(state.has_document, |ui| {
                ui.menu_button("括弧の種類と形", |ui| {
                    if super::palette::bracket_menu(ui, out) {
                        ui.close();
                    }
                });
            });
        });

        ui.menu_button("反応", |ui| {
            use chembuider_rs::molecule::ArrowKind;
            for (label, command) in [
                ("反応矢印を追加", Command::ArrowTool(ArrowKind::Reaction)),
                ("平衡矢印を追加", Command::ArrowTool(ArrowKind::Equilibrium)),
                ("反応条件のテキスト", Command::TextTool),
            ] {
                item(ui, label, "", state.has_document, command, out);
            }
            ui.separator();
            item(
                ui,
                "反応表を挿入",
                "",
                state.has_document,
                Command::InsertReactionTable,
                out,
            );
        });
        ui.menu_button("3D", |ui| {
            item(
                ui,
                "2D から 3D を生成",
                "",
                state.can_generate_3d,
                Command::Generate3d,
                out,
            );
            item(
                ui,
                "構造最適化",
                "",
                state.can_optimize_3d,
                Command::Optimize3d,
                out,
            );
            ui.add_enabled_ui(state.can_export_3d, |ui| {
                ui.menu_button("3D をエクスポート", |ui| {
                    for export in Export3d::ALL {
                        let label = format!("{} (.{})…", export.description(), export.extension());
                        if ui.button(label).clicked() {
                            out.push(Command::Export3d(export));
                            ui.close();
                        }
                    }
                });
            });
            item(
                ui,
                "パラメータスキャン…",
                "",
                state.has_document,
                Command::OpenScan,
                out,
            );
            ui.separator();
            let button = egui::Button::new("分子集合体を作成");
            if ui
                .add_enabled(state.can_create_assembly, button)
                .on_hover_text(
                    "今の 3D モデルを単量体にして、環状・積層・らせんなどの集合体を組み立てます（集合体画面に切り替わります）",
                )
                .clicked()
            {
                out.push(Command::CreateAssembly);
                ui.close();
            }
        });
        ui.menu_button("ツール", |ui| {
            planned(ui, &["周期表"]);
            item(ui, "断片の管理", "", true, Command::ManageFragments, out);
        });

        ui.menu_button("ヘルプ", |ui| {
            item(ui, "バージョン情報", "", true, Command::About, out);
        });
    });
}

pub fn toolbar(ui: &mut egui::Ui, state: &MenuState<'_>, out: &mut Vec<Command>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let size = vec2(32.0, 32.0);
        let mut tool = |ui: &mut egui::Ui, icon, enabled, command, tip: &str| {
            if icons::icon_button(ui, icon, size, Frame::Flat, false, enabled, tip).clicked() {
                out.push(command);
            }
        };
        tool(ui, Icon::New, true, Command::New, "新規 (Ctrl+N)");
        tool(ui, Icon::Open, true, Command::Open, "開く (Ctrl+O)");
        tool(
            ui,
            Icon::Save,
            state.has_document,
            Command::Save,
            "保存 (Ctrl+S)",
        );
        ui.separator();
        tool(
            ui,
            Icon::Undo,
            state.can_undo,
            Command::Undo,
            "元に戻す (Ctrl+Z)",
        );
        tool(
            ui,
            Icon::Redo,
            state.can_redo,
            Command::Redo,
            "やり直し (Ctrl+Y)",
        );
        ui.separator();
        let text_button = |ui: &mut egui::Ui, text: &str, enabled: bool| {
            ui.add_enabled(enabled, egui::Button::new(text).min_size(vec2(0.0, 32.0)))
                .clicked()
        };
        if text_button(ui, "全体表示", state.has_document) {
            out.push(Command::FitView);
        }
        let cleanup = if state.cleaning {
            "クリーンアップを停止"
        } else {
            "クリーンアップ"
        };
        if text_button(ui, cleanup, state.has_document) {
            out.push(Command::ToggleCleanup);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            view_switch(ui, state.view, out);
            language_button(ui, state.language, out);
        });
    });
}

/// The 2D / 3D / 集合体 segmented control.
fn view_switch(ui: &mut egui::Ui, current: ViewMode, out: &mut Vec<Command>) {
    let border = ui.visuals().widgets.inactive.bg_stroke;
    egui::Frame::new()
        .stroke(border)
        .corner_radius(7.0)
        .fill(ui.visuals().widgets.inactive.weak_bg_fill)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            // Laid out right to left by the caller; reverse so 2D comes first on screen.
            for view in ViewMode::ALL.into_iter().rev() {
                let selected = view == current;
                let (fill, text) = if selected {
                    // White text needs the deep orange in both themes to reach 4.5:1.
                    (theme::ORANGE_DEEP, egui::Color32::WHITE)
                } else {
                    (
                        egui::Color32::TRANSPARENT,
                        ui.visuals().widgets.inactive.fg_stroke.color,
                    )
                };
                let button = egui::Button::new(egui::RichText::new(view.label()).color(text))
                    .fill(fill)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(0.0)
                    .min_size(vec2(52.0, 30.0));
                if ui.add(button).clicked() {
                    out.push(Command::SetView(view));
                }
            }
        });
}

fn item(
    ui: &mut egui::Ui,
    label: &str,
    shortcut: &str,
    enabled: bool,
    command: Command,
    out: &mut Vec<Command>,
) {
    let button = egui::Button::new(label).shortcut_text(shortcut);
    if ui.add_enabled(enabled, button).clicked() {
        out.push(command);
        ui.close();
    }
}

/// Greyed-out items for a feature that is not built yet.
fn planned(ui: &mut egui::Ui, labels: &[&str]) {
    for label in labels {
        ui.add_enabled(false, egui::Button::new(*label).shortcut_text("準備中"));
    }
}
