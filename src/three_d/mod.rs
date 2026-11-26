//! 2D → 3D: turning the drawn structure into a 3D model with molecule_optimizer_rs.
//!
//! The pipeline is
//!
//! 1. [`molfile::to_molfile`] — the editor's [`Molecule`] (abbreviations expanded) as an MDL
//!    molfile, with wedges kept so the embedder sees the drawn stereochemistry;
//! 2. [`generate`] — the hydrogens are added, ETKDG embeds the structure in 3D and it is
//!    relaxed under the chosen [`ForceField`], giving a [`Model3d`];
//! 3. [`optimize`] / [`export`] — further minimisation, and writing the model out.
//!
//! [`job`] runs the slow part on a background thread.

use std::fmt;

use chembuider_rs::{Config, Molecule};

pub mod edit;
mod engine;
pub mod geom;
pub mod interactions;
pub mod job;
pub mod molfile;
pub mod part;
pub mod read;

pub use edit::{Constraint, Edit3d};
pub use job::{Job, JobState, Task};

/// Minimiser iterations (BFGS) spent relaxing a freshly generated model, times the engine's
/// generate budget.
const GENERATE_STEPS: u32 = 500;
/// Minimiser iterations spent by an explicit "optimise" on an existing model.
pub const OPTIMIZE_STEPS: u32 = 2500;

/// The force fields offered for relaxing a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ForceField {
    /// Also what a setting saved while GAFF was offered comes back as.
    #[default]
    #[serde(alias = "Gaff")]
    Mmff94,
    Mmff94s,
    Uff,
}

impl ForceField {
    pub const ALL: [ForceField; 3] = [ForceField::Mmff94, ForceField::Mmff94s, ForceField::Uff];

    /// The force field's name, which is also how it is shown.
    pub fn id(self) -> &'static str {
        match self {
            ForceField::Mmff94 => "MMFF94",
            ForceField::Mmff94s => "MMFF94s",
            ForceField::Uff => "UFF",
        }
    }
}

impl fmt::Display for ForceField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// The medium MMFF's electrostatics are computed in: its relative permittivity ε divides every
/// charge–charge term, as RDKit's `setMMFFDielectricConstant` does. Only MMFF94/94s has such
/// terms; UFF has no electrostatics, so a solvent changes nothing under it. It is the
/// continuum's screening of the charges and nothing more: no cavity, no solvation free energy.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum Solvent {
    #[default]
    Vacuum,
    Hexane,
    Toluene,
    DiethylEther,
    Chloroform,
    EthylAcetate,
    Thf,
    Dichloromethane,
    Acetone,
    Ethanol,
    Methanol,
    Acetonitrile,
    Dmf,
    Dmso,
    Water,
}

impl Solvent {
    /// Every solvent offered, roughly from the least polar to the most.
    pub const ALL: [Solvent; 15] = [
        Solvent::Vacuum,
        Solvent::Hexane,
        Solvent::Toluene,
        Solvent::DiethylEther,
        Solvent::Chloroform,
        Solvent::EthylAcetate,
        Solvent::Thf,
        Solvent::Dichloromethane,
        Solvent::Acetone,
        Solvent::Ethanol,
        Solvent::Methanol,
        Solvent::Acetonitrile,
        Solvent::Dmf,
        Solvent::Dmso,
        Solvent::Water,
    ];

