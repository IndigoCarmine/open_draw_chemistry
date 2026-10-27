//! 2D → 3D: turning the drawn structure into a 3D model with OpenBabel.
//!
//! The pipeline is
//!
//! 1. [`molfile::to_molfile`] — the editor's [`Molecule`] (abbreviations expanded) as an MDL
//!    molfile, with wedges kept so OpenBabel sees the drawn stereochemistry;
//! 2. [`generate`] — OpenBabel adds the hydrogens, builds 3D coordinates and relaxes them under
//!    the chosen [`ForceField`], giving a [`Model3d`];
//! 3. [`optimize`] / [`export`] — further minimisation, and writing the model out.
//!
//! OpenBabel is behind the `openbabel` cargo feature. Without it everything here still
//! compiles, [`AVAILABLE`] is `false` and generation fails with [`Error::Unavailable`], so the
//! UI can say so instead of offering a button that does nothing. [`job`] runs the slow part on
//! a background thread.

use std::fmt;

use chembuider_rs::{Config, Molecule};

pub mod edit;
#[cfg(feature = "openbabel")]
mod engine;
pub mod geom;
pub mod job;
pub mod molfile;
pub mod read;

pub use edit::{Constraint, Edit3d};
pub use job::{Job, JobState, Task};

/// Whether this build can generate 3D models (it was built with the `openbabel` feature).
pub const AVAILABLE: bool = cfg!(feature = "openbabel");

/// Conjugate-gradient steps spent relaxing a freshly generated model.
const GENERATE_STEPS: u32 = 500;
/// Steps spent by an explicit "optimise" on an existing model.
const OPTIMIZE_STEPS: u32 = 2500;

/// The force fields offered for relaxing a model, as OpenBabel names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ForceField {
    #[default]
    Mmff94,
    Mmff94s,
    Uff,
    Gaff,
}

impl ForceField {
    pub const ALL: [ForceField; 4] = [
        ForceField::Mmff94,
        ForceField::Mmff94s,
        ForceField::Uff,
        ForceField::Gaff,
    ];

    /// OpenBabel's id for the force field, which is also how it is shown.
    pub fn id(self) -> &'static str {
        match self {
            ForceField::Mmff94 => "MMFF94",
            ForceField::Mmff94s => "MMFF94s",
            ForceField::Uff => "UFF",
            ForceField::Gaff => "GAFF",
        }
    }
}

impl fmt::Display for ForceField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// A file format a 3D model can be exported to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Export3d {
    Mol,
    Sdf,
    Xyz,
    Pdb,
}

impl Export3d {
    pub const ALL: [Export3d; 4] = [Export3d::Mol, Export3d::Sdf, Export3d::Xyz, Export3d::Pdb];

    /// OpenBabel's format id, which is also the file extension.
    pub fn extension(self) -> &'static str {
        match self {
            Export3d::Mol => "mol",
            Export3d::Sdf => "sdf",
            Export3d::Xyz => "xyz",
            Export3d::Pdb => "pdb",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Export3d::Mol => "MDL Molfile",
            Export3d::Sdf => "SD File",
            Export3d::Xyz => "XYZ",
            Export3d::Pdb => "Protein Data Bank",
        }
    }
}

/// One atom of a [`Model3d`].
#[derive(Debug, Clone, PartialEq)]
pub struct Atom3d {
    /// Element symbol ("C", "H", …).
    pub element: String,
    /// Position in ångström.
    pub pos: [f64; 3],
}

/// One bond of a [`Model3d`], between indices into [`Model3d::atoms`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bond3d {
    pub begin: usize,
    pub end: usize,
    /// 1–3; OpenBabel's 5 marks an aromatic bond.
    pub order: u8,
}

/// A generated 3D structure, hydrogens included.
#[derive(Debug, Clone, PartialEq)]
pub struct Model3d {
    pub atoms: Vec<Atom3d>,
    pub bonds: Vec<Bond3d>,
    /// The force field the geometry was relaxed with, and its final energy.
    pub force_field: ForceField,
    pub energy: f64,
    /// The force field's energy unit: MMFF94 reports kcal/mol, UFF and GAFF kJ/mol.
    pub energy_unit: String,
    /// The molecular formula, hydrogens included (Hill order).
    pub formula: String,
    /// The model as a 3D MDL molfile: the source for [`optimize`] and [`export`].
    pub molfile: String,
}

impl Model3d {
    /// How many of the atoms are hydrogens.
    pub fn hydrogen_count(&self) -> usize {
        self.atoms.iter().filter(|a| a.element == "H").count()
    }

