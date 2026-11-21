//! The user's custom nodes, kept between runs: one JSON file per node in
//! `<config>/assembly_nodes/`, beside the user's fragments (see [`crate::fragments`]). A file
//! also holds the custom nodes the node uses, so it can be added to any design on its own; the
//! same file is what is exported and imported (`.odnode`).

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::defs::{self, DefId, LibraryStamp, NodeDef, content_hash};

/// The extension of an exported custom node.
pub const EXTENSION: &str = "odnode";

const FORMAT: &str = "odasm-node";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct FileContents {
    format: String,
    version: u32,
    def: NodeDef,
    /// The custom nodes `def` uses, directly or not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deps: Vec<NodeDef>,
}

/// A custom node with those it uses.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub def: NodeDef,
    pub deps: Vec<NodeDef>,
    /// The file it was read from (none for one read from text).
    pub path: Option<PathBuf>,
}

/// `def` (with the custom nodes it uses, from `defs`) as the text of a library file.
pub fn to_string(def: &NodeDef, defs: &[NodeDef]) -> String {
    let deps = defs::graph_uses_all(&def.graph, defs)
        .into_iter()
        .filter_map(|id| defs::find(defs, &id).cloned())
        .map(|mut d| {
            d.stamp = None;
            d
        })
        .collect();
    let mut def = def.clone();
    def.stamp = None;
    let contents = FileContents {
        format: FORMAT.into(),
        version: VERSION,
        def,
        deps,
    };
    serde_json::to_string_pretty(&contents).expect("a custom node always serialises")
}

/// The custom node in a library file's text. The message is for the user (in English for a
/// damaged file, as the parser's).
pub fn from_str(text: &str) -> Result<Entry, String> {
    let contents: FileContents = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if contents.format != FORMAT {
        return Err(format!("not a custom node file ({})", contents.format));
    }
    if contents.version > VERSION {
        return Err(t!(
            "assembly.library.error.newer",
            version = contents.version,
            supported = VERSION
        ));
    }
    let mut all = contents.deps.clone();
    all.push(contents.def.clone());
    defs::validate(&all)?;
    Ok(Entry {
        def: contents.def,
        deps: contents.deps,
        path: None,
    })
}

/// The per-user directory of custom nodes. `None` when the OS reports no configuration
/// directory.
pub fn default_dir() -> Option<PathBuf> {
    crate::fragments::app_config_dir().map(|d| d.join("assembly_nodes"))
}

/// The user's custom nodes, in one directory (injectable, so tests never touch the real one).
#[derive(Debug, Clone)]
pub struct NodeLibrary {
    dir: Option<PathBuf>,
}

impl Default for NodeLibrary {
    fn default() -> Self {
        Self { dir: default_dir() }
    }
}

impl NodeLibrary {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into()),
        }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    fn require_dir(&self) -> Result<&Path, String> {
        self.dir()
            .ok_or_else(|| t!("fragments.error.no_config_dir").to_owned())
    }

    /// Every custom node in the directory, sorted by name. Files that do not read are skipped
    /// with a warning on stderr.
    pub fn list(&self) -> Vec<Entry> {
        let Some(dir) = self.dir() else {
            return Vec::new();
        };
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                eprintln!(
                    "warn: cannot read the custom node folder {}: {e}",
                    dir.display()
                );
                return Vec::new();
            }
        };
        let mut out: Vec<Entry> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json") && p.is_file())
            .filter_map(|path| {
                let read = std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|text| from_str(&text));
                match read {
                    Ok(mut entry) => {
                        entry.path = Some(path);
                        Some(entry)
                    }
                    Err(e) => {
                        eprintln!("warn: skipped the custom node file {}: {e}", path.display());
                        None
                    }
                }
            })
            .collect();
        out.sort_by(|a, b| {
            a.def
                .name
                .to_lowercase()
                .cmp(&b.def.name.to_lowercase())
                .then_with(|| a.def.id.cmp(&b.def.id))
        });
        out
    }

    /// The library's custom node with id `id`.
    pub fn get(&self, id: &DefId) -> Option<Entry> {
        self.list().into_iter().find(|e| &e.def.id == id)
    }

    /// Save `def` (using the custom nodes in `defs`), replacing the library's node of the same
    /// id. Its revision is raised past the library's. Returns the stamp a design's copy should
    /// carry now that it matches the library.
    pub fn save(&self, def: &NodeDef, defs: &[NodeDef]) -> Result<LibraryStamp, String> {
        let dir = self.require_dir()?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let existing = self.get(&def.id);
        let mut def = def.clone();
        def.revision = existing
            .as_ref()
            .map_or(def.revision, |e| e.def.revision.max(def.revision))
            + 1;
        let text = to_string(&def, defs);
        let path = match existing.and_then(|e| e.path) {
            Some(path) => path,
            None => dir.join(file_name(&def)),
        };
        write_atomically(dir, &path, &text)?;
        Ok(LibraryStamp {
            revision: def.revision,
            hash: content_hash(&def),
        })
    }

    /// Delete the library's node with id `id`.
    pub fn remove(&self, id: &DefId) -> Result<(), String> {
        match self.get(id).and_then(|e| e.path) {
            Some(path) => std::fs::remove_file(path).map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }
}

/// `<name>-<id>.json`, the name kept to letters, digits, `-` and `_`.
fn file_name(def: &NodeDef) -> String {
    let slug: String = def
        .name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let slug = if slug.is_empty() {
        "node".to_owned()
    } else {
        slug
    };
    format!("{slug}-{}.json", def.id.short())
}

fn write_atomically(dir: &Path, path: &Path, text: &str) -> Result<(), String> {
    let mut file = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Put the library's `entry` into a design's custom nodes: the node (stamped as matching the
/// library) and those it uses that the design lacks. A node the design already has is replaced
/// by the library's when `replace` (updating it), else left as it is.
pub fn adopt(defs: &mut Vec<NodeDef>, entry: &Entry, replace: bool) {
    let stamp = LibraryStamp {
        revision: entry.def.revision,
        hash: content_hash(&entry.def),
    };
    for dep in &entry.deps {
        if defs::find(defs, &dep.id).is_none() {
            defs.push(dep.clone());
        }
    }
    let mut def = entry.def.clone();
    def.stamp = Some(stamp);
    match defs::find_mut(defs, &def.id) {
        Some(slot) if replace => *slot = def,
        Some(_) => {}
        None => defs.push(def),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::graph::defs::Status;

    #[test]
    fn saving_listing_and_statuses() {
        let dir = tempfile::tempdir().unwrap();
        let lib = NodeLibrary::at(dir.path());
        assert!(lib.list().is_empty(), "no folder yet");
        let def = NodeDef::new("Rosette (6)");
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
