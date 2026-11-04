//! Conformational search (配座探索) of the 3D model: many starting geometries, each minimised
//! under the chosen force field, the duplicates dropped, the rest listed lowest energy first.
//!
//! The search itself is molecule_optimizer_rs's (`calc::conformer`), run on its engine's thread
//! pool, cancellable, with the conformers found so far readable while it runs. The starting
//! geometries come from one of two [`Method`]s:
//!
//! - [`Method::Etkdg`]: the structure embedded afresh with ETKDG, as RDKit's
//!   `EmbedMultipleConfs` does — rings and all are sampled, the stereo of the model kept;
//! - [`Method::RandomTorsion`]: the model's own geometry with every rotatable bond turned to a
//!   random angle — the rings stay as they are (the first start is the model itself).
//!
//! Every conformer is superposed (Kabsch, heavy atoms) on the model the search started from,
//! so putting one into the 3D view leaves the molecule where it was.

use std::sync::Arc;
use std::time::{Duration, Instant};

use molecule_optimizer_rs::Molecule;
use molecule_optimizer_rs::calc::conformer::RandomTorsionGenerator;
use molecule_optimizer_rs::calc::{
    ConformerSearchOptions, ConformerSearchResult, ConformerSearchSnapshot,
};
use molecule_optimizer_rs::core::geometry::kabsch;
use molecule_optimizer_rs::distgeom::EmbedParams;
use molecule_optimizer_rs::io;
use molecule_optimizer_rs::optim::{MinimizeOptions, MinimizeStatus};
use molecule_optimizer_rs::tasks::{Engine, JobHandle};

use crate::three_d::geom::Vec3;
use crate::three_d::{self, ForceField, Model3d};

/// The most atoms (hydrogens included) a search accepts: every start is an embedding and a
/// whole minimisation over all atom pairs.
pub const MAX_ATOMS: usize = 300;

/// The most starting geometries of one search.
pub const MAX_STARTS: usize = 1000;

/// Two conformers closer than the RMSD threshold are the same one only if their energies are
/// also within this (kcal/mol): minimisations of one conformer end a little apart.
const ENERGY_TOLERANCE: f64 = 0.5;

/// Where the starting geometries come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Method {
    /// Embedded afresh with ETKDG (rings sampled too).
    #[default]
    Etkdg,
    /// The model with its rotatable bonds turned at random (rings kept).
    RandomTorsion,
}

impl Method {
    pub const ALL: [Method; 2] = [Method::Etkdg, Method::RandomTorsion];

    pub fn label(self) -> &'static str {
        match self {
            Method::Etkdg => t!("conformer.method.etkdg"),
            Method::RandomTorsion => t!("conformer.method.random_torsion"),
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Method::Etkdg => t!("conformer.method.etkdg_tip"),
            Method::RandomTorsion => t!("conformer.method.random_torsion_tip"),
        }
    }
}

/// What a search is asked to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub method: Method,
    /// How many starting geometries (1 to [`MAX_STARTS`]).
    pub starts: usize,
    /// Conformers whose heavy atoms superpose within this (Å) are one.
    pub rmsd_threshold: f64,
    /// Keep conformers at most this far (kcal/mol) above the lowest; not finite keeps all.
    pub energy_window: f64,
    pub force_field: ForceField,
    /// The random stream's seed: the same seed, the same search.
    pub seed: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            method: Method::default(),
            starts: 50,
            rmsd_threshold: 0.5,
            energy_window: 10.0,
            force_field: ForceField::default(),
            seed: 42,
        }
    }
}

impl Settings {
    fn options(&self) -> ConformerSearchOptions {
        let mut options = match self.method {
            Method::Etkdg => {
                let params = EmbedParams {
                    random_seed: (self.seed & 0x7fff_ffff) as i32,
                    ..EmbedParams::etkdg_v3()
                };
                ConformerSearchOptions::etkdg(params, 0)
            }
            Method::RandomTorsion => {
                ConformerSearchOptions::new(Arc::new(RandomTorsionGenerator { seed: self.seed }))
            }
        };
        options.num_conformers = self.starts.clamp(1, MAX_STARTS);
        options.minimize = MinimizeOptions {
            max_iters: three_d::OPTIMIZE_STEPS as usize,
            ..MinimizeOptions::default()
        };
        options.rmsd_threshold = self.rmsd_threshold.max(0.0);
        options.energy_tolerance = ENERGY_TOLERANCE;
        options.energy_window = if self.energy_window.is_finite() && self.energy_window > 0.0 {
            self.energy_window
        } else {
            f64::INFINITY
        };
        options
    }
}

