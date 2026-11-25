//! One open document: an editor tab and where it is saved.

use std::path::{Path, PathBuf};

use chembuider_rs::{ChemStructEditor, Config, Molecule};

use crate::assembly::AssemblyState;
use crate::io::{self, Format, Loaded};
use crate::ole_server::model3d::{Look3d, Object3d};
use crate::three_d::read::{Format3d, Opened3d};
use crate::three_d::{Atom3d, Bond3d, Job};
use crate::ui::viewport3d::Picture;

pub struct Document {
    pub editor: ChemStructEditor,
    /// Where the document was opened from or last saved to; `None` until it is saved.
    pub path: Option<PathBuf>,
    /// The format [`Document::path`] is written in.
    pub format: Format,
    /// The opened file's object tree as CDX bytes, so saving keeps what the editor cannot show.
    pub original: Option<Vec<u8>>,
    /// Objects in [`Document::original`] other than structures.
    pub other_objects: usize,
    /// The structure as last opened or saved: the document has unsaved changes while the
    /// editor's differs from it. (A snapshot rather than a flag, so undoing back to the saved
    /// state counts as unchanged, and every kind of edit is caught.)
    saved: Molecule,
    /// Keep a newly opened drawing fitted to its canvas until the user takes over the view
    /// (see [`Document::show`]).
    auto_fit: Option<AutoFit>,
    /// Shown as "Untitled N" until the document has a path.
    untitled_number: usize,
    /// The structure's 3D model, generated on demand.
    pub three_d: Job,
    /// Set when [`Document::path`] is a 3D structure file the document was read from. That
    /// file is never written: saving asks for a CDX/CDXML file instead.
    pub imported_3d: Option<Format3d>,
    /// The molecular assembly built from the document's monomers (the 集合体 view). Saved to
    /// its own `.odasm` file, not with the drawing.
    pub assembly: AssemblyState,
    /// Set when the document is an OLE object embedded in another application's document
    /// (Word, PowerPoint, …) and opened for editing here: saving writes back into that
    /// document rather than to a file (see [`crate::ole_server`]).
    pub embedded: Option<Embedded>,
    /// The last parameter scan of the assembly or the 3D model (パラメータスキャン), running or
    /// done.
    pub scan: Option<crate::scan::Run>,
    /// The last conformational search of the 3D model (配座探索), running or done.
    pub conformers: Option<crate::conformer::Run>,
    /// The last soft-core build of the 3D model or the assembly (ソフトコア最適化), running or
    /// done.
    pub softcore: Option<crate::softcore::Run>,
    /// Ctrl+V on the canvas, taken from the editor widget (see [`crate::ole::PasteShortcut`]).
    paste_shortcut: crate::ole::PasteShortcut,
    /// Ctrl+V was pressed on the canvas: the app pastes (see [`Document::take_paste_request`]).
    paste_requested: bool,
    /// The palette's ring and chain tools, which the editor widget does not have.
    pub canvas_tools: crate::ui::canvas_tools::CanvasTools,
}

/// The OLE object a document edits, and the document it is embedded in.
#[derive(Debug, Clone, PartialEq)]
pub struct Embedded {
    /// The object's id in the OLE server.
    pub id: u64,
    /// The containing application, shortened for the tab ("Word", "PowerPoint").
    pub container: String,
    /// The containing document's name, as the container reports it.
    pub document: String,
    /// Set when the object is a 3D model (the 3D view embedded): what is written back is the
    /// document's 3D model, shown as this says.
    pub model3d: Option<Embedded3d>,
}

/// An embedded 3D model's look, and the model and look as last opened or saved (what tells
/// an edit).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Embedded3d {
    /// How the model is shown now: the app keeps it up to date while the 3D view shows it.
    pub look: Look3d,
    saved_look: Look3d,
    saved_model: Option<(Vec<Atom3d>, Vec<Bond3d>)>,
}

impl Embedded3d {
    /// A model shown as `look`, unchanged from `model`.
    pub fn new(look: Look3d, model: Option<&crate::three_d::Model3d>) -> Self {
        Self {
            saved_look: look.clone(),
            look,
            saved_model: model.map(|m| (m.atoms.clone(), m.bonds.clone())),
        }
    }

