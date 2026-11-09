//! The molecular mechanics behind [`super`]: building, minimising and writing models with
//! molecule_optimizer_rs (UFF and MMFF94/94s ported from RDKit, ETKDG for the 3D build).
//!
//! Every call here takes a while on a large structure, so it belongs on a background thread
//! (see [`super::job`]).

use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};

use molecule_optimizer_rs::calc::assembly::geometric_schedule;
use molecule_optimizer_rs::calc::{self, OptimizeOptions};
use molecule_optimizer_rs::distgeom::EmbedParams;
use molecule_optimizer_rs::ff::mmff::{MmffBuilder, MmffVariant};
use molecule_optimizer_rs::ff::softcore::{SoftCoreBuilder, SoftCoreParams};
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

/// MMFF94 or MMFF94s as the app sets them up: no cutoff, the molecules' interactions on.
fn mmff_builder(variant: MmffVariant) -> MmffBuilder {
    MmffBuilder {
        variant,
        nonbonded_thresh: NO_CUTOFF,
        ignore_interfrag_interactions: false,
        ..MmffBuilder::default()
    }
}

/// UFF as the app sets it up (see [`mmff_builder`]).
fn uff_builder() -> UffBuilder {
    UffBuilder {
        vdw_thresh: NO_CUTOFF,
        ignore_interfrag_interactions: false,
    }
}

/// The library's builder for `force_field`.
fn builder(force_field: ForceField) -> Box<dyn ForceFieldBuilder> {
    match force_field {
        ForceField::Mmff94 => Box::new(mmff_builder(MmffVariant::Mmff94)),
        ForceField::Mmff94s => Box::new(mmff_builder(MmffVariant::Mmff94s)),
        ForceField::Uff => Box::new(uff_builder()),
    }
}

/// The library's builder for `force_field`, shareable across threads: what a conformer search
/// (see `crate::conformer`) minimises every conformer with, so its energies are the app's.
pub(crate) fn shared_builder(force_field: ForceField) -> std::sync::Arc<dyn ForceFieldBuilder> {
    std::sync::Arc::from(builder(force_field))
}

/// [`shared_builder`] whose every force field also holds `constraints`: a conformer search
/// builds its one field itself, without restraints, so they ride in on the builder.
pub(crate) fn restrained_builder(
    force_field: ForceField,
    constraints: &[Constraint],
    atoms: usize,
) -> std::sync::Arc<dyn ForceFieldBuilder> {
    let held = restraints(constraints, atoms);
    if held.is_empty() {
        return shared_builder(force_field);
    }
    std::sync::Arc::new(Restrained {
        inner: builder(force_field),
        held,
    })
}

/// A builder adding `held` to the restraints it is asked for (see [`restrained_builder`]).
struct Restrained {
    inner: Box<dyn ForceFieldBuilder>,
    held: Vec<ff::Constraint>,
}

impl Restrained {
    fn with(&self, constraints: &[ff::Constraint]) -> Vec<ff::Constraint> {
        self.held.iter().chain(constraints).cloned().collect()
    }
}

impl ForceFieldBuilder for Restrained {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn build(
        &self,
        mol: &Molecule,
        constraints: &[ff::Constraint],
    ) -> molecule_optimizer_rs::Result<ff::ContribForceField> {
        self.inner.build(mol, &self.with(constraints))
    }

    fn build_dyn(
        &self,
        mol: &Molecule,
        constraints: &[ff::Constraint],
    ) -> molecule_optimizer_rs::Result<Box<dyn Field>> {
        self.inner.build_dyn(mol, &self.with(constraints))
    }
}

