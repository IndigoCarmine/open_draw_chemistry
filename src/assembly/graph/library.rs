//! The user's custom nodes, kept between runs: one JSON file per node in
//! `<config>/assembly_nodes/`, beside the user's fragments (see [`crate::fragments`]). A file
//! also holds the custom nodes the node uses, so it can be added to any design on its own; the
//! same file is what is exported and imported (`.odnode`). The library is the `flow_nodes_rs`
//! crate's ([`flow_nodes_rs::library`]); here it is placed and its errors worded.

use std::path::{Path, PathBuf};

use flow_nodes_rs::library::{self as flow, LibraryError, LibraryFormat};

use super::defs::{DefId, LibraryStamp, NodeDef};
use super::kinds::NodeKind;

/// The extension of an exported custom node.
pub const EXTENSION: &str = "odnode";

/// What the files say they are.
pub const FORMAT: LibraryFormat = LibraryFormat {
    tag: "odasm-node",
    version: 1,
};

/// A custom node with those it uses.
pub type Entry = flow::Entry<NodeKind>;

/// Why the library failed, in the UI language (in English for a damaged file, as the
/// parser's).
pub fn message(e: &LibraryError) -> String {
    match e {
        LibraryError::Newer { version, supported } => t!(
            "assembly.library.error.newer",
            version = version,
            supported = supported
        ),
        LibraryError::NoDir => t!("fragments.error.no_config_dir").to_owned(),
        e => e.to_string(),
    }
}

/// `def` (with the custom nodes it uses, from `defs`) as the text of a library file.
pub fn to_string(def: &NodeDef, defs: &[NodeDef]) -> String {
    flow::to_string(&FORMAT, def, defs)
}

/// The custom node in a library file's text. The message is for the user.
pub fn from_str(text: &str) -> Result<Entry, String> {
    flow::from_str(&FORMAT, text).map_err(|e| message(&e))
}

/// The per-user directory of custom nodes. `None` when the OS reports no configuration
/// directory.
pub fn default_dir() -> Option<PathBuf> {
    crate::fragments::app_config_dir().map(|d| d.join("assembly_nodes"))
}

/// The user's custom nodes, in one directory (injectable, so tests never touch the real one).
#[derive(Debug, Clone)]
pub struct NodeLibrary(flow::NodeLibrary);

impl Default for NodeLibrary {
    fn default() -> Self {
        Self(flow::NodeLibrary::new(default_dir(), FORMAT))
    }
}

impl NodeLibrary {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self(flow::NodeLibrary::at(dir, FORMAT))
    }

    pub fn dir(&self) -> Option<&Path> {
        self.0.dir()
    }

    /// Every custom node in the directory, sorted by name. Files that do not read are skipped
    /// with a warning on stderr.
    pub fn list(&self) -> Vec<Entry> {
        self.0.list()
    }

    /// The library's custom node with id `id`.
    pub fn get(&self, id: &DefId) -> Option<Entry> {
        self.0.get(id)
    }

    /// Save `def` (using the custom nodes in `defs`), replacing the library's node of the same
    /// id. Its revision is raised past the library's. Returns the stamp a design's copy should
    /// carry now that it matches the library.
    pub fn save(&self, def: &NodeDef, defs: &[NodeDef]) -> Result<LibraryStamp, String> {
        self.0.save(def, defs).map_err(|e| message(&e))
    }

    /// Delete the library's node with id `id`.
    pub fn remove(&self, id: &DefId) -> Result<(), String> {
        self.0.remove::<NodeKind>(id).map_err(|e| message(&e))
    }
}

/// Put the library's `entry` into a design's custom nodes: the node (stamped as matching the
/// library) and those it uses that the design lacks. A node the design already has is replaced
/// by the library's when `replace` (updating it), else left as it is.
pub fn adopt(defs: &mut Vec<NodeDef>, entry: &Entry, replace: bool) {
    flow::adopt(defs, entry, replace);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::graph::defs::{self, Status};

    #[test]
    fn saving_listing_and_statuses() {
        let dir = tempfile::tempdir().unwrap();
        let lib = NodeLibrary::at(dir.path());
        assert!(lib.list().is_empty(), "no folder yet");
        let def = defs::new_def("Rosette (6)");
        let stamp = lib.save(&def, &[]).unwrap();
        assert_eq!(stamp.revision, 1);
        let listed = lib.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].def.name, "Rosette (6)");

        // A design adopts it: in sync; the library's changes: an update is available.
        let mut design_defs = Vec::new();
        adopt(&mut design_defs, &listed[0], false);
        assert_eq!(
            defs::status(&design_defs[0], lib.get(&def.id).map(|e| e.def).as_ref()),
            Status::InSync
        );
        let mut changed = def.clone();
        changed.inputs[0].name = "in".into();
        lib.save(&changed, &[]).unwrap();
        let library_now = lib.get(&def.id).unwrap();
        assert_eq!(library_now.def.revision, 2);
        assert_eq!(
            defs::status(&design_defs[0], Some(&library_now.def)),
            Status::UpdateAvailable
        );
        adopt(&mut design_defs, &library_now, true);
        assert_eq!(
            defs::status(&design_defs[0], Some(&library_now.def)),
            Status::InSync
        );
        assert_eq!(lib.list().len(), 1, "saved over the same file");

        std::fs::write(dir.path().join("broken.json"), "{").unwrap();
        assert_eq!(lib.list().len(), 1, "a damaged file is skipped");
        lib.remove(&def.id).unwrap();
        assert!(lib.list().is_empty());
    }
}
