//! The user's own fragments: where they are kept, and adding, renaming and deleting them.
//!
//! chembuider-rs's [`Config::load`] reads fragments from `fragments/` under the *current
//! directory*, and [`ChemStructEditor::save_selection_as_fragment`] writes there too — which for
//! an installed app is wherever it happened to be started from. The app keeps them in a
//! per-user directory instead ([`default_dir`]), one `<name>.json` per fragment in the
//! library's own [`FragmentDef`] format, and lays them over the built-in ones by name, as the
//! library's loader does.
//!
//! Between the two sits the app's own database ([`app_fragments`]): the files of
//! `assets/fragments/` in this repository, compiled into the binary. By name, a fragment of
//! the library is replaced by the app's, and either by the user's.
//!
//! A label can also be a formula written with abbreviations that are never drawn — the ligands
//! of `PdCl2(amphos)2`. Those are the app's formula-only groups ([`formula_only_groups`],
//! `assets/formula_groups.toml`); together with the fragments they are what the library reads
//! a label's formula and molecular weight with ([`install_formula_groups`]).

use std::path::{Path, PathBuf};

use chembuider_rs::ChemStructEditor;
use chembuider_rs::config::{Config, FragAtomDef, FragBondDef, FragmentDef};
use chembuider_rs::molecule::fragment::Fragment;
use chembuider_rs::reaction::{FormulaGroup, LabelFormulaError, MolecularFormula};

/// Where a listed fragment comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Compiled into chembuider-rs; read-only.
    Builtin,
    /// Compiled into the app from `assets/fragments/`; read-only.
    App,
    /// A file in the user's fragment directory.
    User,
    /// A user file named like a built-in or app fragment, which it replaces.
    UserOverride,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Builtin => t!("fragments.source.builtin"),
            Source::App => t!("fragments.source.app"),
            Source::User => t!("fragments.source.user"),
            Source::UserOverride => t!("fragments.source.user_override"),
        }
    }

    pub fn is_user(self) -> bool {
        matches!(self, Source::User | Source::UserOverride)
    }
}

// `APP_FRAGMENT_FILES`: (file name, contents) of every `assets/fragments/*.json`, sorted by
// file name; written by `build.rs`.
include!(concat!(env!("OUT_DIR"), "/app_fragments.rs"));

/// The app's own fragments: the files of `assets/fragments/`, compiled in. A file that is not
/// a valid fragment is skipped with a warning (a test keeps one from being committed).
pub fn app_fragments() -> Vec<FragmentDef> {
    APP_FRAGMENT_FILES
        .iter()
        .filter_map(|(file, text)| match parse_def(text) {
            Ok(def) => Some(def),
            Err(e) => {
                eprintln!("warn: skipped the bundled fragment {file}: {e}");
                None
            }
        })
        .collect()
}

/// `assets/formula_groups.toml`: the abbreviations with a formula and no structure.
const FORMULA_GROUPS: &str = include_str!("../assets/formula_groups.toml");

#[derive(serde::Deserialize)]
struct FormulaGroupFile {
    group: Vec<FormulaGroupEntry>,
}

/// One `[[group]]` of `assets/formula_groups.toml` (its `note` is for readers of the file).
#[derive(serde::Deserialize)]
struct FormulaGroupEntry {
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    formula: String,
}

/// The app's formula-only groups: ligands and reagents a label may name (`dppf`, `cod`,
/// `TMEDA`) that have a formula but no structure to expand into. An entry that does not read
/// is skipped with a warning (a test keeps one from being committed).
pub fn formula_only_groups() -> Vec<FormulaGroup> {
    let file: FormulaGroupFile = match toml::from_str(FORMULA_GROUPS) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("warn: assets/formula_groups.toml does not parse: {e}");
            return Vec::new();
        }
    };
    file.group
        .iter()
        .filter_map(|entry| {
            let aliases: Vec<&str> = entry.aliases.iter().map(String::as_str).collect();
            let group = FormulaGroup::written(&entry.name, &aliases, &entry.formula);
            if group.is_none() {
                eprintln!("warn: skipped the formula group {}", entry.name);
            }
            group
        })
        .collect()
}

