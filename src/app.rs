//! The application: open documents, the window's panels, and the commands they issue.
//!
//! Panels (see [`crate::ui`]) only draw and report [`Command`]s; this module carries them out,
//! so the file dialogs, saving and closing logic live in one place.

use std::path::{Path, PathBuf};

use chembuider_rs::widget::analysis::AnalysisPanel;
use chembuider_rs::{Config, bond_menu};
use serde::{Deserialize, Serialize};

use crate::assembly::{self, AssemblyExport, Component};
use crate::document::Document;
use crate::fragments::{self, FragmentStore};
use crate::i18n::Lang;
use crate::io::{self, Export, Format, Kind};
use crate::theme;
use crate::three_d::read::Format3d;
use crate::three_d::{self, Export3d};
use crate::ui::fragments::{FragmentAction, FragmentManager, ManagerContext};
use crate::ui::{self, Command, ViewMode, menu::MenuState};

mod ole;

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
    /// The assembly view's viewer and display options (the options are remembered).
    assembly_view: ui::assembly::AssemblyView,
    /// The text format bar (書式): its recently used fonts are remembered.
    text_format: ui::text_format::TextFormatBar,
    /// The UI language the user picked; `None` (until they pick one) follows the OS locale.
    #[serde(deserialize_with = "crate::i18n::deserialize_lenient")]
    language: Option<Lang>,
    /// The language the fonts were last set up for (see [`crate::i18n::fonts`]).
    #[serde(skip)]
    fonts_for: Option<Lang>,

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
    /// Set when the process is the OLE server: the link to its COM objects (see [`ole`]).
    #[serde(skip)]
    ole: Option<crate::ole_server::AppBridge>,
    /// The パラメータスキャン window.
    #[serde(skip)]
    scan_window: ui::scan::ScanWindow,
}

impl Default for OpenDrawApp {
    fn default() -> Self {
        Self {
            recent: Vec::new(),
            show_properties: true,
            view: ViewMode::default(),
            view3d: Default::default(),
            assembly_view: Default::default(),
            text_format: Default::default(),
            language: None,
            fonts_for: None,
            docs: Vec::new(),
            active: 0,
            config: Config::default(),
            analysis: AnalysisPanel::default(),
            custom_element: String::new(),
            status: String::new(),
            untitled: 0,
            pending_close: None,
            quit_confirmed: false,
            show_about: false,
            window_title: String::new(),
            fragment_store: FragmentStore::default(),
            fragment_entries: Vec::new(),
            fragment_manager: FragmentManager::default(),
            context_atom: None,
            ole: None,
            scan_window: Default::default(),
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
        app.apply_language(app.language.unwrap_or_else(Lang::system));
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

    /// Show the UI in `lang`: the app's strings at once, the fonts from the next frame (see
    /// [`Self::ui`]) and the structure editor library's own panels.
    fn apply_language(&mut self, lang: Lang) {
        crate::i18n::set_language(lang);

        // ─── chembuider-rs language hook ─────────────────────────────────────────────
        // The one place the library's UI language (its analysis panel, bond menu, …) is set.
        // `Lang::code()` gives the same tags as the library's `Language::tag()`; its global
        // setting overrides the panel's old `ja` flag.
        if let Some(l) = chembuider_rs::i18n::Language::from_tag(lang.code()) {
            chembuider_rs::i18n::set_language(l);
        }
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
                self.status = t!(
                    "app.status.open_failed",
                    path = path.display(),
                    error = io::Error::UnknownFormat
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
                    t!("app.status.opened", name = name)
                } else {
                    t!("app.status.opened_summary", name = name, summary = summary)
                };
            }
            Err(e) => self.status = t!("app.status.open_failed", path = path.display(), error = e),
        }
    }

