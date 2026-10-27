//! The OpenBabel side of [`super`]: building, minimising and writing models.
//!
//! Every call here is slow and serialised by OpenBabel's process-wide lock, so it belongs on
//! a background thread (see [`super::job`]).

use openbabel::{Algorithm, Constraints, Minimizer, Molecule as ObMol};

use super::{Atom3d, Bond3d, Constraint, Error, Export3d, ForceField, Model3d};

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

/// Minimise an existing 3D model (a molfile written by [`model`]) further, holding
/// `constraints`.
pub fn optimize(
    molfile: &str,
    force_field: ForceField,
    steps: u32,
    constraints: &[Constraint],
) -> Result<Model3d, Error> {
    let mut mol = ObMol::parse(molfile, "mol").map_err(|_| Error::Unreadable)?;
    let energy = if constraints.is_empty() {
        mol.optimize_geometry(force_field.id(), steps)
            .ok_or(Error::ForceField(force_field))?
    } else {
        let mut config = Minimizer::new(force_field.id());
        config
            .algorithm(Algorithm::ConjugateGradients)
            .max_steps(steps)
            .constraints(to_openbabel(constraints, mol.num_atoms()));
        mol.optimize_geometry_with(&config)
            .ok_or(Error::ForceField(force_field))?;
        // The minimiser's energy includes the restraint penalty; report the molecule's own.
        mol.energy(force_field.id())
            .ok_or(Error::ForceField(force_field))?
    };
    model(&mol, force_field, energy)
}

/// The single-point energy of a model's geometry.
pub fn energy(molfile: &str, force_field: ForceField) -> Result<f64, Error> {
    let mol = ObMol::parse(molfile, "mol").map_err(|_| Error::Unreadable)?;
    mol.energy(force_field.id())
        .filter(|e| e.is_finite())
        .ok_or(Error::ForceField(force_field))
}

/// How stiff restraints are. OpenBabel uses one factor for the whole set, and a torsion
/// restraint is a cosine well only `0.002 × factor` deep that the force field's own torsion
/// term outweighs at the default 50 000; at 1e8 a pinned dihedral holds, and distance and angle
/// restraints (harmonic) still behave. Measured in chem_3d_builder_rs.
const FORCE_FACTOR: f64 = 1.0e8;

fn to_openbabel(constraints: &[Constraint], atoms: u32) -> Constraints {
    let mut ob = Constraints::new();
    let n = atoms as usize;
    // Indices are checked against the molecule, so the casts below cannot truncate.
    let i = |a: usize| a as u32;
    for c in constraints.iter().filter(|c| c.is_valid_for(n)) {
        match *c {
            Constraint::Fix(a) => ob.fix_atom(i(a)),
            Constraint::Distance([a, b], length) => ob.distance(i(a), i(b), length),
            Constraint::Angle([a, b, c], degrees) => ob.angle(i(a), i(b), i(c), degrees),
            Constraint::Torsion([a, b, c, d], degrees) => {
                ob.torsion(i(a), i(b), i(c), i(d), degrees)
            }
        };
    }
    ob.force_factor(FORCE_FACTOR);
    ob
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::three_d::{edit, geom};

    /// Pentane, built and relaxed by OpenBabel.
    fn pentane() -> Model3d {
        let mut mol = ObMol::parse("CCCCC", "smi").expect("parse");
        mol.add_hydrogens();
        assert!(mol.generate_3d());
        let energy = mol.energy("UFF").expect("energy");
        model(&mol, ForceField::Uff, energy).expect("model")
    }

    /// [`geom`]'s measurements and edits agree with OpenBabel's — the sign of the dihedral in
    /// particular, since a restraint typed in the panel goes to OpenBabel as is.
    #[test]
    fn geometry_agrees_with_openbabel() {
        let mut m = pentane();
        let g = edit::graph(&m);
        let chain = [0usize, 1, 2, 3];
        let coords =
            geom::set_dihedral(&edit::coords(&m), &g, chain, 75.0, None).expect("dihedral");
        edit::set_coords(&mut m, &coords);
        let mol = ObMol::parse(&m.molfile, "mol").expect("edited molfile");
        let ours = geom::dihedral(coords[0], coords[1], coords[2], coords[3]);
        let theirs = mol.torsion(0, 1, 2, 3);
        assert!((ours - 75.0).abs() < 1e-3, "{ours}");
        assert!(
            (geom::wrap_degrees(ours - theirs)).abs() < 0.05,
            "ours {ours}, OpenBabel {theirs}"
        );
        assert!((mol.angle(0, 1, 2) - geom::angle(coords[0], coords[1], coords[2])).abs() < 0.05);
        // The edited molfile is what the energy is computed from.
        assert!(energy(&m.molfile, ForceField::Uff).is_ok());
    }

    #[test]
    fn a_restrained_distance_holds_through_optimisation() {
        let m = pentane();
        let restrained = [Constraint::Distance([0, 4], 3.2), Constraint::Fix(2)];
        let fixed = m.atoms[2].pos;
        let out = optimize(&m.molfile, ForceField::Uff, 500, &restrained).expect("optimise");
        assert!(
            (out.distance(0, 4) - 3.2).abs() < 0.1,
            "{}",
            out.distance(0, 4)
        );
        assert!(
            geom::distance(out.atoms[2].pos, fixed) < 1e-3,
            "the fixed atom stayed"
        );
    }
}