/// The groups a label's formula is read with: the formula-only ones, then `fragments` (the
/// layered list, [`FragmentStore::fragments`]) as the substituents they expand into — so a
/// fragment, the user's included, has a name both of them use.
pub fn formula_groups(fragments: &[FragmentDef]) -> Vec<FormulaGroup> {
    let mut groups = formula_only_groups();
    groups.extend(chembuider_rs::reaction::fragment_groups(fragments));
    groups
}

/// Make [`formula_groups`] of `fragments` the groups every formula and molecular weight in
/// the process is read with (the status bar, the analysis window, the reaction table).
pub fn install_formula_groups(fragments: &[FragmentDef]) {
    chembuider_rs::reaction::set_formula_groups(formula_groups(fragments));
}

/// What to tell the user when Expand left `label` as it was: that it is a formula, with its
/// composition and weight (salts and complexes are kept as formulas, not drawn); which group
/// in it is unknown; that its brackets do not match; or that it stands for no structure.
pub fn not_expanded_message(label: &str) -> String {
    match MolecularFormula::of_label(label) {
        Ok(formula) => match formula.weight() {
            Some(weight) => t!(
                "app.status.label_is_formula",
                label = label,
                formula = formula.plain(),
                weight = format!("{weight:.2}")
            ),
            None => t!("app.status.label_not_expanded", label = label),
        },
        Err(LabelFormulaError::UnknownGroup(group)) => {
            t!(
                "app.status.label_unknown_group",
                label = label,
                group = group
            )
        }
        Err(LabelFormulaError::UnbalancedBrackets) => {
            t!("app.status.label_unbalanced", label = label)
        }
        Err(LabelFormulaError::NotAFormula) => t!("app.status.label_not_expanded", label = label),
    }
}

/// One fragment as the manager lists it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub def: FragmentDef,
    pub source: Source,
}

/// A fragment read from the user directory, and the file it came from.
#[derive(Debug, Clone)]
struct UserFile {
    def: FragmentDef,
    path: PathBuf,
}

/// The per-user fragment directory: `%APPDATA%\OpenDrawChemistry\fragments` on Windows,
/// `~/Library/Application Support/OpenDrawChemistry/fragments` on macOS and
/// `~/.config/open-draw-chemistry/fragments` elsewhere. `None` when the OS reports no
/// configuration directory.
pub fn default_dir() -> Option<PathBuf> {
    app_config_dir().map(|d| d.join("fragments"))
}

/// The app's per-user configuration directory: `%APPDATA%\OpenDrawChemistry` on Windows,
/// `~/Library/Application Support/OpenDrawChemistry` on macOS and
/// `~/.config/open-draw-chemistry` elsewhere.
pub fn app_config_dir() -> Option<PathBuf> {
    let app = if cfg!(any(windows, target_os = "macos")) {
        "OpenDrawChemistry"
    } else {
        "open-draw-chemistry"
    };
    dirs::config_dir().map(|d| d.join(app))
}

/// The user's fragments, in one directory (injectable, so tests never touch the real one).
#[derive(Debug, Clone)]
pub struct FragmentStore {
    dir: Option<PathBuf>,
}

impl Default for FragmentStore {
    /// The store at [`default_dir`].
    fn default() -> Self {
        Self { dir: default_dir() }
    }
}

