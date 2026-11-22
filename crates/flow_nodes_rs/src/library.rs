//! Custom nodes kept between runs: one JSON file per node in a directory the application
//! chooses. A file also holds the custom nodes the node uses, so it can be added to any graph on
//! its own; the same text is what an application exports and imports.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::def::{self, DefId, LibraryStamp, NodeDef, content_hash};
use crate::schema::NodeKind;

/// What a library file says it is: the application's tag and the newest version it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibraryFormat {
    /// Written as `"format"`: files with another tag are refused.
    pub tag: &'static str,
    /// Written as `"version"`: files of a newer version are refused.
    pub version: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
struct FileContents<K: NodeKind> {
    format: String,
    version: u32,
    def: NodeDef<K>,
    /// The custom nodes `def` uses, directly or not.
    #[serde(default = "Vec::new", skip_serializing_if = "Vec::is_empty")]
    deps: Vec<NodeDef<K>>,
}

/// A custom node with those it uses.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry<K: NodeKind> {
    pub def: NodeDef<K>,
    pub deps: Vec<NodeDef<K>>,
    /// The file it was read from (none for one read from text).
    pub path: Option<PathBuf>,
}

/// Why a library file or the library could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryError {
    /// Not JSON of a library file (the parser's message).
    Parse(String),
    /// A file of another format (its tag).
    Format(String),
    /// A file of a newer version than this application reads.
    Newer { version: u32, supported: u32 },
    /// The custom nodes do not hold together (see [`def::validate`]).
    Invalid(String),
    /// There is no directory to keep the library in.
    NoDir,
    /// Reading or writing failed.
    Io(String),
}

impl std::fmt::Display for LibraryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LibraryError::Parse(e) | LibraryError::Invalid(e) | LibraryError::Io(e) => {
                f.write_str(e)
            }
            LibraryError::Format(tag) => write!(f, "not a custom node file ({tag})"),
            LibraryError::Newer { version, supported } => write!(
                f,
                "the file is of version {version}; this program reads up to {supported}"
            ),
            LibraryError::NoDir => f.write_str("there is no folder to keep custom nodes in"),
        }
    }
}

impl std::error::Error for LibraryError {}

fn io(e: impl std::fmt::Display) -> LibraryError {
    LibraryError::Io(e.to_string())
}

/// `def` (with the custom nodes it uses, from `defs`) as the text of a library file.
pub fn to_string<K: NodeKind>(
    format: &LibraryFormat,
    def: &NodeDef<K>,
    defs: &[NodeDef<K>],
) -> String {
    let deps = def::graph_uses_all(&def.graph, defs)
        .into_iter()
        .filter_map(|id| def::find(defs, &id).cloned())
        .map(|mut d| {
            d.stamp = None;
            d
        })
        .collect();
    let mut def = def.clone();
    def.stamp = None;
    let contents = FileContents {
        format: format.tag.into(),
        version: format.version,
        def,
        deps,
    };
    serde_json::to_string_pretty(&contents).expect("a custom node always serialises")
}

/// The custom node in a library file's text.
pub fn from_str<K: NodeKind>(format: &LibraryFormat, text: &str) -> Result<Entry<K>, LibraryError> {
    let contents: FileContents<K> =
        serde_json::from_str(text).map_err(|e| LibraryError::Parse(e.to_string()))?;
    if contents.format != format.tag {
        return Err(LibraryError::Format(contents.format));
    }
    if contents.version > format.version {
        return Err(LibraryError::Newer {
            version: contents.version,
            supported: format.version,
        });
    }
    let mut all = contents.deps.clone();
    all.push(contents.def.clone());
    def::validate(&all).map_err(LibraryError::Invalid)?;
    Ok(Entry {
        def: contents.def,
        deps: contents.deps,
        path: None,
    })
}

/// Custom nodes in one directory (injectable, so tests never touch the real one).
#[derive(Debug, Clone)]
pub struct NodeLibrary {
    dir: Option<PathBuf>,
    format: LibraryFormat,
}

impl NodeLibrary {
    /// The library in `dir` (`None`: there is none, and saving fails with
    /// [`LibraryError::NoDir`]).
    pub fn new(dir: Option<PathBuf>, format: LibraryFormat) -> Self {
        Self { dir, format }
    }

    pub fn at(dir: impl Into<PathBuf>, format: LibraryFormat) -> Self {
        Self::new(Some(dir.into()), format)
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn format(&self) -> &LibraryFormat {
        &self.format
    }

    fn require_dir(&self) -> Result<&Path, LibraryError> {
        self.dir().ok_or(LibraryError::NoDir)
    }

    /// Every custom node in the directory, sorted by name. Files that do not read are skipped
    /// with a warning on stderr.
    pub fn list<K: NodeKind>(&self) -> Vec<Entry<K>> {
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
        let mut out: Vec<Entry<K>> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json") && p.is_file())
            .filter_map(|path| {
                let read = std::fs::read_to_string(&path)
                    .map_err(io)
                    .and_then(|text| from_str(&self.format, &text));
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
    pub fn get<K: NodeKind>(&self, id: &DefId) -> Option<Entry<K>> {
        self.list().into_iter().find(|e| &e.def.id == id)
    }

    /// Save `def` (using the custom nodes in `defs`), replacing the library's node of the same
    /// id. Its revision is raised past the library's. Returns the stamp a copy should carry now
    /// that it matches the library.
    pub fn save<K: NodeKind>(
        &self,
        def: &NodeDef<K>,
        defs: &[NodeDef<K>],
    ) -> Result<LibraryStamp, LibraryError> {
        let dir = self.require_dir()?;
        std::fs::create_dir_all(dir).map_err(io)?;
        let existing = self.get::<K>(&def.id);
        let mut def = def.clone();
        def.revision = existing
            .as_ref()
            .map_or(def.revision, |e| e.def.revision.max(def.revision))
            + 1;
        let text = to_string(&self.format, &def, defs);
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
    pub fn remove<K: NodeKind>(&self, id: &DefId) -> Result<(), LibraryError> {
        match self.get::<K>(id).and_then(|e| e.path) {
            Some(path) => std::fs::remove_file(path).map_err(io),
            None => Ok(()),
        }
    }
}

/// `<name>-<id>.json`, the name kept to letters, digits, `-` and `_`.
fn file_name<K: NodeKind>(def: &NodeDef<K>) -> String {
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

fn write_atomically(dir: &Path, path: &Path, text: &str) -> Result<(), LibraryError> {
    let mut file = tempfile::NamedTempFile::new_in(dir).map_err(io)?;
    file.write_all(text.as_bytes()).map_err(io)?;
    file.persist(path).map_err(io)?;
    Ok(())
}

/// Put the library's `entry` into a graph's custom nodes: the node (stamped as matching the
/// library) and those it uses that are missing. A node already there is replaced by the
/// library's when `replace` (updating it), else left as it is.
pub fn adopt<K: NodeKind>(defs: &mut Vec<NodeDef<K>>, entry: &Entry<K>, replace: bool) {
    let stamp = LibraryStamp {
        revision: entry.def.revision,
        hash: content_hash(&entry.def),
    };
    for dep in &entry.deps {
        if def::find(defs, &dep.id).is_none() {
            defs.push(dep.clone());
        }
    }
    let mut def = entry.def.clone();
    def.stamp = Some(stamp);
    match def::find_mut(defs, &def.id) {
        Some(slot) if replace => *slot = def,
        Some(_) => {}
        None => defs.push(def),
    }
}
