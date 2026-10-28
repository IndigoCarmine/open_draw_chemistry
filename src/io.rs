//! Reading and writing documents, independent of the UI.
//!
//! A document is kept in memory as a [`Molecule`] (its structures, and the reaction arrows and
//! texts on the page as [`Molecule::objects`]) plus, when it came from a file, that file's
//! ChemDraw object tree as CDX bytes. Saving merges the edited structures, arrows and texts
//! back into that tree ([`merge_into_cdx`]), so shapes, brackets, curved arrows and the other
//! objects the editor cannot show yet survive an open → edit → save round trip. CDXML is the same object tree written as XML, so
//! both formats share one path.
//!
//! 3D structure files (MOL2, PDB, XYZ, MOL, SDF) open too ([`Kind::Structure3d`], read by
//! [`load_3d`]), but only as a starting point: they are not saved back to (see
//! [`crate::three_d::read`]).

use std::fmt;
use std::path::Path;

use chembuider_rs::Molecule;
use chembuider_rs::molecule::cdx::{
    cdxml_document, cdxml_of_document, merge_into_cdx, molecule_to_cdx_document, molecule_to_cdxml,
    read_cdx, read_cdxml,
};
use chembuider_rs::molecule::cdx_codec;
use chembuider_rs::molecule::mol2::to_mol2_string;
use chembuider_rs::molecule::smiles::to_smiles;

use crate::three_d::read::{self as read_3d, Format3d, Opened3d};

/// A format a document can be opened from and saved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Format {
    /// ChemDraw binary (`.cdx`).
    Cdx,
    /// ChemDraw XML (`.cdxml`).
    Cdxml,
}

impl Format {
    pub const ALL: [Format; 2] = [Format::Cdx, Format::Cdxml];

    /// The format a path's extension names, ignoring case.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "cdx" => Some(Format::Cdx),
            "cdxml" => Some(Format::Cdxml),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Cdx => "cdx",
            Format::Cdxml => "cdxml",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Format::Cdx => "ChemDraw (CDX)",
            Format::Cdxml => "ChemDraw XML (CDXML)",
        }
    }
}

/// What kind of file a path is, by its extension: a document to open and save, or a 3D
/// structure that is only read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Document(Format),
    Structure3d(Format3d),
}

impl Kind {
    pub fn from_path(path: &Path) -> Option<Self> {
        Format::from_path(path)
            .map(Kind::Document)
            .or_else(|| Format3d::from_path(path).map(Kind::Structure3d))
    }
}

/// A one-way export of the structure (the file is not reopened as the document).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Export {
    Mol2,
    Smiles,
    /// Windows only: the picture is drawn by the Windows-only image backend.
    #[cfg(windows)]
    Png,
}

