//! Single-point MM energies for a scan: the structure is handed to OpenBabel once, and every
//! grid point only replaces its coordinates ([`openbabel::Molecule::set_coordinates`]) before
//! asking for the energy. OpenBabel keeps the force field set up for a molecule of the same
//! atoms and bonds, so the atom typing and parameter lookup are paid at the first point only.
//! A relaxed scan minimises each point on that same molecule ([`Evaluator::relax`]).

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};

use openbabel::{Algorithm, Minimizer, StopReason};

use crate::three_d::geom::Vec3;
use crate::three_d::{self, Atom3d, Bond3d, Constraint, ForceField};

/// A V2000 count field is three digits wide; beyond it the molfile is written as V3000.
const V2000_MAX: usize = 999;

/// The energy unit of `force_field` as OpenBabel reports it (MMFF94: kcal/mol, UFF and GAFF:
/// kJ/mol). Should OpenBabel not report one, the same answer from the table.
pub fn unit(force_field: ForceField) -> String {
    if let Some(unit) = openbabel::forcefield_energy_unit(force_field.id()) {
        return unit;
    }
    match force_field {
        ForceField::Mmff94 | ForceField::Mmff94s => "kcal/mol",
        ForceField::Uff | ForceField::Gaff => "kJ/mol",
    }
    .to_owned()
}

/// `atoms` and `bonds` as a 3D MDL molfile: V2000 when the counts fit, V3000 otherwise.
/// Bond order 5 (OpenBabel's aromatic) is written as the molfile's aromatic type 4.
pub fn molfile(atoms: &[Atom3d], bonds: &[Bond3d]) -> String {
    let order = |o: u8| if o == 5 { 4 } else { o.clamp(1, 4) };
    let mut out = String::with_capacity(80 * (atoms.len() + bonds.len()) + 200);
    let _ = writeln!(out, "scan");
    let _ = writeln!(out, "  OpenDraw          3D");
    let _ = writeln!(out);
    if atoms.len() <= V2000_MAX && bonds.len() <= V2000_MAX {
        let _ = writeln!(
            out,
            "{:>3}{:>3}  0  0  0  0  0  0  0  0999 V2000",
            atoms.len(),
            bonds.len()
        );
        for a in atoms {
            let [x, y, z] = a.pos;
            let _ = writeln!(
                out,
                "{x:>10.4}{y:>10.4}{z:>10.4} {:<3} 0  0  0  0  0  0  0  0  0  0  0  0",
                a.element
            );
        }
        for b in bonds {
            let _ = writeln!(
                out,
                "{:>3}{:>3}{:>3}  0  0  0  0",
                b.begin + 1,
                b.end + 1,
                order(b.order)
            );
        }
    } else {
        let _ = writeln!(out, "  0  0  0     0  0            999 V3000");
        let _ = writeln!(out, "M  V30 BEGIN CTAB");
        let _ = writeln!(out, "M  V30 COUNTS {} {} 0 0 0", atoms.len(), bonds.len());
        let _ = writeln!(out, "M  V30 BEGIN ATOM");
        for (i, a) in atoms.iter().enumerate() {
            let [x, y, z] = a.pos;
            let _ = writeln!(out, "M  V30 {} {} {x:.4} {y:.4} {z:.4} 0", i + 1, a.element);
        }
        let _ = writeln!(out, "M  V30 END ATOM");
        let _ = writeln!(out, "M  V30 BEGIN BOND");
        for (i, b) in bonds.iter().enumerate() {
            let _ = writeln!(
                out,
                "M  V30 {} {} {} {}",
                i + 1,
                order(b.order),
                b.begin + 1,
                b.end + 1
            );
        }
        let _ = writeln!(out, "M  V30 END BOND");
        let _ = writeln!(out, "M  V30 END CTAB");
    }
    let _ = writeln!(out, "M  END");
    out
}

