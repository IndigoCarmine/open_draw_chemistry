//! The application: open documents, the window's panels, and the commands they issue.
//!
//! Panels (see [`crate::ui`]) only draw and report [`Command`]s; this module carries them out,
//! so the file dialogs, saving and closing logic live in one place.

use std::path::{Path, PathBuf};

use chembuider_rs::widget::analysis::AnalysisPanel;
use chembuider_rs::{Config, bond_menu};
use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::fragments::{self, FragmentStore};
use crate::io::{self, Export, Format, Kind};
use crate::theme;
use crate::three_d::read::Format3d;
use crate::three_d::{self, Export3d};
use crate::ui::fragments::{FragmentAction, FragmentManager, ManagerContext};
use crate::ui::{self, Command, ViewMode, menu::MenuState};

/// How many files "Open Recent" remembers.
const MAX_RECENT: usize = 10;

/// A request to close something that has unsaved changes, waiting on the user's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingClose {
    Tab(usize),
    Quit,
}

/// Persisted application state, plus the open documents (which are not persisted).
///
/// `#[serde(default)]` lets fields be added later without invalidating the state files already
/// on users' machines.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct OpenDrawApp {
    recent: Vec<PathBuf>,
    show_properties: bool,
    view: ViewMode,
    /// The 3D panel's viewer and display options (the options are remembered).
    view3d: ui::view3d::View3d,

    #[serde(skip)]
    docs: Vec<Document>,
    #[serde(skip)]
    active: usize,
    /// Shortcuts and fragments handed to every new editor.
    #[serde(skip)]
    config: Config,
    #[serde(skip)]
    analysis: AnalysisPanel,
    /// The palette's "other element" field.
    #[serde(skip)]
    custom_element: String,
    #[serde(skip)]
    status: String,
    /// Numbers the next untitled document.
    #[serde(skip)]
    untitled: usize,
    #[serde(skip)]
    pending_close: Option<PendingClose>,
    /// The user agreed to quit; let the next close request through.
    #[serde(skip)]
    quit_confirmed: bool,
    #[serde(skip)]
    show_about: bool,
    /// The window title last sent, so it is only sent when it changes.
    #[serde(skip)]
    window_title: String,
    /// The user's own fragments (see [`crate::fragments`]).
    #[serde(skip)]
    fragment_store: FragmentStore,
    /// Every fragment with its source, as the 断片の管理 window lists it.
    #[serde(skip)]
    fragment_entries: Vec<fragments::Entry>,
    #[serde(skip)]
    fragment_manager: FragmentManager,
    /// The atom the canvas was last right-clicked on, for the context menu's 3D化.
    #[serde(skip)]
    context_atom: Option<u32>,
}

impl Default for OpenDrawApp {
    fn default() -> Self {
        Self {
            recent: Vec::new(),
            show_properties: true,
            view: ViewMode::default(),
            view3d: Default::default(),
            docs: Vec::new(),
            active: 0,
            config: Config::default(),
            analysis: japanese_analysis(),
            custom_element: String::new(),
            status: "準備完了".to_owned(),
            untitled: 0,
            pending_close: None,
            quit_confirmed: false,
            show_about: false,
            window_title: String::new(),
            fragment_store: FragmentStore::default(),
            fragment_entries: Vec::new(),
            fragment_manager: FragmentManager::default(),
            context_atom: None,
        }
    }
}