/// The restraints' own penalty (kcal/mol) at `coords` of `mol`: what a minimisation under
/// [`restrained_builder`] adds to the molecule's energy.
pub(crate) fn restraint_energy(
    mol: &Molecule,
    constraints: &[Constraint],
    coords: &[[f64; 3]],
) -> f64 {
    let held = restraints(constraints, mol.num_atoms());
    if held.is_empty() || coords.len() != mol.num_atoms() {
        return 0.0;
    }
    let mut penalty = ff::ContribForceField::new(mol.num_atoms(), 3);
    penalty.add_constraints(&mol.coords, &held);
    let flat: Vec<f64> = coords.iter().flatten().copied().collect();
    penalty.energy(&flat)
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

/// A minimiser run whose energy (restraints included) has not dropped by [`SETTLED_ENERGY`]
/// for this many iterations has stalled (see [`relax`]).
const STALL_ITERATIONS: usize = 200;

/// Watches one minimiser run's energy for a stall: [`STALL_ITERATIONS`] iterations without a
/// drop of [`SETTLED_ENERGY`].
#[derive(Debug, Default)]
struct StallWatch {
    /// The energy and iteration of the last drop of [`SETTLED_ENERGY`] or more.
    mark: Option<(f64, usize)>,
}

impl StallWatch {
    /// Take in the energy at `iteration`; whether the run has stalled.
    fn stalled(&mut self, iteration: usize, energy: f64) -> bool {
        match self.mark {
            Some((at, since)) if energy > at - SETTLED_ENERGY => {
                iteration.saturating_sub(since) >= STALL_ITERATIONS
            }
            _ => {
                self.mark = Some((energy, iteration));
                false
            }
        }
    }
}

/// Minimise `mol` from `coords` under `force_field` holding `constraints`, for at most `steps`
/// iterations of the library's BFGS in all, looking at `cancel` (if any) after every one.
///
/// BFGS stops on RDKit's convergence test, which scales the gradient by the energy and the
/// coordinates: on a large, restrained structure it can stop on a slope that still leads half
/// a kcal/mol lower. So a run that converged is restarted from where it stopped (with a fresh
/// Hessian), and the minimisation is converged once a restart no longer lowers the energy by
/// [`SETTLED_ENERGY`].
///
/// Near a stiffly restrained minimum BFGS can also creep on without ever meeting that test:
/// the gradient there is mostly rounding, and whether it passes depends on the platform's
/// arithmetic (one 0° point of a relaxed dihedral scan spent all its 5000 iterations at its
/// minimum on aarch64 macOS and converged on x86-64). So a run that has not lowered the energy
/// by [`SETTLED_ENERGY`] for [`STALL_ITERATIONS`] iterations is stopped and treated as one
/// that converged: restarted, and settled when the restart does not lower the energy either.
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
    let failed = || Error::ForceField(force_field);
    let mut left = steps.max(1) as usize;
    let mut at = coords.to_vec();
    let mut before = f64::INFINITY;
    loop {
        let mut stall = StallWatch::default();
        let mut stalled = false;
        let mut watch = |info: &IterationInfo<'_>| {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                ControlFlow::Break(())
            } else if stall.stalled(info.iteration, info.energy) {
                stalled = true;
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
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
        if done.status == MinimizeStatus::Cancelled && !stalled {
            return Ok(Relaxed::Cancelled);
        }
        if !done.ff_energy.is_finite() {
            return Err(failed());
        }
        at = done.coords;
        left = left.saturating_sub(done.iterations.max(1));
        let converged = stalled || done.status == MinimizeStatus::Converged;
        let settled = converged && before - done.energy < SETTLED_ENERGY;
        before = done.energy;
        if settled || !converged || left == 0 {
            return Ok(Relaxed::Done {
                coords: at,
                energy: done.ff_energy,
                converged: settled,
            });
        }
    }
}

/// Where a soft-core build has got (see [`soft_core`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SoftCoreStage {
    /// Minimising at step `step` (from 0) of `of`, the molecules' interactions at `lambda`.
    Lambda { step: usize, of: usize, lambda: f64 },
    /// The last, ordinary minimisation at full strength.
    Final,
}

/// The coupling of the molecules' interactions at the first soft-core step: they start this
/// weak (and soft at short range) and grow geometrically to full strength.
const SOFT_CORE_START: f64 = 0.02;
/// How many soft-core steps lead up to full strength.
const SOFT_CORE_STEPS: usize = 12;

/// How many separate molecules (bonded fragments) `mol` holds.
pub(crate) fn fragment_count(mol: &Molecule) -> usize {
    mol.fragments().len()
}

