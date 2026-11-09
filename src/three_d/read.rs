//! Opening 3D structure files (MOL2, PDB, XYZ, MOL, SDF) as documents.
//!
//! molecule_optimizer_rs reads the file. Its coordinates become the document's [`Model3d`] as
//! they are — nothing is regenerated or relaxed — with every explicit hydrogen kept. The editor
//! gets a 2D depiction of the same structure: the depictor's 2D layout (a port of RDKit's
//! `Compute2DCoords`) of the heavy atoms, scaled to the editor's 1.5-unit bonds, with the
//! hydrogens on carbon and on heteroatoms left to the labels as implicit ones. Bond orders come
//! out Kekulé, and formal charges are kept.
//!
//! A file whose coordinates are flat (a 2D molfile) has no 3D model to show: its own layout is
//! used for the drawing and the model is left to be generated on demand.

use std::fmt;
use std::path::Path;

use chembuider_rs::Molecule;

use super::Model3d;

/// The largest file read at all, before it is parsed.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Average bond length of the editor's drawings, in its units.
const BOND_LENGTH_2D: f64 = 1.5;

/// A 3D structure format that can be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format3d {
    Mol2,
    Pdb,
    Xyz,
    Mol,
    Sdf,
}

impl Format3d {
    pub const ALL: [Format3d; 5] = [
        Format3d::Mol2,
        Format3d::Pdb,
        Format3d::Xyz,
        Format3d::Mol,
        Format3d::Sdf,
    ];

    /// Every file extension the formats are recognised by (lower case).
    pub const EXTENSIONS: [&'static str; 8] =
        ["mol2", "pdb", "ent", "xyz", "mol", "mdl", "sdf", "sd"];

    /// The format a path's extension names, ignoring case.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "mol2" => Some(Format3d::Mol2),
            "pdb" | "ent" => Some(Format3d::Pdb),
            "xyz" => Some(Format3d::Xyz),
            "mol" | "mdl" => Some(Format3d::Mol),
            "sdf" | "sd" => Some(Format3d::Sdf),
            _ => None,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Format3d::Mol2 => "Tripos MOL2",
            Format3d::Pdb => "Protein Data Bank",
            Format3d::Xyz => "XYZ",
            Format3d::Mol => "MDL Molfile",
            Format3d::Sdf => "SD File",
        }
    }
}

/// A structure read from a 3D file.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened3d {
    /// The file's own coordinates; `None` when they are flat (a 2D molfile).
    pub model: Option<Model3d>,
    /// The 2D depiction for the editor.
    pub drawing: Molecule,
    /// How many structures the file holds; only the first is read.
    pub records: usize,
    /// The depiction is a projection of the 3D coordinates rather than a proper 2D layout
    /// (the depictor failed), so it wants a cleanup.
    pub projected: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The file is larger than [`MAX_FILE_BYTES`].
    FileTooLarge,
    /// The file holds no atoms.
    Empty,
    /// The file could not be read in this format.
    Unreadable(Format3d),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::FileTooLarge => f.write_str(&t!(
                "three_d.error.read_file_too_large",
                max = MAX_FILE_BYTES / (1024 * 1024)
            )),
            Error::Empty => f.write_str(t!("three_d.error.read_empty")),
            Error::Unreadable(format) => f.write_str(&t!(
                "three_d.error.read_unreadable",
                format = format.description()
            )),
        }
    }
}

impl std::error::Error for Error {}

/// Read the first structure of `text`, a file in `format`.
pub fn read(format: Format3d, text: &str) -> Result<Opened3d, Error> {
    if text.trim().is_empty() {
        return Err(Error::Empty);
    }
    let records = record_count(format, text);
    engine::read(format, text, records)
}

/// How many structures the file holds (at least one).
fn record_count(format: Format3d, text: &str) -> usize {
    let count = |pred: &dyn Fn(&str) -> bool| text.lines().filter(|l| pred(l)).count();
    let n = match format {
        Format3d::Sdf => {
            let ends = count(&|l| l.trim_end() == "$$$$");
            // The last record may lack its terminator.
            let tail = text
                .rsplit_once("$$$$")
                .map_or(text, |(_, rest)| rest)
                .trim();
            ends + usize::from(!tail.is_empty())
        }
        Format3d::Mol2 => count(&|l| l.trim_end() == "@<TRIPOS>MOLECULE"),
        Format3d::Pdb => count(&|l| l.starts_with("MODEL ")),
        Format3d::Xyz | Format3d::Mol => 1,
    };
    n.max(1)
}