impl Export {
    pub fn all() -> &'static [Export] {
        &[
            Export::Mol2,
            Export::Smiles,
            #[cfg(windows)]
            Export::Png,
        ]
    }

    pub fn extension(self) -> &'static str {
        match self {
            Export::Mol2 => "mol2",
            Export::Smiles => "smi",
            #[cfg(windows)]
            Export::Png => "png",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Export::Mol2 => "Tripos MOL2",
            Export::Smiles => "SMILES",
            #[cfg(windows)]
            Export::Png => t!("io.export.png"),
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// The extension names no format this app reads or writes.
    UnknownFormat,
    /// The bytes are not a document of the expected format (or are too large to read).
    Unreadable(Format),
    /// Nothing is drawn, so there is nothing to export.
    Empty,
    /// The exporter could not produce the file.
    ExportFailed(Export),
    /// A 3D structure file could not be read.
    Structure3d(read_3d::Error),
    /// The format cannot say what the drawing's brackets (a repeat unit, a multiple group)
    /// mean, so writing the atoms as drawn would give the wrong compound.
    Brackets(Export),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::UnknownFormat => f.write_str(t!("io.error.unknown_format")),
            Error::Unreadable(format) => {
                f.write_str(&t!("io.error.unreadable", format = format.description()))
            }
            Error::Empty => f.write_str(t!("io.error.empty")),
            Error::ExportFailed(export) => {
                f.write_str(&t!("io.error.export_failed", format = export.description()))
            }
            Error::Structure3d(e) => write!(f, "{e}"),
            Error::Brackets(export) => {
                f.write_str(&t!("io.error.brackets", format = export.description()))
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// A document as read from a file.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub molecule: Molecule,
    /// The file's object tree as CDX bytes, to merge the edited structures back into.
    pub original: Vec<u8>,
    /// Objects other than structures and the arrows and texts the editor shows (shapes,
    /// brackets, curved arrows, reaction schemes, …): kept but not shown.
    pub other_objects: usize,
}

impl Loaded {
    /// What opening the file brought, for the status bar after 「… を開きました」: the arrows,
    /// texts and reaction tables that can be edited, and the objects that are kept but not
    /// shown. Empty when there are neither.
    pub fn summary(&self) -> String {
        let objects = &self.molecule.objects;
        let arrows = objects.iter().filter(|o| o.as_arrow().is_some()).count();
        let texts = objects.iter().filter(|o| o.as_text().is_some()).count();
        let tables = objects.iter().filter(|o| o.as_table().is_some()).count();
        let brackets = objects.iter().filter(|o| o.as_bracket().is_some()).count();
        let mut parts = Vec::new();
        if arrows > 0 {
            parts.push(tn!("io.summary.arrows", arrows));
        }
        if texts > 0 {
            parts.push(tn!("io.summary.texts", texts));
        }
        if tables > 0 {
            parts.push(tn!("io.summary.tables", tables));
        }
        if brackets > 0 {
            parts.push(tn!("io.summary.brackets", brackets));
        }
        let mut out = String::new();
        if !parts.is_empty() {
            out.push_str(&t!("io.summary.editable", items = join_list(&parts)));
        }
        if self.other_objects > 0 {
            if !out.is_empty() {
                out.push_str(t!("io.summary.sentence_separator"));
            }
            out.push_str(&tn!("io.summary.kept", self.other_objects));
        }
        out
    }
}

/// `items` as one list in the current language: "a, b and c" in English, 「a・b・c」 in
/// Japanese.
fn join_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [init @ .., last] => format!(
            "{}{}{last}",
            init.join(t!("io.summary.list_separator")),
            t!("io.summary.list_last_separator")
        ),
    }
}

/// Read a document from `path`, choosing the format by extension.
pub fn load(path: &Path) -> Result<Loaded, Error> {
    let format = Format::from_path(path).ok_or(Error::UnknownFormat)?;
    decode(format, &std::fs::read(path)?)
}

/// Read a document from the bytes of a file in `format`.
pub fn decode(format: Format, bytes: &[u8]) -> Result<Loaded, Error> {
    let unreadable = || Error::Unreadable(format);
    let original = match format {
        Format::Cdx => bytes.to_vec(),
        Format::Cdxml => {
            let xml = std::str::from_utf8(bytes).map_err(|_| unreadable())?;
            cdx_codec::write(&cdxml_document(xml).ok_or_else(unreadable)?)
        }
    };
    let import = match format {
        Format::Cdx => read_cdx(&original),
        // Read from the XML itself rather than the re-encoded CDX: the same tree, but this is
        // the path chembuider-rs tests for CDXML.
        Format::Cdxml => read_cdxml(std::str::from_utf8(bytes).map_err(|_| unreadable())?),
    }
    .ok_or_else(unreadable)?;
    Ok(Loaded {
        molecule: import.molecule,
        original,
        other_objects: import.other_objects,
    })
}

/// Read the first structure of the 3D file at `path` (see [`crate::three_d::read`]).
pub fn load_3d(path: &Path) -> Result<Opened3d, Error> {
    let format = Format3d::from_path(path).ok_or(Error::UnknownFormat)?;
    if std::fs::metadata(path)?.len() > read_3d::MAX_FILE_BYTES {
        return Err(Error::Structure3d(read_3d::Error::FileTooLarge));
    }
    let bytes = std::fs::read(path)?;
    read_3d::read(format, &String::from_utf8_lossy(&bytes)).map_err(Error::Structure3d)
}