impl OpenDrawApp {
    /// Builds the app, restoring persisted state, and opens `files` (or an empty document).
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> Self {
        // Slightly roomier than the default; tweak to taste.
        cc.egui_ctx.set_pixels_per_point(1.1);
        theme::apply(&cc.egui_ctx);

        let mut app: Self = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, eframe::APP_KEY))
            .unwrap_or_default();
        // User shortcuts from the working directory when present; fragments are the built-in
        // ones plus the user's, from their config directory.
        app.config = Config::load();
        app.reload_fragments();
        for file in &files {
            app.open_path(file);
        }
        if app.docs.is_empty() {
            app.new_document();
        }
        app
    }

    fn active_doc(&self) -> Option<&Document> {
        self.docs.get(self.active)
    }

    fn active_doc_mut(&mut self) -> Option<&mut Document> {
        self.docs.get_mut(self.active)
    }

    fn new_document(&mut self) {
        self.untitled += 1;
        self.docs.push(Document::new(&self.config, self.untitled));
        self.active = self.docs.len() - 1;
    }

    /// Open `path`, or switch to it when it is already open. An untouched untitled document in
    /// the active tab is replaced rather than left behind.
    fn open_path(&mut self, path: &Path) {
        if let Some(i) = self
            .docs
            .iter()
            .position(|d| d.path.as_deref() == Some(path))
        {
            self.active = i;
            return;
        }
        let format = match Kind::from_path(path) {
            Some(Kind::Document(format)) => format,
            Some(Kind::Structure3d(format)) => return self.open_3d(path, format),
            None => {
                self.status = format!(
                    "{} を開けませんでした: {}",
                    path.display(),
                    io::Error::UnknownFormat
                );
                return;
            }
        };
        match io::load(path) {
            Ok(loaded) => {
                let summary = loaded.summary();
                let doc = Document::from_loaded(&self.config, path, format, loaded);
                let name = doc.title();
                self.add_opened(doc, path);
                self.status = if summary.is_empty() {
                    format!("{name} を開きました")
                } else {
                    format!("{name} を開きました（{summary}）")
                };
            }
            Err(e) => self.status = format!("{} を開けませんでした: {e}", path.display()),
        }
    }

    /// Open the 3D structure file at `path`: its coordinates become the document's 3D model
    /// and a 2D depiction goes into the editor. The 3D view is shown when there is a model.
    fn open_3d(&mut self, path: &Path, format: Format3d) {
        let opened = match io::load_3d(path) {
            Ok(opened) => opened,
            Err(e) => {
                self.status = format!("{} を開けませんでした: {e}", path.display());
                return;
            }
        };
        let records = opened.records;
        let projected = opened.projected;
        let has_model = opened.model.is_some();
        let doc = Document::from_structure_3d(&self.config, path, format, opened);
        let name = doc.title();
        self.add_opened(doc, path);
        if has_model {
            self.view = ViewMode::ThreeD;
        }
        let mut status = format!("{name} を開きました");
        if records > 1 {
            status += &format!("（{records} 件の構造のうち最初の 1 件のみ）");
        }
        if !has_model {
            status += "。座標が平面のため 3D モデルは「2D から 3D を生成」で作成してください";
        }
        if projected {
            status += "。2D 配置は 3D 座標の投影です（クリーンアップで整えられます）";
        }
        status += "。保存は CDX/CDXML への「名前を付けて保存」になります";
        self.status = status;
    }

    /// Show a newly opened document, in place of an untouched untitled one in the active tab.
    fn add_opened(&mut self, doc: Document, path: &Path) {
        if self.active_doc().is_some_and(Document::is_pristine) {
            self.docs[self.active] = doc;
        } else {
            self.docs.push(doc);
            self.active = self.docs.len() - 1;
        }
        self.remember(path);
    }

    fn remember(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_owned());
        self.recent.truncate(MAX_RECENT);
    }

    fn open_dialog(&mut self) {
        let chemdraw = Format::ALL.map(Format::extension);
        let all: Vec<&str> = chemdraw
            .iter()
            .chain(Format3d::EXTENSIONS.iter())
            .copied()
            .collect();
        let mut dialog = rfd::FileDialog::new()
            .add_filter("対応するすべての形式", &all)
            .add_filter("ChemDraw", &chemdraw)
            .add_filter("3D 構造 (MOL2, PDB, XYZ, MOL, SDF)", &Format3d::EXTENSIONS);
        if let Some(dir) = self.recent.first().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        for path in dialog.pick_files().unwrap_or_default() {
            self.open_path(&path);
        }
    }

    /// Save the document at `index` to its own file, asking for one when it has none. Returns
    /// whether it was saved.
    fn save_doc(&mut self, index: usize) -> bool {
        match self.docs[index].save() {
            Ok(true) => {
                let doc = &self.docs[index];
                let path = doc.path.clone().expect("a saved document has a path");
                self.status = format!("{} を保存しました", doc.title());
                self.remember(&path);
                true
            }
            Ok(false) => self.save_doc_as(index),
            Err(e) => {
                self.status = format!("保存できませんでした: {e}");
                false
            }
        }
    }

    /// Ask where to save the document at `index`, then save it there. Returns whether it was
    /// saved.
    fn save_doc_as(&mut self, index: usize) -> bool {
        let doc = &self.docs[index];
        let stem = doc
            .path
            .as_deref()
            .and_then(Path::file_stem)
            .map_or_else(|| doc.title(), |s| s.to_string_lossy().into_owned());
        // The document's own format first, so the dialog offers it by default.
        let mut formats = vec![doc.format];
        formats.extend(Format::ALL.into_iter().filter(|&f| f != doc.format));
        let mut dialog =
            rfd::FileDialog::new().set_file_name(format!("{stem}.{}", doc.format.extension()));
        for format in formats {
            dialog = dialog.add_filter(format.description(), &[format.extension()]);
        }
        if let Some(dir) = doc.path.as_deref().and_then(Path::parent) {
            dialog = dialog.set_directory(dir);
        }
        let Some(mut path) = dialog.save_file() else {
            return false;
        };
        let format = Format::from_path(&path).unwrap_or_else(|| {
            path.set_extension(doc.format.extension());
            doc.format
        });
        match self.docs[index].save_as(&path, format) {
            Ok(()) => {
                self.status = format!("{} を保存しました", self.docs[index].title());
                self.remember(&path);
                true
            }
            Err(e) => {
                self.status = format!("保存できませんでした: {e}");
                false
            }
        }
    }

    fn export(&mut self, export: Export) {
        let Some(doc) = self.active_doc() else { return };
        let bytes = match io::export(export, &doc.editor.molecule) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.status = format!("エクスポートできませんでした: {e}");
                return;
            }
        };
        let stem = doc
            .path
            .as_deref()
            .and_then(Path::file_stem)
            .map_or_else(|| doc.title(), |s| s.to_string_lossy().into_owned());
        let Some(path) = rfd::FileDialog::new()
            .add_filter(export.description(), &[export.extension()])
            .set_file_name(format!("{stem}.{}", export.extension()))
            .save_file()
        else {
            return;
        };
        self.status = match std::fs::write(&path, bytes) {
            Ok(()) => format!("{} に書き出しました", path.display()),
            Err(e) => format!("エクスポートできませんでした: {e}"),
        };
    }

    /// Write the active document's 3D model to a file the user picks.
    fn export_3d(&mut self, export: Export3d) {
        let Some(doc) = self.active_doc() else { return };
        let Some(model) = doc.three_d.model() else {
            self.status = "エクスポートする 3D モデルがありません".to_owned();
            return;
        };
        let text = match three_d::export(model, export) {
            Ok(text) => text,
            Err(e) => {
                self.status = format!("エクスポートできませんでした: {e}");
                return;
            }
        };
        let stem = doc
            .path
            .as_deref()
            .and_then(Path::file_stem)
            .map_or_else(|| doc.title(), |s| s.to_string_lossy().into_owned());
        let Some(path) = rfd::FileDialog::new()
            .add_filter(export.description(), &[export.extension()])
            .set_file_name(format!("{stem}.{}", export.extension()))
            .save_file()
        else {
            return;
        };
        self.status = match std::fs::write(&path, text) {
            Ok(()) => format!("{} に書き出しました", path.display()),
            Err(e) => format!("エクスポートできませんでした: {e}"),
        };
    }

    /// Close the tab at `index`, first asking about unsaved changes.
    fn close_tab(&mut self, index: usize) {
        if index >= self.docs.len() {
            return;
        }
        if self.docs[index].is_dirty() {
            self.pending_close = Some(PendingClose::Tab(index));
        } else {
            self.remove_tab(index);
        }
    }

    fn remove_tab(&mut self, index: usize) {
        self.docs.remove(index);
        if self.active > index || self.active >= self.docs.len() {
            self.active = self.active.saturating_sub(1);
        }
    }

    fn request_quit(&mut self, ctx: &egui::Context) {
        if self.docs.iter().any(Document::is_dirty) {
            self.pending_close = Some(PendingClose::Quit);
        } else {
            self.quit_confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn run(&mut self, ctx: &egui::Context, command: Command) {
        match command {
            Command::New => self.new_document(),
            Command::Open => self.open_dialog(),
            Command::OpenPath(path) => self.open_path(&path),
            Command::Save => {
                if !self.docs.is_empty() {
                    self.save_doc(self.active);
                }
            }
            Command::SaveAs => {
                if !self.docs.is_empty() {
                    self.save_doc_as(self.active);
                }
            }
            Command::Export(export) => self.export(export),
            Command::CloseTab(index) => self.close_tab(index),
            Command::CloseActive => self.close_tab(self.active),
            Command::SelectTab(index) => {
                if index < self.docs.len() {
                    self.active = index;
                }
            }
            Command::Quit => self.request_quit(ctx),
            Command::ToggleProperties => self.show_properties = !self.show_properties,
            Command::SetView(view) => self.view = view,
            Command::About => self.show_about = true,
            Command::Generate3d => {
                if let Some(doc) = self.docs.get_mut(self.active) {
                    doc.three_d.generate(&doc.editor.molecule, &self.config);
                    self.view = ViewMode::ThreeD;
                }
            }
            Command::Make3d(scope) => self.make_3d(scope),
            Command::Optimize3d => {
                if let Some(doc) = self.active_doc_mut() {
                    doc.three_d.optimize();
                }
            }
            Command::Export3d(export) => self.export_3d(export),
            Command::SetForceField(ff) => {
                if let Some(doc) = self.active_doc_mut() {
                    doc.three_d.force_field = ff;
                }
            }
            Command::ManageFragments => {
                // Pick up files added to the folder by hand since the last look.
                self.reload_fragments();
                self.fragment_manager.open = true;
            }
            Command::ArrowTool(kind) => {
                if let Some(doc) = self.active_doc_mut() {
                    crate::ui::palette::select_arrow_tool(&mut doc.editor, kind);
                }
            }
            Command::TextTool => {
                if let Some(doc) = self.active_doc_mut() {
                    crate::ui::palette::select_text_tool(&mut doc.editor);
                }
            }
            Command::InsertReactionTable => {
                if let Some(doc) = self.active_doc_mut() {
                    let message = crate::ui::palette::insert_reaction_table(&mut doc.editor);
                    self.status = message.to_owned();
                }
            }
            Command::Undo
            | Command::Redo
            | Command::Copy
            | Command::Paste
            | Command::FitView
            | Command::ToggleCleanup => {
                let Some(doc) = self.active_doc_mut() else {
                    return;
                };
                let editor = &mut doc.editor;
                let message = match command {
                    Command::Undo => (!editor.undo()).then_some("元に戻す操作はありません"),
                    Command::Redo => (!editor.redo()).then_some("やり直す操作はありません"),
                    Command::Copy => Some(if editor.copy(ctx) {
                        "コピーしました（Office には図として、ChemDraw には構造として貼り付けられます）"
                    } else {
                        "コピーするものがありません"
                    }),
                    Command::Paste => Some(if editor.paste_clipboard() {
                        "貼り付けました"
                    } else {
                        "クリップボードに構造がありません"
                    }),
                    Command::FitView => {
                        editor.request_fit();
                        None
                    }
                    Command::ToggleCleanup => {
                        editor.toggle_cleanup();
                        None
                    }
                    _ => unreachable!("handled above"),
                };
                if let Some(message) = message {
                    self.status = message.to_owned();
                }
            }
        }
    }

    /// 3D化: start generating the model of the molecules holding `scope`'s atoms (the whole
    /// drawing for `None`) and switch to the 3D view, which shows it once it is ready.
    fn make_3d(&mut self, scope: Option<Vec<u32>>) {
        let Some(doc) = self.docs.get_mut(self.active) else {
            return;
        };
        doc.three_d
            .generate_part(&doc.editor.molecule, scope, &self.config);
        self.status = match &doc.three_d.state {
            three_d::JobState::Failed(message) => message.clone(),
            three_d::JobState::Running(task) => task.running_label().to_owned(),
            _ => String::new(),
        };
        self.view = ViewMode::ThreeD;
    }

    /// The app's own shortcuts. The editor handles the rest (undo, copy, paste, drawing keys).
    fn shortcuts(ctx: &egui::Context, out: &mut Vec<Command>) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        // Ctrl+Shift+S before Ctrl+S: a shortcut without Shift also matches with it held.
        let table = [
            (
                Modifiers::COMMAND | Modifiers::SHIFT,
                Key::S,
                Command::SaveAs,
            ),
            (Modifiers::COMMAND, Key::S, Command::Save),
            (Modifiers::COMMAND, Key::N, Command::New),
            (Modifiers::COMMAND, Key::O, Command::Open),
            (Modifiers::COMMAND, Key::W, Command::CloseActive),
        ];
        ctx.input_mut(|i| {
            for (modifiers, key, command) in table {
                if i.consume_shortcut(&KeyboardShortcut::new(modifiers, key)) {
                    out.push(command);
                }
            }
        });
    }

    fn menu_state(&self) -> MenuState<'_> {
        let editor = self.active_doc().map(|d| &d.editor);
        let job = self.active_doc().map(|d| &d.three_d);
        let running = job.is_some_and(|j| j.is_running());
        let has_model = job.is_some_and(|j| j.model().is_some());
        MenuState {
            has_document: editor.is_some(),
            can_undo: editor.is_some_and(|e| !e.undo_stack.is_empty()),
            can_redo: editor.is_some_and(|e| !e.redo_stack.is_empty()),
            cleaning: editor.is_some_and(|e| e.is_cleaning()),
            show_properties: self.show_properties,
            view: self.view,
            recent: &self.recent,
            can_generate_3d: three_d::AVAILABLE && job.is_some() && !running,
            can_optimize_3d: has_model && !running,
            can_export_3d: has_model,
        }
    }

    /// Collect finished 3D work, and keep frames coming while any is still running (the
    /// background thread cannot wake the UI itself).
    fn poll_3d(&mut self, ctx: &egui::Context) {
        let mut running = false;
        for (i, doc) in self.docs.iter_mut().enumerate() {
            if let Some(message) = doc.three_d.poll()
                && i == self.active
            {
                self.status = message;
            }
            running |= doc.three_d.is_running();
        }
        if running {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn update_window_title(&mut self, ctx: &egui::Context) {
        let title = match self.active_doc() {
            Some(doc) if doc.is_dirty() => format!("{} ● — {}", doc.title(), crate::APP_NAME),
            Some(doc) => format!("{} — {}", doc.title(), crate::APP_NAME),
            None => crate::APP_NAME.to_owned(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn unsaved_changes_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_close else {
            return;
        };
        enum Choice {
            Save,
            Discard,
            Cancel,
        }
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::unique("unsaved_changes")).show(ctx, |ui| {
            ui.set_width(360.0);
            match pending {
                PendingClose::Tab(i) => {
                    ui.heading("変更を保存しますか？");
                    ui.label(format!(
                        "「{}」には保存されていない変更があります。",
                        self.docs[i].title()
                    ));
                }
                PendingClose::Quit => {
                    let count = self.docs.iter().filter(|d| d.is_dirty()).count();
                    ui.heading("終了する前に保存しますか？");
                    ui.label(format!("保存されていない文書が {count} 件あります。"));
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("保存").clicked() {
                    choice = Some(Choice::Save);
                }
                if ui.button("保存しない").clicked() {
                    choice = Some(Choice::Discard);
                }
                if ui.button("キャンセル").clicked() {
                    choice = Some(Choice::Cancel);
                }
            });
        });
        if modal.should_close() && choice.is_none() {
            choice = Some(Choice::Cancel);
        }
        let Some(choice) = choice else { return };
        self.pending_close = None;
        match (pending, choice) {
            (_, Choice::Cancel) => {}
            (PendingClose::Tab(i), Choice::Save) => {
                if self.save_doc(i) {
                    self.remove_tab(i);
                }
            }
            (PendingClose::Tab(i), Choice::Discard) => self.remove_tab(i),
            (PendingClose::Quit, Choice::Save) => {
                let dirty: Vec<usize> = (0..self.docs.len())
                    .filter(|&i| self.docs[i].is_dirty())
                    .collect();
                // Stop at the first one not saved (a cancelled dialog or an error): the user
                // has changed their mind or needs to see what went wrong.
                if dirty.into_iter().all(|i| self.save_doc(i)) {
                    self.quit_confirmed = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            (PendingClose::Quit, Choice::Discard) => {
                self.quit_confirmed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// Re-read the user's fragments and hand the merged list to the app's config and every open
    /// editor, so a change applies at once.
    fn reload_fragments(&mut self) {
        self.fragment_entries = self.fragment_store.entries();
        self.config.fragments = self
            .fragment_entries
            .iter()
            .map(|e| e.def.clone())
            .collect();
        for doc in &mut self.docs {
            doc.editor.config.fragments = self.config.fragments.clone();
        }
    }

    fn fragment_window(&mut self, ctx: &egui::Context) {
        if !self.fragment_manager.open {
            return;
        }
        let cx = ManagerContext {
            entries: &self.fragment_entries,
            style: &self.config.style,
            selected: self.active_doc().map(|d| d.editor.selected_atoms.len()),
            dir: self.fragment_store.dir(),
        };
        let Some(action) = self.fragment_manager.show(ctx, &cx) else {
            return;
        };
        let store = &self.fragment_store;
        let result = match &action {
            FragmentAction::AddFromSelection(name) => self
                .active_doc()
                .ok_or_else(|| "文書が開かれていません".to_owned())
                .and_then(|doc| fragments::selection_fragment(&doc.editor, name))
                .and_then(|def| {
                    store.save(&def)?;
                    Ok(format!(
                        "断片「{}」を追加しました（{} 原子）",
                        def.name,
                        def.atoms.len()
                    ))
                }),
            FragmentAction::Delete(name) => store
                .delete(name)
                .map(|()| format!("断片「{name}」を削除しました")),
            FragmentAction::Rename { from, to } => store
                .rename(from, to)
                .map(|()| format!("「{from}」を「{to}」に名前変更しました")),
            FragmentAction::OpenFolder => match store.dir() {
                Some(dir) => fragments::open_in_file_manager(dir).map(|()| String::new()),
                None => Err("ユーザー設定フォルダーが見つかりません".to_owned()),
            },
        };
        if result.is_ok() && matches!(action, FragmentAction::AddFromSelection(_)) {
            self.fragment_manager.clear_new_name();
        }
        if action != FragmentAction::OpenFolder {
            self.reload_fragments();
        }
        match &result {
            // Opening the folder needs no message.
            Ok(message) if message.is_empty() => return,
            Ok(message) => self.status = message.clone(),
            Err(_) => {}
        }
        self.fragment_manager.set_result(result);
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_about;
        egui::Window::new("バージョン情報")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.heading(crate::APP_NAME);
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.separator();
                ui.label(env!("CARGO_PKG_DESCRIPTION"));
                ui.label("License: GPL-2.0-or-later");
                ui.hyperlink(env!("CARGO_PKG_REPOSITORY"));
            });
        self.show_about = open;
    }
}

/// The analysis panel, labelled in Japanese like the rest of the window.
fn japanese_analysis() -> AnalysisPanel {
    let mut panel = AnalysisPanel::default();
    panel.ja = true;
    panel
}

/// The canvas of the active document, with a menu on right-click: ChemDraw's bond menu on a
/// bond, and 3D化 for the molecule clicked on (or the selected ones, or the whole drawing).
fn editor_canvas(
    ui: &mut egui::Ui,
    doc: &mut Document,
    context_atom: &mut Option<u32>,
    out: &mut Vec<Command>,
) {
    let response = doc.show(ui).response;
    let editor = &mut doc.editor;
    // The pointer is over the menu while it is open, so the atom under it is taken now, the
    // way the editor keeps the bond (see `context_bond`).
    if response.secondary_clicked() {
        *context_atom = response
            .interact_pointer_pos()
            .and_then(|pos| editor.hit_test_atom(pos, response.rect.center()));
    }
    // The choice is applied after the menu closure lets go of the editor.
    let mut chosen = None;
    let mut make_3d = None;
    let menu = response.context_menu(|ui| {
        let bond = editor
            .context_bond()
            .and_then(|id| editor.molecule.bond_by_id(id));
        match bond {
            Some(bond) => chosen = bond_menu(ui, bond, true),
            None => {
                ui.weak("結合を右クリックすると種類を変更できます");
            }
        }
        ui.separator();
        let clicked = context_atom.or(bond.map(|b| b.begin));
        let target = three_d::part::target(
            &editor.molecule,
            editor.selected_atoms.iter().copied(),
            clicked,
        );
        if ui
            .add_enabled(target.is_some(), egui::Button::new("3D化"))
            .on_hover_text(make_3d_tip(editor, clicked))
            .on_disabled_hover_text("描画が空です")
            .clicked()
        {
            make_3d = target;
            ui.close();
        }
    });
    if menu.is_none() {
        editor.clear_context_bond();
        *context_atom = None;
    }
    if let (Some(action), Some(id)) = (chosen, editor.context_bond()) {
        editor.apply_bond_menu(id, action);
    }
    if let Some(scope) = make_3d {
        out.push(Command::Make3d(scope));
    }
}

/// What 3D化 in the canvas's menu turns into a model, for its tooltip.
fn make_3d_tip(editor: &chembuider_rs::ChemStructEditor, clicked: Option<u32>) -> &'static str {
    let exists = |id: &u32| editor.molecule.atom_by_id(*id).is_some();
    if editor.selected_atoms.iter().any(exists) {
        "選択した原子を含む分子の 3D モデルを作り、3D 画面に表示します"
    } else if clicked.as_ref().is_some_and(exists) {
        "この分子の 3D モデルを作り、3D 画面に表示します"
    } else {
        "描画全体の 3D モデルを作り、3D 画面に表示します"
    }
}

/// Shown when every document is closed.
fn welcome(ui: &mut egui::Ui, out: &mut Vec<Command>) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            ui.heading(crate::APP_NAME);
            ui.add_space(12.0);
            if ui.button("新規作成").clicked() {
                out.push(Command::New);
            }
            if ui.button("ファイルを開く…").clicked() {
                out.push(Command::Open);
            }
            ui.add_space(8.0);
            ui.weak(
                "CDX / CDXML や 3D 構造（MOL2・PDB・XYZ・MOL・SDF）のファイルをここにドロップして開くこともできます",
            );
        });
    });
}