/// Place a 2D layout (`[x, y]` with y up, any scale) in the editor's frame: y down, bonds
/// [`BOND_LENGTH_2D`] long on average, centred on the origin.
fn editor_positions(points: &[[f64; 2]], bonds: &[(usize, usize)]) -> Vec<[f32; 2]> {
    let lengths: Vec<f64> = bonds
        .iter()
        .map(|&(a, b)| {
            let (p, q) = (points[a], points[b]);
            ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()
        })
        .filter(|l| *l > 1e-6)
        .collect();
    let scale = if lengths.is_empty() {
        1.0
    } else {
        BOND_LENGTH_2D / (lengths.iter().sum::<f64>() / lengths.len() as f64)
    };
    let n = points.len().max(1) as f64;
    let cx = points.iter().map(|p| p[0]).sum::<f64>() / n;
    let cy = points.iter().map(|p| p[1]).sum::<f64>() / n;
    points
        .iter()
        .map(|p| [((p[0] - cx) * scale) as f32, (-(p[1] - cy) * scale) as f32])
        .collect()
}

/// Project 3D points onto their best-fit plane (the two principal axes), as plane
/// coordinates. The fallback when no proper 2D layout can be made.
fn project_to_plane(points: &[[f64; 3]]) -> Vec<[f64; 2]> {
    let n = points.len().max(1) as f64;
    let mut c = [0.0; 3];
    for p in points {
        for k in 0..3 {
            c[k] += p[k] / n;
        }
    }
    let centred: Vec<[f64; 3]> = points
        .iter()
        .map(|p| [p[0] - c[0], p[1] - c[1], p[2] - c[2]])
        .collect();
    let mut cov = [[0.0; 3]; 3];
    for p in &centred {
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += p[i] * p[j];
            }
        }
    }
    let first = dominant_axis(&cov, [1.0, 0.0, 0.0], None);
    let second = dominant_axis(&cov, [0.0, 1.0, 0.0], Some(first));
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    centred
        .iter()
        .map(|&p| [dot(p, first), dot(p, second)])
        .collect()
}

