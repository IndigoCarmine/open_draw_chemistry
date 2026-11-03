//! Single-point MM energies for a scan: the force field is set up for the structure once (atom
//! typing and parameter lookup), and every grid point only evaluates it at its coordinates.
//! A relaxed scan minimises each point under the same force field ([`Evaluator::relax`]).

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};

use molecule_optimizer_rs::Molecule;
use molecule_optimizer_rs::ff::ForceField as Field;

use crate::three_d::geom::Vec3;
use crate::three_d::{self, Atom3d, Bond3d, Constraint, ForceField, Relaxed};

/// A V2000 count field is three digits wide; beyond it the molfile is written as V3000.
const V2000_MAX: usize = 999;

/// The energy unit of `force_field`: kcal/mol for every one offered.
pub fn unit(_force_field: ForceField) -> String {
    three_d::ENERGY_UNIT.to_owned()
}

/// `atoms` and `bonds` as a 3D MDL molfile: V2000 when the counts fit, V3000 otherwise.
/// Bond order 5 (aromatic) is written as the molfile's aromatic type 4.
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

/// A structure with its force field set up, for repeated single points.
pub struct Evaluator {
    mol: Molecule,
    field: Box<dyn Field>,
    force_field: ForceField,
    /// Where the structure's atoms are now (flat): the last energy's coordinates, or where a
    /// segment of [`Evaluator::relax_held`] left them.
    at: Vec<f64>,
}

impl Evaluator {
    /// Read `molfile` once. Fails on an unreadable file, or when the force
    /// field cannot be set up for the structure (an atom it has no type for).
    pub fn new(molfile: &str, force_field: ForceField) -> Result<Self, String> {
        let mol = three_d::read_molfile(molfile).map_err(|e| e.to_string())?;
        let field = three_d::field(&mol, force_field, &[]).map_err(|e| e.to_string())?;
        let at = mol.flat_coords();
        if !field.energy(&at).is_finite() {
            return Err(three_d::Error::ForceField(force_field).to_string());
        }
        Ok(Self {
            mol,
            field,
            force_field,
            at,
        })
    }

    pub fn atoms(&self) -> usize {
        self.mol.num_atoms()
    }

    pub fn force_field(&self) -> ForceField {
        self.force_field
    }

    /// The energy with the atoms at `coords` (NaN when it cannot be had).
    pub fn energy(&mut self, coords: &[Vec3]) -> f64 {
        if coords.len() != self.atoms() {
            return f64::NAN;
        }
        let flat: Vec<f64> = coords.iter().flatten().copied().collect();
        if !flat.iter().all(|c| c.is_finite()) {
            return f64::NAN;
        }
        let energy = self.field.energy(&flat);
        self.at = flat;
        if energy.is_finite() { energy } else { f64::NAN }
    }
}

/// The most minimiser iterations run in one segment of a held relaxation (see
/// [`Evaluator::relax_held`]), between two looks at the cancel flag.
const RELAX_CHUNK_STEPS: u32 = 50;

/// The fewest minimiser steps a segment of [`Evaluator::relax_held`] is cut down to (one line
/// search along the forces): a relaxation held is converged only once a segment this short, looked
/// along closer, no longer lowers its energy.
const SHORTEST_HELD_SEGMENT: u32 = 1;

/// How many times a held step that did not lower the energy is halved, looking for a lower
/// structure along it, before [`Evaluator::relax_held`] calls the relaxation converged.
const HELD_STEP_HALVINGS: u32 = 10;

