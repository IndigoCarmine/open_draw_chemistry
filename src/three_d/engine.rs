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
        let mut energy = relax(&mut mol, force_field, steps)?;
        // A drawing with a straight run of atoms (say a chain drawn along one line) often
        // comes out of the builder with that run still straight: sp³ centres at 180°. That is
        // a saddle point the minimiser cannot leave, because every force along the line is
        // symmetric, so the model keeps an absurd energy and its torsions are undefined —
        // a dihedral set or restrained there lands anywhere. A small kick off the line breaks
        // the symmetry and the same minimisation then relaxes it properly.
        for kick in 0..KICKS {
            let straight = model(&mol, force_field, energy)
                .map(|m| has_straightened_centre(&m))
                .unwrap_or(false);
            if !straight {
                break;
            }
            jitter(&mut mol, kick);
            energy = relax(&mut mol, force_field, steps)?;
        }
        match model(&mol, force_field, energy) {
            Ok(model) if has_straightened_centre(&model) => last = Error::GenerationFailed,
            Ok(model) => return Ok(model),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Minimise in rounds of `steps` until a round no longer lowers the energy noticeably (at most
/// [`RELAX_ROUNDS`]). One round is enough from a builder's usual output, but a geometry coming
/// off a saddle point is still sliding downhill after it — hundreds of kJ/mol above the
/// minimum — and would be handed back half-relaxed.
fn relax(mol: &mut ObMol, force_field: ForceField, steps: u32) -> Result<f64, Error> {
    let mut energy = mol
        .optimize_geometry(force_field.id(), steps)
        .ok_or(Error::ForceField(force_field))?;
    for _ in 1..RELAX_ROUNDS {
        let next = mol
            .optimize_geometry(force_field.id(), steps)
            .ok_or(Error::ForceField(force_field))?;
        let settled = (energy - next).abs() < 0.01 * energy.abs().max(1.0);
        energy = next;
        if settled {
            break;
        }
    }
    Ok(energy)
}

const RELAX_ROUNDS: u32 = 8;

/// How many times a straightened model is kicked and re-minimised before the next attempt.
const KICKS: u32 = 3;

fn has_straightened_centre(model: &Model3d) -> bool {
    let coords: Vec<[f64; 3]> = model.atoms.iter().map(|a| a.pos).collect();
    let elements: Vec<&str> = model.atoms.iter().map(|a| a.element.as_str()).collect();
    let graph = super::edit::graph(model);
    !super::geom::straightened_centres(&coords, &elements, &graph, |a, b| {
        super::edit::bond_order(model, a, b)
    })
    .is_empty()
}

/// Move every atom by up to 0.15 Å in each direction, deterministically (a fixed sequence
/// per `round`), so a builder's symmetric saddle point can relax.
fn jitter(mol: &mut ObMol, round: u32) {
    let mut state = 0x9E37_79B9_7F4A_7C15u64 ^ u64::from(round).wrapping_mul(0xD1B5_4A32_D192_ED03);
    let mut next = || {
        // xorshift64*: plenty for a reproducible kick.
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let x = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (x >> 11) as f64 / (1u64 << 53) as f64 * 0.3 - 0.15
    };
    let flat: Vec<f64> = mol
        .atoms()
        .flat_map(|a| {
            let (x, y, z) = a.coords();
            [x, y, z]
        })
        .map(|c| c + next())
        .collect();
    mol.set_coordinates(&flat);
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

    fn butane_drawing() -> chembuider_rs::Molecule {
        let mut drawing = chembuider_rs::Molecule::default();
        let mut prev = drawing.add_atom("C".to_owned(), [0.0, 0.0], 0);
        for i in 1..4 {
            let next = drawing.add_atom("C".to_owned(), [1.3 * i as f32, 0.0], 0);
            drawing.add_bond(prev, next, chembuider_rs::BondOrder::Single);
            prev = next;
        }
        drawing
    }

    /// A chain drawn along one straight line used to come out of the builder about half the
    /// time still straight (C–C–C at 180°, E ≈ 470 kJ/mol under UFF): a saddle point that
    /// minimisation cannot leave, with undefined torsions — so a dihedral set and restrained
    /// there drifted anywhere on optimisation. Generation must never hand one back.
    #[test]
    fn a_straight_drawing_gives_a_relaxed_chain() {
        let drawing = butane_drawing();
        for _ in 0..12 {
            let m = crate::three_d::generate(
                &drawing,
                &chembuider_rs::Config::default(),
                ForceField::Uff,
            )
            .expect("generate");
            assert!(
                !has_straightened_centre(&m),
                "a straight sp3 centre was kept"
            );
            let c = edit::coords(&m);
            let bend = geom::angle(c[0], c[1], c[2]);
            assert!((100.0..125.0).contains(&bend), "C-C-C {bend}");
            // Relaxed butane is ~3 kJ/mol (UFF); the straight saddle ~470, half-way off it ~300.
            assert!(m.energy < 30.0, "unrelaxed: E {}", m.energy);
        }
    }
}