/// Methane, to clear OpenBabel's cached force-field setup (see [`Evaluator::new`]).
const METHANE: &str = "methane
  OpenDraw          3D

  5  4  0  0  0  0  0  0  0  0999 V2000
    0.0000    0.0000    0.0000 C   0  0  0  0  0  0  0  0  0  0  0  0
    0.6291    0.6291    0.6291 H   0  0  0  0  0  0  0  0  0  0  0  0
   -0.6291   -0.6291    0.6291 H   0  0  0  0  0  0  0  0  0  0  0  0
   -0.6291    0.6291   -0.6291 H   0  0  0  0  0  0  0  0  0  0  0  0
    0.6291   -0.6291   -0.6291 H   0  0  0  0  0  0  0  0  0  0  0  0
  1  2  1  0  0  0  0
  1  3  1  0  0  0  0
  1  4  1  0  0  0  0
  1  5  1  0  0  0  0
M  END
";

/// A structure held by OpenBabel for repeated single points.
pub struct Evaluator {
    mol: openbabel::Molecule,
    force_field: ForceField,
    atoms: usize,
}

impl Evaluator {
    /// Read `molfile` once. Fails on an unreadable file, or when the force
    /// field cannot be set up for the structure (an atom it has no type for).
    pub fn new(molfile: &str, force_field: ForceField) -> Result<Self, String> {
        let mol = openbabel::Molecule::parse(molfile, "mol")
            .map_err(|_| crate::three_d::Error::Unreadable.to_string())?;
        let atoms = mol.num_atoms() as usize;
        let evaluator = Self {
            mol,
            force_field,
            atoms,
        };
        // OpenBabel keeps one setup per force field and reuses it for any molecule with the
        // same elements, degrees and bond orders — even one typed differently (say the
        // builder's copy this model came from). Setting the force field up for a methane
        // first makes the next setup a full one, of this molecule.
        if let Ok(methane) = openbabel::Molecule::parse(METHANE, "mol") {
            let _ = methane.energy(force_field.id());
        }
        // Sets the force field up for this molecule; the points then reuse it.
        evaluator
            .mol
            .energy(force_field.id())
            .filter(|e| e.is_finite())
            .ok_or_else(|| crate::three_d::Error::ForceField(force_field).to_string())?;
        Ok(evaluator)
    }

    pub fn atoms(&self) -> usize {
        self.atoms
    }

    pub fn force_field(&self) -> ForceField {
        self.force_field
    }

    /// The energy with the atoms at `coords` (NaN when it cannot be had).
    pub fn energy(&mut self, coords: &[Vec3]) -> f64 {
        if coords.len() != self.atoms {
            return f64::NAN;
        }
        let flat: Vec<f64> = coords.iter().flatten().copied().collect();
        if !flat.iter().all(|c| c.is_finite()) || !self.mol.set_coordinates(&flat) {
            return f64::NAN;
        }
        self.mol
            .energy(self.force_field.id())
            .filter(|e| e.is_finite())
            .unwrap_or(f64::NAN)
    }
}

/// Minimiser steps run in one segment of a point's relaxation, between two looks at the cancel
/// flag: a few milliseconds for a small molecule, so cancelling takes effect within one point.
const RELAX_CHUNK_STEPS: u32 = 50;

/// Steps between the minimiser's looks at its own convergence within a segment: OpenBabel's
/// "converged" can only be told from "out of steps" when it stops before a segment's end.
const RELAX_FRAME_STEPS: u32 = 10;

/// The most a segment that OpenBabel calls converged may still lower the energy (in the force
/// field's unit) for a stage to count as converged.
const SETTLED_ENERGY: f64 = 1e-5;

/// The restraints' stiffness in the first stage of a point's relaxation (see
/// [`Evaluator::relax`]): soft enough for the minimiser to move freely, stiff enough to keep
/// the parameters within a degree or so; the second stage holds them as an optimisation does.
const SOFT_FORCE_FACTOR: f64 = 1.0e6;

/// How relaxing one point ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Relaxation {
    /// Minimised: the structure, its own energy (the restraints' penalty left out), and whether
    /// the minimiser converged within the step budget (else it was still moving).
    Done {
        coords: Vec<Vec3>,
        energy: f64,
        converged: bool,
    },
    /// The cancel flag was raised part-way.
    Cancelled,
    /// The minimiser could not run, or left an unusable structure.
    Failed,
}

