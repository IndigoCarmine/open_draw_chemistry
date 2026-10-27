//! The OpenBabel side of [`super`]: building, minimising and writing models.
//!
//! Every call here is slow and serialised by OpenBabel's process-wide lock, so it belongs on
//! a background thread (see [`super::job`]).

use openbabel::Molecule as ObMol;

use super::{Atom3d, Bond3d, Error, Export3d, ForceField, Model3d};

/// The quality settings tried in turn until one gives a usable model. OpenBabel's builder can
/// leave atoms piled up or unrelaxable on an awkward structure; another setting usually
/// does not.
const ATTEMPTS: [&str; 3] = ["med", "slow", "fast"];

/// Two atoms closer than this (Å) mean the builder failed; the shortest real bond, H–H, is 0.74.
const MIN_DISTANCE: f64 = 0.5;

/// Build a 3D model from a 2D molfile: add hydrogens, generate coordinates, then relax them
/// for `steps` steps under `force_field`.
pub fn generate(molfile: &str, force_field: ForceField, steps: u32) -> Result<Model3d, Error> {
    let mut last = Error::GenerationFailed;
    for speed in ATTEMPTS {
        let mut mol = ObMol::parse(molfile, "mol").map_err(|_| Error::Unreadable)?;
        mol.add_hydrogens();
        if !mol.generate_3d_with(speed) {
            continue;
        }
        let energy = mol
            .optimize_geometry(force_field.id(), steps)
            .ok_or(Error::ForceField(force_field))?;
        match model(&mol, force_field, energy) {
            Ok(model) => return Ok(model),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Minimise an existing 3D model (a molfile written by [`model`]) further.
pub fn optimize(molfile: &str, force_field: ForceField, steps: u32) -> Result<Model3d, Error> {
    let mut mol = ObMol::parse(molfile, "mol").map_err(|_| Error::Unreadable)?;
    let energy = mol
        .optimize_geometry(force_field.id(), steps)
        .ok_or(Error::ForceField(force_field))?;
    model(&mol, force_field, energy)
}

/// Write a model's molfile out in `format`.
pub fn export(molfile: &str, format: Export3d) -> Result<String, Error> {
    let mol = ObMol::parse(molfile, "mol").map_err(|_| Error::Unreadable)?;
    mol.write(format.extension())
        .map_err(|_| Error::ExportFailed(format))
}

/// The model OpenBabel's molecule holds, or [`Error::GenerationFailed`] when its geometry or
/// energy is unusable.
fn model(mol: &ObMol, force_field: ForceField, energy: f64) -> Result<Model3d, Error> {
    let atoms: Vec<Atom3d> = mol
        .atoms()
        .map(|a| {
            let (x, y, z) = a.coords();
            Atom3d {
                element: openbabel::elements::symbol(a.atomic_number()),
                pos: [x, y, z],
            }
        })
        .collect();
    let bonds: Vec<Bond3d> = mol
        .bonds()
        .map(|b| Bond3d {
            begin: b.begin_atom_index() as usize,
            end: b.end_atom_index() as usize,
            order: u8::try_from(b.order()).unwrap_or(1),
        })
        .collect();
    let model = Model3d {
        atoms,
        bonds,
        force_field,
        energy,
        energy_unit: openbabel::forcefield_energy_unit(force_field.id())
            .unwrap_or_else(|| "kcal/mol".to_owned()),
        formula: mol.formula(),
        molfile: mol.write("mol").map_err(|_| Error::GenerationFailed)?,
    };
    if usable(&model) {
        Ok(model)
    } else {
        Err(Error::GenerationFailed)
    }
}

/// Finite energy and coordinates, and no two atoms on top of each other.
fn usable(model: &Model3d) -> bool {
    if !model.energy.is_finite() || model.atoms.is_empty() {
        return false;
    }
    if model
        .atoms
        .iter()
        .any(|a| a.pos.iter().any(|c| !c.is_finite()))
    {
        return false;
    }
    let n = model.atoms.len();
    (0..n).all(|i| (i + 1..n).all(|j| model.distance(i, j) >= MIN_DISTANCE))
}
