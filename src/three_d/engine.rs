//! The molecular mechanics behind [`super`]: building, minimising and writing models with
//! molecule_optimizer_rs (UFF and MMFF94/94s ported from RDKit, ETKDG for the 3D build).
//!
//! Every call here takes a while on a large structure, so it belongs on a background thread
//! (see [`super::job`]).

use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};

use molecule_optimizer_rs::calc::{self, OptimizeOptions};
use molecule_optimizer_rs::distgeom::EmbedParams;
use molecule_optimizer_rs::ff::mmff::{MmffBuilder, MmffVariant};
use molecule_optimizer_rs::ff::uff::UffBuilder;
use molecule_optimizer_rs::ff::{self, ForceField as Field, ForceFieldBuilder};
use molecule_optimizer_rs::io::{self, Format};
use molecule_optimizer_rs::optim::{self, IterationInfo, MinimizeOptions, MinimizeStatus};
use molecule_optimizer_rs::{BondOrder, Molecule};

use super::{Atom3d, Bond3d, Constraint, Error, Export3d, ForceField, Model3d};

/// The embedding seeds tried in turn until one gives a usable model.
const SEEDS: [i32; 3] = [42, 7, 1234];

/// Two atoms closer than this (Å) mean the build failed; the shortest real bond, H–H, is 0.74.
const MIN_DISTANCE: f64 = 0.5;

/// How many times its `steps` a freshly built model may take to relax: an embedded structure
/// starts further from its minimum than one the user already relaxed.
const GENERATE_BUDGET: u32 = 8;

/// How many times its `steps` a restrained Optimize (or a relaxed scan's point) may take: the
/// structure first has to be pulled to the restrained values.
pub(crate) const RESTRAINED_BUDGET: u32 = 2;

/// The restraints' stiffness: kcal/mol/Å² for a distance (½·k·Δ²), kcal/mol/deg² for an angle
/// or a dihedral (k·Δ²) — what the library's own scans use. A restrained distance then ends
/// within a few thousandths of an ångström of its value, an angle within a few hundredths of a
/// degree.
const DISTANCE_FORCE: f64 = 1.0e4;
const ANGLE_FORCE: f64 = 1.0e3;

/// Non-bonded terms over every pair of atoms, wherever they start: a scan moves molecules
/// that begin far apart up against each other, and a salt's ions interact.
const NO_CUTOFF: f64 = 1.0e9;

/// A molfile as a molecule the force fields can type (rings, aromaticity, hybridisation
/// perceived). Hydrogens the molfile leaves implicit stay implicit.
pub(crate) fn read(molfile: &str) -> Result<Molecule, Error> {
    io::read_molfile(molfile).map_err(|_| Error::Unreadable)
}

/// The library's builder for `force_field`.
fn builder(force_field: ForceField) -> Box<dyn ForceFieldBuilder> {
    match force_field {
        ForceField::Mmff94 | ForceField::Mmff94s => Box::new(MmffBuilder {
            variant: if force_field == ForceField::Mmff94 {
                MmffVariant::Mmff94
            } else {
                MmffVariant::Mmff94s
            },
            nonbonded_thresh: NO_CUTOFF,
            ignore_interfrag_interactions: false,
            ..MmffBuilder::default()
        }),
        ForceField::Uff => Box::new(UffBuilder {
            vdw_thresh: NO_CUTOFF,
            ignore_interfrag_interactions: false,
        }),
    }
}

/// `force_field` set up for `mol` holding `constraints`, or [`Error::ForceField`] when it has
/// no type or parameter for one of its atoms.
pub(crate) fn field(
    mol: &Molecule,
    force_field: ForceField,
    constraints: &[Constraint],
) -> Result<Box<dyn Field>, Error> {
    builder(force_field)
        .build_dyn(mol, &restraints(constraints, mol.num_atoms()))
        .map_err(|_| Error::ForceField(force_field))
}

