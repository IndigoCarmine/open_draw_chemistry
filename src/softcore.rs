//! ソフトコア最適化: building a structure of several molecules by λ-continuation of soft-core
//! interactions between them (see [`crate::three_d::soft_core`]).
//!
//! The interactions between molecules start weak and soft — atoms of different molecules may
//! overlap and pass through each other — and are turned up step by step to full strength,
//! the whole structure (every atom, each molecule flexible) minimised by gradient at every
//! step, then minimised as usual. Molecules that start overlapping or tangled so come apart
//! and settle against each other where an ordinary minimisation would blow them apart or get
//! stuck.
//!
//! Two structures can be built: the 3D model (the result replaces it, one undoable step) and
//! the molecular assembly (the result opens as a new document's 3D model, since the
//! assembly's design holds rigid copies and no per-atom coordinates).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crate::assembly::build::Built;
use crate::three_d::{self, Constraint, ForceField, Model3d, Relaxed, SoftCoreStage};

/// The most atoms a build takes: what a V2000 molfile holds, and what a new document can lay
/// out in 2D (see [`three_d::read::MAX_ATOMS`]).
pub const MAX_ATOMS: usize = three_d::read::MAX_ATOMS;

/// Minimiser iterations at each step, and for the last minimisation.
const STEPS: u32 = 1000;

/// What is being built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The document's 3D model: the result replaces it.
    Model,
    /// The document's assembly: the result opens as a new document.
    Assembly,
}

/// Where a build has got.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stage {
    Starting,
    /// At soft-core step `step` (from 0) of `of`, the interactions between molecules at
    /// `lambda` of their full strength.
    Lambda {
        step: usize,
        of: usize,
        lambda: f64,
    },
    /// The last, ordinary minimisation.
    Final,
}

/// How a build ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Built(Model3d),
    Cancelled,
    Failed(String),
}

enum Message {
    Stage(Stage),
    Done(Outcome),
}

/// A soft-core build of one structure, running or done.
pub struct Run {
    pub target: Target,
    pub force_field: ForceField,
    receiver: Option<Receiver<Message>>,
    cancel: Arc<AtomicBool>,
    stage: Stage,
    /// The elements of the structure built, to tell whether the model is still that structure.
    elements: Vec<String>,
    /// The outcome, until taken.
    outcome: Option<Outcome>,
    started: Instant,
    finished: Option<Instant>,
}

/// Why a structure of `atoms` atoms in `molecules` molecules is refused, if it is.
pub fn refusal(atoms: usize, molecules: usize) -> Option<String> {
    if atoms > MAX_ATOMS {
        Some(t!(
            "softcore.too_many_atoms",
            atoms = atoms,
            max = MAX_ATOMS
        ))
    } else if molecules < 2 {
        Some(t!("softcore.one_molecule").to_owned())
    } else {
        None
    }
}

/// How many separate molecules `model` holds (bonded fragments).
pub fn molecule_count(model: &Model3d) -> usize {
    let n = model.atoms.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for b in &model.bonds {
        if b.begin < n && b.end < n {
            let (x, y) = (root(&mut parent, b.begin), root(&mut parent, b.end));
            parent[x] = y;
        }
    }
    (0..n).filter(|&i| root(&mut parent, i) == i).count()
}

impl Run {
    /// Start building the 3D model `model` under `force_field`, holding `constraints`.
    pub fn start_model(
        model: &Model3d,
        force_field: ForceField,
        constraints: Vec<Constraint>,
    ) -> Result<Run, String> {
        if let Some(why) = refusal(model.atoms.len(), molecule_count(model)) {
            return Err(why);
        }
        Self::start(
            Target::Model,
            &model.molfile,
            model,
            force_field,
            constraints,
        )
    }

    /// Start building the assembly `built` under `force_field`.
    pub fn start_assembly(built: &Built, force_field: ForceField) -> Result<Run, String> {
        if let Some(why) = refusal(built.atoms.len(), built.molecule_count()) {
            return Err(why);
        }
        let molfile = crate::scan::energy::molfile(&built.atoms, &built.bonds);
        let mol = three_d::read_molfile(&molfile).map_err(|e| e.to_string())?;
        // The assembly as a model (its energy is the build's business): what the build starts
        // from and is checked against.
        let start = three_d::assemble(&mol, &mol.coords, force_field, f64::NAN);
        Self::start(
            Target::Assembly,
            &start.molfile,
            &start,
            force_field,
            Vec::new(),
        )
    }