impl FragmentStore {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into()),
        }
    }

    /// The directory, which may not exist yet (it is created on the first save).
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    fn require_dir(&self) -> Result<&Path, String> {
        self.dir()
            .ok_or_else(|| t!("fragments.error.no_config_dir").to_owned())
    }

    /// Every valid fragment file in the directory, sorted by file name. A file that does not
    /// parse, or that the library's loader would refuse (bad indices, a name that is not a
    /// plain file name), is skipped like the library skips it, with a warning on stderr: its
    /// fragment is otherwise just missing, and a label of its name does not expand.
    fn user_files(&self) -> Vec<UserFile> {
        let Some(dir) = self.dir() else {
            return Vec::new();
        };
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            // Not there until the first save.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                eprintln!(
                    "warn: cannot read the fragment folder {}: {e}",
                    dir.display()
                );
                return Vec::new();
            }
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json") && p.is_file())
            .collect();
        paths.sort();
        paths
            .into_iter()
            .filter_map(|path| match read_def(&path) {
                Ok(def) => Some(UserFile { def, path }),
                Err(e) => {
                    eprintln!("warn: skipped the fragment file {}: {e}", path.display());
                    None
                }
            })
            .collect()
    }

    /// The user's fragments.
    pub fn user_fragments(&self) -> Vec<FragmentDef> {
        self.user_files().into_iter().map(|f| f.def).collect()
    }

    /// The built-in fragments with the app's and then the user's laid over them by name: what
    /// the editor's [`Config::fragments`] should hold.
    pub fn fragments(&self) -> Vec<FragmentDef> {
        self.entries().into_iter().map(|e| e.def).collect()
    }

    /// [`FragmentStore::fragments`], each with where it comes from.
    pub fn entries(&self) -> Vec<Entry> {
        let mut entries: Vec<Entry> = Config::embedded()
            .fragments
            .into_iter()
            .map(|def| Entry {
                def,
                source: Source::Builtin,
            })
            .collect();
        for def in app_fragments() {
            let entry = Entry {
                def,
                source: Source::App,
            };
            match entries.iter_mut().find(|e| e.def.name == entry.def.name) {
                Some(slot) => *slot = entry,
                None => entries.push(entry),
            }
        }
        for def in self.user_fragments() {
            match entries.iter_mut().find(|e| e.def.name == def.name) {
                Some(slot) => {
                    // A later file with the same name wins, as in the library's merge.
                    if !slot.source.is_user() {
                        slot.source = Source::UserOverride;
                    }
                    slot.def = def;
                }
                None => entries.push(Entry {
                    def,
                    source: Source::User,
                }),
            }
        }
        entries
    }

    /// The file holding the user fragment `name`, if there is one.
    fn file_of(&self, name: &str) -> Option<PathBuf> {
        self.user_files()
            .into_iter()
            .find(|f| f.def.name == name)
            .map(|f| f.path)
    }

    /// Refuse `name` when another user fragment differs from it only in case: on Windows and
    /// macOS their files would be the same file.
    fn check_case_clash(&self, name: &str, except: Option<&str>) -> Result<(), String> {
        match self.user_fragments().into_iter().find(|d| {
            d.name != name && Some(d.name.as_str()) != except && d.name.eq_ignore_ascii_case(name)
        }) {
            Some(other) => Err(t!("fragments.error.case_clash", name = other.name)),
            None => Ok(()),
        }
    }

    /// Write `def` as a user fragment, replacing a user fragment of the same name. Returns the
    /// file written.
    pub fn save(&self, def: &FragmentDef) -> Result<PathBuf, String> {
        check_name(&def.name)?;
        def.validate()
            .map_err(|e| t!("fragments.error.invalid_structure", error = e))?;
        self.check_case_clash(&def.name, None)?;
        let path = match self.file_of(&def.name) {
            Some(path) => path,
            None => {
                let dir = self.require_dir()?;
                std::fs::create_dir_all(dir)
                    .map_err(|e| t!("fragments.error.create_folder", error = e))?;
                dir.join(format!("{}.json", def.name))
            }
        };
        write_def(&path, def)?;
        Ok(path)
    }

    /// Delete the user fragment `name`. A built-in or app fragment it overrode comes back.
    pub fn delete(&self, name: &str) -> Result<(), String> {
        let path = self
            .file_of(name)
            .ok_or_else(|| t!("fragments.error.not_found", name = name))?;
        std::fs::remove_file(path).map_err(|e| t!("fragments.error.delete", error = e))
    }

    /// Rename the user fragment `old` to `new`, renaming its file to `<new>.json`.
    pub fn rename(&self, old: &str, new: &str) -> Result<(), String> {
        let new = new.trim();
        check_name(new)?;
        if old == new {
            return Ok(());
        }
        let file = self
            .user_files()
            .into_iter()
            .find(|f| f.def.name == old)
            .ok_or_else(|| t!("fragments.error.not_found", name = old))?;
        if self.file_of(new).is_some() {
            return Err(t!("fragments.error.exists", name = new));
        }
        self.check_case_clash(new, Some(old))?;
        let target = file
            .path
            .parent()
            .expect("a fragment file is in a directory")
            .join(format!("{new}.json"));
        // A case-only rename on a case-insensitive file system is the same file; `rename`
        // handles it, but the "already exists" check must not refuse it.
        let same_file = old.eq_ignore_ascii_case(new);
        if target.exists() && !same_file {
            return Err(t!(
                "fragments.error.file_exists",
                file = target.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
        let mut def = file.def;
        def.name = new.to_owned();
        // Rewrite in place first, then move: a failed move leaves one valid file, never none.
        write_def(&file.path, &def)?;
        std::fs::rename(&file.path, &target).map_err(|e| t!("fragments.error.rename", error = e))
    }
}

/// The fragment in the file `path`, if it is one the library's loader would take.
fn read_def(path: &Path) -> Result<FragmentDef, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse_def(&text)
}

/// The fragment the JSON `text` holds, if it is one the library's loader would take.
fn parse_def(text: &str) -> Result<FragmentDef, String> {
    let def: FragmentDef = serde_json::from_str(text).map_err(|e| e.to_string())?;
    def.validate()?;
    Fragment::validate_name(&def.name)?;
    Ok(def)
}

fn write_def(path: &Path, def: &FragmentDef) -> Result<(), String> {
    let json = serde_json::to_string_pretty(def).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| t!("fragments.error.save", error = e))
}