/// Build up the structure of `mol` from `coords` by λ-continuation of soft-core interactions
/// between its molecules: the interactions between molecules start weak and soft (atoms of
/// different molecules may pass through each other) and are turned up to full strength over
/// [`SOFT_CORE_STEPS`] steps, the whole structure minimised by gradient (BFGS) for up to
/// `steps` iterations at every step; then it is minimised as usual ([`relax`]) for up to
/// `steps` more. Each molecule stays flexible; `constraints` hold throughout. `stage` hears of
/// each step as it starts. The energy is the plain force field's.
pub(crate) fn soft_core(
    mol: &Molecule,
    coords: &[[f64; 3]],
    force_field: ForceField,
    constraints: &[Constraint],
    steps: u32,
    cancel: Option<&AtomicBool>,
    stage: &mut dyn FnMut(SoftCoreStage),
) -> Result<Relaxed, Error> {
    let params = SoftCoreParams {
        include_all_pairs: true,
        ..SoftCoreParams::default()
    };
    let held = restraints(constraints, mol.num_atoms());
    let mut placed = mol.clone();
    placed.coords = coords.to_vec();
    let built = match force_field {
        ForceField::Mmff94 | ForceField::Mmff94s => {
            let variant = if force_field == ForceField::Mmff94 {
                MmffVariant::Mmff94
            } else {
                MmffVariant::Mmff94s
            };
            SoftCoreBuilder::new(mmff_builder(variant))
                .with_params(params)
                .build_with_handle(&placed, &held)
        }
        ForceField::Uff => SoftCoreBuilder::new(uff_builder())
            .with_params(params)
            .build_with_handle(&placed, &held),
    };
    let (field, handle) = built.map_err(|_| Error::ForceField(force_field))?;
    let mut watch = |_: &IterationInfo<'_>| {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = MinimizeOptions {
        max_iters: steps.max(1) as usize,
        ..MinimizeOptions::default()
    };
    let schedule = geometric_schedule(SOFT_CORE_START, SOFT_CORE_STEPS);
    let mut pos = placed.flat_coords();
    for (step, &(vdw, ele)) in schedule.iter().enumerate() {
        stage(SoftCoreStage::Lambda {
            step,
            of: schedule.len(),
            lambda: vdw,
        });
        handle.set_lambda(vdw, ele);
        let done = optim::minimize_with(&field, &mut pos, &options, false, &mut watch);
        if done.status == MinimizeStatus::Cancelled {
            return Ok(Relaxed::Cancelled);
        }
        if !done.energy.is_finite() {
            return Err(Error::ForceField(force_field));
        }
    }
    stage(SoftCoreStage::Final);
    let built: Vec<[f64; 3]> = pos.as_chunks::<3>().0.to_vec();
    relax(mol, &built, force_field, constraints, steps, cancel)
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
    Ok(match format {
        Format::Mol => molblock(&mol, &mol.coords),
        Format::Sdf => sdf_record(&mol, &mol.coords, &[]),
        _ => io::write_str(&mol, None, format),
    })
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
        molfile: molblock(&placed, coords),
    }
}

/// A V2000 count field is three digits wide; beyond it a molblock is written as V3000.
const V2000_MAX: usize = 999;

/// `mol` at `coords` as a molblock: the library's V2000 one when the counts fit, V3000
/// (Kekulé bond orders and formal charges, as the V2000 one) otherwise.
pub(crate) fn molblock(mol: &Molecule, coords: &[[f64; 3]]) -> String {
    use std::fmt::Write as _;
    if mol.num_atoms() <= V2000_MAX && mol.num_bonds() <= V2000_MAX {
        return io::molfile::write_molblock_coords(mol, coords);
    }
    let km = molecule_optimizer_rs::perception::kekulized_copy(mol).unwrap_or_else(|_| mol.clone());
    let mut s = String::with_capacity(80 * (km.num_atoms() + km.num_bonds()) + 200);
    let _ = writeln!(s, "{}", mol.name);
    let _ = writeln!(s, "  molecule_optimizer_rs   3D");
    let _ = writeln!(s);
    let _ = writeln!(s, "  0  0  0     0  0            999 V3000");
    let _ = writeln!(s, "M  V30 BEGIN CTAB");
    let _ = writeln!(
        s,
        "M  V30 COUNTS {} {} 0 0 0",
        km.num_atoms(),
        km.num_bonds()
    );
    let _ = writeln!(s, "M  V30 BEGIN ATOM");
    for (i, (a, p)) in km.atoms().iter().zip(coords).enumerate() {
        let _ = write!(
            s,
            "M  V30 {} {} {:.4} {:.4} {:.4} 0",
            i + 1,
            a.symbol(),
            p[0],
            p[1],
            p[2]
        );
        if a.formal_charge != 0 {
            let _ = write!(s, " CHG={}", a.formal_charge);
        }
        let _ = writeln!(s);
    }
    let _ = writeln!(s, "M  V30 END ATOM");
    let _ = writeln!(s, "M  V30 BEGIN BOND");
    for (i, b) in km.bonds().iter().enumerate() {
        let t = match b.order {
            BondOrder::Single => 1,
            BondOrder::Double => 2,
            BondOrder::Triple => 3,
            BondOrder::Aromatic => 4,
            BondOrder::Dative => 9,
            BondOrder::Zero => 0,
        };
        let _ = write!(s, "M  V30 {} {} {} {}", i + 1, t, b.begin + 1, b.end + 1);
        if b.order == BondOrder::Double
            && b.dir == molecule_optimizer_rs::core::molecule::BondDir::Either
        {
            let _ = write!(s, " CFG=2");
        }
        let _ = writeln!(s);
    }
    let _ = writeln!(s, "M  V30 END BOND");
    let _ = writeln!(s, "M  V30 END CTAB");
    let _ = writeln!(s, "M  END");
    s
}