/// The app's constraints as the library's restraints, those that do not fit the molecule left
/// out.
fn restraints(constraints: &[Constraint], atoms: usize) -> Vec<ff::Constraint> {
    constraints
        .iter()
        .filter(|c| c.is_valid_for(atoms))
        .map(|c| match *c {
            Constraint::Fix(i) => ff::Constraint::Fixed { i },
            Constraint::Distance([i, j], length) => {
                ff::Constraint::distance(i, j, length, DISTANCE_FORCE)
            }
            Constraint::Angle([i, j, k], degrees) => {
                ff::Constraint::angle(i, j, k, degrees, ANGLE_FORCE)
            }
            Constraint::Torsion([i, j, k, l], degrees) => {
                ff::Constraint::dihedral(i, j, k, l, degrees, ANGLE_FORCE)
            }
        })
        .collect()
}

/// How a minimisation ended (see [`relax`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Relaxed {
    /// The geometry reached, the molecule's own energy there (the restraints' penalty left
    /// out), and whether it is a minimum: the minimiser converged before the steps ran out.
    Done {
        coords: Vec<[f64; 3]>,
        energy: f64,
        converged: bool,
    },
    /// The cancel flag was raised part-way.
    Cancelled,
}

/// A restart that lowers the minimised energy (restraints included) by less than this
/// (kcal/mol) finds the minimum reached (see [`relax`]).
const SETTLED_ENERGY: f64 = 1.0e-4;

/// Minimise `mol` from `coords` under `force_field` holding `constraints`, for at most `steps`
/// iterations of the library's BFGS in all, looking at `cancel` (if any) after every one.
///
/// BFGS stops on RDKit's convergence test, which scales the gradient by the energy and the
/// coordinates: on a large, restrained structure it can stop on a slope that still leads half
/// a kcal/mol lower. So a run that converged is restarted from where it stopped (with a fresh
/// Hessian), and the minimisation is converged once a restart no longer lowers the energy by
/// [`SETTLED_ENERGY`].
pub(crate) fn relax(
    mol: &Molecule,
    coords: &[[f64; 3]],
    force_field: ForceField,
    constraints: &[Constraint],
    steps: u32,
    cancel: Option<&AtomicBool>,
) -> Result<Relaxed, Error> {
    let builder = builder(force_field);
    let constraints = restraints(constraints, mol.num_atoms());
    let mut watch = |_: &IterationInfo<'_>| {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let failed = || Error::ForceField(force_field);
    let mut left = steps.max(1) as usize;
    let mut at = coords.to_vec();
    let mut before = f64::INFINITY;
    #[cfg(test)]
    let mut trace: Vec<String> = Vec::new();
    loop {
        let options = OptimizeOptions {
            minimize: MinimizeOptions {
                max_iters: left,
                ..MinimizeOptions::default()
            },
            constraints: constraints.clone(),
        };
        let done =
            calc::optimize::optimize_coords(mol, &at, builder.as_ref(), &options, &mut watch)
                .map_err(|_| failed())?;
        if done.status == MinimizeStatus::Cancelled {
            return Ok(Relaxed::Cancelled);
        }
        if !done.ff_energy.is_finite() {
            return Err(failed());
        }
        at = done.coords;
        left = left.saturating_sub(done.iterations.max(1));
        let converged = done.status == MinimizeStatus::Converged;
        let settled = converged && before - done.energy < SETTLED_ENERGY;
        #[cfg(test)]
        trace.push(format!(
            "it={} status={:?} E={:.10} ff={:.10} drop={:.3e} left={}",
            done.iterations,
            done.status,
            done.energy,
            done.ff_energy,
            before - done.energy,
            left
        ));
        before = done.energy;
        if settled || !converged || left == 0 {
            #[cfg(test)]
            if !settled {
                eprintln!("RELAX-TRACE unconverged after {} runs (steps {steps}):", trace.len());
                let n = trace.len();
                for (k, t) in trace.iter().enumerate() {
                    if k < 5 || k + 15 >= n {
                        eprintln!("RELAX-TRACE   #{k}: {t}");
                    } else if k == 5 {
                        eprintln!("RELAX-TRACE   ...");
                    }
                }
            }
            return Ok(Relaxed::Done {
                coords: at,
                energy: done.ff_energy,
                converged: settled,
            });
        }
    }
}