/// The least a segment of [`Evaluator::relax_held`] must lower the held energy (in the force
/// field's unit) not to count as settled.
const SETTLED_ENERGY: f64 = 1e-5;

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
    /// atoms…) as Optimize does ([`three_d::optimize_with`]), in at most `steps` minimiser
    /// iterations, looking at `cancel` after every one. Converged when it ended at the
    /// restrained minimum rather than out of steps (see [`Relaxed`]).
    pub fn relax(
        &mut self,
        coords: &[Vec3],
        constraints: &[Constraint],
        steps: u32,
        cancel: &AtomicBool,
    ) -> Relaxation {
        if coords.len() != self.atoms() || !coords.iter().flatten().all(|c| c.is_finite()) {
            return Relaxation::Failed;
        }
        let relaxed = three_d::relax(
            &self.mol,
            coords,
            self.force_field,
            constraints,
            steps,
            Some(cancel),
        );
        let (coords, converged) = match relaxed {
            Ok(Relaxed::Done {
                coords, converged, ..
            }) => (coords, converged),
            Ok(Relaxed::Cancelled) => return Relaxation::Cancelled,
            Err(_) => return Relaxation::Failed,
        };
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

    /// Minimise the structure from `coords` with no restraint, for at most `steps` minimiser
    /// steps in segments of at most [`RELAX_CHUNK_STEPS`] (looking at `cancel` before each),
    /// putting it back after each segment as `hold` puts a structure (an assembly's molecules
    /// moved rigidly back to their places: [`super::source::Placed::hold`]) — so what is
    /// minimised is everything `hold` leaves free, here the molecules' shapes.
    ///
    /// The energy is that of the held structure (as `hold` leaves it), and the structure kept
    /// the held one of the lowest energy seen, the start (held) among them: it is never above
    /// the start's. The minimiser does not know of `hold`: where the forces between the
    /// molecules mostly push them about rigidly (close contacts, strong torques), a long
    /// segment spends its steps on moves `hold` undoes and ends, held, no lower or even higher,
    /// far above the held minimum. So a segment that does not lower the held energy by more
    /// than [`SETTLED_ENERGY`] is run again from the lowest structure with half the steps, down
    /// to [`SHORTEST_HELD_SEGMENT`]; a segment that short is one line search along the forces
    /// (steepest descent), whose part `hold` keeps lowers the held energy when short enough,
    /// and one that does not lower the energy is looked along closer, [`HELD_STEP_HALVINGS`]
    /// times. Converged only once that finds nothing lower either — not by the minimiser's own
    /// test, which it never meets while `hold` keeps undoing the molecules' rigid moves.
    pub fn relax_held(
        &mut self,
        coords: &[Vec3],
        steps: u32,
        cancel: &AtomicBool,
        hold: &dyn Fn(&mut [Vec3]),
    ) -> Relaxation {
        self.relax_held_from(coords, steps, cancel, hold, RELAX_CHUNK_STEPS)
    }

    /// [`Evaluator::relax_held`], its segments starting at `longest` steps.
    pub(super) fn relax_held_from(
        &mut self,
        coords: &[Vec3],
        steps: u32,
        cancel: &AtomicBool,
        hold: &dyn Fn(&mut [Vec3]),
        longest: u32,
    ) -> Relaxation {
        let mut best = coords.to_vec();
        hold(&mut best);
        // Also puts the molecule's atoms at the held start.
        let mut lowest = self.energy(&best);
        if lowest.is_nan() {
            return Relaxation::Failed;
        }
        let mut taken = 0;
        let mut segment = longest.max(SHORTEST_HELD_SEGMENT);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Relaxation::Cancelled;
            }
            if taken >= steps {
                return Relaxation::Done {
                    coords: best,
                    energy: lowest,
                    converged: false,
                };
            }
            let shortest = segment <= SHORTEST_HELD_SEGMENT;
            let most = segment.min(steps - taken);
            // From where the atoms are: a segment of the library's BFGS, whose first iteration
            // is a line search along the forces (a step of steepest descent).
            taken += three_d::descend(self.field.as_ref(), &mut self.at, most);
            let free = self.coords();
            let mut coords = free.clone();
            hold(&mut coords);
            let mut energy = self.energy(&coords);
            if energy.is_nan() {
                return Relaxation::Failed;
            }
            // Whether the molecule's atoms are at `coords` (the last energy taken).
            let mut at_coords = true;
            if shortest && energy >= lowest - SETTLED_ENERGY {
                // The step's length suits the moves the minimiser sees, most of which `hold`
                // undoes: look for a lower structure closer along it.
                let mut part = 1.0;
                for _ in 0..HELD_STEP_HALVINGS {
                    part /= 2.0;
                    let mut trial: Vec<Vec3> = best
                        .iter()
                        .zip(&free)
                        .map(|(q, p)| std::array::from_fn(|k| q[k] + part * (p[k] - q[k])))
                        .collect();
                    hold(&mut trial);
                    let e = self.energy(&trial);
                    // An energy costs about what a step does.
                    taken += 1;
                    at_coords = e < energy;
                    if at_coords {
                        coords = trial;
                        energy = e;
                    }
                    if energy < lowest - SETTLED_ENERGY {
                        break;
                    }
                }
            }
            let settled = energy >= lowest - SETTLED_ENERGY;
            if energy < lowest {
                best = coords;
                lowest = energy;
            } else {
                at_coords = false;
            }
            if !at_coords {
                // Go on from the lowest, not from wherever the molecule's atoms were left.
                self.energy(&best);
            }
            if settled {
                if shortest {
                    return Relaxation::Done {
                        coords: best,
                        energy: lowest,
                        converged: true,
                    };
                }
                segment = (segment / 2).max(SHORTEST_HELD_SEGMENT);
            }
        }
    }

    /// The molecule's coordinates now.
    fn coords(&self) -> Vec<Vec3> {
        self.at.as_chunks::<3>().0.to_vec()
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
        // The 3D editor's coordinate rewriter understands it, as it does the force field's own.
        let moved: Vec<Vec3> = big_atoms.iter().map(|a| [a.pos[0], 1.0, 2.0]).collect();
        assert!(crate::three_d::edit::molfile_with_coords(&text, &moved).is_some());
    }

    #[test]
    fn every_force_field_reports_kcal_per_mol() {
        for force_field in ForceField::ALL {
            assert_eq!(unit(force_field), "kcal/mol");
        }
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
