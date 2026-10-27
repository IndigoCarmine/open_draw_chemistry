//! One open document: an editor tab and where it is saved.

use std::path::{Path, PathBuf};

use chembuider_rs::{ChemStructEditor, Config, Molecule};

use crate::io::{self, Format, Loaded};
use crate::three_d::Job;
use crate::three_d::read::{Format3d, Opened3d};

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
        }
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
        }
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
        let response = self.editor.show(ui);
        if let Some(fit) = self.auto_fit.as_mut().filter(|_| fitting) {
            fit.view = Some((self.editor.zoom, self.editor.pan));
        }
        response
    }

    /// Whether the structure differs from what was last opened or saved.
    pub fn is_dirty(&self) -> bool {
        self.editor.molecule != self.saved
    }

    /// An untitled document nobody has drawn in: opening a file replaces it.
    pub fn is_pristine(&self) -> bool {
        self.path.is_none()
            && self.editor.molecule.atoms.is_empty()
            && self.editor.undo_stack.is_empty()
    }

    /// The tab's title: the file name, or "Untitled N".
    pub fn title(&self) -> String {
        match self.path.as_deref().and_then(Path::file_name) {
            Some(name) => name.to_string_lossy().into_owned(),
            None => format!("無題 {}", self.untitled_number),
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
        assert_eq!(doc.title(), "無題 3");
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
}