    /// Distance in ångström between the atoms at indices `a` and `b`.
    pub fn distance(&self, a: usize, b: usize) -> f64 {
        let (p, q) = (self.atoms[a].pos, self.atoms[b].pos);
        ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// Built without the `openbabel` feature.
    Unavailable,
    /// Nothing is drawn.
    Empty,
    /// Atom labels that are neither elements nor abbreviations the editor can expand.
    UnknownLabels(Vec<String>),
    /// A quadruple bond, which a molfile cannot express.
    QuadrupleBond,
    /// More atoms or bonds than a V2000 molfile holds.
    TooLarge,
    /// OpenBabel could not read the structure handed to it.
    Unreadable,
    /// No usable 3D geometry came out, even after retrying.
    GenerationFailed,
    /// The force field cannot be set up for this structure (an atom it has no type for).
    ForceField(ForceField),
    /// OpenBabel could not write the model in this format.
    ExportFailed(Export3d),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unavailable => write!(
                f,
                "この版では 3D 生成を利用できません（OpenBabel なしでビルドされています）"
            ),
            Error::Empty => write!(f, "構造が描かれていません"),
            Error::UnknownLabels(labels) => write!(
                f,
                "3D に変換できないラベルがあります: {}",
                labels.join("、")
            ),
            Error::QuadrupleBond => write!(f, "四重結合は 3D 生成に対応していません"),
            Error::TooLarge => write!(f, "構造が大きすぎます（原子・結合は各 999 個まで）"),
            Error::Unreadable => write!(f, "OpenBabel が構造を読み込めませんでした"),
            Error::GenerationFailed => write!(f, "3D 座標を生成できませんでした"),
            Error::ForceField(ff) => write!(f, "力場 {ff} をこの構造に適用できませんでした"),
            Error::ExportFailed(export) => {
                write!(f, "{} として書き出せませんでした", export.description())
            }
        }
    }
}

impl std::error::Error for Error {}

/// Generate a 3D model of `mol` relaxed under `force_field`. Slow (OpenBabel builds and
/// minimises the structure): call it off the UI thread, as [`Job`] does.
pub fn generate(
    mol: &Molecule,
    config: &Config,
    force_field: ForceField,
) -> Result<Model3d, Error> {
    let molfile = molfile::to_molfile(mol, config)?;
    generate_from_molfile(&molfile, force_field)
}

/// [`generate`] from a 2D molfile already made by [`molfile::to_molfile`].
pub fn generate_from_molfile(molfile: &str, force_field: ForceField) -> Result<Model3d, Error> {
    #[cfg(feature = "openbabel")]
    {
        engine::generate(molfile, force_field, GENERATE_STEPS)
    }
    #[cfg(not(feature = "openbabel"))]
    {
        let _ = (molfile, force_field, GENERATE_STEPS);
        Err(Error::Unavailable)
    }
}

/// Minimise `model` further under `force_field`, which may differ from the one it was made with.
pub fn optimize(model: &Model3d, force_field: ForceField) -> Result<Model3d, Error> {
    optimize_with(model, force_field, &[])
}

/// [`optimize`] holding `constraints` (fixed atoms, restrained lengths, angles and dihedrals).
pub fn optimize_with(
    model: &Model3d,
    force_field: ForceField,
    constraints: &[Constraint],
) -> Result<Model3d, Error> {
    #[cfg(feature = "openbabel")]
    {
        engine::optimize(&model.molfile, force_field, OPTIMIZE_STEPS, constraints)
    }
    #[cfg(not(feature = "openbabel"))]
    {
        let _ = (model, force_field, constraints, OPTIMIZE_STEPS);
        Err(Error::Unavailable)
    }
}

/// The single-point energy of `model`'s current geometry under `force_field`, in the force
/// field's unit. For the energy after a hand edit; slow-ish, so off the UI thread.
pub fn energy(model: &Model3d, force_field: ForceField) -> Result<f64, Error> {
    #[cfg(feature = "openbabel")]
    {
        engine::energy(&model.molfile, force_field)
    }
    #[cfg(not(feature = "openbabel"))]
    {
        let _ = (model, force_field);
        Err(Error::Unavailable)
    }
}

/// `model` as the text of a file in `format`.
pub fn export(model: &Model3d, format: Export3d) -> Result<String, Error> {
    #[cfg(feature = "openbabel")]
    {
        engine::export(&model.molfile, format)
    }
    #[cfg(not(feature = "openbabel"))]
    {
        let _ = (model, format);
        Err(Error::Unavailable)
    }
}
