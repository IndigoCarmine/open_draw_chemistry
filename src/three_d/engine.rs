//! The OpenBabel side of [`super`]: building, minimising and writing models.
//!
//! Every call here is slow and serialised by OpenBabel's process-wide lock, so it belongs on
//! a background thread (see [`super::job`]).

use openbabel::{Algorithm, Constraints, Minimizer, Molecule as ObMol, StopReason};

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
        let mut energy = relax(&mut mol, force_field, steps, &[])?;
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
            energy = relax(&mut mol, force_field, steps, &[])?;
        }
        match model(&mol, force_field, energy) {
            Ok(model) if has_straightened_centre(&model) => last = Error::GenerationFailed,
            Ok(model) => return Ok(model),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Minimise holding `constraints` (none for a plain relaxation), spending at most
/// [`BUDGET`] × `steps` minimiser steps over OpenBabel's restarts (`steps` for `minimize_rs`,
/// which needs none), and return the molecule's own energy — without the
/// restraint penalty — at the geometry reached, as [`energy`] reports it.
///
/// OpenBabel's minimisers (steepest descent, conjugate gradients, L-BFGS) all stop as soon as
/// one step lowers the energy by less than 1e-6 while the *smallest* per-atom gradient is
/// under 0.1 (`maxgrad` in forcefield.cpp keeps a minimum, not a maximum). That holds the first
/// time a line search makes no progress — often a few steps into the thousands asked for, well
/// short of the minimum — and their conjugate gradients, which mix the previous direction in
/// atom by atom, get there especially often. So a run is restarted from where it stopped (see
/// [`descend`]), and:
///
/// * L-BFGS does the work: restarted, it reaches the minimum quickly, restraints as stiff as
///   [`FORCE_FACTOR`] makes them included.
/// * Under UFF the openbabel crate refuses L-BFGS (it corrupts the heap with that force field).
///   Without restraints, the crate's own minimiser over the force field's terms (`minimize_rs`)
///   does the work: it stops only on a vanishing gradient or an energy that has stopped
///   changing for two steps running, so it needs no restarting. (It differentiates
///   numerically, so it is slow on a large molecule: ~10 ms a step for a hundred atoms.) It takes no restraints, so with them it
///   is OpenBabel's steepest descent: slower, but OpenBabel's conjugate gradients stall far
///   above the restrained minimum.
fn relax(
    mol: &mut ObMol,
    force_field: ForceField,
    steps: u32,
    constraints: &[Constraint],
) -> Result<f64, Error> {
    let failed = || Error::ForceField(force_field);
    let mut left = steps.saturating_mul(BUDGET);
    let algorithm = match force_field {
        ForceField::Uff if constraints.is_empty() => {
            mol.minimize_rs(
                force_field.id(),
                Algorithm::ConjugateGradients,
                steps,
                ECONV,
            )
            .ok_or_else(failed)?;
            return mol.energy(force_field.id()).ok_or_else(failed);
        }
        ForceField::Uff => Algorithm::SteepestDescent,
        ForceField::Mmff94 | ForceField::Mmff94s | ForceField::Gaff => Algorithm::Lbfgs,
    };
    if constraints.is_empty() {
        descend(mol, force_field, algorithm, &[], None, &mut left).ok_or_else(failed)?;
        return mol.energy(force_field.id()).ok_or_else(failed);
    }
    // Restraints at their full stiffness leave the minimiser hardly any room: their curvature
    // (2e8 per Å² for a distance) demands steps far below the first ones a fresh run tries, so
    // a restart often cannot take a single one and the structure stays a few tenths of a
    // kcal/mol above its restrained minimum. Under softer ones the rest of the structure moves
    // freely and the restrained values end up within about a degree; the full stiffness then
    // has only that to correct. (The scan does the same, see `scan::energy`.) Another such pass
    // still finds a little more; passes repeat until one no longer lowers the energy.
    let mut energy = f64::INFINITY;
    for _ in 0..RESTRAINED_PASSES {
        for factor in [Some(SOFT_FORCE_FACTOR), None] {
            descend(mol, force_field, algorithm, constraints, factor, &mut left)
                .ok_or_else(failed)?;
        }
        let reached = mol.energy(force_field.id()).ok_or_else(failed)?;
        let settled = energy - reached < SETTLED_ENERGY || left == 0;
        energy = reached;
        if settled {
            break;
        }
    }
    Ok(energy)
}

/// Run OpenBabel's `algorithm` again and again from where it stopped, holding `constraints`
/// (at `force_factor` instead of [`FORCE_FACTOR`] if given), until a run no longer moves the atoms or the `left` steps are spent (and deducted). A run
/// that does not move them is answered with steepest descent before believing it: L-BFGS
/// trials only short steps, and on some structures (taxol under GAFF) cannot take its first
/// one where a longer step goes downhill. `None` if the force field cannot be set up.
fn descend(
    mol: &mut ObMol,
    force_field: ForceField,
    algorithm: Algorithm,
    constraints: &[Constraint],
    force_factor: Option<f64>,
    left: &mut u32,
) -> Option<()> {
    let run = |mol: &mut ObMol, algorithm: Algorithm, left: &mut u32| -> Option<bool> {
        let mut config = Minimizer::new(force_field.id());
        config
            .algorithm(algorithm)
            .max_steps(*left)
            .steps_per_frame(FRAME_STEPS);
        if !constraints.is_empty() {
            let mut restraints = to_openbabel(constraints, mol.num_atoms());
            if let Some(factor) = force_factor {
                restraints.force_factor(factor);
            }
            config.constraints(restraints);
        }
        let before = coordinates(mol);
        let done = mol.minimize(&config);
        if done.stop_reason() == StopReason::Failed {
            return None;
        }
        *left = left.saturating_sub(done.steps_taken().max(1));
        Some(rms_shift(&before, &coordinates(mol)) >= SETTLED_SHIFT)
    };
    while *left > 0 {
        if run(mol, algorithm, left)? {
            continue;
        }
        if algorithm == Algorithm::SteepestDescent || *left == 0 {
            break;
        }
        if !run(mol, Algorithm::SteepestDescent, left)? {
            break;
        }
    }
    Some(())
}

/// How many times its `steps` a minimisation may spend in all, over its restarts and stages.
const BUDGET: u32 = 2;

/// Steps between the frames a run records; only its step count is used.
const FRAME_STEPS: u32 = 50;

/// The restraints' stiffness while the structure first relaxes around them (see [`relax`]).
const SOFT_FORCE_FACTOR: f64 = 1.0e6;

/// At most this many soft-then-stiff passes for a restrained minimisation; one that lowers the
/// energy by less than [`SETTLED_ENERGY`] (force field units) is the last.
const RESTRAINED_PASSES: u32 = 4;
const SETTLED_ENERGY: f64 = 1.0e-3;

/// The energy change per step (force field units) under which `minimize_rs` counts two steps
/// running as converged.
const ECONV: f64 = 1.0e-8;

/// A run that moves the atoms less than this (RMS, Å) found nothing left to do.
const SETTLED_SHIFT: f64 = 1.0e-6;

fn coordinates(mol: &ObMol) -> Vec<f64> {
    mol.atoms()
        .flat_map(|a| {
            let (x, y, z) = a.coords();
            [x, y, z]
        })
        .collect()
}

/// Root-mean-square displacement of the atoms between two flat coordinate lists.
fn rms_shift(a: &[f64], b: &[f64]) -> f64 {
    let atoms = (a.len() / 3).max(1) as f64;
    (a.iter().zip(b).map(|(p, q)| (p - q).powi(2)).sum::<f64>() / atoms).sqrt()
}

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
    let energy = relax(&mut mol, force_field, steps, constraints)?;
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

pub(super) fn to_openbabel(constraints: &[Constraint], atoms: u32) -> Constraints {
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
    use crate::three_d::{OPTIMIZE_STEPS, edit, geom};

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

    /// Structures with the groups a force field types specially: alkane, aromatic ring,
    /// carboxylic acid, amide, carboxylate, ammonium, a salt (two ions), a nitro group, an
    /// aromatic cation.
    const TYPED: [(&str, &str); 9] = [
        ("butane", "CCCC"),
        ("benzene", "c1ccccc1"),
        ("acetic acid", "CC(=O)O"),
        ("acetamide", "CC(=O)N"),
        ("acetate", "CC(=O)[O-]"),
        ("methylammonium", "C[NH3+]"),
        ("ammonium acetate", "[NH4+].CC(=O)[O-]"),
        ("nitromethane", "C[N+](=O)[O-]"),
        ("pyridinium", "c1cc[nH+]cc1"),
    ];

    /// The 2D molfile the app hands [`generate`] for `smiles` (OpenBabel's layout of it).
    fn drawing(smiles: &str) -> String {
        let mut mol = ObMol::parse(smiles, "smi").expect("smiles");
        assert!(mol.generate_2d());
        mol.write("mol").expect("molfile")
    }

    fn built(smiles: &str, force_field: ForceField) -> Model3d {
        generate(&drawing(smiles), force_field, super::super::GENERATE_STEPS)
            .unwrap_or_else(|e| panic!("{smiles} under {force_field}: {e:?}"))
    }

    /// Element, formal charge, aromaticity, OpenBabel atom type.
    type TypedAtom = (u32, i32, bool, String);

    /// What a force field's typing reads off a molecule: per atom its element, charge,
    /// aromaticity and OpenBabel type; the bonds with their orders, in a fixed order.
    fn typing(mol: &ObMol) -> (Vec<TypedAtom>, Vec<(u32, u32, u32)>) {
        let atoms = mol
            .atoms()
            .map(|a| {
                (
                    a.atomic_number(),
                    a.formal_charge(),
                    a.is_aromatic(),
                    a.type_name(),
                )
            })
            .collect();
        let mut bonds: Vec<_> = mol
            .bonds()
            .map(|b| {
                let (i, j) = (b.begin_atom_index(), b.end_atom_index());
                (i.min(j), i.max(j), b.order())
            })
            .collect();
        bonds.sort_unstable();
        (atoms, bonds)
    }

    /// Root-mean-square of the force field's gradient over all coordinates (energy unit per
    /// Å), by central differences of OpenBabel's own energy.
    fn rms_gradient(molfile: &str, force_field: ForceField) -> f64 {
        let mut mol = ObMol::parse(molfile, "mol").expect("molfile");
        let x0: Vec<f64> = mol
            .atoms()
            .flat_map(|a| {
                let (x, y, z) = a.coords();
                [x, y, z]
            })
            .collect();
        let h = 1e-5;
        let mut sum = 0.0;
        for i in 0..x0.len() {
            let mut x = x0.clone();
            x[i] = x0[i] + h;
            mol.set_coordinates(&x);
            let up = mol.energy(force_field.id()).expect("energy");
            x[i] = x0[i] - h;
            mol.set_coordinates(&x);
            let down = mol.energy(force_field.id()).expect("energy");
            sum += ((up - down) / (2.0 * h)).powi(2);
        }
        (sum / x0.len() as f64).sqrt()
    }

    /// Root-mean-square displacement between two models of the same molecule (Å).
    fn shift(a: &Model3d, b: &Model3d) -> f64 {
        let n = a.atoms.len();
        ((0..n)
            .map(|i| geom::distance(a.atoms[i].pos, b.atoms[i].pos).powi(2))
            .sum::<f64>()
            / n as f64)
            .sqrt()
    }

    /// `model` with every atom kicked off its relaxed position (see [`jitter`]).
    fn kicked(model: &Model3d, force_field: ForceField) -> Model3d {
        let mut mol = ObMol::parse(&model.molfile, "mol").expect("molfile");
        jitter(&mut mol, 7);
        let energy = mol.energy(force_field.id()).expect("energy");
        super::model(&mol, force_field, energy).expect("model")
    }

    /// How far apart two energies of one geometry may be (times 1 + |E|) when one is computed
    /// from the molfile, whose coordinates are rounded to 1e-4 Å: the residual gradient of a
    /// relaxed model times that rounding. A wrong atom type or charge moves it far more.
    const ROUNDING: f64 = 1e-3;

    /// Every force field sets up on every typed structure, and the molfile a model carries —
    /// what Optimize and the energy readout parse — gives the force field the same molecule
    /// generation typed: same atoms, charges, aromaticity, atom types and bond orders, so
    /// the same energy at the same geometry.
    #[test]
    fn every_force_field_types_the_model_molfile_as_generated() {
        for (name, smiles) in TYPED {
            let mut fresh = ObMol::parse(&drawing(smiles), "mol").expect("drawing");
            fresh.add_hydrogens();
            for force_field in ForceField::ALL {
                let model = built(smiles, force_field);
                let read = ObMol::parse(&model.molfile, "mol").expect("model molfile");
                assert_eq!(
                    typing(&read),
                    typing(&fresh),
                    "{name}: the molfile changed it"
                );
                let again = energy(&model.molfile, force_field)
                    .unwrap_or_else(|e| panic!("{name} under {force_field}: {e:?}"));
                // Only as close as the molfile's coordinates, written to 1e-4 Å.
                assert!(
                    (again - model.energy).abs() < ROUNDING * (1.0 + model.energy.abs()),
                    "{name} under {force_field}: generated E {}, read back {again}",
                    model.energy
                );
            }
        }
    }

    /// Optimize must finish the minimisation. OpenBabel's own minimisers stop after a handful
    /// of their 2500 steps: they stop once the energy changes by less than 1e-6 in one step
    /// and the *smallest* per-atom gradient is below 0.1 (`maxgrad` keeps a minimum), which
    /// holds the first time a line search makes no progress — leaving ibuprofen 0.5 kJ/mol
    /// above its minimum under GAFF and every further Optimize creeping down a little more.
    #[test]
    fn optimize_reaches_the_minimum() {
        for (name, smiles) in [
            // Not ammonium acetate: OpenBabel's UFF has no electrostatics, so its two ions
            // barely attract and sit on a plateau no minimiser settles on reliably.
            ("acetamide", "CC(=O)N"),
            ("sodium acetate", "[Na+].CC(=O)[O-]"),
            ("ibuprofen", "CC(C)Cc1ccc(cc1)C(C)C(=O)O"),
        ] {
            for force_field in ForceField::ALL {
                let model = built(smiles, force_field);
                let once = optimize(&model.molfile, force_field, OPTIMIZE_STEPS, &[]).unwrap();
                let twice = optimize(&once.molfile, force_field, OPTIMIZE_STEPS, &[]).unwrap();
                let what = format!("{name} under {force_field}");
                assert!(
                    once.energy <= model.energy + ROUNDING * (1.0 + model.energy.abs()),
                    "{what}: E rose {} -> {}",
                    model.energy,
                    once.energy
                );
                assert!(
                    (once.energy - twice.energy).abs() < 0.02,
                    "{what}: a second Optimize still moved E {} -> {}",
                    once.energy,
                    twice.energy
                );
                let g = rms_gradient(&once.molfile, force_field);
                assert!(g < 0.3, "{what}: RMS gradient {g} after Optimize");
                // The energy reported is the energy of the geometry handed back.
                let at = energy(&once.molfile, force_field).unwrap();
                assert!(
                    (at - once.energy).abs() < ROUNDING * (1.0 + at.abs()),
                    "{what}"
                );
                // An independent minimiser, given far more steps from the same start, gets no
                // lower: OpenBabel's L-BFGS, or its steepest descent under UFF (which refuses
                // L-BFGS).
                let algorithm = match force_field {
                    ForceField::Uff => Algorithm::SteepestDescent,
                    _ => Algorithm::Lbfgs,
                };
                let mut mol = ObMol::parse(&model.molfile, "mol").unwrap();
                let mut config = Minimizer::new(force_field.id());
                config.algorithm(algorithm).max_steps(20_000);
                mol.optimize_geometry_with(&config).unwrap();
                let best = mol.energy(force_field.id()).unwrap();
                assert!(
                    once.energy < best + 0.01,
                    "{what}: Optimize stopped at {}, {algorithm:?} reaches {best}",
                    once.energy
                );
            }
        }
    }

    /// From a disturbed geometry, Optimize moves the atoms back down to a minimum, with and
    /// without restraints; a restrained one holds its restraints and is settled (another
    /// Optimize under them changes little). With restraints as stiff as [`FORCE_FACTOR`]
    /// makes them, OpenBabel's conjugate gradients used to stop within a step or two — or
    /// crawl through all of them — far above the restrained minimum.
    #[test]
    fn optimize_moves_the_atoms_with_and_without_restraints() {
        // Ibuprofen: 0 and 1 are a methyl and its CH, 3 the CH2 and 4 the ring carbon on it.
        let smiles = "CC(C)Cc1ccc(cc1)C(C)C(=O)O";
        let restraint_sets = [
            vec![],
            vec![Constraint::Fix(4)],
            vec![Constraint::Distance([0, 4], 3.0)],
            vec![Constraint::Torsion([0, 1, 3, 4], 60.0)],
        ];
        for force_field in ForceField::ALL {
            let relaxed = built(smiles, force_field);
            let start = kicked(&relaxed, force_field);
            for restraints in &restraint_sets {
                let what = format!("{force_field} holding {restraints:?}");
                let once = optimize(&start.molfile, force_field, OPTIMIZE_STEPS, restraints)
                    .unwrap_or_else(|e| panic!("{what}: {e:?}"));
                assert!(
                    shift(&start, &once) > 0.05,
                    "{what}: the atoms barely moved"
                );
                assert!(
                    once.energy < start.energy - 10.0,
                    "{what}: E {} -> {}",
                    start.energy,
                    once.energy
                );
                let c = edit::coords(&once);
                for r in restraints {
                    match *r {
                        Constraint::Fix(a) => assert!(
                            geom::distance(c[a], start.atoms[a].pos) < 1e-3,
                            "{what}: the fixed atom moved"
                        ),
                        Constraint::Distance([a, b], d) => {
                            let at = geom::distance(c[a], c[b]);
                            assert!((at - d).abs() < 0.01, "{what}: distance {at}");
                        }
                        Constraint::Torsion([a, b, cc, d], t) => {
                            let at = geom::dihedral(c[a], c[b], c[cc], c[d]);
                            assert!(
                                geom::wrap_degrees(at - t).abs() < 1.0,
                                "{what}: dihedral {at}"
                            );
                        }
                        Constraint::Angle(..) => unreachable!(),
                    }
                }
                let twice =
                    optimize(&once.molfile, force_field, OPTIMIZE_STEPS, restraints).unwrap();
                // Restrained, a first Optimize ends up to ~0.1 above the restrained minimum (and
                // a rerun may still turn a floppy group a little): at full stiffness OpenBabel's
                // minimisers can hardly step (see `relax`). Before, it ended tens to hundreds
                // above it.
                let settled = if restraints.is_empty() { 0.02 } else { 0.2 };
                assert!(
                    (once.energy - twice.energy).abs() < settled,
                    "{what}: not settled, a second Optimize moved E {} -> {}",
                    once.energy,
                    twice.energy
                );
                if restraints.is_empty() {
                    assert!(
                        shift(&once, &twice) < 0.01,
                        "{what}: a second Optimize moved the atoms"
                    );
                }
            }
        }
    }
}