    /// The solvent's English name: what files record (a scan's CSV, a conformer SD file),
    /// whatever the UI language.
    pub fn id(self) -> &'static str {
        match self {
            Solvent::Vacuum => "vacuum",
            Solvent::Hexane => "hexane",
            Solvent::Toluene => "toluene",
            Solvent::DiethylEther => "diethyl ether",
            Solvent::Chloroform => "chloroform",
            Solvent::EthylAcetate => "ethyl acetate",
            Solvent::Thf => "THF",
            Solvent::Dichloromethane => "dichloromethane",
            Solvent::Acetone => "acetone",
            Solvent::Ethanol => "ethanol",
            Solvent::Methanol => "methanol",
            Solvent::Acetonitrile => "acetonitrile",
            Solvent::Dmf => "DMF",
            Solvent::Dmso => "DMSO",
            Solvent::Water => "water",
        }
    }

    /// The solvent's name in the UI language.
    pub fn name(self) -> &'static str {
        match self {
            Solvent::Vacuum => t!("three_d.solvent.vacuum"),
            Solvent::Hexane => t!("three_d.solvent.hexane"),
            Solvent::Toluene => t!("three_d.solvent.toluene"),
            Solvent::DiethylEther => t!("three_d.solvent.diethyl_ether"),
            Solvent::Chloroform => t!("three_d.solvent.chloroform"),
            Solvent::EthylAcetate => t!("three_d.solvent.ethyl_acetate"),
            Solvent::Thf => t!("three_d.solvent.thf"),
            Solvent::Dichloromethane => t!("three_d.solvent.dichloromethane"),
            Solvent::Acetone => t!("three_d.solvent.acetone"),
            Solvent::Ethanol => t!("three_d.solvent.ethanol"),
            Solvent::Methanol => t!("three_d.solvent.methanol"),
            Solvent::Acetonitrile => t!("three_d.solvent.acetonitrile"),
            Solvent::Dmf => t!("three_d.solvent.dmf"),
            Solvent::Dmso => t!("three_d.solvent.dmso"),
            Solvent::Water => t!("three_d.solvent.water"),
        }
    }

    /// The relative permittivity ε at 25 °C (1 for a vacuum).
    pub fn dielectric(self) -> f64 {
        match self {
            Solvent::Vacuum => 1.0,
            Solvent::Hexane => 1.88,
            Solvent::Toluene => 2.38,
            Solvent::DiethylEther => 4.33,
            Solvent::Chloroform => 4.81,
            Solvent::EthylAcetate => 6.02,
            Solvent::Thf => 7.58,
            Solvent::Dichloromethane => 8.93,
            Solvent::Acetone => 20.7,
            Solvent::Ethanol => 24.5,
            Solvent::Methanol => 32.7,
            Solvent::Acetonitrile => 37.5,
            Solvent::Dmf => 36.7,
            Solvent::Dmso => 46.7,
            Solvent::Water => 78.4,
        }
    }
}

/// How MMFF screens its charges: by the solvent's ε, and whether that also grows with the
/// distance (ε·r, RDKit's distance-dependent dielectric, a common stand-in for the solvent
/// that gets between charges far apart). The default — a vacuum, ε a constant 1 — is MMFF as
/// it was parametrised. Ignored under UFF, which has no electrostatics.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub struct Solvation {
    pub solvent: Solvent,
    /// ε·r rather than ε: each charge–charge term falls with the square of the distance.
    pub distance_dependent: bool,
}

impl Solvation {
    /// MMFF's own electrostatics: a vacuum, ε constant.
    pub fn is_vacuum(self) -> bool {
        self == Solvation::default()
    }

    /// The setting as it applies under `force_field`: itself under MMFF, a vacuum under UFF
    /// (which has no electrostatics for a solvent to screen), so what was computed in effect
    /// is what is recorded and shown.
    pub fn under(self, force_field: ForceField) -> Solvation {
        match force_field {
            ForceField::Mmff94 | ForceField::Mmff94s => self,
            ForceField::Uff => Solvation::default(),
        }
    }

    /// The setting in English, for a file or an info row: "water (ε 78.4)", or with a
    /// distance-dependent dielectric "water (ε 78.4·r)".
    pub fn label(self) -> String {
        let r = if self.distance_dependent { "·r" } else { "" };
        format!("{} (ε {}{r})", self.solvent.id(), self.solvent.dielectric())
    }
}

/// A file format a 3D model can be exported to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Export3d {
    Mol,
    Sdf,
    Xyz,
    Pdb,
    Mol2,
}

impl Export3d {
    pub const ALL: [Export3d; 5] = [
        Export3d::Mol,
        Export3d::Sdf,
        Export3d::Xyz,
        Export3d::Pdb,
        Export3d::Mol2,
    ];

    /// The file extension.
    pub fn extension(self) -> &'static str {
        match self {
            Export3d::Mol => "mol",
            Export3d::Sdf => "sdf",
            Export3d::Xyz => "xyz",
            Export3d::Pdb => "pdb",
            Export3d::Mol2 => "mol2",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Export3d::Mol => "MDL Molfile",
            Export3d::Sdf => "SD File",
            Export3d::Xyz => "XYZ",
            Export3d::Pdb => "Protein Data Bank",
            Export3d::Mol2 => "Tripos MOL2",
        }
    }
}

/// One atom of a [`Model3d`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Atom3d {
    /// Element symbol ("C", "H", …).
    pub element: String,
    /// Position in ångström.
    pub pos: [f64; 3],
}