    /// Whether `model` or the look differs from what was last saved. The view's size does
    /// not count (the window is not the object), nor a camera moved by less than rounding.
    fn changed(&self, model: Option<&crate::three_d::Model3d>) -> bool {
        let model = model.map(|m| (&m.atoms, &m.bonds));
        let saved = self.saved_model.as_ref().map(|(a, b)| (a, b));
        model != saved || self.look.differs_visibly(&self.saved_look)
    }

    fn mark_saved(&mut self, model: &crate::three_d::Model3d) {
        self.saved_look = self.look.clone();
        self.saved_model = Some((model.atoms.clone(), model.bonds.clone()));
    }
}

impl Document {
    /// An empty, untitled document.
    pub fn new(config: &Config, untitled_number: usize) -> Self {
        Self {
            editor: editor(config),
            path: None,
            format: Format::Cdxml,
            original: None,
            other_objects: 0,
            saved: Molecule::default(),
            auto_fit: None,
            untitled_number,
            three_d: Job::default(),
            imported_3d: None,
            assembly: AssemblyState::default(),
            embedded: None,
            scan: None,
            conformers: None,
            softcore: None,
            paste_shortcut: Default::default(),
            paste_requested: false,
            canvas_tools: Default::default(),
        }
    }

    /// A document holding what was read from `path`.
    pub fn from_loaded(config: &Config, path: &Path, format: Format, loaded: Loaded) -> Self {
        let mut editor = editor(config);
        editor.molecule = loaded.molecule.clone();
        Self {
            editor,
            path: Some(path.to_owned()),
            format,
            original: Some(loaded.original),
            other_objects: loaded.other_objects,
            saved: loaded.molecule,
            auto_fit: Some(AutoFit::default()),
            untitled_number: 0,
            three_d: Job::default(),
            imported_3d: None,
            assembly: AssemblyState::default(),
            embedded: None,
            scan: None,
            conformers: None,
            softcore: None,
            paste_shortcut: Default::default(),
            paste_requested: false,
            canvas_tools: Default::default(),
        }
    }

    /// An untitled document holding a structure made here (a built assembly): `opened`'s
    /// model as the 3D state and its 2D depiction in the editor, unchanged until edited.
    pub fn untitled_with_structure_3d(
        config: &Config,
        untitled_number: usize,
        opened: Opened3d,
    ) -> Self {
        let mut doc = Self::new(config, untitled_number);
        doc.editor.molecule = opened.drawing.clone();
        if let Some(model) = opened.model {
            doc.three_d = Job::with_model(model, &opened.drawing);
        }
        doc.saved = opened.drawing;
        doc.auto_fit = Some(AutoFit::default());
        doc
    }

    /// A document holding the structure read from the 3D file at `path`: the file's model as
    /// the 3D state and its 2D depiction in the editor. Saving goes to a new CDX/CDXML file.
    pub fn from_structure_3d(
        config: &Config,
        path: &Path,
        format: Format3d,
        opened: Opened3d,
    ) -> Self {
        let mut editor = editor(config);
        editor.molecule = opened.drawing.clone();
        let three_d = match opened.model {
            Some(model) => Job::with_model(model, &opened.drawing),
            None => Job::default(),
        };
        Self {
            editor,
            path: Some(path.to_owned()),
            format: Format::Cdxml,
            original: None,
            other_objects: 0,
            saved: opened.drawing,
            auto_fit: Some(AutoFit::default()),
            untitled_number: 0,
            three_d,
            imported_3d: Some(format),
            assembly: AssemblyState::default(),
            embedded: None,
            scan: None,
            conformers: None,
            softcore: None,
            paste_shortcut: Default::default(),
            paste_requested: false,
            canvas_tools: Default::default(),
        }
    }

    /// A document editing the embedded OLE object `embedded`, holding what was read from the
    /// object's CDX (`None` for a newly inserted, empty object).
    pub fn embedded(config: &Config, loaded: Option<Loaded>, embedded: Embedded) -> Self {
        let mut doc = Self::new(config, 0);
        if let Some(loaded) = loaded {
            doc.editor.molecule = loaded.molecule.clone();
            doc.saved = loaded.molecule;
            doc.original = Some(loaded.original);
            doc.other_objects = loaded.other_objects;
            doc.auto_fit = Some(AutoFit::default());
        }
        doc.format = Format::Cdx;
        doc.embedded = Some(embedded);
        doc
    }