    /// Open the 3D structure file at `path`: its coordinates become the document's 3D model
    /// and a 2D depiction goes into the editor. The 3D view is shown when there is a model.
    fn open_3d(&mut self, path: &Path, format: Format3d) {
        let opened = match io::load_3d(path) {
            Ok(opened) => opened,
            Err(e) => {
                self.status = t!("app.status.open_failed", path = path.display(), error = e);
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
        let mut status = t!("app.status.opened", name = name);
        if records > 1 {
            status += &tn!("app.status.opened_first_record", records);
        }
        if !has_model {
            status += t!("app.status.opened_flat");
        }
        if projected {
            status += t!("app.status.opened_projected");
        }
        status += t!("app.status.opened_save_note");
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
            .add_filter(t!("dialog.filter.all_supported"), &all)
            .add_filter("ChemDraw", &chemdraw)
            .add_filter(t!("dialog.filter.structure_3d"), &Format3d::EXTENSIONS);
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
        if self.docs[index].embedded.is_some() {
            return self.save_embedded(index);
        }
        match self.docs[index].save() {
            Ok(true) => {
                let doc = &self.docs[index];
                let path = doc.path.clone().expect("a saved document has a path");
                self.status = t!("app.status.saved", name = doc.title());
                self.remember(&path);
                true
            }
            Ok(false) => self.save_doc_as(index),
            Err(e) => {
                self.status = t!("app.status.save_failed", error = e);
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
                self.status = t!("app.status.saved", name = self.docs[index].title());
                self.remember(&path);
                true
            }
            Err(e) => {
                self.status = t!("app.status.save_failed", error = e);
                false
            }
        }
    }

    fn export(&mut self, export: Export) {
        let Some(doc) = self.active_doc() else { return };
        let bytes = match io::export(export, &doc.editor.molecule) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.status = t!("app.status.export_failed", error = e);
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
            Ok(()) => t!("app.status.exported", path = path.display()),
            Err(e) => t!("app.status.export_failed", error = e),
        };
    }

    /// Write the active document's 3D model to a file the user picks.
    fn export_3d(&mut self, export: Export3d) {
        let Some(doc) = self.active_doc() else { return };
        let Some(model) = doc.three_d.model() else {
            self.status = t!("app.status.no_3d_model_to_export").to_owned();
            return;
        };
        let text = match three_d::export(model, export) {
            Ok(text) => text,
            Err(e) => {
                self.status = t!("app.status.export_failed", error = e);
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
            Ok(()) => t!("app.status.exported", path = path.display()),
            Err(e) => t!("app.status.export_failed", error = e),
        };
    }

    /// Close the tab at `index`, first asking about unsaved changes.
    fn close_tab(&mut self, index: usize) {
        if index >= self.docs.len() {
            return;
        }
        if self.docs[index].embedded.is_some() {
            return self.close_embedded(index);
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
        self.close_all_embedded();
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
            Command::SetLanguage(lang) => {
                self.language = Some(lang);
                self.apply_language(lang);
            }
            Command::About => self.show_about = true,
            Command::Generate3d => {
                if let Some(doc) = self.docs.get_mut(self.active) {
                    doc.three_d.generate(&doc.editor.molecule, &self.config);
                    if let (three_d::JobState::Running(_), Some(note)) = (
                        &doc.three_d.state,
                        three_d::molfile::polymer_note(&doc.editor.molecule),
                    ) {
                        self.status = note.to_owned();
                    }
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
            Command::CopyIupacName | Command::InsertIupacName => {
                let config = &self.config;
                let Some(doc) = self.docs.get_mut(self.active) else {
                    return;
                };
                self.status = if command == Command::CopyIupacName {
                    crate::iupac::name(&doc.editor, config).map(|name| {
                        ctx.copy_text(name.clone());
                        t!("app.status.iupac_copied", name = name)
                    })
                } else {
                    crate::iupac::insert_name(&mut doc.editor, config)
                        .map(|name| t!("app.status.iupac_inserted", name = name))
                }
                .unwrap_or_else(|message| message);
            }
            Command::CreateAssembly => self.create_assembly(),
            Command::AssemblyAddFile => self.assembly_add_file(),
            Command::AssemblyExport(export) => self.export_assembly(export),
            Command::AssemblySave => self.save_assembly(),
            Command::AssemblyOpen => self.open_assembly(),
            Command::Undo | Command::Redo if self.view.is_assembly() => {
                let Some(doc) = self.active_doc_mut() else {
                    return;
                };
                let redo = command == Command::Redo;
                let done = if redo {
                    doc.assembly.redo()
                } else {
                    doc.assembly.undo()
                };
                self.status = match (done, redo) {
                    (true, false) => t!("app.status.assembly_undone"),
                    (true, true) => t!("app.status.assembly_redone"),
                    (false, false) => t!("app.status.nothing_to_undo"),
                    (false, true) => t!("app.status.nothing_to_redo"),
                }
                .to_owned();
            }
            Command::OpenScan => self.scan_window.open = true,
            Command::InsertReactionTable => {
                if let Some(doc) = self.active_doc_mut() {
                    let message = crate::ui::palette::insert_reaction_table(&mut doc.editor);
                    self.status = message.to_owned();
                }
            }
            Command::Bracket(usage) => {
                if let Some(doc) = self.active_doc_mut() {
                    self.status = crate::ui::palette::bracket_selection(&mut doc.editor, usage);
                }
            }
            Command::BracketShape(shape) => {
                if let Some(doc) = self.active_doc_mut() {
                    self.status = crate::ui::palette::bracket_shape(&mut doc.editor, shape);
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
                // A message of the paste's own (an embedded object it cannot read).
                let mut failure = None;
                let message = match command {
                    Command::Undo => (!editor.undo()).then_some(t!("app.status.nothing_to_undo")),
                    Command::Redo => (!editor.redo()).then_some(t!("app.status.nothing_to_redo")),
                    Command::Copy => Some(if crate::ole::copy(editor, ctx) {
                        crate::ole::copied_message()
                    } else {
                        t!("app.status.nothing_to_copy")
                    }),
                    Command::Paste => match crate::ole::paste(editor) {
                        Ok(true) => Some(t!("app.status.pasted")),
                        Ok(false) => Some(t!("app.status.no_structure_on_clipboard")),
                        Err(message) => {
                            failure = Some(message);
                            None
                        }
                    },
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
                if let Some(message) = message.map(str::to_owned).or(failure) {
                    self.status = message;
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
            three_d::JobState::Running(task) => {
                match three_d::molfile::polymer_note(&doc.editor.molecule) {
                    Some(note) => t!(
                        "app.status.running_with_note",
                        label = task.running_label(),
                        note = note
                    ),
                    None => task.running_label().to_owned(),
                }
            }
            _ => String::new(),
        };
        self.view = ViewMode::ThreeD;
    }

    /// 分子集合体を作成: give the active document's assembly its first monomer, the document's
    /// 3D model, and show the assembly view.
    fn create_assembly(&mut self) {
        let Some(doc) = self.docs.get_mut(self.active) else {
            return;
        };
        self.view = ViewMode::Assembly;
        if !doc.assembly.is_empty() {
            self.status = t!("app.status.assembly_shown").to_owned();
            return;
        }
        let Some(model) = doc.three_d.model() else {
            self.status = t!("app.status.no_model_for_monomer").to_owned();
            return;
        };
        let component = Component::from_model(ui::assembly::model_name(doc), model);
        self.status = tn!(
            "app.status.assembly_created",
            component.atoms.len(),
            name = component.name
        );
        doc.assembly.edit(|d| d.components.push(component));
    }

    /// The file name (without extension) suggested when saving the active document's
    /// assembly or its exports.
    fn assembly_stem(doc: &Document) -> String {
        let stem = doc
            .path
            .as_deref()
            .and_then(Path::file_stem)
            .map_or_else(|| doc.title(), |s| s.to_string_lossy().into_owned());
        format!("{stem}_assembly")
    }

    fn assembly_add_file(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(t!("dialog.filter.structure_3d"), &Format3d::EXTENSIONS)
            .pick_file()
        else {
            return;
        };
        match read_component(&path) {
            Ok(component) => {
                let Some(doc) = self.active_doc_mut() else {
                    return;
                };
                let message = tn!(
                    "app.status.monomer_added",
                    component.atoms.len(),
                    name = component.name
                );
                doc.assembly.edit(|d| d.components.push(component));
                self.status = message;
            }
            Err(e) => self.status = t!("app.status.load_failed", path = path.display(), error = e),
        }
    }

    fn export_assembly(&mut self, export: AssemblyExport) {
        let Some(doc) = self.docs.get_mut(self.active) else {
            return;
        };
        let stem = Self::assembly_stem(doc);
        let built = match doc.assembly.built() {
            Ok(built) if !built.is_empty() => built,
            Ok(_) => {
                self.status = t!("app.status.no_assembly_to_export").to_owned();
                return;
            }
            Err(e) => {
                self.status = t!("app.status.write_failed", error = e);
                return;
            }
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter(export.description(), &[export.extension()])
            .set_file_name(format!("{stem}.{}", export.extension()))
            .save_file()
        else {
            return;
        };
        let text = assembly::export::export(export, &built, &doc.assembly.design, &stem);
        self.status = match std::fs::write(&path, text) {
            Ok(()) => t!(
                "app.status.assembly_exported",
                path = path.display(),
                molecules = built.molecule_count(),
                atoms = built.atoms.len()
            ),
            Err(e) => t!("app.status.write_failed", error = e),
        };
    }

    fn save_assembly(&mut self) {
        let Some(doc) = self.active_doc() else { return };
        let Some(path) = rfd::FileDialog::new()
            .add_filter(t!("dialog.filter.assembly"), &[assembly::file::EXTENSION])
            .set_file_name(format!(
                "{}.{}",
                Self::assembly_stem(doc),
                assembly::file::EXTENSION
            ))
            .save_file()
        else {
            return;
        };
        self.status = match assembly::file::save(&path, &doc.assembly.design) {
            Ok(()) => t!("app.status.assembly_saved", path = path.display()),
            Err(e) => t!("app.status.assembly_save_failed", error = e),
        };
    }

    fn open_assembly(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(t!("dialog.filter.assembly"), &[assembly::file::EXTENSION])
            .pick_file()
        else {
            return;
        };
        self.open_assembly_path(&path);
    }

    /// Open the `.odasm` file at `path` into the active document's assembly (as one undo step)
    /// and show it.
    fn open_assembly_path(&mut self, path: &Path) {
        let Some(doc) = self.docs.get_mut(self.active) else {
            return;
        };
        match assembly::file::open(path) {
            Ok(design) => {
                doc.assembly.replace(design);
                self.view = ViewMode::Assembly;
                self.status = t!("app.status.assembly_opened", path = path.display());
            }
            Err(e) => self.status = t!("app.status.open_failed", path = path.display(), error = e),
        }
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
        // In the assembly view, undo and redo act on the assembly.
        let (can_undo, can_redo) = match self.active_doc() {
            Some(doc) if self.view.is_assembly() => {
                (doc.assembly.can_undo(), doc.assembly.can_redo())
            }
            _ => (
                editor.is_some_and(|e| !e.undo_stack.is_empty()),
                editor.is_some_and(|e| !e.redo_stack.is_empty()),
            ),
        };
        MenuState {
            has_document: editor.is_some(),
            can_undo,
            can_redo,
            cleaning: editor.is_some_and(|e| e.is_cleaning()),
            show_properties: self.show_properties,
            view: self.view,
            recent: &self.recent,
            can_generate_3d: job.is_some() && !running,
            can_optimize_3d: has_model && !running,
            can_export_3d: has_model,
            can_create_assembly: editor.is_some(),
            language: crate::i18n::language(),
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
                    ui.heading(t!("dialog.unsaved.tab_title"));
                    ui.label(t!(
                        "dialog.unsaved.tab_message",
                        name = self.docs[i].title()
                    ));
                }
                PendingClose::Quit => {
                    let count = self.docs.iter().filter(|d| d.is_dirty()).count();
                    ui.heading(t!("dialog.unsaved.quit_title"));
                    ui.label(tn!("dialog.unsaved.quit_message", count));
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(t!("dialog.unsaved.save")).clicked() {
                    choice = Some(Choice::Save);
                }
                if ui.button(t!("dialog.unsaved.discard")).clicked() {
                    choice = Some(Choice::Discard);
                }
                if ui.button(t!("common.cancel")).clicked() {
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
                .ok_or_else(|| t!("app.status.no_document").to_owned())
                .and_then(|doc| fragments::selection_fragment(&doc.editor, name))
                .and_then(|def| {
                    store.save(&def)?;
                    Ok(tn!(
                        "app.status.fragment_added",
                        def.atoms.len(),
                        name = def.name
                    ))
                }),
            FragmentAction::Delete(name) => store
                .delete(name)
                .map(|()| t!("app.status.fragment_deleted", name = name)),
            FragmentAction::Rename { from, to } => store
                .rename(from, to)
                .map(|()| t!("app.status.fragment_renamed", from = from, to = to)),
            FragmentAction::OpenFolder => match store.dir() {
                Some(dir) => fragments::open_in_file_manager(dir).map(|()| String::new()),
                None => Err(t!("app.status.settings_folder_missing").to_owned()),
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
        egui::Window::new(t!("dialog.about.title"))
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

/// A monomer read from the 3D structure file at `path`, named after the file.
fn read_component(path: &Path) -> Result<Component, String> {
    let format = Format3d::from_path(path).ok_or_else(|| io::Error::UnknownFormat.to_string())?;
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let opened = three_d::read::read(format, &text).map_err(|e| e.to_string())?;
    let model = opened
        .model
        .ok_or_else(|| t!("app.status.flat_coordinates").to_owned())?;
    let name = path.file_stem().map_or_else(
        || "monomer".to_owned(),
        |s| s.to_string_lossy().into_owned(),
    );
    Ok(Component::from_model(name, &model))
}

/// The analysis panel, labelled in Japanese like the rest of the window.
/// The canvas of the active document, with a menu on right-click: ChemDraw's bond menu on a
/// bond, and 3D化 for the molecule clicked on (or the selected ones, or the whole drawing);
/// and over it, while there is text to format, the 書式 bar.
fn editor_canvas(
    ui: &mut egui::Ui,
    doc: &mut Document,
    context_atom: &mut Option<u32>,
    text_format: &mut ui::text_format::TextFormatBar,
    out: &mut Vec<Command>,
) {
    let response = doc.show(ui).response;
    if doc.take_paste_request() {
        out.push(Command::Paste);
    }
    let editor = &mut doc.editor;
    ui::text_format::show(ui, response.rect, editor, text_format);
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
                ui.weak(t!("app.canvas.bond_hint"));
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
            .add_enabled(
                target.is_some(),
                egui::Button::new(t!("app.canvas.make_3d")),
            )
            .on_hover_text(make_3d_tip(editor, clicked))
            .on_disabled_hover_text(t!("app.canvas.drawing_empty"))
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
        t!("app.canvas.make_3d_selection_tip")
    } else if clicked.as_ref().is_some_and(exists) {
        t!("app.canvas.make_3d_molecule_tip")
    } else {
        t!("app.canvas.make_3d_drawing_tip")
    }
}

/// Shown when every document is closed.
fn welcome(ui: &mut egui::Ui, out: &mut Vec<Command>) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            ui.heading(crate::APP_NAME);
            ui.add_space(12.0);
            if ui.button(t!("app.welcome.new")).clicked() {
                out.push(Command::New);
            }
            if ui.button(t!("app.welcome.open")).clicked() {
                out.push(Command::Open);
            }
            ui.add_space(8.0);
            ui.weak(t!("app.welcome.drop_hint"));
        });
    });
}

impl eframe::App for OpenDrawApp {
    /// Called by eframe on a timer and at shutdown (needs the `persistence` feature).
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    /// Runs every frame, also while the window is hidden: the OLE server's messages.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_ole(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Cloning a Context is cheap (it is an Arc) and frees up `ui` for the panels below.
        let ctx = ui.ctx().clone();
        let mut commands = Vec::new();

        let lang = crate::i18n::language();
        if self.fonts_for != Some(lang) {
            crate::i18n::fonts::install(&ctx, lang);
            self.fonts_for = Some(lang);
        }
        Self::shortcuts(&ctx, &mut commands);
        ctx.input(|i| {
            for file in &i.raw.dropped_files {
                commands.push(Command::OpenPath(file.path().to_owned()));
            }
        });
        if ctx.input(|i| i.viewport().close_requested()) {
            // Embedded objects are written back into their documents rather than asked about.
            self.close_all_embedded();
        }
        if ctx.input(|i| i.viewport().close_requested())
            && !self.quit_confirmed
            && self.docs.iter().any(Document::is_dirty)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending_close = Some(PendingClose::Quit);
        }

        self.poll_3d(&ctx);
        ui::scan::poll(&ctx, &mut self.docs);
        // A gesture on the assembly (dragging a value) is one undo step, closed on release.
        if !ctx.input(|i| i.pointer.any_down())
            && let Some(doc) = self.docs.get_mut(self.active)
        {
            doc.assembly.settle();
        }
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
            // The drawing tools only act on the canvas, which the other views replace.
            if view.is_drawing() {
                egui::Panel::left("palette")
                    .frame(bar(visuals.panel_fill, 0, 0).inner_margin(ui::palette::PALETTE_MARGIN))
                    .resizable(false)
                    .exact_size(ui::palette::PALETTE_WIDTH)
                    .show(ui, |ui| {
                        ui::palette::palette(
                            ui,
                            &mut doc.editor,
                            &mut self.custom_element,
                            &mut commands,
                        )
                    });
            }
            // The right panel follows the view: 解析 of the drawing, 3D 編集 of the model, or
            // the 集合体 panel.
            let title = ui::properties::title(view);
            if self.show_properties {
                egui::Panel::right("properties")
                    .frame(bar(visuals.panel_fill, 14, 12))
                    .resizable(true)
                    .default_size(280.0)
                    .size_range(220.0..=420.0)
                    .show(ui, |ui| {
                        // The 集合体 panel scrolls between its own tabs and footer.
                        let collapse = if view.is_assembly() {
                            ui::properties::properties_unscrolled(ui, |ui| {
                                ui::assembly::panel(
                                    ui,
                                    Some(doc),
                                    &mut self.assembly_view,
                                    &mut commands,
                                );
                            })
                        } else {
                            ui::properties::properties(ui, |ui| {
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
                                    ui::smiles_row::show(
                                        ui,
                                        &doc.editor.molecule,
                                        &doc.editor.selected_atoms,
                                        &self.config,
                                    );
                                }
                            })
                        };
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
        // The central area shows one view: the drawing, or the 3D model or the assembly (dark
        // in both themes).
        let central = if !view.is_drawing() && !self.docs.is_empty() {
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
                Some(doc) if view.is_assembly() => ui::assembly::viewer_area(
                    ui,
                    &title,
                    Some(doc),
                    &mut self.assembly_view,
                    render_state,
                ),
                Some(doc) => editor_canvas(
                    ui,
                    doc,
                    &mut self.context_atom,
                    &mut self.text_format,
                    &mut commands,
                ),
                None => welcome(ui, &mut commands),
            }
        });

        if let Some(message) = ui::scan::window(
            &ctx,
            &mut self.scan_window,
            self.docs.get_mut(self.active),
            self.view,
            &mut commands,
        ) {
            self.status = message;
        }
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
        assert!(app.status.contains("Save as…"), "{}", app.status);
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
        assert_eq!(app.docs[app.active].title(), "Untitled 3");
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
    #[test]
    fn the_3d_menu_generates_and_optimises_in_the_background() {
        // The 3D job's messages, checked in Japanese.
        let _lang = crate::i18n::ScopedLanguage::new(Lang::Ja);
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
    /// molecule only. Editing the other molecule leaves the model current; editing its own makes
    /// it stale.
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

    /// A hand-built water model, so the assembly tests need no OpenBabel generation.
    fn water_model() -> three_d::Model3d {
        use three_d::{Atom3d, Bond3d};
        let atom = |element: &str, pos| Atom3d {
            element: element.to_owned(),
            pos,
        };
        three_d::Model3d {
            atoms: vec![
                atom("O", [0.0, 0.0, 0.0]),
                atom("H", [0.96, 0.0, 0.0]),
                atom("H", [-0.24, 0.93, 0.0]),
            ],
            bonds: vec![
                Bond3d {
                    begin: 0,
                    end: 1,
                    order: 1,
                },
                Bond3d {
                    begin: 0,
                    end: 2,
                    order: 1,
                },
            ],
            force_field: three_d::ForceField::Uff,
            energy: 0.0,
            energy_unit: String::new(),
            formula: "H2O".to_owned(),
            molfile: String::new(),
        }
    }

    /// 分子集合体を作成: without a 3D model it says how to make one; with one it becomes the
    /// first monomer (once), the assembly view is shown, and the edit menu's undo acts on the
    /// assembly there. The panel and viewer draw headless.
    #[test]
    fn create_assembly_starts_from_the_3d_model_in_the_assembly_view() {
        let ctx = egui::Context::default();
        let mut app = app_with_one_untitled();
        app.run(&ctx, Command::SetView(ViewMode::Assembly));
        assert_eq!(app.view, ViewMode::Assembly);
        app.run(&ctx, Command::SetView(ViewMode::TwoD));

        app.run(&ctx, Command::CreateAssembly);
        assert_eq!(app.view, ViewMode::Assembly);
        assert!(app.status.contains("Generate 3D from 2D"), "{}", app.status);
        assert!(app.docs[0].assembly.is_empty());

        let drawing = app.docs[0].editor.molecule.clone();
        app.docs[0].three_d = three_d::Job::with_model(water_model(), &drawing);
        app.view = ViewMode::ThreeD;
        assert!(app.menu_state().can_create_assembly);
        app.run(&ctx, Command::CreateAssembly);
        assert_eq!(app.view, ViewMode::Assembly);
        let design = &app.docs[0].assembly.design;
        assert_eq!(design.components.len(), 1);
        assert_eq!(design.components[0].name, "H2O");
        assert_eq!(design.components[0].atoms.len(), 3);
        app.run(&ctx, Command::CreateAssembly);
        assert_eq!(
            app.docs[0].assembly.design.components.len(),
            1,
            "added once"
        );

        // Undo from the menu undoes the assembly, not the drawing.
        assert!(app.menu_state().can_undo);
        app.run(&ctx, Command::Undo);
        assert!(app.docs[0].assembly.is_empty());
        app.run(&ctx, Command::Redo);
        assert_eq!(app.docs[0].assembly.design.components.len(), 1);

        app.docs[0]
            .assembly
            .edit(|d| d.steps.push(assembly::Step::new_ring()));
        let mut commands = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui::assembly::panel(
                ui,
                app.docs.first_mut(),
                &mut app.assembly_view,
                &mut commands,
            );
            ui::assembly::viewer_area(
                ui,
                "test",
                app.docs.first_mut(),
                &mut app.assembly_view,
                None,
            );
        });
        output.textures_delta.clear();
        assert!(commands.is_empty());
        assert_eq!(app.docs[0].assembly.built().unwrap().atoms.len(), 18);
    }

    /// A saved `.odasm` opens into the active document's assembly, as one undo step.
    #[test]
    fn an_assembly_file_opens_into_the_active_document() {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join(format!("odc-app-asm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rosette.odasm");
        let design = assembly::Design {
            components: vec![assembly::Component::from_model("water", &water_model())],
            steps: vec![assembly::Step::new_ring(), assembly::Step::new_stack()],
            ..Default::default()
        };
        assembly::file::save(&path, &design).unwrap();

        let mut app = app_with_one_untitled();
        app.open_assembly_path(&path);
        assert_eq!(app.view, ViewMode::Assembly);
        assert_eq!(app.docs[0].assembly.design, design);
        assert_eq!(app.docs[0].assembly.built().unwrap().molecule_count(), 60);
        app.run(&ctx, Command::Undo);
        assert!(app.docs[0].assembly.is_empty());

        app.open_assembly_path(&dir.join("missing.odasm"));
        assert!(app.status.contains("Could not open"), "{}", app.status);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A saved assembly view (and its display options) loads as saved.
    #[test]
    fn the_assembly_view_and_its_options_are_remembered() {
        let storage = SavedState(
            "(view: Assembly, assembly_view: (style: Wireframe, color: Molecule))".to_owned(),
        );
        let app: OpenDrawApp = eframe::get_value(&storage, eframe::APP_KEY).unwrap();
        assert_eq!(app.view, ViewMode::Assembly);
        assert_eq!(app.assembly_view.color, ui::assembly::ColorMode::Molecule);
        assert_eq!(
            app.assembly_view.style,
            crate::ui::viewport3d::Style3d::Wireframe
        );
    }

    /// The canvas's 3D化 is disabled with nothing drawn.
    #[test]
    fn make_3d_needs_a_drawing() {
        let app = app_with_one_untitled();
        let editor = &app.docs[0].editor;
        assert_eq!(three_d::part::target(&editor.molecule, [], None), None);
    }

    /// The language list (behind the toolbar's globe button and in the View menu) offers every
    /// language; choosing one switches the UI to it, and the choice is saved.
    #[test]
    fn choosing_a_language_switches_the_ui_and_is_remembered() {
        let ctx = egui::Context::default();
        let mut app = OpenDrawApp::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };

        // A headless frame of the toolbar (with its globe button) and of the language list,
        // whose rows are the languages in order.
        let mut out = Vec::new();
        let mut list = egui::Rect::NOTHING;
        let frame = |events, out: &mut Vec<Command>, list: &mut egui::Rect| {
            let state = app.menu_state();
            let mut output = ctx.run_ui(input(events), |ui| {
                ui::menu::toolbar(ui, &state, out);
                let top = ui.next_widget_position();
                ui.vertical(|ui| ui::menu::language_list(ui, state.language, out));
                *list = egui::Rect::from_min_max(top, ui.min_rect().max);
            });
            output.textures_delta.clear();
        };
        frame(Vec::new(), &mut out, &mut list);
        assert!(out.is_empty());
        assert!(list.height() > 0.0);

        // Click the row of Deutsch.
        let index = Lang::ALL.iter().position(|&l| l == Lang::De).unwrap();
        let row = list.height() / Lang::ALL.len() as f32;
        let pos = egui::pos2(list.left() + 8.0, list.top() + row * (index as f32 + 0.5));
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(pos)], &mut out, &mut list);
        frame(vec![press(true)], &mut out, &mut list);
        frame(vec![press(false)], &mut out, &mut list);
        assert_eq!(out, vec![Command::SetLanguage(Lang::De)]);

        for command in out {
            app.run(&ctx, command);
        }
        assert_eq!(app.language, Some(Lang::De));
        assert_eq!(crate::i18n::language(), Lang::De);
        assert_eq!(app.menu_state().language, Lang::De);

        let mut storage = SavedState(String::new());
        eframe::set_value(&mut storage, eframe::APP_KEY, &app);
        assert!(storage.0.contains("\"de\""), "{}", storage.0);
        let back: OpenDrawApp = eframe::get_value(&storage, eframe::APP_KEY).unwrap();
        assert_eq!(back.language, Some(Lang::De));
    }
}