/// One bond of a [`Model3d`], between indices into [`Model3d::atoms`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Bond3d {
    pub begin: usize,
    pub end: usize,
    /// 1–3; 5 marks an aromatic bond.
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
    /// The energy's unit (kcal/mol).
    pub energy_unit: String,
    /// The solvent MMFF's charges were screened by, for the geometry and the energy (always a
    /// vacuum under UFF, which has no electrostatics).
    pub solvation: Solvation,
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
    /// Nothing is drawn.
    Empty,
    /// Atom labels that are neither elements nor abbreviations the editor can expand.
    UnknownLabels(Vec<String>),
    /// A quadruple bond, which a molfile cannot express.
    QuadrupleBond,
    /// A multiple-group bracket (×n) that cannot be written out (its count no number, say).
    MultipleGroup,
    /// The structure handed to the force field could not be read.
    Unreadable,
    /// No usable 3D geometry came out, even after retrying.
    GenerationFailed,
    /// The force field cannot be set up for this structure (an atom it has no type for).
    ForceField(ForceField),
    /// The model could not be written in this format.
    ExportFailed(Export3d),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Empty => f.write_str(t!("three_d.error.empty")),
            Error::UnknownLabels(labels) => f.write_str(&t!(
                "three_d.error.unknown_labels",
                labels = labels.join(t!("three_d.error.list_separator"))
            )),
            Error::QuadrupleBond => f.write_str(t!("three_d.error.quadruple_bond")),
            Error::MultipleGroup => f.write_str(t!("three_d.error.multiple_group")),
            Error::Unreadable => f.write_str(t!("three_d.error.unreadable")),
            Error::GenerationFailed => f.write_str(t!("three_d.error.generation_failed")),
            Error::ForceField(ff) => {
                f.write_str(&t!("three_d.error.force_field", force_field = ff))
            }
            Error::ExportFailed(export) => f.write_str(&t!(
                "three_d.error.export_failed",
                format = export.description()
            )),
        }
    }
}

impl std::error::Error for Error {}

/// Generate a 3D model of `mol` relaxed under `force_field` in `solvation`. Slow on a large
/// structure (it is embedded and minimised): call it off the UI thread, as [`Job`] does.
pub fn generate(
    mol: &Molecule,
    config: &Config,
    force_field: ForceField,
    solvation: Solvation,
) -> Result<Model3d, Error> {
    let molfile = molfile::to_molfile(mol, config)?;
    generate_from_molfile(&molfile, force_field, solvation)
}

/// [`generate`] from a 2D molfile already made by [`molfile::to_molfile`].
pub fn generate_from_molfile(
    molfile: &str,
    force_field: ForceField,
    solvation: Solvation,
) -> Result<Model3d, Error> {
    with_uff_fallback(force_field, |ff| {
        engine::generate(molfile, ff, solvation, GENERATE_STEPS)
    })
}

/// `work` under `force_field`, or under UFF when `force_field` has no type for one of the
/// structure's atoms: MMFF94 knows no metal, UFF the whole periodic table, so a complex is
/// still built and relaxed. The model says which force field it was made with — and, as UFF
/// ignores the solvent, a model that fell back says it was made in a vacuum.
fn with_uff_fallback(
    force_field: ForceField,
    work: impl Fn(ForceField) -> Result<Model3d, Error>,
) -> Result<Model3d, Error> {
    match work(force_field) {
        Err(Error::ForceField(_)) if force_field != ForceField::Uff => work(ForceField::Uff),
        result => result,
    }
}

/// Minimise `model` further under `force_field` in `solvation`, which may differ from those it
/// was made with.
pub fn optimize(
    model: &Model3d,
    force_field: ForceField,
    solvation: Solvation,
) -> Result<Model3d, Error> {
    optimize_with(model, force_field, solvation, &[])
}

/// [`optimize`] holding `constraints` (fixed atoms, restrained lengths, angles and dihedrals).
pub fn optimize_with(
    model: &Model3d,
    force_field: ForceField,
    solvation: Solvation,
    constraints: &[Constraint],
) -> Result<Model3d, Error> {
    with_uff_fallback(force_field, |ff| {
        engine::optimize(&model.molfile, ff, solvation, OPTIMIZE_STEPS, constraints)
    })
}

/// The minimiser behind [`optimize_with`]: a relaxed scan minimises each point with it (see
/// `scan::energy`).
pub(crate) use engine::{
    ENERGY_UNIT, Relaxed, SoftCoreStage, assemble, descend, field, fragment_count,
    model as checked_model, read as read_molfile, relax, restrained_builder, restraint_energy,
    sdf_record, soft_core,
};

#[cfg(test)]
pub(crate) use engine::tests::model_from_molfile;

/// The most minimiser iterations a restrained [`optimize_with`] takes.
pub const RESTRAINED_OPTIMIZE_STEPS: u32 = OPTIMIZE_STEPS * engine::RESTRAINED_BUDGET;

/// The single-point energy of `model`'s current geometry under `force_field` in `solvation` (kcal/mol). For the energy after a hand edit; slow-ish, so off the UI thread.
pub fn energy(
    model: &Model3d,
    force_field: ForceField,
    solvation: Solvation,
) -> Result<f64, Error> {
    engine::energy(&model.molfile, force_field, solvation)
}

/// `model` as the text of a file in `format`.
pub fn export(model: &Model3d, format: Export3d) -> Result<String, Error> {
    engine::export(&model.molfile, format)
}