    /// A document editing the embedded 3D model `embedded`, holding `object`'s model as its
    /// 3D model and a depiction of it in the editor (nothing for a newly inserted object).
    /// The error says why an object's MOL2 could not be read; the document is empty then.
    pub fn embedded_3d(
        config: &Config,
        object: Option<&Object3d>,
        mut embedded: Embedded,
    ) -> (Self, Option<String>) {
        let mut doc = Self::new(config, 0);
        let mut error = None;
        if let Some(object) = object {
            match object.open() {
                Ok(opened) => {
                    doc.editor.molecule = opened.drawing.clone();
                    if let Some(model) = opened.model {
                        doc.three_d = Job::with_model(model, &opened.drawing);
                    }
                    doc.saved = opened.drawing;
                    doc.auto_fit = Some(AutoFit::default());
                }
                Err(e) => error = Some(e.to_string()),
            }
        }
        let look = object.map(|o| o.look.clone()).unwrap_or_default();
        embedded.model3d = Some(Embedded3d::new(look, doc.three_d.model()));
        doc.embedded = Some(embedded);
        (doc, error)
    }

    /// The embedded 3D model's look, when the document is one.
    pub fn embedded_3d_mut(&mut self) -> Option<&mut Embedded3d> {
        self.embedded.as_mut().and_then(|e| e.model3d.as_mut())
    }

    /// The embedded 3D model to write back: the 3D model as MOL2, its look and `picture`
    /// (drawn from that look; without one the object keeps the picture it has). Marks it
    /// saved. An error when there is no model to write.
    pub fn commit_embedded_3d(&mut self, picture: Option<&Picture>) -> Result<Object3d, String> {
        let Some(model) = self.three_d.model() else {
            return Err(t!("app.status.no_3d_model_to_export").to_owned());
        };
        let Some(embedded) = self.embedded.as_mut().and_then(|e| e.model3d.as_mut()) else {
            return Err(t!("app.status.no_3d_model_to_export").to_owned());
        };
        let object =
            Object3d::new(model, embedded.look.clone(), picture).map_err(|e| e.to_string())?;
        embedded.look.extent = object.look.extent;
        embedded.mark_saved(model);
        self.saved = self.editor.molecule.clone();
        Ok(object)
    }

    /// The structure as CDX, for writing back into the embedding document: merged into the
    /// object's original CDX so what the editor cannot show is kept. Marks it saved.
    pub fn commit_embedded(&mut self) -> Result<Vec<u8>, io::Error> {
        let cdx = io::encode(Format::Cdx, &self.editor.molecule, self.original.as_deref())?;
        self.saved = self.editor.molecule.clone();
        Ok(cdx)
    }

    /// Draw the editor into `ui`, keeping a newly opened drawing fitted to the canvas.
    ///
    /// The canvas only settles over the first frames — the window's scale set at start-up
    /// applies from the second, and side panels find their size after that — and it changes
    /// again when the 3D view is shown or hidden. So the drawing is refitted whenever the canvas
    /// changes size, until the user zooms, pans or edits; from then on the view is theirs.
    pub fn show(&mut self, ui: &mut egui::Ui) -> chembuider_rs::EditorResponse {
        let size = ui.available_size();
        let first_pass = ui.ctx().cumulative_pass_nr() == 0;
        let mut fitting = false;
        if let Some(fit) = &mut self.auto_fit {
            let view = (self.editor.zoom, self.editor.pan);
            if fit.view.is_some_and(|v| v != view) || self.editor.molecule != self.saved {
                self.auto_fit = None;
            } else if !first_pass && fit.size != Some(size) {
                fit.size = Some(size);
                fitting = true;
                self.editor.request_fit();
            }
        }
        // Ctrl+C goes through the app's copy (an OLE object Office embeds), not the widget's.
        if crate::ole::take_copy_shortcut(ui.ctx()) {
            crate::ole::copy(&self.editor, ui.ctx());
        }
        // So does Ctrl+V: the app's paste also takes objects copied in Word/PowerPoint.
        if self.paste_shortcut.take(ui.ctx()) {
            self.paste_requested = true;
        }
        self.canvas_tools.before_show(&mut self.editor, ui.ctx());
        let mut response = self.editor.show(ui);
        self.canvas_tools
            .after_show(&mut self.editor, ui, &mut response);
        self.paste_shortcut.release(ui.ctx());
        if let Some(fit) = self.auto_fit.as_mut().filter(|_| fitting) {
            fit.view = Some((self.editor.zoom, self.editor.pan));
        }
        response
    }