/// Why a search of a structure of `atoms` atoms is refused, if it is.
pub fn refusal(atoms: usize) -> Option<String> {
    (atoms > MAX_ATOMS).then(|| {
        t!(
            "conformer.window.too_many_atoms",
            atoms = atoms,
            max = MAX_ATOMS
        )
    })
}

/// One conformer found: its geometry (superposed on the starting model) and energy.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// The starting geometry it came from.
    pub start: usize,
    pub coords: Vec<Vec3>,
    /// The force field's energy (kcal/mol).
    pub energy: f64,
    /// Whether the minimiser converged before its iterations ran out.
    pub converged: bool,
}

/// How far a search has got.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    /// Starts minimised, or failed.
    pub done: usize,
    pub failed: usize,
    pub total: usize,
    pub elapsed: Duration,
    /// Estimated from the pace so far; `None` before the first start is done, or once all are.
    pub left: Option<Duration>,
}

type Handle = JobHandle<ConformerSearchResult, ConformerSearchSnapshot>;

/// A conformational search of one model, running or done.
pub struct Run {
    handle: Option<Handle>,
    /// The structure searched (the model's, hydrogens explicit, atom order kept).
    mol: Molecule,
    /// The model's elements, to tell whether the model is still that structure.
    elements: Vec<String>,
    pub settings: Settings,
    /// The unique conformers, lowest energy first (those found so far while running).
    pub conformers: Vec<Found>,
    completed: usize,
    failed: usize,
    cancelled: bool,
    error: Option<String>,
    started: Instant,
    finished: Option<Instant>,
    /// The snapshot version last read.
    seen: u64,
}

impl Run {
    /// Start searching `model`'s conformers in the background.
    pub fn start(model: &Model3d, settings: Settings) -> Result<Run, String> {
        if let Some(why) = refusal(model.atoms.len()) {
            return Err(why);
        }
        let mol = three_d::read_molfile(&model.molfile).map_err(|e| e.to_string())?;
        if mol.num_atoms() != model.atoms.len() {
            return Err(three_d::Error::Unreadable.to_string());
        }
        // Set up once here, so an atom the force field cannot type is said at once.
        three_d::field(&mol, settings.force_field, &[]).map_err(|e| e.to_string())?;
        let handle = Engine::global().conformer_search(
            Arc::new(mol.clone()),
            three_d::shared_builder(settings.force_field),
            settings.options(),
        );
        Ok(Run {
            handle: Some(handle),
            mol,
            elements: model.atoms.iter().map(|a| a.element.clone()).collect(),
            settings,
            conformers: Vec::new(),
            completed: 0,
            failed: 0,
            cancelled: false,
            error: None,
            started: Instant::now(),
            finished: None,
            seen: 0,
        })
    }

    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    /// Ask the search to stop; the conformers minimised by then are kept.
    pub fn cancel(&mut self) {
        if let Some(handle) = &self.handle {
            handle.cancel();
        }
    }

    pub fn was_cancelled(&self) -> bool {
        self.cancelled
    }