    fn start(
        target: Target,
        molfile: &str,
        model: &Model3d,
        force_field: ForceField,
        constraints: Vec<Constraint>,
    ) -> Result<Run, String> {
        let mol = three_d::read_molfile(molfile).map_err(|e| e.to_string())?;
        if mol.num_atoms() != model.atoms.len() {
            return Err(three_d::Error::Unreadable.to_string());
        }
        if three_d::fragment_count(&mol) < 2 {
            return Err(t!("softcore.one_molecule").to_owned());
        }
        // Typed once here, so an atom the force field has no type for is said at once.
        three_d::field(&mol, force_field, &[]).map_err(|e| e.to_string())?;
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let flag = cancel.clone();
        std::thread::Builder::new()
            .name("soft-core".into())
            .spawn(move || {
                let mut stage = |s: SoftCoreStage| {
                    let stage = match s {
                        SoftCoreStage::Lambda { step, of, lambda } => {
                            Stage::Lambda { step, of, lambda }
                        }
                        SoftCoreStage::Final => Stage::Final,
                    };
                    let _ = sender.send(Message::Stage(stage));
                };
                let relaxed = three_d::soft_core(
                    &mol,
                    &mol.coords,
                    force_field,
                    &constraints,
                    STEPS,
                    Some(&flag),
                    &mut stage,
                );
                let outcome = match relaxed {
                    Ok(Relaxed::Done { coords, energy, .. }) => {
                        match three_d::checked_model(&mol, &coords, force_field, energy) {
                            Ok(model) => Outcome::Built(model),
                            Err(e) => Outcome::Failed(e.to_string()),
                        }
                    }
                    Ok(Relaxed::Cancelled) => Outcome::Cancelled,
                    Err(e) => Outcome::Failed(e.to_string()),
                };
                let _ = sender.send(Message::Done(outcome));
            })
            .map_err(|e| e.to_string())?;
        Ok(Run {
            target,
            force_field,
            receiver: Some(receiver),
            cancel,
            stage: Stage::Starting,
            elements: model.atoms.iter().map(|a| a.element.clone()).collect(),
            outcome: None,
            started: Instant::now(),
            finished: None,
        })
    }

    pub fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    /// Ask the build to stop; nothing is applied.
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn elapsed(&self) -> Duration {
        self.finished.unwrap_or_else(Instant::now) - self.started
    }

    /// Take in what the build has done; its outcome once, when it ends.
    pub fn poll(&mut self) -> Option<Outcome> {
        let receiver = self.receiver.as_ref()?;
        loop {
            match receiver.try_recv() {
                Ok(Message::Stage(stage)) => self.stage = stage,
                Ok(Message::Done(outcome)) => {
                    self.outcome = Some(outcome);
                    break;
                }
                Err(TryRecvError::Empty) => return None,
                // The thread ended without a word: it panicked.
                Err(TryRecvError::Disconnected) => {
                    self.outcome = Some(Outcome::Failed(
                        three_d::Error::GenerationFailed.to_string(),
                    ));
                    break;
                }
            }
        }
        self.receiver = None;
        self.finished = Some(Instant::now());
        self.outcome.take()
    }