impl Evaluator {
    /// Minimise the structure from `coords` holding `constraints` (scanned parameters, fixed
    /// atoms…), for at most `steps` conjugate-gradient steps, looking at `cancel` every
    /// [`RELAX_CHUNK_STEPS`].
    ///
    /// The restraints that hold a dihedral are stiff (see [`three_d::optimize_with`]), and
    /// conjugate gradients crawl under them: OpenBabel's convergence test (a tiny energy change
    /// from one step to the next) fires while the structure is still well above its relaxed
    /// energy — a single run leaves gauche butane ~2.7 kcal/mol (MMFF94) too high, and even
    /// restarted runs stall ~0.04 too high. So a point relaxes in two stages: first under
    /// restraints [`SOFT_FORCE_FACTOR`] stiff, where the rest of the structure moves freely
    /// (the parameters then sit within about a degree), then under the full stiffness, which
    /// only has that last bit to correct. The first stage may take half the budget. Each stage
    /// runs in segments, each restarting the conjugate gradients, and is converged once a
    /// fresh segment converges without lowering the energy by more than [`SETTLED_ENERGY`];
    /// the point is converged when its second stage is.
    pub fn relax(
        &mut self,
        coords: &[Vec3],
        constraints: &[Constraint],
        steps: u32,
        cancel: &AtomicBool,
    ) -> Relaxation {
        let flat: Vec<f64> = coords.iter().flatten().copied().collect();
        if coords.len() != self.atoms
            || !flat.iter().all(|c| c.is_finite())
            || !self.mol.set_coordinates(&flat)
        {
            return Relaxation::Failed;
        }
        let soft = self.minimise(constraints, Some(SOFT_FORCE_FACTOR), steps / 2, cancel);
        let Some((_, taken)) = soft else {
            return if cancel.load(Ordering::Relaxed) {
                Relaxation::Cancelled
            } else {
                Relaxation::Failed
            };
        };
        let Some((converged, _)) = self.minimise(constraints, None, steps - taken, cancel) else {
            return if cancel.load(Ordering::Relaxed) {
                Relaxation::Cancelled
            } else {
                Relaxation::Failed
            };
        };
        let coords: Vec<Vec3> = self
            .mol
            .atoms()
            .map(|a| {
                let (x, y, z) = a.coords();
                [x, y, z]
            })
            .collect();
        let energy = self.energy(&coords);
        if energy.is_nan() {
            return Relaxation::Failed;
        }
        Relaxation::Done {
            coords,
            energy,
            converged,
        }
    }

    /// One stage of [`Evaluator::relax`] from the molecule's coordinates: at most `steps`
    /// steps under `constraints`, as stiff as `force_factor` (else as an optimisation's).
    /// Whether it converged and the steps taken, or `None` when cancelled or failed.
    fn minimise(
        &mut self,
        constraints: &[Constraint],
        force_factor: Option<f64>,
        steps: u32,
        cancel: &AtomicBool,
    ) -> Option<(bool, u32)> {
        let mut taken = 0;
        // The energy (restraints included) where the last segment ended.
        let mut last = f64::INFINITY;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            if taken >= steps {
                return Some((false, taken));
            }
            let mut restraints = three_d::restraints(constraints, self.atoms);
            if let Some(factor) = force_factor {
                restraints.force_factor(factor);
            }
            let mut config = Minimizer::new(self.force_field.id());
            config
                .algorithm(Algorithm::ConjugateGradients)
                .max_steps(RELAX_CHUNK_STEPS.min(steps - taken))
                .steps_per_frame(RELAX_FRAME_STEPS)
                .constraints(restraints);
            let run = self.mol.minimize(&config);
            let stop = run.stop_reason();
            taken += run.steps_taken().max(1);
            let end = run
                .last()
                .map(|frame| frame.energy)
                .filter(|e| e.is_finite())?;
            let settled = last - end <= SETTLED_ENERGY;
            last = end;
            match stop {
                StopReason::Converged if settled => return Some((true, taken)),
                StopReason::Converged | StopReason::MaxSteps => {}
                StopReason::Failed => return None,
            }
        }
    }
}