impl eframe::App for OpenDrawApp {
    /// Called by eframe on a timer and at shutdown (needs the `persistence` feature).
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Cloning a Context is cheap (it is an Arc) and frees up `ui` for the panels below.
        let ctx = ui.ctx().clone();
        let mut commands = Vec::new();

        Self::shortcuts(&ctx, &mut commands);
        ctx.input(|i| {
            for file in &i.raw.dropped_files {
                commands.push(Command::OpenPath(file.path().to_owned()));
            }
        });
        if ctx.input(|i| i.viewport().close_requested())
            && !self.quit_confirmed
            && self.docs.iter().any(Document::is_dirty)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending_close = Some(PendingClose::Quit);
        }

        self.poll_3d(&ctx);
        let state = self.menu_state();
        let visuals = ui.visuals().clone();
        let dark = visuals.dark_mode;
        // Bars as in the design: the menu and status bars on the lightest surface, the toolbar
        // and side panels on the panel colour, each separated by a hairline.
        let bar = |fill: egui::Color32, x: i8, y: i8| {
            egui::Frame::new()
                .fill(fill)
                .inner_margin(egui::Margin::symmetric(x, y))
                .stroke(visuals.widgets.noninteractive.bg_stroke)
        };
        let surface = if dark {
            visuals.panel_fill
        } else {
            egui::Color32::WHITE
        };
        egui::Panel::top("menu_bar")
            .frame(bar(surface, 8, 2))
            .show(ui, |ui| ui::menu::menu_bar(ui, &state, &mut commands));
        egui::Panel::top("toolbar")
            .frame(bar(visuals.panel_fill, 10, 6))
            .show(ui, |ui| ui::menu::toolbar(ui, &state, &mut commands));
        egui::Panel::bottom("status_bar")
            .frame(bar(surface, 12, 4))
            .show(ui, |ui| {
                ui::status::status_bar(ui, self.docs.get(self.active), &self.status)
            });

        let view = self.view;
        if let Some(doc) = self.docs.get_mut(self.active) {
            // The drawing tools only act on the canvas, which the 3D view replaces.
            if !view.is_3d() {
                egui::Panel::left("palette")
                    .frame(bar(visuals.panel_fill, 8, 10))
                    .resizable(false)
                    .exact_size(112.0)
                    .show(ui, |ui| {
                        ui::palette::palette(
                            ui,
                            &mut doc.editor,
                            &mut self.custom_element,
                            &mut commands,
                        )
                    });
            }
            // The right panel follows the view: 解析 of the drawing, or 3D 編集 of the model.
            let title = ui::properties::title(view);
            if self.show_properties {
                egui::Panel::right("properties")
                    .frame(bar(visuals.panel_fill, 14, 12))
                    .resizable(true)
                    .default_size(280.0)
                    .size_range(220.0..=420.0)
                    .show(ui, |ui| {
                        let collapse = ui::properties::properties(ui, |ui| {
                            if view.is_3d() {
                                ui::view3d::edit_panel(
                                    ui,
                                    Some(doc),
                                    &mut self.view3d,
                                    &mut commands,
                                );
                            } else {
                                // The panel reports a click on its copy button; the status bar
                                // is not told here.
                                let _ = self.analysis.ui(
                                    ui,
                                    &doc.editor.molecule,
                                    &doc.editor.selected_atoms,
                                );
                            }
                        });
                        if collapse {
                            commands.push(Command::ToggleProperties);
                        }
                    });
            } else {
                // Collapsed: a narrow strip that expands the panel back to its last width.
                egui::Panel::right("properties_collapsed")
                    .frame(bar(visuals.panel_fill, 4, 10))
                    .resizable(false)
                    .exact_size(ui::properties::COLLAPSED_WIDTH)
                    .show(ui, |ui| {
                        if ui::properties::collapsed(ui, title) {
                            commands.push(Command::ToggleProperties);
                        }
                    });
            }
        }
        if !self.docs.is_empty() {
            egui::Panel::top("tabs")
                .frame(
                    bar(theme::canvas_back(dark), 10, 0).inner_margin(egui::Margin {
                        left: 10,
                        right: 10,
                        top: 6,
                        bottom: 0,
                    }),
                )
                .show(ui, |ui| {
                    ui::tabs::tab_strip(ui, &self.docs, self.active, &mut commands)
                });
        }
        let title = self.active_doc().map(Document::title).unwrap_or_default();
        let render_state = frame.wgpu_render_state();
        // The central area shows one view: the drawing, or the 3D model (dark in both themes).
        let central = if view.is_3d() && !self.docs.is_empty() {
            egui::Frame::new().fill(theme::DARK_SURFACE)
        } else {
            egui::Frame::new()
                .fill(theme::canvas_back(dark))
                .inner_margin(egui::Margin::same(18))
        };
        egui::CentralPanel::default().frame(central).show(ui, |ui| {
            match self.docs.get_mut(self.active) {
                Some(doc) if view.is_3d() => {
                    ui::view3d::viewer_area(ui, &title, Some(doc), &mut self.view3d, render_state)
                }
                Some(doc) => editor_canvas(ui, doc, &mut self.context_atom, &mut commands),
                None => welcome(ui, &mut commands),
            }
        });

        for command in commands {
            self.run(&ctx, command);
        }
        self.unsaved_changes_dialog(&ctx);
        self.about_window(&ctx);
        self.fragment_window(&ctx);
        self.update_window_title(&ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// eframe's storage holding one saved state, as `app.ron` would.
    struct SavedState(String);

    impl eframe::Storage for SavedState {
        fn get_string(&self, key: &str) -> Option<String> {
            (key == eframe::APP_KEY).then(|| self.0.clone())
        }
        fn set_string(&mut self, _key: &str, value: String) {
            self.0 = value;
        }
        fn remove_string(&mut self, _key: &str) {
            self.0.clear();
        }
        fn flush(&mut self) {}
    }

    /// 3D styles that were dropped load as the light spheres; the state around them (and the
    /// styles that remain) loads as saved.
    #[test]
    fn a_dropped_3d_style_loads_as_light_spheres() {
        use crate::ui::viewport3d::Style3d;
        for (saved, expected) in [
            ("BallStick", Style3d::Light),
            ("SpaceFilling", Style3d::Light),
            ("Spheres", Style3d::Light),
            ("SomethingNewer", Style3d::Light),
            ("Light", Style3d::Light),
            ("Wireframe", Style3d::Wireframe),
        ] {
            let storage = SavedState(format!(
                "(show_properties: false, view3d: (style: {saved}, numbers: true))"
            ));
            let app: OpenDrawApp = eframe::get_value(&storage, eframe::APP_KEY)
                .unwrap_or_else(|| panic!("{saved}: the saved state did not load"));
            assert_eq!(app.view3d.style, expected, "{saved}");
            assert!(app.view3d.numbers, "{saved}");
            assert!(!app.show_properties, "{saved}");
        }

        // And the choice round-trips.
        let mut storage = SavedState(String::new());
        let mut app = OpenDrawApp::default();
        app.view3d.style = Style3d::Wireframe;
        eframe::set_value(&mut storage, eframe::APP_KEY, &app);
        let back: OpenDrawApp = eframe::get_value(&storage, eframe::APP_KEY).unwrap();
        assert_eq!(back.view3d.style, Style3d::Wireframe);
    }

    /// A file holding one carbon atom, in a fresh directory.
    fn one_atom_file(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("odc-app-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut mol = chembuider_rs::Molecule::default();
        mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
        io::save(&path, Format::from_path(&path).unwrap(), &mol, None).unwrap();
        (dir, path)
    }

    fn app_with_one_untitled() -> OpenDrawApp {
        let mut app = OpenDrawApp::default();
        app.new_document();
        app
    }

    #[test]
    fn opening_a_file_replaces_an_untouched_untitled_document() {
        let (dir, path) = one_atom_file("a.cdx");
        let mut app = app_with_one_untitled();
        app.open_path(&path);
        assert_eq!(app.docs.len(), 1);
        assert_eq!(app.docs[0].title(), "a.cdx");
        assert_eq!(app.recent, vec![path]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn opening_an_open_file_switches_to_its_tab() {
        let (dir, path) = one_atom_file("b.cdxml");
        let mut app = app_with_one_untitled();
        app.open_path(&path);
        app.new_document();
        assert_eq!(app.active, 1);
        app.open_path(&path);
        assert_eq!(app.docs.len(), 2);
        assert_eq!(app.active, 0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_unreadable_file_leaves_the_documents_alone() {
        let mut app = app_with_one_untitled();
        app.open_path(Path::new("does/not/exist.cdx"));
        app.open_path(Path::new("picture.png"));
        assert_eq!(app.docs.len(), 1);
        assert!(app.recent.is_empty());
        assert!(app.status.contains("picture.png"));
    }

    #[test]
    fn opening_a_3d_file_shows_its_model_and_saves_elsewhere() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ethanol.xyz");
        let mut app = app_with_one_untitled();
        app.view = ViewMode::TwoD;
        app.open_path(&path);
        if !three_d::AVAILABLE {
            assert_eq!(app.docs.len(), 1);
            assert!(app.docs[0].path.is_none(), "the untitled document stays");
            assert!(app.status.contains("OpenBabel が必要"), "{}", app.status);
            assert!(app.recent.is_empty());
            return;
        }
        assert_eq!(
            app.docs.len(),
            1,
            "replaces the untouched untitled document"
        );
        let doc = &app.docs[0];
        assert_eq!(doc.title(), "ethanol.xyz");
        assert!(doc.three_d.model().is_some(), "{}", app.status);
        assert_eq!(doc.editor.molecule.atoms.len(), 3);
        assert_eq!(app.view, ViewMode::ThreeD, "the 3D view is shown");
        assert_eq!(app.recent, vec![path.clone()]);
        assert!(app.status.contains("名前を付けて保存"), "{}", app.status);
        // Opening it again switches to it rather than reading it twice.
        app.new_document();
        app.open_path(&path);
        assert_eq!((app.docs.len(), app.active), (2, 0));
    }

    #[test]
    fn recent_files_are_newest_first_and_capped() {
        let mut app = OpenDrawApp::default();
        for i in 0..MAX_RECENT + 3 {
            app.remember(Path::new(&format!("{i}.cdx")));
        }
        app.remember(Path::new("5.cdx"));
        assert_eq!(app.recent.len(), MAX_RECENT);
        assert_eq!(app.recent[0], PathBuf::from("5.cdx"));
        assert_eq!(
            app.recent
                .iter()
                .filter(|p| p.as_path() == Path::new("5.cdx"))
                .count(),
            1
        );
    }

    #[test]
    fn closing_asks_only_when_there_are_unsaved_changes() {
        let mut app = app_with_one_untitled();
        app.new_document();
        app.docs[1]
            .editor
            .molecule
            .add_atom("C".to_owned(), [0.0, 0.0], 0);

        app.close_tab(1);
        assert_eq!(app.pending_close, Some(PendingClose::Tab(1)));
        assert_eq!(app.docs.len(), 2);

        app.pending_close = None;
        app.close_tab(0);
        assert_eq!(app.docs.len(), 1);
        assert_eq!(app.active, 0);
        assert!(app.docs[0].is_dirty());
    }

    #[test]
    fn removing_a_tab_keeps_the_active_document() {
        let mut app = app_with_one_untitled();
        app.new_document();
        app.new_document();
        app.active = 2;
        app.remove_tab(0);
        assert_eq!(app.docs[app.active].title(), "無題 3");
        app.remove_tab(1);
        assert_eq!(app.active, 0);
        app.remove_tab(0);
        assert!(app.docs.is_empty());
        assert_eq!(app.active, 0);
    }

    #[test]
    fn reloading_fragments_reaches_every_open_editor() {
        let dir = std::env::temp_dir().join(format!("odc-app-frag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = app_with_one_untitled();
        app.fragment_store = FragmentStore::at(&dir);
        app.new_document();

        let editor = &mut app.docs[0].editor;
        let a = editor.molecule.add_atom("C".to_owned(), [0.0, 0.0], 0);
        let b = editor.molecule.add_atom("N".to_owned(), [1.5, 0.0], 0);
        editor
            .molecule
            .add_bond(a, b, chembuider_rs::BondOrder::Triple);
        editor.selected_atoms.extend([a, b]);
        let def = fragments::selection_fragment(editor, "MyCN").unwrap();
        app.fragment_store.save(&def).unwrap();
        app.reload_fragments();

        assert!(app.config.resolve_fragment("MyCN").is_some());
        for doc in &app.docs {
            assert!(doc.editor.config.resolve_fragment("MyCN").is_some());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Guards the persistence contract: state must survive a serialize round-trip, which is
    /// what eframe does between launches.
    #[test]
    fn state_survives_a_serde_round_trip() {
        let mut app = OpenDrawApp::default();
        app.remember(Path::new("x.cdxml"));
        app.show_properties = false;
        app.new_document();

        let json = serde_json::to_string(&app).expect("serialize");
        let restored: OpenDrawApp = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(restored.recent, vec![PathBuf::from("x.cdxml")]);
        assert!(!restored.show_properties);
        assert!(restored.docs.is_empty(), "documents are not persisted");
    }

    /// The 3D menu's generate → (poll) → optimise path, and the panel drawing the result.
    #[cfg(feature = "openbabel")]
    #[test]
    fn the_3d_menu_generates_and_optimises_in_the_background() {
        let ctx = egui::Context::default();
        let mut app = app_with_one_untitled();
        let mol = &mut app.docs[0].editor.molecule;
        let c = mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
        let o = mol.add_atom("O".to_owned(), [1.5, 0.0], 0);
        mol.add_bond(c, o, chembuider_rs::BondOrder::Single);
        assert!(app.menu_state().can_generate_3d);
        assert!(!app.menu_state().can_export_3d);

        let finish = |app: &mut OpenDrawApp| {
            while app.docs[0].three_d.is_running() {
                std::thread::sleep(std::time::Duration::from_millis(20));
                app.poll_3d(&ctx);
            }
        };
        app.run(&ctx, Command::SetForceField(three_d::ForceField::Uff));
        app.run(&ctx, Command::Generate3d);
        finish(&mut app);
        let model = app.docs[0].three_d.model().expect(&app.status);
        assert_eq!(model.force_field, three_d::ForceField::Uff);
        assert!(app.status.contains("生成しました"), "{}", app.status);
        assert!(app.menu_state().can_optimize_3d && app.menu_state().can_export_3d);

        app.run(&ctx, Command::Optimize3d);
        finish(&mut app);
        assert!(app.docs[0].three_d.model().is_some(), "{}", app.status);
        assert!(app.status.contains("最適化しました"), "{}", app.status);

        // One headless frame of the viewer and the panel with the model in it.
        let mut commands = Vec::new();
        let mut view = ui::view3d::View3d::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui::view3d::edit_panel(ui, app.docs.first_mut(), &mut view, &mut commands);
            ui::view3d::viewer_area(ui, "test", app.docs.first_mut(), &mut view, None);
        });
        output.textures_delta.clear();
        assert!(commands.is_empty());
    }

    /// The 2D＋3D split view is gone: a saved state in it opens in 2D, the rest as saved.
    #[test]
    fn a_saved_split_view_opens_in_2d() {
        for (saved, expected) in [
            ("Split", ViewMode::TwoD),
            ("TwoD", ViewMode::TwoD),
            ("ThreeD", ViewMode::ThreeD),
            ("SomethingNewer", ViewMode::TwoD),
        ] {
            let storage = SavedState(format!(
                "(show_properties: false, view: {saved}, view3d: (numbers: true))"
            ));
            let app: OpenDrawApp = eframe::get_value(&storage, eframe::APP_KEY)
                .unwrap_or_else(|| panic!("{saved}: the saved state did not load"));
            assert_eq!(app.view, expected, "{saved}");
            assert!(!app.show_properties && app.view3d.numbers, "{saved}");
        }
    }

    /// Two molecules drawn: ethanol (C–C–O) and, apart from it, methylamine (C–N). Returns
    /// the ids [c1, c2, o, c3, n].
    fn draw_two_molecules(app: &mut OpenDrawApp) -> [u32; 5] {
        use chembuider_rs::BondOrder;
        let mol = &mut app.docs[app.active].editor.molecule;
        let c1 = mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
        let c2 = mol.add_atom("C".to_owned(), [1.3, 0.0], 0);
        let o = mol.add_atom("O".to_owned(), [2.0, 1.0], 0);
        mol.add_bond(c1, c2, BondOrder::Single);
        mol.add_bond(c2, o, BondOrder::Single);
        let c3 = mol.add_atom("C".to_owned(), [10.0, 0.0], 0);
        let n = mol.add_atom("N".to_owned(), [11.3, 0.0], 0);
        mol.add_bond(c3, n, BondOrder::Single);
        [c1, c2, o, c3, n]
    }

    /// 3D化 on one molecule of the drawing: the view switches to 3D, and the model is of that
    /// molecule only (or, without OpenBabel, the 3D view says why there is none). Editing the
    /// other molecule leaves the model current; editing its own makes it stale.
    #[test]
    fn make_3d_shows_the_molecule_clicked_on_in_the_3d_view() {
        let ctx = egui::Context::default();
        let mut app = app_with_one_untitled();
        let [_, _, o, c3, n] = draw_two_molecules(&mut app);
        assert_eq!(app.view, ViewMode::TwoD);
        let editor = &app.docs[0].editor;
        let scope =
            three_d::part::target(&editor.molecule, [], Some(c3)).expect("something is drawn");
        app.run(&ctx, Command::Make3d(scope));
        assert_eq!(app.view, ViewMode::ThreeD);

        if !three_d::AVAILABLE {
            assert!(app.docs[0].three_d.model().is_none());
            assert!(app.status.contains("利用できません"), "{}", app.status);
        } else {
            assert!(app.docs[0].three_d.is_running());
            while app.docs[0].three_d.is_running() {
                std::thread::sleep(std::time::Duration::from_millis(20));
                app.poll_3d(&ctx);
            }
            let model = app.docs[0].three_d.model().expect(&app.status);
            // Methylamine, CH3NH2: the ethanol is not part of it.
            assert_eq!(model.atoms.len(), 7, "{}", model.formula);
            assert!(!app.docs[0].three_d.is_stale(&app.docs[0].editor.molecule));
            let mol = &mut app.docs[0].editor.molecule;
            let cl = mol.add_atom("Cl".to_owned(), [3.0, 1.0], 0);
            mol.add_bond(o, cl, chembuider_rs::BondOrder::Single);
            assert!(
                !app.docs[0].three_d.is_stale(&app.docs[0].editor.molecule),
                "the other molecule was edited"
            );
            let mol = &mut app.docs[0].editor.molecule;
            mol.atom_by_id_mut(n).unwrap().element = "O".to_owned();
            assert!(app.docs[0].three_d.is_stale(&app.docs[0].editor.molecule));
        }

        // A headless frame of the 3D view's two parts: 3D 編集 for the right panel and the
        // viewer (no renderer here, so it shows the summary).
        let mut commands = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui::view3d::edit_panel(ui, app.docs.first_mut(), &mut app.view3d, &mut commands);
            ui::view3d::viewer_area(ui, "test", app.docs.first_mut(), &mut app.view3d, None);
        });
        output.textures_delta.clear();
        assert!(commands.is_empty());
    }

    /// The canvas's 3D化 is disabled with nothing drawn.
    #[test]
    fn make_3d_needs_a_drawing() {
        let app = app_with_one_untitled();
        let editor = &app.docs[0].editor;
        assert_eq!(three_d::part::target(&editor.molecule, [], None), None);
    }
}