/// [`Fragment::validate_name`], with the library's (English) reason under a Japanese message.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err(t!("fragments.error.empty_name").to_owned());
    }
    Fragment::validate_name(name).map_err(|e| t!("fragments.error.invalid_name", error = e))
}

/// The editor's selection as a fragment named `name`, by the rules of
/// [`ChemStructEditor::save_selection_as_fragment`]: the attach atom is the hotspot atom when
/// it is selected, otherwise the lowest-id selected atom; only bonds with both ends selected
/// are kept; positions are relative to the attach atom.
///
/// (That method also writes `fragments/<name>.json` under the current directory, so it cannot
/// be called as is; this builds the same definition without the write.)
pub fn selection_fragment(editor: &ChemStructEditor, name: &str) -> Result<FragmentDef, String> {
    let name = name.trim();
    check_name(name)?;
    let mut ids: Vec<u32> = editor.selected_atoms.iter().copied().collect();
    if ids.is_empty() {
        return Err(t!("fragments.error.no_selection").to_owned());
    }
    ids.sort_unstable();
    let attach_id = match editor.hotspot_atom {
        Some(h) if ids.contains(&h) => h,
        _ => ids[0],
    };
    let attach_idx = ids.iter().position(|&i| i == attach_id).unwrap_or(0);
    let atom = |id: u32| {
        editor
            .molecule
            .atom_by_id(id)
            .ok_or_else(|| t!("fragments.error.atom_missing").to_owned())
    };
    let base = atom(attach_id)?.pos;
    let atoms = ids
        .iter()
        .map(|&id| {
            let a = atom(id)?;
            Ok(FragAtomDef {
                element: a.element.clone(),
                pos: [a.pos[0] - base[0], a.pos[1] - base[1]],
                charge: a.charge,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let bonds = editor
        .molecule
        .bonds
        .iter()
        .filter_map(|b| {
            Some(FragBondDef {
                begin: ids.iter().position(|&i| i == b.begin)?,
                end: ids.iter().position(|&i| i == b.end)?,
                order: b.order.clone(),
            })
        })
        .collect();
    Ok(FragmentDef {
        name: name.to_owned(),
        atoms,
        bonds,
        attach_idx,
    })
}

/// Show `dir` in the OS file manager, creating it first.
pub fn open_in_file_manager(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| t!("fragments.error.create_folder", error = e))?;
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(dir)
        .spawn()
        .map(drop)
        .map_err(|e| t!("fragments.error.open_folder", error = e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chembuider_rs::BondOrder;

    /// A fresh, empty directory for one test.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("odc-fragments-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn two_atoms(name: &str) -> FragmentDef {
        let atom = |element: &str, x: f32| FragAtomDef {
            element: element.into(),
            pos: [x, 0.0],
            charge: 0,
        };
        FragmentDef {
            name: name.into(),
            atoms: vec![atom("C", 0.0), atom("O", 1.5)],
            bonds: vec![FragBondDef {
                begin: 0,
                end: 1,
                order: BondOrder::Single,
            }],
            attach_idx: 0,
        }
    }

    fn names(store: &FragmentStore) -> Vec<String> {
        store.user_fragments().into_iter().map(|d| d.name).collect()
    }

    #[test]
    fn save_creates_the_directory_and_loads_back() {
        let dir = temp_dir("save").join("nested");
        let store = FragmentStore::at(&dir);
        assert!(store.user_fragments().is_empty(), "a missing dir is empty");

        let path = store.save(&two_atoms("Mine")).unwrap();
        assert_eq!(path, dir.join("Mine.json"));
        assert_eq!(names(&store), ["Mine"]);
        let entry = store
            .entries()
            .into_iter()
            .find(|e| e.def.name == "Mine")
            .unwrap();
        assert_eq!(entry.source, Source::User);
        // The editor can resolve it by name.
        let cfg = Config {
            fragments: store.fragments(),
            ..Config::embedded()
        };
        assert_eq!(cfg.resolve_fragment("Mine").unwrap().atoms.len(), 2);
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    #[test]
    fn rename_moves_the_file_and_refuses_a_taken_name() {
        let dir = temp_dir("rename");
        let store = FragmentStore::at(&dir);
        store.save(&two_atoms("A")).unwrap();
        store.save(&two_atoms("B")).unwrap();

        assert!(store.rename("A", "B").is_err(), "B is taken");
        assert!(store.rename("A", "x/y").is_err(), "not a file name");
        assert!(store.rename("Nope", "C").is_err(), "no such fragment");

        store.rename("A", "C").unwrap();
        assert_eq!(names(&store), ["B", "C"]);
        assert!(!dir.join("A.json").exists());
        assert!(dir.join("C.json").exists());

        // A case-only rename is allowed (and is one file on Windows/macOS).
        store.rename("C", "c").unwrap();
        assert_eq!(names(&store), ["B", "c"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_removes_only_that_fragment() {
        let dir = temp_dir("delete");
        let store = FragmentStore::at(&dir);
        store.save(&two_atoms("A")).unwrap();
        store.save(&two_atoms("B")).unwrap();
        store.delete("A").unwrap();
        assert_eq!(names(&store), ["B"]);
        assert!(store.delete("A").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_user_fragment_overrides_the_builtin_until_deleted() {
        let dir = temp_dir("override");
        let store = FragmentStore::at(&dir);
        let builtin_atoms = |store: &FragmentStore| {
            store
                .entries()
                .into_iter()
                .find(|e| e.def.name == "OH")
                .map(|e| (e.source, e.def.atoms.len()))
        };
        let before = builtin_atoms(&store).unwrap();
        assert_eq!(before.0, Source::Builtin);

        let mut mine = two_atoms("OH");
        mine.atoms.push(FragAtomDef {
            element: "N".into(),
            pos: [3.0, 0.0],
            charge: 0,
        });
        store.save(&mine).unwrap();
        assert_eq!(builtin_atoms(&store), Some((Source::UserOverride, 3)));
        assert_eq!(
            store.fragments().iter().filter(|d| d.name == "OH").count(),
            1,
            "the override replaces the built-in rather than duplicating it"
        );

        store.delete("OH").unwrap();
        assert_eq!(builtin_atoms(&store), Some(before));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_bundled_file_is_a_fragment_named_like_its_file() {
        assert!(!APP_FRAGMENT_FILES.is_empty());
        for (file, text) in APP_FRAGMENT_FILES {
            let def = parse_def(text).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert_eq!(format!("{}.json", def.name), *file);
        }
        // Each file is listed once, whatever the library has under the same name.
        let store = FragmentStore::at(temp_dir("bundled"));
        for def in app_fragments() {
            let listed: Vec<Entry> = store
                .entries()
                .into_iter()
                .filter(|e| e.def.name == def.name)
                .collect();
            assert_eq!(listed.len(), 1, "{}", def.name);
            assert_eq!(listed[0].source, Source::App, "{}", def.name);
        }
    }

    #[test]
    fn a_user_fragment_overrides_the_app_one_until_deleted() {
        let dir = temp_dir("override-app");
        let store = FragmentStore::at(&dir);
        let listed = |store: &FragmentStore| {
            store
                .entries()
                .into_iter()
                .find(|e| e.def.name == "THF")
                .map(|e| (e.source, e.def.atoms.len()))
        };
        assert_eq!(listed(&store), Some((Source::App, 5)));
        store.save(&two_atoms("THF")).unwrap();
        assert_eq!(listed(&store), Some((Source::UserOverride, 2)));
        assert_eq!(
            store.fragments().iter().filter(|d| d.name == "THF").count(),
            1
        );
        store.delete("THF").unwrap();
        assert_eq!(listed(&store), Some((Source::App, 5)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn names_that_are_not_plain_file_names_are_refused() {
        let dir = temp_dir("names");
        let store = FragmentStore::at(&dir);
        for bad in [
            "",
            "  ",
            "../evil",
            "a/b",
            "a\\b",
            "C:x",
            "NUL",
            "trailing.",
        ] {
            assert!(check_name(bad).is_err(), "{bad:?}");
            assert!(store.save(&two_atoms(bad)).is_err(), "{bad:?}");
        }
        assert!(check_name("CO2tBu").is_ok());
        assert!(!dir.exists(), "nothing was written");
    }

    #[test]
    fn a_name_differing_only_in_case_is_refused() {
        let dir = temp_dir("case");
        let store = FragmentStore::at(&dir);
        store.save(&two_atoms("Foo")).unwrap();
        assert!(store.save(&two_atoms("foo")).is_err());
        store.save(&two_atoms("Bar")).unwrap();
        assert!(store.rename("Bar", "FOO").is_err());
        assert_eq!(names(&store), ["Bar", "Foo"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_files_in_the_directory_are_skipped() {
        let dir = temp_dir("invalid");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("junk.json"), "not json").unwrap();
        let mut bad = two_atoms("Bad");
        bad.attach_idx = 9;
        std::fs::write(dir.join("Bad.json"), serde_json::to_string(&bad).unwrap()).unwrap();
        let store = FragmentStore::at(&dir);
        store.save(&two_atoms("Good")).unwrap();
        assert_eq!(names(&store), ["Good"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_selection_becomes_a_fragment_attached_at_the_hotspot() {
        let mut editor = ChemStructEditor::default();
        let mol = &mut editor.molecule;
        let a = mol.add_atom("C".into(), [1.0, 1.0], 0);
        let b = mol.add_atom("O".into(), [2.5, 1.0], 0);
        let c = mol.add_atom("N".into(), [4.0, 1.0], 0);
        mol.add_bond(a, b, BondOrder::Double);
        mol.add_bond(b, c, BondOrder::Single);

        assert!(
            selection_fragment(&editor, "X").is_err(),
            "nothing selected"
        );

        editor.selected_atoms.extend([a, b]);
        let def = selection_fragment(&editor, " X ").unwrap();
        assert_eq!(def.name, "X");
        assert_eq!(def.attach_idx, 0, "lowest id without a selected hotspot");
        assert_eq!(def.atoms[0].pos, [0.0, 0.0]);
        assert_eq!(def.atoms[1].pos, [1.5, 0.0]);
        assert_eq!(
            def.bonds.len(),
            1,
            "the bond to the unselected N is dropped"
        );

        editor.hotspot_atom = Some(b);
        let def = selection_fragment(&editor, "X").unwrap();
        assert_eq!(def.attach_idx, 1);
        assert_eq!(def.atoms[1].pos, [0.0, 0.0]);

        assert!(selection_fragment(&editor, "../x").is_err());
    }
}