/// The file bytes of `mol` in `format`, merged into `original` (see [`Loaded::original`]) when
/// the document came from a file.
pub fn encode(format: Format, mol: &Molecule, original: Option<&[u8]>) -> Result<Vec<u8>, Error> {
    match (format, original) {
        (Format::Cdx, Some(original)) => Ok(merge_into_cdx(original, mol)),
        (Format::Cdx, None) => Ok(molecule_to_cdx_document(mol)),
        (Format::Cdxml, Some(original)) => {
            let merged = merge_into_cdx(original, mol);
            let doc = cdx_codec::parse(&merged).ok_or(Error::Unreadable(Format::Cdx))?;
            Ok(cdxml_of_document(&doc).into_bytes())
        }
        (Format::Cdxml, None) => Ok(molecule_to_cdxml(mol).into_bytes()),
    }
}

/// Write `mol` to `path` in `format`; see [`encode`].
pub fn save(
    path: &Path,
    format: Format,
    mol: &Molecule,
    original: Option<&[u8]>,
) -> Result<(), Error> {
    std::fs::write(path, encode(format, mol, original)?)?;
    Ok(())
}

/// The bytes of `mol` exported as `export`.
pub fn export(export: Export, mol: &Molecule) -> Result<Vec<u8>, Error> {
    // A picture of arrows and text alone is still a picture; the chemical formats need atoms.
    let empty = match export {
        #[cfg(windows)]
        Export::Png => mol.is_blank(),
        _ => mol.atoms.is_empty(),
    };
    if empty {
        return Err(Error::Empty);
    }
    // The chemical formats would write the atoms as drawn: a repeat unit's open ends as
    // methyls, a multiple group once. Wrong chemistry is worse than none.
    if matches!(export, Export::Mol2 | Export::Smiles) && mol.brackets().next().is_some() {
        return Err(Error::Brackets(export));
    }
    match export {
        Export::Mol2 => Ok(to_mol2_string(mol).into_bytes()),
        Export::Smiles => Ok(format!("{}\n", to_smiles(mol)).into_bytes()),
        #[cfg(windows)]
        Export::Png => {
            chembuider_rs::molecule::image::molecule_to_png(mol).ok_or(Error::ExportFailed(export))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ethanol() -> Molecule {
        let mut mol = Molecule::default();
        let c1 = mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
        let c2 = mol.add_atom("C".to_owned(), [1.3, 0.75], 0);
        let o = mol.add_atom("O".to_owned(), [2.6, 0.0], 0);
        mol.add_bond(c1, c2, chembuider_rs::BondOrder::Single);
        mol.add_bond(c2, o, chembuider_rs::BondOrder::Single);
        mol
    }

    /// SMILES and MOL2 would write a bracketed drawing's atoms as drawn — wrong chemistry —
    /// so they refuse it; the ChemDraw formats keep the brackets.
    #[test]
    fn brackets_are_not_exported_to_smiles_or_mol2() {
        let mut mol = ethanol();
        let middle = mol.atoms[1].id;
        mol.add_bracket(&[middle].into(), chembuider_rs::molecule::BracketUsage::Sru);
        for export in [Export::Smiles, Export::Mol2] {
            let err = super::export(export, &mol).unwrap_err();
            assert!(matches!(err, Error::Brackets(_)), "{export:?}");
            assert!(err.to_string().contains("bracket"));
        }
        for format in [Format::Cdx, Format::Cdxml] {
            let bytes = encode(format, &mol, None).unwrap();
            let back = match format {
                Format::Cdx => chembuider_rs::molecule::cdx::read_cdx(&bytes),
                Format::Cdxml => {
                    chembuider_rs::molecule::cdx::read_cdxml(std::str::from_utf8(&bytes).unwrap())
                }
            }
            .unwrap();
            assert_eq!(back.molecule.brackets().count(), 1, "{format:?}");
            assert_eq!(back.other_objects, 0, "{format:?}: not a hidden object");
        }
    }

    #[test]
    fn format_follows_the_extension_in_any_case() {
        assert_eq!(Format::from_path(Path::new("a/b.CDX")), Some(Format::Cdx));
        assert_eq!(Format::from_path(Path::new("b.cdxml")), Some(Format::Cdxml));
        assert_eq!(Format::from_path(Path::new("b.mol")), None);
        assert_eq!(Format::from_path(Path::new("cdx")), None);
    }

    #[test]
    fn kind_routes_documents_and_3d_structures() {
        assert_eq!(
            Kind::from_path(Path::new("a.CDXML")),
            Some(Kind::Document(Format::Cdxml))
        );
        assert_eq!(
            Kind::from_path(Path::new("a.pdb")),
            Some(Kind::Structure3d(Format3d::Pdb))
        );
        assert_eq!(
            Kind::from_path(Path::new("a.sdf")),
            Some(Kind::Structure3d(Format3d::Sdf))
        );
        assert_eq!(Kind::from_path(Path::new("a.png")), None);
    }

    #[test]
    fn a_new_structure_survives_both_formats() {
        let mol = ethanol();
        for format in Format::ALL {
            let bytes = encode(format, &mol, None).unwrap();
            let back = decode(format, &bytes).unwrap();
            assert_eq!(back.molecule.atoms.len(), 3, "{format:?}");
            assert_eq!(back.molecule.bonds.len(), 2, "{format:?}");
            assert_eq!(back.other_objects, 0, "{format:?}");
        }
    }

    #[test]
    fn drawn_arrows_and_text_survive_both_formats() {
        let mut mol = ethanol();
        mol.add_arrow(
            [4.0, 0.0],
            [8.5, 0.0],
            chembuider_rs::molecule::ArrowKind::Reaction,
        );
        mol.add_arrow(
            [4.0, 2.0],
            [8.5, 2.0],
            chembuider_rs::molecule::ArrowKind::Equilibrium,
        );
        mol.add_text([4.5, -0.6], "H_2SO_4\nreflux");
        for format in Format::ALL {
            let bytes = encode(format, &mol, None).unwrap();
            let back = decode(format, &bytes).unwrap();
            assert_eq!(back.molecule.objects.len(), 3, "{format:?}");
            assert_eq!(back.other_objects, 0, "{format:?}");
            let text = back.molecule.objects[2].as_text().unwrap();
            assert_eq!(text.text, "H_2SO_4\nreflux", "{format:?}");
            assert_eq!(back.summary(), "2 arrows and 1 text can be edited");
        }
    }

    #[test]
    fn the_summary_names_what_is_editable_and_what_is_kept() {
        let loaded = Loaded {
            molecule: ethanol(),
            original: Vec::new(),
            other_objects: 2,
        };
        assert_eq!(
            loaded.summary(),
            "2 hidden objects will be kept when saving"
        );
        let plain = Loaded {
            other_objects: 0,
            ..loaded
        };
        assert_eq!(plain.summary(), "");
    }

    /// ChemDraw's own reaction table (its 「Reaction Analysis」 grid) opens as an editable
    /// table, not as a hidden object, and both formats keep it.
    #[test]
    fn a_chemdraw_reaction_table_is_editable_and_kept() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/ReactionAnalysis.cdx");
        let loaded = load(&path).unwrap();
        assert_eq!(
            loaded.summary(),
            "1 arrow and 1 reaction table can be edited. 1 hidden object will be kept when saving"
        );
        {
            let _lang = crate::i18n::ScopedLanguage::new(crate::i18n::Lang::Ja);
            assert_eq!(
                loaded.summary(),
                "矢印 1 本・反応表 1 個を編集できます。表示されないオブジェクト 1 個は保存時に保持されます"
            );
        }
        for format in Format::ALL {
            let bytes = encode(format, &loaded.molecule, Some(&loaded.original)).unwrap();
            let back = decode(format, &bytes).unwrap();
            let tables: Vec<_> = back
                .molecule
                .objects
                .iter()
                .filter_map(|o| o.as_table())
                .collect();
            assert_eq!(tables.len(), 1, "{format:?}");
            assert!(tables[0].reactants[0].limiting, "{format:?}");
            assert_eq!(back.other_objects, 1, "{format:?}");
        }
    }

    #[test]
    fn garbage_is_reported_not_read() {
        assert!(matches!(
            decode(Format::Cdx, b"not a cdx"),
            Err(Error::Unreadable(Format::Cdx))
        ));
        assert!(matches!(
            decode(Format::Cdxml, b"<html/>"),
            Err(Error::Unreadable(Format::Cdxml))
        ));
    }

    #[test]
    fn nothing_drawn_exports_nothing() {
        assert!(matches!(
            export(Export::Smiles, &Molecule::default()),
            Err(Error::Empty)
        ));
        let smiles = String::from_utf8(export(Export::Smiles, &ethanol()).unwrap()).unwrap();
        assert_eq!(smiles.trim(), "CCO");
    }
}