/// The energy of a structure read afresh (a grid point whose atoms differ from the
/// reference's). NaN when it cannot be had.
pub fn single_point(atoms: &[Atom3d], bonds: &[Bond3d], force_field: ForceField) -> f64 {
    let Ok(mut evaluator) = Evaluator::new(&molfile(atoms, bonds), force_field) else {
        return f64::NAN;
    };
    let coords: Vec<Vec3> = atoms.iter().map(|a| a.pos).collect();
    evaluator.energy(&coords)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(element: &str, pos: Vec3) -> Atom3d {
        Atom3d {
            element: element.into(),
            pos,
        }
    }

    fn water(at: f64) -> (Vec<Atom3d>, Vec<Bond3d>) {
        let atoms = vec![
            atom("O", [at, 0.0, 0.0]),
            atom("H", [at + 0.96, 0.0, 0.0]),
            atom("H", [at - 0.24, 0.93, 0.0]),
        ];
        let bonds = vec![
            Bond3d {
                begin: 0,
                end: 1,
                order: 1,
            },
            Bond3d {
                begin: 0,
                end: 2,
                order: 1,
            },
        ];
        (atoms, bonds)
    }

    #[test]
    fn small_structures_are_v2000_and_large_ones_v3000() {
        let (atoms, bonds) = water(0.0);
        let text = molfile(&atoms, &bonds);
        assert!(text.lines().nth(3).unwrap().ends_with("V2000"));
        assert_eq!(text.lines().count(), 4 + 3 + 2 + 1);
        assert!(text.lines().nth(5).unwrap().contains(" H "));

        let mut big_atoms = Vec::new();
        let mut big_bonds = Vec::new();
        for k in 0..400 {
            let (a, b) = water(3.0 * k as f64);
            let first = big_atoms.len();
            big_atoms.extend(a);
            big_bonds.extend(b.into_iter().map(|b| Bond3d {
                begin: b.begin + first,
                end: b.end + first,
                order: b.order,
            }));
        }
        let text = molfile(&big_atoms, &big_bonds);
        assert!(text.contains("V3000"));
        assert!(text.contains("M  V30 COUNTS 1200 800 0 0 0"));
        assert!(text.contains("M  V30 1200 H "));
        // The 3D editor's coordinate rewriter understands it, as it does OpenBabel's own.
        let moved: Vec<Vec3> = big_atoms.iter().map(|a| [a.pos[0], 1.0, 2.0]).collect();
        assert!(crate::three_d::edit::molfile_with_coords(&text, &moved).is_some());
    }

    #[test]
    fn units_follow_the_force_field() {
        assert_eq!(unit(ForceField::Mmff94), "kcal/mol");
        assert_eq!(unit(ForceField::Uff), "kJ/mol");
    }

    /// Replacing the coordinates gives the same energy as reading the moved structure afresh,
    /// and both V2000 and V3000 files are read.
    #[test]
    fn coordinates_replaced_in_place_give_the_energy_of_a_fresh_read() {
        for copies in [2usize, 400] {
            let mut atoms = Vec::new();
            let mut bonds = Vec::new();
            for k in 0..copies {
                let (a, b) = water(3.1 * k as f64);
                let first = atoms.len();
                atoms.extend(a);
                bonds.extend(b.into_iter().map(|b| Bond3d {
                    begin: b.begin + first,
                    end: b.end + first,
                    order: b.order,
                }));
            }
            let mut ev = Evaluator::new(&molfile(&atoms, &bonds), ForceField::Uff).unwrap();
            assert_eq!(ev.atoms(), atoms.len());
            let mut moved = atoms.clone();
            // Stretch every O–H bond a little and push the molecules closer.
            for (i, a) in moved.iter_mut().enumerate() {
                a.pos[0] *= 0.97;
                if i % 3 == 1 {
                    a.pos[0] += 0.1;
                }
            }
            let coords: Vec<Vec3> = moved.iter().map(|a| a.pos).collect();
            let in_place = ev.energy(&coords);
            let fresh = single_point(&moved, &bonds, ForceField::Uff);
            assert!(in_place.is_finite());
            assert!(
                (in_place - fresh).abs() < 1e-3 * fresh.abs().max(1.0),
                "{copies}: in place {in_place}, fresh {fresh}"
            );
            let original: Vec<Vec3> = atoms.iter().map(|a| a.pos).collect();
            assert!((ev.energy(&original) - in_place).abs() > 1e-3, "it moved");
            assert!(ev.energy(&coords[1..]).is_nan(), "wrong atom count");
        }
    }
}