    /// Block until the build ends, and give its outcome (tests).
    pub fn wait(&mut self) -> Outcome {
        loop {
            if let Some(outcome) = self.poll() {
                return outcome;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Whether `model` is still the structure this build was of (the same atoms in the same
    /// order), so the result can replace it.
    pub fn fits(&self, model: &Model3d) -> bool {
        model.atoms.len() == self.elements.len()
            && model
                .atoms
                .iter()
                .zip(&self.elements)
                .all(|(a, e)| &a.element == e)
    }

    /// The stage as text: "λ = 0.12 (step 5 of 12)" or the last minimisation.
    pub fn stage_label(&self) -> String {
        match self.stage {
            Stage::Starting => t!("softcore.stage.starting").to_owned(),
            Stage::Lambda { step, of, lambda } => t!(
                "softcore.stage.lambda",
                lambda = format!("{lambda:.3}"),
                step = step + 1,
                of = of
            ),
            Stage::Final => t!("softcore.stage.final").to_owned(),
        }
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::three_d::{Atom3d, Bond3d};

    fn butane() -> Model3d {
        three_d::generate_from_molfile(
            include_str!("../tests/data/drawn/butane.mol"),
            ForceField::Uff,
        )
        .expect("butane")
    }

    /// Two butanes, the second put almost on top of the first: an ordinary minimisation
    /// starts with atoms of the two a fraction of an ångström apart.
    pub(crate) fn overlapping_butanes() -> Model3d {
        let one = butane();
        let n = one.atoms.len();
        let mut atoms: Vec<Atom3d> = one.atoms.clone();
        atoms.extend(one.atoms.iter().map(|a| Atom3d {
            element: a.element.clone(),
            pos: [a.pos[0] + 0.35, a.pos[1] + 0.25, a.pos[2] + 0.3],
        }));
        let mut bonds: Vec<Bond3d> = one.bonds.clone();
        bonds.extend(one.bonds.iter().map(|b| Bond3d {
            begin: b.begin + n,
            end: b.end + n,
            order: b.order,
        }));
        let molfile = crate::scan::energy::molfile(&atoms, &bonds);
        let mol = three_d::read_molfile(&molfile).expect("molfile");
        three_d::assemble(&mol, &mol.coords, ForceField::Uff, f64::NAN)
    }

    fn closest_between(model: &Model3d, split: usize) -> f64 {
        let mut best = f64::INFINITY;
        for i in 0..split {
            for j in split..model.atoms.len() {
                best = best.min(model.distance(i, j));
            }
        }
        best
    }

    /// The two butanes come apart and settle against each other: every atom of one well clear
    /// of the other's, a finite energy, the same atoms.
    #[test]
    fn overlapping_molecules_come_apart() {
        let start = overlapping_butanes();
        assert!(closest_between(&start, start.atoms.len() / 2) < 0.6);
        for ff in ForceField::ALL {
            let mut run = Run::start_model(&start, ff, Vec::new()).expect("starts");
            let Outcome::Built(model) = run.wait() else {
                panic!("{ff}: not built");
            };
            assert!(model.energy.is_finite(), "{ff}");
            assert_eq!(model.atoms.len(), start.atoms.len());
            assert!(run.fits(&model));
            let closest = closest_between(&model, start.atoms.len() / 2);
            assert!(closest > 1.5, "{ff}: atoms of the two {closest:.2} Å apart");
            // Ended at the last minimisation.
            assert_eq!(run.stage(), Stage::Final);
        }
    }

    /// A fixed atom stays where it was.
    #[test]
    fn fixed_atoms_hold() {
        let start = overlapping_butanes();
        let mut run =
            Run::start_model(&start, ForceField::Uff, vec![Constraint::Fix(0)]).expect("starts");
        let Outcome::Built(model) = run.wait() else {
            panic!("not built");
        };
        let (a, b) = (start.atoms[0].pos, model.atoms[0].pos);
        let moved = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        assert!(moved < 1e-6, "moved {moved}");
    }

    #[test]
    fn one_molecule_is_refused() {
        assert!(Run::start_model(&butane(), ForceField::Uff, Vec::new()).is_err());
        assert_eq!(molecule_count(&butane()), 1);
        assert_eq!(molecule_count(&overlapping_butanes()), 2);
        assert!(refusal(MAX_ATOMS + 1, 2).is_some());
        assert!(refusal(10, 2).is_none());
    }

    /// Cancel ends a build at once, with nothing built.
    #[test]
    fn cancel_ends_a_build() {
        let mut run = Run::start_model(&overlapping_butanes(), ForceField::Mmff94, Vec::new())
            .expect("starts");
        run.cancel();
        let asked = Instant::now();
        let outcome = run.wait();
        assert!(asked.elapsed() < Duration::from_secs(2));
        // It may have finished before the flag was seen.
        assert!(matches!(outcome, Outcome::Cancelled | Outcome::Built(_)));
        assert!(!run.is_running());
    }
}