/// An SD file record of `mol` at `coords` with `extra` data fields, written through
/// [`molblock`] so a structure of any size is written whole.
pub(crate) fn sdf_record(mol: &Molecule, coords: &[[f64; 3]], extra: &[(&str, String)]) -> String {
    use std::fmt::Write as _;
    let mut s = molblock(mol, coords);
    for (k, v) in mol.props.iter() {
        let _ = writeln!(s, "> <{k}>\n{v}\n");
    }
    for (k, v) in extra {
        let _ = writeln!(s, "> <{k}>\n{v}\n");
    }
    s.push_str("$$$$\n");
    s
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

    /// A model beyond V2000's three-digit counts carries a V3000 molfile that reads back
    /// whole, as do its MOL and SDF exports.
    #[test]
    fn a_large_model_is_written_as_v3000() {
        let atoms: Vec<Atom3d> = (0..1200)
            .map(|k| Atom3d {
                element: "C".to_owned(),
                pos: [k as f64 * 1.5, (k % 2) as f64, 0.0],
            })
            .collect();
        let bonds: Vec<Bond3d> = (1..1200)
            .map(|k| Bond3d {
                begin: k - 1,
                end: k,
                order: 1,
            })
            .collect();
        let mol = read(&crate::scan::energy::molfile(&atoms, &bonds)).expect("read");
        let model = assemble(&mol, &mol.coords, ForceField::Uff, 0.0);
        assert!(model.molfile.lines().nth(3).unwrap().ends_with("V3000"));
        assert_eq!(
            read(&model.molfile).expect("reads back").num_atoms(),
            mol.num_atoms()
        );
        for format in [Export3d::Mol, Export3d::Sdf] {
            let text = export(&model.molfile, format).expect("export");
            assert!(text.contains("V3000"));
            assert_eq!(
                read(&text).expect("reads back").num_atoms(),
                mol.num_atoms()
            );
        }
    }

    /// A run whose energy keeps dropping has not stalled, however long it takes.
    #[test]
    fn a_falling_energy_is_no_stall() {
        let mut watch = StallWatch::default();
        for i in 0..10 * STALL_ITERATIONS {
            assert!(
                !watch.stalled(i, -(i as f64) * 2.0 * SETTLED_ENERGY),
                "iteration {i}"
            );
        }
    }

    /// A run creeping by less than [`SETTLED_ENERGY`] per [`STALL_ITERATIONS`] iterations has
    /// stalled, once that many iterations have passed since its last real drop.
    #[test]
    fn a_flat_energy_stalls_after_the_window() {
        let mut watch = StallWatch::default();
        assert!(!watch.stalled(0, 10.0));
        assert!(!watch.stalled(1, 9.0));
        for i in 2..1 + STALL_ITERATIONS {
            let creep = 9.0 - (i as f64) * 1.0e-8;
            assert!(!watch.stalled(i, creep), "iteration {i}");
        }
        assert!(watch.stalled(1 + STALL_ITERATIONS, 9.0 - 1.0e-6));
    }

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

    /// trans-But-2-ene turned about its C=C in the 3D view stays cis through Optimize, and an
    /// embedding of the edited model (as the ETKDG conformer search does) keeps it cis too.
    #[test]
    fn a_double_bond_turned_by_hand_stays_turned() {
        let mut drawing = chembuider_rs::Molecule::default();
        let points = [[0.0, 0.0], [1.3, -0.75], [2.6, 0.0], [3.9, -0.75]];
        let atoms: Vec<_> = points
            .iter()
            .map(|&p| drawing.add_atom("C".to_owned(), p, 0))
            .collect();
        drawing.add_bond(atoms[0], atoms[1], chembuider_rs::BondOrder::Single);
        drawing.add_bond(atoms[1], atoms[2], chembuider_rs::BondOrder::Double);
        drawing.add_bond(atoms[2], atoms[3], chembuider_rs::BondOrder::Single);
        let mut m =
            crate::three_d::generate(&drawing, &chembuider_rs::Config::default(), ForceField::Uff)
                .expect("generate");
        let torsion = |m: &Model3d| {
            let c = edit::coords(m);
            geom::dihedral(c[0], c[1], c[2], c[3])
        };
        assert!(torsion(&m).abs() > 160.0, "drawn trans: {}", torsion(&m));

        let mut edit3d = crate::three_d::Edit3d::default();
        edit3d.picked = vec![1, 2];
        assert!(edit::rotate_picked(&mut m, &mut edit3d, 180.0));
        assert!(torsion(&m).abs() < 20.0, "turned: {}", torsion(&m));

        let relaxed = optimize(&m.molfile, ForceField::Uff, OPTIMIZE_STEPS, &[]).unwrap();
        assert!(
            torsion(&relaxed).abs() < 20.0,
            "optimised: {}",
            torsion(&relaxed)
        );
        let embedded = generate(&m.molfile, ForceField::Uff, GENERATE_STEPS).unwrap();
        assert!(
            torsion(&embedded).abs() < 20.0,
            "re-embedded: {}",
            torsion(&embedded)
        );
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