/// The unit eigenvector of the largest eigenvalue of the symmetric `m` (by power
/// iteration), kept orthogonal to `exclude`.
fn dominant_axis(m: &[[f64; 3]; 3], start: [f64; 3], exclude: Option<[f64; 3]>) -> [f64; 3] {
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let orthogonalise = |v: [f64; 3]| match exclude {
        Some(e) => {
            let d = dot(v, e);
            [v[0] - d * e[0], v[1] - d * e[1], v[2] - d * e[2]]
        }
        None => v,
    };
    let normalise = |v: [f64; 3]| {
        let len = dot(v, v).sqrt();
        (len > 1e-12).then(|| [v[0] / len, v[1] / len, v[2] / len])
    };
    // A start along the excluded axis would vanish; try the others.
    let starts = [start, [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
    let Some(mut v) = starts.into_iter().find_map(|s| normalise(orthogonalise(s))) else {
        return [0.0, 0.0, 1.0];
    };
    for _ in 0..100 {
        let w = [dot(m[0], v), dot(m[1], v), dot(m[2], v)];
        match normalise(orthogonalise(w)) {
            Some(next) => v = next,
            // Degenerate (everything on a line or a point): any orthogonal axis will do.
            None => break,
        }
    }
    v
}

mod engine {
    use chembuider_rs::molecule::normal_valence;
    use chembuider_rs::{BondOrder, Molecule};
    use molecule_optimizer_rs::depict::{self, Compute2DCoordsParams};
    use molecule_optimizer_rs::io::{self, Format};
    use molecule_optimizer_rs::perception::{self, SanitizeOptions};
    use molecule_optimizer_rs::{Atom, BondOrder as Order, Molecule as Structure};

    use super::{Error, Format3d, Opened3d};
    use crate::three_d::{self, ForceField, Model3d};

    pub fn read(format: Format3d, text: &str, records: usize) -> Result<Opened3d, Error> {
        let unreadable = || Error::Unreadable(format);
        let first = match format {
            // A bad record further on must not stop the first from opening.
            Format3d::Sdf => io::molfile::read_sdf_lenient(text).into_iter().next(),
            Format3d::Mol => Some(io::read_molfile(text)),
            Format3d::Mol2 | Format3d::Pdb | Format3d::Xyz => {
                let reader = match format {
                    Format3d::Mol2 => Format::Mol2,
                    Format3d::Pdb => Format::Pdb,
                    _ => Format::Xyz,
                };
                io::read_str(text, reader)
                    .map(|mols| mols.into_iter().next())
                    .transpose()
            }
        };
        let mut mol = first.ok_or(Error::Empty)?.map_err(|_| unreadable())?;
        let atoms = mol.num_atoms();
        if atoms == 0 {
            return Err(Error::Empty);
        }
        if mol.coords.iter().flatten().any(|c| !c.is_finite()) {
            return Err(unreadable());
        }
        // Only a molfile can be 2D (every z zero); a planar molecule in the other formats is
        // still a 3D structure.
        let flat =
            matches!(format, Format3d::Mol | Format3d::Sdf) && !perception::has_3d_coords(&mol);
        // A coordinates-only file (XYZ, a PDB without CONECT) has its bonds perceived by the
        // reader already; make sure of it.
        if !flat && mol.num_bonds() == 0 && atoms > 1 {
            perception::perceive_from_coordinates(&mut mol).map_err(|_| unreadable())?;
        }
        let model = (!flat).then(|| model(&mol));
        let (drawing, projected) = depict(&mol, flat);
        Ok(Opened3d {
            model,
            drawing,
            records,
            projected,
        })
    }

    /// The file's structure as a model, at its own coordinates. The energy is a single point
    /// under the default force field (UFF when that cannot type the structure).
    fn model(mol: &Structure) -> Model3d {
        let (force_field, energy) = [ForceField::default(), ForceField::Uff]
            .into_iter()
            .find_map(|ff| {
                let field = three_d::field(mol, ff, &[]).ok()?;
                let e = field.energy(&mol.flat_coords());
                e.is_finite().then_some((ff, e))
            })
            .unwrap_or((ForceField::default(), f64::NAN));
        three_d::assemble(mol, &mol.coords, force_field, energy)
    }

    /// Whether atom `i` is a hydrogen the drawing leaves to its neighbour's label: one bonded
    /// to a single atom that has a standard valence (C, N, O, S, …). A lone H, H₂, a bridging
    /// or metal-bound hydrogen, deuterium and tritium stay.
    fn implicit_hydrogen(mol: &Structure, i: usize) -> bool {
        let atom = mol.atom(i);
        if atom.atomic_num != 1 || atom.isotope > 1 {
            return false;
        }
        let neighbours: Vec<usize> = mol.neighbors(i).collect();
        let [heavy] = neighbours[..] else {
            return false;
        };
        let heavy = mol.atom(heavy);
        heavy.atomic_num != 1 && normal_valence(heavy.symbol()).is_some()
    }

    /// The structure without the hydrogens [`implicit_hydrogen`] folds into labels — each atom
    /// keeping those it lost as an explicit count — at the file's coordinates, bond orders
    /// Kekulé.
    fn heavy_atoms(mol: &Structure) -> Structure {
        let kekule = perception::kekulized_copy(mol).unwrap_or_else(|_| mol.clone());
        let mut index = vec![usize::MAX; mol.num_atoms()];
        let mut heavy = Structure::new();
        for i in (0..mol.num_atoms()).filter(|&i| !implicit_hydrogen(mol, i)) {
            let source = kekule.atom(i);
            let folded = mol
                .neighbors(i)
                .filter(|&n| implicit_hydrogen(mol, n))
                .count();
            let mut atom = Atom::new(source.atomic_num);
            atom.formal_charge = source.formal_charge;
            atom.isotope = source.isotope;
            atom.num_radical_electrons = source.num_radical_electrons;
            atom.num_explicit_hs = u8::try_from(folded).unwrap_or(u8::MAX);
            atom.no_implicit = true;
            index[i] = heavy.add_atom(atom, mol.coords[i]);
        }
        for b in kekule.bonds() {
            let (a, c) = (index[b.begin], index[b.end]);
            if a != usize::MAX && c != usize::MAX {
                let order = match b.order {
                    Order::Aromatic => Order::Single,
                    o => o,
                };
                heavy.add_bond(a, c, order);
            }
        }
        heavy
    }

    /// The depictor's 2D layout of `heavy` (see [`heavy_atoms`]), keeping the stereo its 3D
    /// geometry shows. `None` when it cannot lay it out.
    fn layout(mut heavy: Structure) -> Option<Vec<[f64; 2]>> {
        let options = SanitizeOptions {
            aromaticity: false,
            stereo_from_3d: true,
        };
        perception::sanitize_with(&mut heavy, &options).ok()?;
        let coords = depict::compute_2d_coords(&heavy, &Compute2DCoordsParams::default()).ok()?;
        let points: Vec<[f64; 2]> = coords.iter().map(|p| [p[0], p[1]]).collect();
        points
            .iter()
            .flatten()
            .all(|c| c.is_finite())
            .then_some(points)
    }

    /// The 2D drawing of `mol`: hydrogens folded into labels, laid out by the depictor (or, if
    /// that fails, projected onto the best-fit plane — reported by the `bool`). A `flat` file
    /// keeps its own layout.
    fn depict(mol: &Structure, flat: bool) -> (Molecule, bool) {
        let heavy = heavy_atoms(mol);
        let mut projected = false;
        let points: Vec<[f64; 2]> = if flat {
            heavy.coords.iter().map(|p| [p[0], p[1]]).collect()
        } else if let Some(points) = (heavy.num_atoms() > 1)
            .then(|| layout(heavy.clone()))
            .flatten()
        {
            points
        } else {
            projected = heavy.num_atoms() > 1;
            super::project_to_plane(&heavy.coords)
        };
        let pairs: Vec<(usize, usize)> = heavy.bonds().iter().map(|b| (b.begin, b.end)).collect();
        let positions = super::editor_positions(&points, &pairs);

        let mut drawing = Molecule::default();
        let ids: Vec<u32> = heavy
            .atoms()
            .iter()
            .zip(positions)
            .map(|(a, pos)| drawing.add_atom(a.symbol().to_owned(), pos, a.formal_charge))
            .collect();
        for b in heavy.bonds() {
            let order = match b.order {
                Order::Double => BondOrder::Double,
                Order::Triple => BondOrder::Triple,
                _ => BondOrder::Single,
            };
            drawing.add_bond(ids[b.begin], ids[b.end], order);
        }
        (drawing, projected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_follow_the_extension_in_any_case() {
        assert_eq!(
            Format3d::from_path(Path::new("a/b.MOL2")),
            Some(Format3d::Mol2)
        );
        assert_eq!(Format3d::from_path(Path::new("x.pdb")), Some(Format3d::Pdb));
        assert_eq!(Format3d::from_path(Path::new("x.Xyz")), Some(Format3d::Xyz));
        assert_eq!(Format3d::from_path(Path::new("x.mol")), Some(Format3d::Mol));
        assert_eq!(Format3d::from_path(Path::new("x.sdf")), Some(Format3d::Sdf));
        assert_eq!(Format3d::from_path(Path::new("x.cdx")), None);
        for ext in Format3d::EXTENSIONS {
            assert!(
                Format3d::from_path(Path::new(&format!("f.{ext}"))).is_some(),
                "{ext}"
            );
        }
    }

    #[test]
    fn records_are_counted() {
        let one = "m\n  x\n\n  0  0  0  0  0  0            999 V2000\nM  END\n$$$$\n";
        assert_eq!(record_count(Format3d::Sdf, one), 1);
        assert_eq!(record_count(Format3d::Sdf, &format!("{one}{one}{one}")), 3);
        assert_eq!(
            record_count(Format3d::Sdf, one.trim_end_matches("$$$$\n")),
            1
        );
        assert_eq!(record_count(Format3d::Pdb, "ATOM  \nEND\n"), 1);
        assert_eq!(
            record_count(Format3d::Pdb, "MODEL        1\nENDMDL\nMODEL        2\n"),
            2
        );
    }

    #[test]
    fn projection_keeps_a_planar_shape() {
        // A square tilted out of the xy plane projects back onto a square.
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let points = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, s, s], [0.0, s, s]];
        let flat = project_to_plane(&points);
        let d = |a: [f64; 2], b: [f64; 2]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
        for i in 0..4 {
            assert!((d(flat[i], flat[(i + 1) % 4]) - 1.0).abs() < 1e-6);
        }
        assert!((d(flat[0], flat[2]) - 2f64.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn positions_are_scaled_to_editor_bonds_with_y_down() {
        let pos = editor_positions(&[[0.0, 0.0], [0.0, 2.0]], &[(0, 1)]);
        // Centred, 1.5 apart, and the upper point (larger y) now has the smaller y.
        assert!((pos[0][1] - 0.75).abs() < 1e-6, "{pos:?}");
        assert!((pos[1][1] + 0.75).abs() < 1e-6, "{pos:?}");
    }
}