    /// Why the search failed, if it did.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Take in what the search has done since last asked. Whether anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(handle) = &self.handle else {
            return false;
        };
        if handle.is_finished() {
            let handle = self.handle.take().expect("running");
            self.finished = Some(Instant::now());
            match handle.wait_blocking() {
                Ok(result) => {
                    self.cancelled = result.cancelled;
                    self.failed = result.failed;
                    self.completed = result.all.len();
                    self.conformers = self.superposed(&result.conformers);
                }
                Err(e) => self.error = Some(e.to_string()),
            }
            return true;
        }
        let snapshot = handle.snapshot();
        if snapshot.version == self.seen {
            return false;
        }
        self.seen = snapshot.version;
        self.completed = snapshot.data.completed;
        self.failed = snapshot.data.failed;
        self.conformers = self.superposed(&snapshot.data.found);
        true
    }

    /// Block until the search ends (tests).
    pub fn wait(&mut self) {
        while self.is_running() {
            self.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// `found` (lowest energy first) superposed on the starting model, those past the energy
    /// window left out.
    fn superposed(&self, found: &[molecule_optimizer_rs::calc::Conformer]) -> Vec<Found> {
        let heavy = self.mol.heavy_atoms();
        let reference: Vec<Vec3> = heavy.iter().map(|&i| self.mol.coords[i]).collect();
        let lowest = found.first().map_or(0.0, |c| c.energy);
        let window = self.settings.options().energy_window;
        found
            .iter()
            .filter(|c| c.energy.is_finite() && c.energy - lowest <= window)
            .map(|c| {
                let coords = if heavy.len() >= 3 && c.coords.len() == self.mol.coords.len() {
                    let moving: Vec<Vec3> = heavy.iter().map(|&i| c.coords[i]).collect();
                    let (_, r, t) = kabsch(&reference, &moving);
                    c.coords.iter().map(|&p| transform(&r, t, p)).collect()
                } else {
                    c.coords.clone()
                };
                Found {
                    start: c.id,
                    coords,
                    energy: c.energy,
                    converged: c.status == MinimizeStatus::Converged,
                }
            })
            .collect()
    }

    pub fn progress(&self) -> Progress {
        let total = self.settings.starts.clamp(1, MAX_STARTS);
        let done = (self.completed + self.failed).min(total);
        let elapsed = self.finished.unwrap_or_else(Instant::now) - self.started;
        let left = (self.is_running() && done > 0 && done < total)
            .then(|| elapsed.mul_f64((total - done) as f64 / done as f64));
        Progress {
            done,
            failed: self.failed,
            total,
            elapsed,
            left,
        }
    }

    /// The lowest energy found (kcal/mol).
    pub fn lowest(&self) -> Option<f64> {
        self.conformers.first().map(|c| c.energy)
    }

    /// Whether `model` is still the structure this search was of (the same atoms in the same
    /// order), so a conformer can be put into it.
    pub fn fits(&self, model: &Model3d) -> bool {
        model.atoms.len() == self.elements.len()
            && model
                .atoms
                .iter()
                .zip(&self.elements)
                .all(|(a, e)| &a.element == e)
    }

    /// Every conformer as one SD file, lowest energy first, each with its energy, its energy
    /// above the lowest and the force field as data fields.
    pub fn to_sdf(&self) -> String {
        let lowest = self.lowest().unwrap_or(0.0);
        let force_field = self.settings.force_field.id();
        self.conformers
            .iter()
            .enumerate()
            .map(|(rank, c)| {
                io::write_sdf_record(
                    &self.mol,
                    Some(&c.coords),
                    &[
                        ("Conformer", (rank + 1).to_string()),
                        ("Energy", format!("{:.4}", c.energy)),
                        ("RelativeEnergy", format!("{:.4}", c.energy - lowest)),
                        ("EnergyUnit", three_d::ENERGY_UNIT.to_owned()),
                        ("ForceField", force_field.to_owned()),
                    ],
                )
            })
            .collect()
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// `r · p + t`.
fn transform(r: &[[f64; 3]; 3], t: Vec3, p: Vec3) -> Vec3 {
    std::array::from_fn(|i| r[i][0] * p[0] + r[i][1] * p[1] + r[i][2] * p[2] + t[i])
}

#[cfg(test)]
impl Run {
    /// A search that runs until cancelled (a long one of butane).
    pub fn running_for_test() -> Run {
        let model = tests::butane(ForceField::Uff);
        let settings = Settings {
            starts: MAX_STARTS,
            force_field: ForceField::Uff,
            ..Settings::default()
        };
        Run::start(&model, settings).expect("search")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn butane(force_field: ForceField) -> Model3d {
        three_d::generate_from_molfile(
            include_str!("../../tests/data/drawn/butane.mol"),
            force_field,
        )
        .expect("butane")
    }

    fn pentane(force_field: ForceField) -> Model3d {
        three_d::generate_from_molfile(
            include_str!("../../tests/data/drawn/pentane.mol"),
            force_field,
        )
        .expect("pentane")
    }

    fn search(model: &Model3d, settings: Settings) -> Run {
        let mut run = Run::start(model, settings).expect("search starts");
        run.wait();
        assert!(run.error().is_none(), "{:?}", run.error());
        run
    }

    fn heavy_rmsd(a: &[Vec3], b: &[Vec3], heavy: &[usize]) -> f64 {
        let p: Vec<Vec3> = heavy.iter().map(|&i| a[i]).collect();
        let q: Vec<Vec3> = heavy.iter().map(|&i| b[i]).collect();
        molecule_optimizer_rs::core::geometry::kabsch_rmsd(&p, &q)
    }

    /// Butane has two conformers, anti and gauche (twice, mirror images): both methods find
    /// anti as the lowest, gauche above it, sorted, without duplicates.
    #[test]
    fn butane_has_anti_and_gauche() {
        let model = butane(ForceField::Mmff94);
        for method in Method::ALL {
            let run = search(
                &model,
                Settings {
                    method,
                    starts: 20,
                    force_field: ForceField::Mmff94,
                    ..Settings::default()
                },
            );
            let found = &run.conformers;
            assert!(found.len() >= 2, "{method:?}: {} conformers", found.len());
            assert!(
                found.windows(2).all(|w| w[0].energy <= w[1].energy),
                "{method:?}: sorted"
            );
            let gap = found[1].energy - found[0].energy;
            // MMFF94's anti–gauche gap is about 0.8 kcal/mol.
            assert!(
                (0.3..2.0).contains(&gap),
                "{method:?}: gauche {gap:.2} above anti"
            );
            let heavy = run.mol.heavy_atoms();
            for (i, a) in found.iter().enumerate() {
                assert_eq!(a.coords.len(), model.atoms.len());
                for b in &found[i + 1..] {
                    let same = heavy_rmsd(&a.coords, &b.coords, &heavy)
                        < run.settings.rmsd_threshold
                        && (a.energy - b.energy).abs() < ENERGY_TOLERANCE;
                    assert!(!same, "{method:?}: a duplicate");
                }
            }
            assert!(!run.was_cancelled());
            assert_eq!(run.progress().done, 20);
        }
    }

    /// The conformers are superposed on the model: the heavy atoms of the anti conformer of
    /// pentane lie close to where the model's are, not somewhere the embedder put them.
    #[test]
    fn conformers_are_superposed_on_the_model() {
        let model = pentane(ForceField::Uff);
        let run = search(
            &model,
            Settings {
                method: Method::Etkdg,
                starts: 10,
                force_field: ForceField::Uff,
                ..Settings::default()
            },
        );
        let centre = |coords: &[Vec3]| -> Vec3 {
            let heavy = run.mol.heavy_atoms();
            std::array::from_fn(|k| {
                heavy.iter().map(|&i| coords[i][k]).sum::<f64>() / heavy.len() as f64
            })
        };
        let model_coords: Vec<Vec3> = model.atoms.iter().map(|a| a.pos).collect();
        let (a, b) = (centre(&model_coords), centre(&run.conformers[0].coords));
        let shift = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        assert!(shift < 0.5, "centroid moved {shift:.2} Å");
        assert!(run.fits(&model));
    }

    /// Energies are the app's own: putting a conformer into the model and computing its
    /// energy gives what the search reported.
    #[test]
    fn energies_match_the_force_field() {
        let model = butane(ForceField::Uff);
        let run = search(
            &model,
            Settings {
                method: Method::RandomTorsion,
                starts: 8,
                force_field: ForceField::Uff,
                ..Settings::default()
            },
        );
        let mut placed = model.clone();
        crate::three_d::edit::set_coords(&mut placed, &run.conformers[0].coords);
        let energy = three_d::energy(&placed, ForceField::Uff).expect("energy");
        assert!(
            (energy - run.conformers[0].energy).abs() < 1e-3,
            "{energy} vs {}",
            run.conformers[0].energy
        );
    }

    /// The SD file holds one record per conformer, each with its energy.
    #[test]
    fn sdf_holds_every_conformer() {
        let model = butane(ForceField::Uff);
        let run = search(
            &model,
            Settings {
                starts: 10,
                force_field: ForceField::Uff,
                ..Settings::default()
            },
        );
        let sdf = run.to_sdf();
        assert_eq!(sdf.matches("$$$$").count(), run.conformers.len());
        assert_eq!(sdf.matches("> <Energy>").count(), run.conformers.len());
    }

    /// Cancel ends a long search soon, keeping what it found.
    #[test]
    fn cancel_ends_a_search() {
        let mut run = Run::running_for_test();
        assert!(run.is_running());
        std::thread::sleep(Duration::from_millis(50));
        let asked = Instant::now();
        run.cancel();
        run.wait();
        assert!(!run.is_running());
        assert!(
            asked.elapsed() < Duration::from_secs(5),
            "{:?}",
            asked.elapsed()
        );
        assert!(run.was_cancelled() || run.progress().done == MAX_STARTS);
    }

    #[test]
    fn too_many_atoms_is_refused() {
        assert!(refusal(MAX_ATOMS).is_none());
        assert!(refusal(MAX_ATOMS + 1).is_some());
    }
}
