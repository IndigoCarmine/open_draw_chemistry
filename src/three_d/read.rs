//! Opening 3D structure files (MOL2, PDB, XYZ, MOL, SDF) as documents.
//!
//! OpenBabel reads the file. Its coordinates become the document's [`Model3d`] as they are —
//! nothing is regenerated or relaxed — with every explicit hydrogen kept. The editor gets a 2D
//! depiction of the same structure: OpenBabel's 2D layout (`gen2D`) of the heavy atoms, scaled
//! to the editor's 1.5-unit bonds, with the hydrogens on carbon and on heteroatoms left to the
//! labels as implicit ones. Bond orders come out Kekulé (OpenBabel kekulizes aromatic input),
//! and formal charges are kept.
//!
//! A file whose coordinates are flat (a 2D molfile) has no 3D model to show: its own layout is
//! used for the drawing and the model is left to be generated on demand.

use std::fmt;
use std::path::Path;

use chembuider_rs::Molecule;

use super::Model3d;

/// The most atoms (hydrogens included) a file may hold. The 2D editor is no place for a
/// protein, and OpenBabel's bond perception and layout grow quickly with size.
pub const MAX_ATOMS: usize = 999;

/// The largest file read at all, before OpenBabel sees it.
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

    /// OpenBabel's format id.
    pub fn openbabel_id(self) -> &'static str {
        match self {
            Format3d::Mol2 => "mol2",
            Format3d::Pdb => "pdb",
            Format3d::Xyz => "xyz",
            Format3d::Mol => "mol",
            Format3d::Sdf => "sdf",
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
    /// (OpenBabel's layout failed), so it wants a cleanup.
    pub projected: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The file is larger than [`MAX_FILE_BYTES`].
    FileTooLarge,
    /// The file holds no atoms.
    Empty,
    /// More than [`MAX_ATOMS`] atoms (the count found).
    TooManyAtoms(usize),
    /// OpenBabel could not read the file in this format.
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
            Error::TooManyAtoms(n) => f.write_str(&t!(
                "three_d.error.read_too_many_atoms",
                count = n,
                max = MAX_ATOMS
            )),
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
    // Refuse a protein before OpenBabel spends seconds perceiving its bonds.
    if let Some(n) = quick_atom_count(format, text)
        && n > MAX_ATOMS
    {
        return Err(Error::TooManyAtoms(n));
    }
    let records = record_count(format, text);
    engine::read(format, text, records)
}