/// [`relax`] where nothing cancels it: Generate and Optimize run to the end.
fn relax_all(
    mol: &Molecule,
    coords: &[[f64; 3]],
    force_field: ForceField,
    constraints: &[Constraint],
    steps: u32,
) -> Result<(Vec<[f64; 3]>, f64), Error> {
    match relax(mol, coords, force_field, constraints, steps, None)? {
        Relaxed::Done { coords, energy, .. } => Ok((coords, energy)),
        Relaxed::Cancelled => unreachable!("nothing raises the flag"),
    }
}

/// Up to `steps` iterations of the library's BFGS on `field` from the flat coordinates `pos`
/// (moved in place), without restraints or a cancel flag: one segment of a held relaxation
/// (see `scan::energy`). The iterations taken, one at least.
pub(crate) fn descend(field: &dyn Field, pos: &mut [f64], steps: u32) -> u32 {
    let options = MinimizeOptions {
        max_iters: steps.max(1) as usize,
        ..MinimizeOptions::default()
    };
    let done = optim::minimize_with(field, pos, &options, false, &mut optim::NoObserver);
    (done.iterations as u32).max(1)
}

/// Build a 3D model from a 2D molfile: add the hydrogens, embed the structure with ETKDG (the
/// stereo its wedges and double bonds show kept), then relax it under `force_field` for up to
/// [`GENERATE_BUDGET`] × `steps` iterations.
pub fn generate(molfile: &str, force_field: ForceField, steps: u32) -> Result<Model3d, Error> {
    let drawing = read(molfile)?;
    let mut last = Error::GenerationFailed;
    for seed in SEEDS {
        let Some(built) = embed(&drawing, seed) else {
            continue;
        };
        let budget = steps.saturating_mul(GENERATE_BUDGET);
        let (coords, energy) = relax_all(&built, &built.coords, force_field, &[], budget)?;
        match model(&built, &coords, force_field, energy) {
            // An sp³ centre left straight is a saddle point the minimiser cannot leave; another
            // embedding does not start there.
            Ok(model) if has_straightened_centre(&model) => last = Error::GenerationFailed,
            Ok(model) => return Ok(model),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// `drawing` embedded in 3D with `seed`. ETKDG starts from the distance matrix's
/// eigenvectors, which fails on a long flexible structure (a C166 chain, say); then it starts
/// from random coordinates, as RDKit advises for large molecules.
fn embed(drawing: &Molecule, seed: i32) -> Option<Molecule> {
    [false, true].into_iter().find_map(|use_random_coords| {
        let params = EmbedParams {
            random_seed: seed,
            use_random_coords,
            ..EmbedParams::etkdg_v3()
        };
        calc::build_3d(drawing, &params, &mut optim::NoObserver).ok()
    })
}

fn has_straightened_centre(model: &Model3d) -> bool {
    let coords: Vec<[f64; 3]> = model.atoms.iter().map(|a| a.pos).collect();
    let elements: Vec<&str> = model.atoms.iter().map(|a| a.element.as_str()).collect();
    let graph = super::edit::graph(model);
    !super::geom::straightened_centres(&coords, &elements, &graph, |a, b| {
        super::edit::bond_order(model, a, b)
    })
    .is_empty()
}

/// Minimise an existing 3D model (a molfile written by [`model`]) further, holding
/// `constraints`, for at most `steps` iterations — [`RESTRAINED_BUDGET`] times that with
/// restraints.
pub fn optimize(
    molfile: &str,
    force_field: ForceField,
    steps: u32,
    constraints: &[Constraint],
) -> Result<Model3d, Error> {
    let mol = read(molfile)?;
    let steps = if constraints.is_empty() {
        steps
    } else {
        steps.saturating_mul(RESTRAINED_BUDGET)
    };
    let (coords, energy) = relax_all(&mol, &mol.coords, force_field, constraints, steps)?;
    model(&mol, &coords, force_field, energy)
}

/// The single-point energy of a model's geometry (kcal/mol).
pub fn energy(molfile: &str, force_field: ForceField) -> Result<f64, Error> {
    let mol = read(molfile)?;
    let field = field(&mol, force_field, &[])?;
    Some(field.energy(&mol.flat_coords()))
        .filter(|e| e.is_finite())
        .ok_or(Error::ForceField(force_field))
}

/// Write a model's molfile out in `format`.
pub fn export(molfile: &str, format: Export3d) -> Result<String, Error> {
    let mol = read(molfile)?;
    let format = match format {
        Export3d::Mol => Format::Mol,
        Export3d::Sdf => Format::Sdf,
        Export3d::Xyz => Format::Xyz,
        Export3d::Pdb => Format::Pdb,
        Export3d::Mol2 => Format::Mol2,
    };
    Ok(io::write_str(&mol, None, format))
}

/// The model of `mol` at `coords`, or [`Error::GenerationFailed`] when its geometry or energy
/// is unusable.
pub(crate) fn model(
    mol: &Molecule,
    coords: &[[f64; 3]],
    force_field: ForceField,
    energy: f64,
) -> Result<Model3d, Error> {
    let model = assemble(mol, coords, force_field, energy);
    if usable(&model) {
        Ok(model)
    } else {
        Err(Error::GenerationFailed)
    }
}

/// The model of `mol` at `coords`, as it is (a file opened keeps its geometry, usable or not).
pub(crate) fn assemble(
    mol: &Molecule,
    coords: &[[f64; 3]],
    force_field: ForceField,
    energy: f64,
) -> Model3d {
    let mut placed = mol.clone();
    placed.coords = coords.to_vec();
    // Bond orders as the molfile has them: Kekulé, not aromatic.
    let kekule = molecule_optimizer_rs::perception::kekulized_copy(&placed)
        .unwrap_or_else(|_| placed.clone());
    let atoms: Vec<Atom3d> = placed
        .atoms()
        .iter()
        .zip(coords)
        .map(|(a, &pos)| Atom3d {
            element: a.symbol().to_owned(),
            pos,
        })
        .collect();
    let bonds: Vec<Bond3d> = kekule
        .bonds()
        .iter()
        .map(|b| Bond3d {
            begin: b.begin,
            end: b.end,
            order: match b.order {
                BondOrder::Double => 2,
                BondOrder::Triple => 3,
                BondOrder::Aromatic => 5,
                _ => 1,
            },
        })
        .collect();
    Model3d {
        formula: formula(&atoms),
        atoms,
        bonds,
        force_field,
        energy,
        energy_unit: ENERGY_UNIT.to_owned(),
        molfile: io::write_molblock(&placed),
    }
}

/// The unit every force field here reports its energy in.
pub(crate) const ENERGY_UNIT: &str = "kcal/mol";

/// The molecular formula in Hill order: C, then H, then the rest alphabetically (all
/// alphabetically when there is no carbon).
pub(crate) fn formula(atoms: &[Atom3d]) -> String {
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for a in atoms {
        *counts.entry(a.element.as_str()).or_default() += 1;
    }
    let mut out = String::new();
    let mut put = |element: &str, n: usize| {
        out.push_str(element);
        if n > 1 {
            out.push_str(&n.to_string());
        }
    };
    if let Some(c) = counts.remove("C") {
        put("C", c);
        if let Some(h) = counts.remove("H") {
            put("H", h);
        }
    }
    for (element, n) in counts {
        put(element, n);
    }
    out
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
pub(super) mod tests {
    use super::*;
    use crate::three_d::{GENERATE_STEPS, OPTIMIZE_STEPS, edit, geom};

    /// The model of a 3D molfile under `force_field`, as the 3D view holds it.
    pub(crate) fn model_from_molfile(molfile: &str, force_field: ForceField) -> Model3d {
        let mol = read(molfile).expect("molfile");
        let energy = field(&mol, force_field, &[])
            .expect("force field")
            .energy(&mol.flat_coords());
        model(&mol, &mol.coords, force_field, energy).expect("model")
    }

    /// A 2D molfile as a structure editor writes it (laid out by RDKit once), from
    /// `tests/data/drawn/`.
    fn drawing(name: &str) -> &'static str {
        macro_rules! drawn {
            ($($name:literal),*) => {
                match name {
                    $($name => include_str!(concat!("../../tests/data/drawn/", $name, ".mol")),)*
                    _ => panic!("no drawing of {name}"),
                }
            };
        }
        drawn!(
            "butane",
            "benzene",
            "acetic_acid",
            "acetamide",
            "acetate",
            "methylammonium",
            "ammonium_acetate",
            "nitromethane",
            "pyridinium",
            "pentane",
            "ethanol"
        )
    }

    fn built(name: &str, force_field: ForceField) -> Model3d {
        generate(drawing(name), force_field, GENERATE_STEPS)
            .unwrap_or_else(|e| panic!("{name} under {force_field}: {e:?}"))
    }

    fn pentane() -> Model3d {
        built("pentane", ForceField::Uff)
    }

    /// Structures with the groups a force field types specially: alkane, aromatic ring,
    /// carboxylic acid, amide, carboxylate, ammonium, a salt (two ions), a nitro group, an
    /// aromatic cation.
    const TYPED: [(&str, &str); 9] = [
        ("butane", "C4H10"),
        ("benzene", "C6H6"),
        ("acetic_acid", "C2H4O2"),
        ("acetamide", "C2H5NO"),
        ("acetate", "C2H3O2"),
        ("methylammonium", "CH6N"),
        ("ammonium_acetate", "C2H7NO2"),
        ("nitromethane", "CH3NO2"),
        ("pyridinium", "C5H6N"),
    ];

    /// Every force field sets up on every typed structure, adds the hydrogens it should, and
    /// the molfile a model carries — what Optimize and the energy readout read — gives the
    /// same energy at the same geometry.
    #[test]
    fn every_force_field_types_the_model_molfile_as_generated() {
        for (name, formula) in TYPED {
            for force_field in ForceField::ALL {
                let model = built(name, force_field);
                assert_eq!(model.formula, formula, "{name} under {force_field}");
                assert_eq!(model.energy_unit, "kcal/mol");
                let again = energy(&model.molfile, force_field)
                    .unwrap_or_else(|e| panic!("{name} under {force_field}: {e:?}"));
                // Only as close as the molfile's coordinates, written to 1e-4 Å.
                assert!(
                    (again - model.energy).abs() < 1e-2 * (1.0 + model.energy.abs()),
                    "{name} under {force_field}: generated E {}, read back {again}",
                    model.energy
                );
            }
        }
    }

    /// [`geom`]'s measurements agree with the library's: a dihedral restrained to a value the
    /// panel shows ends at that value as the panel measures it.
    #[test]
    fn a_restrained_dihedral_ends_where_the_panel_measures_it() {
        let m = pentane();
        for target in [75.0, -75.0, 180.0] {
            let out = optimize(
                &m.molfile,
                ForceField::Uff,
                OPTIMIZE_STEPS,
                &[Constraint::Torsion([0, 1, 2, 3], target)],
            )
            .expect("optimise");
            let c = edit::coords(&out);
            let at = geom::dihedral(c[0], c[1], c[2], c[3]);
            assert!(
                geom::wrap_degrees(at - target).abs() < 0.25,
                "target {target}, dihedral {at}"
            );
        }
    }

    #[test]
    fn a_restrained_distance_and_angle_hold_through_optimisation() {
        let m = pentane();
        let restrained = [
            Constraint::Distance([0, 4], 3.2),
            Constraint::Angle([1, 2, 3], 118.0),
            Constraint::Fix(2),
        ];
        let fixed = m.atoms[2].pos;
        for force_field in ForceField::ALL {
            let out =
                optimize(&m.molfile, force_field, OPTIMIZE_STEPS, &restrained).expect("optimise");
            let c = edit::coords(&out);
            assert!(
                (out.distance(0, 4) - 3.2).abs() < 0.01,
                "{force_field}: {}",
                out.distance(0, 4)
            );
            let angle = geom::angle(c[1], c[2], c[3]);
            assert!((angle - 118.0).abs() < 0.1, "{force_field}: {angle}");
            assert!(
                geom::distance(out.atoms[2].pos, fixed) < 1e-3,
                "{force_field}: the fixed atom moved"
            );
        }
    }

    /// An angle and a dihedral restrained together both hold (OpenBabel dropped the dihedral).
    #[test]
    fn an_angle_and_a_dihedral_hold_together() {
        let m = pentane();
        let restrained = [
            Constraint::Angle([0, 1, 2], 120.0),
            Constraint::Torsion([1, 2, 3, 4], 65.0),
        ];
        for force_field in ForceField::ALL {
            let out =
                optimize(&m.molfile, force_field, OPTIMIZE_STEPS, &restrained).expect("optimise");
            let c = edit::coords(&out);
            let angle = geom::angle(c[0], c[1], c[2]);
            let dihedral = geom::dihedral(c[1], c[2], c[3], c[4]);
            assert!((angle - 120.0).abs() < 0.1, "{force_field}: angle {angle}");
            assert!(
                geom::wrap_degrees(dihedral - 65.0).abs() < 0.25,
                "{force_field}: dihedral {dihedral}"
            );
        }
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

    /// A chain drawn along one straight line gives a relaxed chain, never one with an sp³
    /// centre left straight (a saddle point with undefined torsions).
    #[test]
    fn a_straight_drawing_gives_a_relaxed_chain() {
        let m = crate::three_d::generate(
            &butane_drawing(),
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
        // Relaxed butane is under 1 kcal/mol (UFF).
        assert!(m.energy < 5.0, "unrelaxed: E {}", m.energy);
    }

    /// Where the Optimize tests start: acetamide, sodium acetate and ibuprofen as OpenBabel's
    /// builder once placed them.
    const OPTIMIZE_STARTS: [(&str, ForceField, &str); 9] = {
        use ForceField::{Mmff94, Mmff94s, Uff};
        macro_rules! start {
            ($name:literal, $force_field:expr, $file:literal) => {
                (
                    $name,
                    $force_field,
                    include_str!(concat!("../../tests/data/optimize/", $file)),
                )
            };
        }
        [
            start!("acetamide", Mmff94, "acetamide_mmff94.mol"),
            start!("acetamide", Mmff94s, "acetamide_mmff94s.mol"),
            start!("acetamide", Uff, "acetamide_uff.mol"),
            start!("sodium acetate", Mmff94, "sodium_acetate_mmff94.mol"),
            start!("sodium acetate", Mmff94s, "sodium_acetate_mmff94s.mol"),
            start!("sodium acetate", Uff, "sodium_acetate_uff.mol"),
            start!("ibuprofen", Mmff94, "ibuprofen_mmff94.mol"),
            start!("ibuprofen", Mmff94s, "ibuprofen_mmff94s.mol"),
            start!("ibuprofen", Uff, "ibuprofen_uff.mol"),
        ]
    };

    /// Root-mean-square of the force field's gradient (kcal/mol/Å) at the model's own
    /// coordinates, by central differences of the energy.
    fn rms_gradient(model: &Model3d) -> f64 {
        let mol = read(&model.molfile).expect("molfile");
        let field = field(&mol, model.force_field, &[]).expect("force field");
        let x0: Vec<f64> = model.atoms.iter().flat_map(|a| a.pos).collect();
        let h = 1e-5;
        let mut sum = 0.0;
        for i in 0..x0.len() {
            let mut x = x0.clone();
            x[i] = x0[i] + h;
            let up = field.energy(&x);
            x[i] = x0[i] - h;
            let down = field.energy(&x);
            sum += ((up - down) / (2.0 * h)).powi(2);
        }
        (sum / x0.len() as f64).sqrt()
    }

    /// Optimize finishes the minimisation: it ends at a minimum (a small gradient), within its
    /// steps, lower than it started; a second Optimize hardly moves it; and the energy reported
    /// is the energy of the geometry handed back.
    #[test]
    fn optimize_reaches_the_minimum() {
        for (name, force_field, molfile) in OPTIMIZE_STARTS {
            let what = format!("{name} under {force_field}");
            let model = model_from_molfile(molfile, force_field);
            let once = optimize(&model.molfile, force_field, OPTIMIZE_STEPS, &[]).unwrap();
            let twice = optimize(&once.molfile, force_field, OPTIMIZE_STEPS, &[]).unwrap();
            assert!(
                once.energy <= model.energy + 1e-6,
                "{what}: E rose {} -> {}",
                model.energy,
                once.energy
            );
            let at = energy(&once.molfile, force_field).unwrap();
            assert!(
                (at - once.energy).abs() < 1e-2,
                "{what}: {at} vs {}",
                once.energy
            );
            let mol = read(&model.molfile).unwrap();
            let Relaxed::Done { converged, .. } =
                relax(&mol, &mol.coords, force_field, &[], OPTIMIZE_STEPS, None).unwrap()
            else {
                unreachable!()
            };
            assert!(
                converged,
                "{what}: not at a minimum after {OPTIMIZE_STEPS} steps"
            );
            let g = rms_gradient(&once);
            assert!(g < 0.1, "{what}: RMS gradient {g} after Optimize");
            assert!(
                (once.energy - twice.energy).abs() < 0.02,
                "{what}: a second Optimize still moved E {} -> {}",
                once.energy,
                twice.energy
            );
        }
    }

    #[test]
    fn a_raised_flag_cancels_a_relaxation() {
        let m = pentane();
        let mol = read(&m.molfile).unwrap();
        let cancel = AtomicBool::new(true);
        let kicked: Vec<[f64; 3]> = mol
            .coords
            .iter()
            .enumerate()
            .map(|(i, p)| [p[0] + 0.1 * (i % 3) as f64, p[1], p[2]])
            .collect();
        let relaxed = relax(&mol, &kicked, ForceField::Uff, &[], 100, Some(&cancel)).unwrap();
        assert_eq!(relaxed, Relaxed::Cancelled);
    }

    #[test]
    fn exports_every_format() {
        let m = built("ethanol", ForceField::Mmff94);
        for format in Export3d::ALL {
            let text = export(&m.molfile, format).expect("export");
            assert!(!text.is_empty(), "{format:?}");
        }
    }

    #[test]
    fn hill_formula() {
        let atoms = |symbols: &[&str]| -> Vec<Atom3d> {
            symbols
                .iter()
                .map(|s| Atom3d {
                    element: (*s).to_owned(),
                    pos: [0.0; 3],
                })
                .collect()
        };
        assert_eq!(formula(&atoms(&["O", "C", "H", "H", "C", "Br"])), "C2H2BrO");
        assert_eq!(formula(&atoms(&["O", "H", "H"])), "H2O");
        assert_eq!(formula(&atoms(&["Na", "Cl"])), "ClNa");
    }

    /// Paclitaxel (taxol): 113 atoms with its hydrogens, a crowded polycyclic core and floppy
    /// side chains, as OpenBabel's builder once placed it.
    const TAXOL_3D: &str = include_str!("../../tests/data/taxol_3d.mol");

    /// A mid-sized molecule relaxes to a minimum within Optimize's steps under MMFF, and a
    /// restrained Optimize holds its restraint.
    #[test]
    fn relaxes_a_mid_sized_molecule_under_mmff() {
        for force_field in [ForceField::Mmff94, ForceField::Mmff94s] {
            let start = model_from_molfile(TAXOL_3D, force_field);
            assert_eq!(start.atoms.len(), 113);
            let mol = read(TAXOL_3D).unwrap();
            let Relaxed::Done {
                energy, converged, ..
            } = relax(&mol, &mol.coords, force_field, &[], OPTIMIZE_STEPS, None).unwrap()
            else {
                unreachable!()
            };
            assert!(converged, "taxol under {force_field}: not converged");
            assert!(energy < start.energy, "taxol under {force_field}");

            let ends = [0, 22];
            let target = start.distance(ends[0], ends[1]) + 0.1;
            let out = optimize(
                TAXOL_3D,
                force_field,
                OPTIMIZE_STEPS,
                &[Constraint::Distance(ends, target)],
            )
            .unwrap();
            let at = out.distance(ends[0], ends[1]);
            assert!(
                (at - target).abs() < 0.01,
                "{force_field}: {at}, restrained to {target}"
            );
        }
    }
}