    /// Whether Ctrl+V was pressed on the canvas since last asked; the caller pastes with
    /// [`crate::ole::paste`] (Edit > Paste), which reports to the status bar.
    pub fn take_paste_request(&mut self) -> bool {
        std::mem::take(&mut self.paste_requested)
    }

    /// Whether the structure differs from what was last opened or saved (and, for an embedded
    /// 3D model, the model or how it is shown).
    pub fn is_dirty(&self) -> bool {
        self.editor.molecule != self.saved
            || self
                .embedded
                .as_ref()
                .and_then(|e| e.model3d.as_ref())
                .is_some_and(|e| e.changed(self.three_d.model()))
    }

    /// Whether a job of this document is running that quitting would end. These results are
    /// not in the drawing, so [`Document::is_dirty`] says nothing about them.
    pub fn has_running_work(&self) -> bool {
        self.three_d.is_running()
            || self.scan.as_ref().is_some_and(crate::scan::Run::is_running)
            || self
                .conformers
                .as_ref()
                .is_some_and(crate::conformer::Run::is_running)
            || self
                .softcore
                .as_ref()
                .is_some_and(crate::softcore::Run::is_running)
    }

    /// An untitled document nobody has drawn in: opening a file replaces it.
    pub fn is_pristine(&self) -> bool {
        self.path.is_none() && self.editor.molecule.is_blank() && self.editor.undo_stack.is_empty()
    }

    /// The tab's title: the file name, or "Untitled N".
    pub fn title(&self) -> String {
        if let Some(e) = &self.embedded {
            return if e.document.is_empty() {
                t!("document.embedded", app = e.container)
            } else {
                t!(
                    "document.embedded_named",
                    app = e.container,
                    name = e.document
                )
            };
        }
        match self.path.as_deref().and_then(Path::file_name) {
            Some(name) => name.to_string_lossy().into_owned(),
            None => t!("document.untitled", number = self.untitled_number),
        }
    }

    /// Save to `path` in `format`, which become the document's from now on.
    pub fn save_as(&mut self, path: &Path, format: Format) -> Result<(), io::Error> {
        io::save(
            path,
            format,
            &self.editor.molecule,
            self.original.as_deref(),
        )?;
        self.path = Some(path.to_owned());
        self.format = format;
        self.imported_3d = None;
        self.saved = self.editor.molecule.clone();
        Ok(())
    }

    /// Save to the document's own path; `Ok(false)` when it has none yet, or only the 3D file
    /// it was read from (ask for one).
    pub fn save(&mut self) -> Result<bool, io::Error> {
        let Some(path) = self.path.clone().filter(|_| self.imported_3d.is_none()) else {
            return Ok(false);
        };
        self.save_as(&path, self.format)?;
        Ok(true)
    }
}

/// The state of [`Document::show`]'s automatic fit.
#[derive(Debug, Default)]
struct AutoFit {
    /// The canvas size the drawing was last fitted to.
    size: Option<egui::Vec2>,
    /// The zoom and pan that fit produced; any other means the user moved the view.
    view: Option<(f32, egui::Vec2)>,
}