/// The atom count a PDB or XYZ file declares, read cheaply without parsing it.
fn quick_atom_count(format: Format3d, text: &str) -> Option<usize> {
    match format {
        Format3d::Xyz => text.lines().next()?.trim().parse().ok(),
        Format3d::Pdb => Some(
            text.lines()
                .take_while(|l| !l.starts_with("ENDMDL"))
                .filter(|l| l.starts_with("ATOM  ") || l.starts_with("HETATM"))
                .count(),
        ),
        _ => None,
    }
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
    use openbabel::Molecule as ObMol;

    use super::{Error, Format3d, MAX_ATOMS, Opened3d};
    use crate::three_d::{Atom3d, Bond3d, ForceField, Model3d};

    pub fn read(format: Format3d, text: &str, records: usize) -> Result<Opened3d, Error> {
        let mut mol =
            ObMol::parse(text, format.openbabel_id()).map_err(|_| Error::Unreadable(format))?;
        let atoms = mol.num_atoms() as usize;
        if atoms == 0 {
            return Err(Error::Empty);
        }
        if atoms > MAX_ATOMS {
            return Err(Error::TooManyAtoms(atoms));
        }
        let positions: Vec<[f64; 3]> = mol
            .atoms()
            .map(|a| {
                let (x, y, z) = a.coords();
                [x, y, z]
            })
            .collect();
        if positions.iter().flatten().any(|c| !c.is_finite()) {
            return Err(Error::Unreadable(format));
        }
        // Only a molfile can be 2D (its header says so, or every z is zero); a planar
        // molecule in the other formats is still a 3D structure.
        let flat = matches!(format, Format3d::Mol | Format3d::Sdf) && !mol.has_3d();
        // A coordinates-only file (XYZ, a PDB without CONECT) normally has its bonds perceived
        // by OpenBabel's reader already; make sure of it.
        if !flat && mol.num_bonds() == 0 && atoms > 1 {
            mol.set_dimension(3);
            mol.connect_the_dots();
            mol.perceive_bond_orders();
        }
        let model = if flat { None } else { Some(model(&mol)?) };
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
    fn model(mol: &ObMol) -> Result<Model3d, Error> {
        let atoms = mol
            .atoms()
            .map(|a| {
                let (x, y, z) = a.coords();
                Atom3d {
                    element: openbabel::elements::symbol(a.atomic_number()),
                    pos: [x, y, z],
                }
            })
            .collect();
        let bonds = mol
            .bonds()
            .map(|b| Bond3d {
                begin: b.begin_atom_index() as usize,
                end: b.end_atom_index() as usize,
                order: u8::try_from(b.order()).unwrap_or(1),
            })
            .collect();
        let (force_field, energy) = [ForceField::default(), ForceField::Uff]
            .into_iter()
            .find_map(|ff| {
                mol.energy(ff.id())
                    .filter(|e| e.is_finite())
                    .map(|e| (ff, e))
            })
            .unwrap_or((ForceField::default(), f64::NAN));
        Ok(Model3d {
            atoms,
            bonds,
            force_field,
            energy,
            energy_unit: openbabel::forcefield_energy_unit(force_field.id())
                .unwrap_or_else(|| "kcal/mol".to_owned()),
            formula: mol.formula(),
            molfile: mol
                .write("mol")
                .map_err(|_| Error::Unreadable(Format3d::Mol))?,
        })
    }

    /// Whether the atom at `index` is a hydrogen the drawing leaves to its neighbour's label:
    /// one bonded to a single atom that has a standard valence (C, N, O, S, …). A lone H, H₂,
    /// a bridging or metal-bound hydrogen stays.
    fn implicit_hydrogen(mol: &ObMol, index: u32) -> bool {
        let Some(atom) = mol.atom(index) else {
            return false;
        };
        if atom.atomic_number() != 1 || atom.isotope() > 1 {
            return false;
        }
        let neighbours = atom.neighbors();
        let [heavy] = neighbours.as_slice() else {
            return false;
        };
        let number = heavy.atomic_number();
        number != 1 && normal_valence(&openbabel::elements::symbol(number)).is_some()
    }

    /// The 2D drawing of `mol`: hydrogens folded into labels, laid out by OpenBabel (or, if
    /// that fails, projected onto the best-fit plane — reported by the `bool`). A `flat` file
    /// keeps its own layout.
    fn depict(mol: &ObMol, flat: bool) -> (Molecule, bool) {
        let mut heavy = mol.clone();
        let drop: Vec<u32> = (0..mol.num_atoms())
            .filter(|&i| implicit_hydrogen(mol, i))
            .collect();
        heavy.begin_modify();
        for &i in drop.iter().rev() {
            heavy.delete_atom(i);
        }
        heavy.end_modify();

        let original: Vec<[f64; 3]> = heavy
            .atoms()
            .map(|a| {
                let (x, y, z) = a.coords();
                [x, y, z]
            })
            .collect();
        let mut projected = false;
        let points: Vec<[f64; 2]> = if flat {
            original.iter().map(|p| [p[0], p[1]]).collect()
        } else if heavy.num_atoms() > 1 && heavy.generate_2d() {
            heavy
                .atoms()
                .map(|a| {
                    let (x, y, _) = a.coords();
                    [x, y]
                })
                .collect()
        } else {
            projected = heavy.num_atoms() > 1;
            super::project_to_plane(&original)
        };
        let bonds: Vec<(usize, usize, u32)> = heavy
            .bonds()
            .map(|b| {
                (
                    b.begin_atom_index() as usize,
                    b.end_atom_index() as usize,
                    b.order(),
                )
            })
            .collect();
        let pairs: Vec<(usize, usize)> = bonds.iter().map(|&(a, b, _)| (a, b)).collect();
        let positions = super::editor_positions(&points, &pairs);

        let mut drawing = Molecule::default();
        let ids: Vec<u32> = heavy
            .atoms()
            .zip(positions)
            .map(|(a, pos)| {
                let charge = i8::try_from(a.formal_charge()).unwrap_or(0);
                drawing.add_atom(openbabel::elements::symbol(a.atomic_number()), pos, charge)
            })
            .collect();
        for (a, b, order) in bonds {
            let order = match order {
                2 => BondOrder::Double,
                3 => BondOrder::Triple,
                4 => BondOrder::Quadruple,
                _ => BondOrder::Single,
            };
            drawing.add_bond(ids[a], ids[b], order);
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
    fn a_huge_file_is_refused_before_parsing() {
        let text = format!("{}\n\n", MAX_ATOMS + 1);
        assert_eq!(
            read(Format3d::Xyz, &text),
            Err(Error::TooManyAtoms(MAX_ATOMS + 1))
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