fn editor(config: &Config) -> ChemStructEditor {
    let mut editor = ChemStructEditor::default();
    editor.config = config.clone();
    editor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untitled_documents_are_numbered() {
        let doc = Document::new(&Config::default(), 3);
        assert_eq!(doc.title(), "Untitled 3");
        assert!(!doc.is_dirty());
        assert!(doc.is_pristine());
    }

    #[test]
    fn saving_without_a_path_asks_for_one() {
        let mut doc = Document::new(&Config::default(), 1);
        assert!(!doc.save().unwrap());
    }

    #[test]
    fn save_as_names_the_document_and_clears_dirty() {
        let dir = std::env::temp_dir().join(format!("odc-doc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("drawn.cdx");

        let mut doc = Document::new(&Config::default(), 1);
        doc.editor.molecule.add_atom("C".to_owned(), [0.0, 0.0], 0);
        assert!(doc.is_dirty());
        doc.save_as(&path, Format::Cdx).unwrap();

        assert_eq!(doc.title(), "drawn.cdx");
        assert!(!doc.is_dirty());
        assert_eq!(io::load(&path).unwrap().molecule.atoms.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// One headless frame of `doc` on a canvas `width` × 600. egui debug-asserts that texture
    /// uploads are applied before a frame's output is dropped; there is no renderer here.
    fn frame(ctx: &egui::Context, doc: &mut Document, width: f32) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 600.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| {
            doc.show(ui);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn an_arrow_or_text_alone_makes_the_document_dirty() {
        let mut doc = Document::new(&Config::default(), 1);
        doc.editor.molecule.add_arrow(
            [0.0, 0.0],
            [4.5, 0.0],
            chembuider_rs::molecule::ArrowKind::Reaction,
        );
        assert!(doc.is_dirty());
        assert!(!doc.is_pristine());

        let mut doc = opened_reaction();
        assert!(!doc.is_dirty());
        if let Some(chembuider_rs::molecule::PageObject::Text(t)) = doc
            .editor
            .molecule
            .objects
            .iter_mut()
            .find(|o| o.as_text().is_some())
        {
            t.text = "(II)".to_owned();
        }
        assert!(doc.is_dirty(), "editing a caption is an edit");
    }

    fn embedded_3d_doc() -> (Document, Object3d) {
        let model = crate::ole_server::model3d::tests::ethanol();
        let look = Look3d {
            style: crate::ui::viewport3d::Style3d::Spheres,
            ..Look3d::default()
        };
        let object = Object3d::new(&model, look, None).unwrap();
        let embedded = Embedded {
            id: 9,
            container: "Word".into(),
            document: String::new(),
            model3d: None,
        };
        let (doc, error) = Document::embedded_3d(&Config::default(), Some(&object), embedded);
        assert_eq!(error, None);
        (doc, object)
    }

    #[test]
    fn an_embedded_3d_model_opens_as_the_documents_model_and_its_drawing() {
        let (doc, object) = embedded_3d_doc();
        let model = doc.three_d.model().expect("the model");
        assert_eq!(model.atoms.len(), 9);
        assert_eq!(doc.editor.molecule.atoms.len(), 3, "the heavy atoms drawn");
        assert!(!doc.three_d.is_stale(&doc.editor.molecule));
        assert!(!doc.is_dirty());
        let look = &doc
            .embedded
            .as_ref()
            .unwrap()
            .model3d
            .as_ref()
            .unwrap()
            .look;
        assert_eq!(*look, object.look);
        assert_eq!(doc.title(), "Embedded in Word");
    }

    #[test]
    fn editing_or_turning_an_embedded_model_is_an_edit_and_committing_saves_it() {
        let (mut doc, object) = embedded_3d_doc();
        // A view of another size is no edit.
        doc.embedded_3d_mut().unwrap().look.view_points = [999.0, 333.0];
        assert!(!doc.is_dirty());
        // A turned camera is.
        doc.embedded_3d_mut().unwrap().look.camera = Some(crate::ui::viewport3d::CameraState {
            center: [0.0; 3],
            rotation: [0.0, 1.0, 0.0, 0.0],
            radius: 2.0,
        });
        assert!(doc.is_dirty());
        let committed = doc.commit_embedded_3d(None).unwrap();
        assert!(!doc.is_dirty());
        assert_eq!(committed.mol2, object.mol2, "the same model");
        assert!(
            committed.png.is_empty(),
            "no picture drawn: the object keeps its own"
        );

        // Moving an atom is an edit too, and goes into the MOL2.
        let model = doc.three_d.model_mut().unwrap();
        let mut coords = crate::three_d::edit::coords(model);
        coords[0][0] += 0.5;
        crate::three_d::edit::set_coords(model, &coords);
        assert!(doc.is_dirty());
        let moved = doc.commit_embedded_3d(None).unwrap();
        assert!(!doc.is_dirty());
        assert_ne!(moved.mol2, object.mol2);
    }

    #[test]
    fn a_new_embedded_3d_object_has_nothing_to_commit_until_a_model_is_made() {
        let embedded = Embedded {
            id: 1,
            container: "PowerPoint".into(),
            document: String::new(),
            model3d: None,
        };
        let (mut doc, error) = Document::embedded_3d(&Config::default(), None, embedded);
        assert_eq!(error, None);
        assert!(doc.three_d.model().is_none());
        assert!(!doc.is_dirty());
        assert!(doc.commit_embedded_3d(None).is_err());
    }

    fn opened_reaction() -> Document {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/Reaction.cdx");
        let loaded = io::load(&path).unwrap();
        Document::from_loaded(&Config::default(), &path, Format::Cdx, loaded)
    }

    #[test]
    fn an_opened_drawing_follows_the_canvas_until_the_user_moves_the_view() {
        let ctx = egui::Context::default();
        let mut doc = opened_reaction();

        // The first pass is skipped; the drawing is fitted from the second on.
        for _ in 0..3 {
            frame(&ctx, &mut doc, 1200.0);
        }
        let wide = doc.editor.zoom;
        assert!(
            wide < 1.0,
            "a reaction this wide is shrunk to fit: zoom {wide}"
        );

        // The canvas narrows (the 3D view opens): the drawing is fitted again.
        for _ in 0..2 {
            frame(&ctx, &mut doc, 600.0);
        }
        assert!(doc.editor.zoom < wide, "refitted to the narrower canvas");

        // The user pans: a later resize leaves the view alone.
        doc.editor.pan += egui::vec2(30.0, 0.0);
        let panned = (doc.editor.zoom, doc.editor.pan);
        for _ in 0..2 {
            frame(&ctx, &mut doc, 1200.0);
        }
        assert_eq!((doc.editor.zoom, doc.editor.pan), panned);
    }

    /// An opened drawing with curly arrows: as it was read until the user edits it, nothing
    /// selected, whatever the pointer does over it that is no edit (it moves over the arrows,
    /// or a button comes up there whose press went to the open dialog).
    #[test]
    fn an_opened_drawing_with_curly_arrows_is_neither_edited_nor_selected() {
        use chembuider_rs::molecule::{BondOrder, CurlyArrow, CurlyEnd, CurlyKind, Molecule};
        let mut m = Molecule::default();
        let c = m.add_atom("C".into(), [3.0, 3.0], 0);
        let o = m.add_atom("O".into(), [3.0, 1.5], 0);
        let co = m.add_bond(c, o, BondOrder::Double);
        let h = m.add_atom("H".into(), [5.0, 0.2], 0);
        let nu = m.add_atom("O".into(), [6.2, 5.2], -1);
        m.add_curly_arrow(CurlyArrow::new(
            CurlyEnd::Bond { bond: co, onto: o },
            CurlyEnd::Atom(h),
            CurlyKind::Pair,
        ));
        m.add_curly_arrow(CurlyArrow::new(
            CurlyEnd::Atom(nu),
            CurlyEnd::Atom(c),
            CurlyKind::Pair,
        ));
        let dir = std::env::temp_dir().join(format!("odc-curly-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mechanism.cdxml");
        std::fs::write(&path, chembuider_rs::molecule::cdx::molecule_to_cdxml(&m)).unwrap();
        let loaded = io::load(&path).unwrap();
        let mut doc = Document::from_loaded(&Config::default(), &path, Format::Cdxml, loaded);
        let ctx = egui::Context::default();
        for _ in 0..4 {
            frame(&ctx, &mut doc, 1200.0);
        }
        assert!(!doc.is_dirty(), "opening is no edit");
        assert!(doc.editor.selected_objects.is_empty());
        // The pointer sweeps the canvas, and a lone button release comes.
        for k in 0..40 {
            let p = egui::pos2(30.0 * k as f32, 15.0 * k as f32);
            let events = vec![
                egui::Event::PointerMoved(p),
                egui::Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ];
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw, |ui| {
                doc.show(ui);
            });
            output.textures_delta.clear();
        }
        assert!(!doc.is_dirty(), "hovering is no edit");
        assert!(doc.editor.selected_objects.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
