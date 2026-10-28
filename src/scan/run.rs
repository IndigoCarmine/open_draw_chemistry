//! A scan running on a background thread: the points' energies stream back over a channel in
//! grid order, so the plot fills as they arrive; the UI polls each frame ([`Run::poll`]) and
//! can cancel between points.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use super::energy::{self, Evaluator, Relaxation};
use super::source::{Point, Reference, Source};
use super::{Axis, MAX_ATOMS, MAX_RELAX_ATOMS, MAX_RELAX_POINTS, Scan, ScanAxis, grid};
use crate::three_d::geom::Vec3;
use crate::three_d::{Constraint, ForceField, OPTIMIZE_STEPS, edit};

/// Up to this many atoms the drift sentinel (see [`Run::drifted`]) runs after every point.
const SENTINEL_EVERY_POINT_ATOMS: usize = 2_000;

/// What the background thread reports.
#[derive(Debug)]
enum Message {
    /// The structure was read and the force field set up, in `setup`.
    Ready { atoms: usize, setup: Duration },
    /// Point `index`'s energy (NaN when it failed).
    Point { index: usize, energy: f64 },
    /// Point `index`'s relaxed structure, sent before its energy, and whether its minimisation
    /// converged.
    Relaxed {
        index: usize,
        coords: Vec<Vec3>,
        converged: bool,
    },
    /// The reference structure's energy at the end less that at the start: not zero when
    /// OpenBabel's force field was set up for another molecule meanwhile (see
    /// [`Run::drifted`]).
    Drift { start: f64, end: f64 },
    /// Every point is done, or the scan was cancelled.
    Finished,
    /// The scan could not start.
    Failed(String),
}

/// Where a run is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub elapsed: Duration,
    /// The time still needed, estimated from the points done so far (none before the first).
    pub remaining: Option<Duration>,
    /// How long reading the structure and setting up the force field took.
    pub setup: Option<Duration>,
}

/// How a relaxed scan minimises each point: everything but the scanned parameters moves.
#[derive(Debug, Clone, PartialEq)]
pub struct Relax {
    /// Restraints kept at every point besides the scanned parameters — those set in the 3D
    /// view (fixed atoms, held lengths…). One over the same atoms as a scanned parameter gives
    /// way to the scanned value.
    pub constraints: Vec<Constraint>,
    /// The most minimiser steps per point; a point still moving after them is unconverged.
    pub steps: u32,
}

impl Relax {
    /// Relaxing under `constraints` with an optimisation's step budget.
    pub fn new(constraints: Vec<Constraint>) -> Self {
        Self {
            constraints,
            steps: OPTIMIZE_STEPS,
        }
    }

    /// The restraints of the point where `params` take `values`.
    fn at(&self, params: &[super::Param], values: &[f64]) -> Vec<Constraint> {
        let held: Vec<Constraint> = params
            .iter()
            .zip(values)
            .filter_map(|(p, &v)| p.restraint(v))
            .collect();
        self.constraints
            .iter()
            .filter(|c| !held.iter().any(|h| h.same_atoms(c)))
            .copied()
            .chain(held.iter().copied())
            .collect()
    }
}

/// A scan: its grid, the energies so far, and the thread computing the rest.
pub struct Run {
    pub scan: Scan,
    /// What was scanned, kept so a point can be applied to the design or the model.
    pub source: Arc<Source>,
    /// A relaxed scan's minimised structure at each point (`None` until it arrives, and for
    /// every point of a rigid scan).
    structures: Vec<Option<Vec<Vec3>>>,
    /// Atoms in the evaluated structure (known once the thread has read it).
    pub atoms: Option<usize>,
    /// Why the scan stopped early (a failure to start), for the window.
    pub error: Option<String>,
    receiver: Option<Receiver<Message>>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    /// When the last point arrived (the clock stops when the run ends).
    finished: Option<Instant>,
    setup: Option<Duration>,
    cancelled: bool,
    drifted: bool,
}

impl Run {
    /// Start scanning `axes` (one or two, the parameters of `source` in order) under
    /// `force_field`. Refuses at once — without a thread — a structure of more than
    /// [`MAX_ATOMS`] atoms and a scan without axes.
    pub fn start(source: Source, axes: &[Axis], force_field: ForceField) -> Result<Run, String> {
        let atoms = source.atom_count();
        if atoms > MAX_ATOMS {
            return Err(t!(
                "scan.error.too_many_atoms",
                atoms = atoms,
                max = MAX_ATOMS
            ));
        }
        let points = grid(axes);
        let work = move |source: &Source, send: &Sender<Message>, cancel: &AtomicBool| {
            let Some((reference, mut evaluator)) = set_up(source, force_field, send) else {
                return;
            };
            let start = evaluator.energy(&reference.coords);
            let check_every = sentinel_every(&reference);
            for (index, values) in points.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let energy = match source.at(values) {
                    Ok(Point::Coords(coords)) => evaluator.energy(&coords),
                    Ok(Point::Rebuilt { atoms, bonds }) => {
                        energy::single_point(&atoms, &bonds, force_field)
                    }
                    Err(_) => f64::NAN,
                };
                if send.send(Message::Point { index, energy }).is_err() {
                    // The window went away.
                    return;
                }
                if index % check_every == check_every - 1 {
                    let end = evaluator.energy(&reference.coords);
                    let _ = send.send(Message::Drift { start, end });
                }
            }
            let end = evaluator.energy(&reference.coords);
            let _ = send.send(Message::Drift { start, end });
        };
        Ok(Self::spawn(source, axes, force_field, work))
    }

    /// Start a relaxed scan of the 3D model: at every point the scanned parameters are set
    /// and held while everything else is minimised under `relax`, and the energy is that of
    /// the minimised structure. Each point starts from its neighbour's minimised structure
    /// (the previous point along the second axis, else the first point of the previous row).
    /// Refuses at once a source other than the model, more than [`MAX_RELAX_ATOMS`] atoms or
    /// [`MAX_RELAX_POINTS`] points, and restraints OpenBabel cannot hold together.
    pub fn start_relaxed(
        source: Source,
        axes: &[Axis],
        force_field: ForceField,
        relax: Relax,
    ) -> Result<Run, String> {
        if !matches!(source, Source::Model { .. }) {
            return Err(t!("scan.window.relax_model_only").into());
        }
        let atoms = source.atom_count();
        if atoms > MAX_RELAX_ATOMS {
            return Err(t!(
                "scan.window.relax_too_many_atoms",
                atoms = atoms,
                max = MAX_RELAX_ATOMS
            ));
        }
        let points = grid(axes);
        if points.len() > MAX_RELAX_POINTS {
            return Err(t!(
                "scan.window.relax_too_many_points",
                points = points.len(),
                max = MAX_RELAX_POINTS
            ));
        }
        let params = source.params().to_vec();
        if let Some(values) = points.first()
            && edit::conflict(&relax.at(&params, values)).is_some()
        {
            return Err(t!("scan.window.relax_conflict").into());
        }
        let row = axes.get(1).map_or(1, |a| a.values().len()).max(1);
        let work = move |source: &Source, send: &Sender<Message>, cancel: &AtomicBool| {
            let Some((reference, mut evaluator)) = set_up(source, force_field, send) else {
                return;
            };
            let start = evaluator.energy(&reference.coords);
            let check_every = sentinel_every(&reference);
            // The minimised structures to start from: the last point's, and the first point's
            // of the last row.
            let mut previous: Option<Vec<Vec3>> = None;
            let mut row_first: Option<Vec<Vec3>> = None;
            for (index, values) in points.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let first_of_row = index % row == 0;
                let from = if first_of_row { &row_first } else { &previous };
                let from = from.as_deref().unwrap_or(&reference.coords);
                let relaxed = match source.at_from(from, values) {
                    Ok(Point::Coords(coords)) => {
                        evaluator.relax(&coords, &relax.at(&params, values), relax.steps, cancel)
                    }
                    Ok(Point::Rebuilt { .. }) | Err(_) => Relaxation::Failed,
                };
                let (energy, done) = match relaxed {
                    Relaxation::Cancelled => break,
                    Relaxation::Failed => (f64::NAN, None),
                    Relaxation::Done {
                        coords,
                        energy,
                        converged,
                    } => {
                        let message = Message::Relaxed {
                            index,
                            coords: coords.clone(),
                            converged,
                        };
                        if send.send(message).is_err() {
                            return;
                        }
                        (energy, Some(coords))
                    }
                };
                if first_of_row {
                    row_first.clone_from(&done);
                }
                previous = done;
                if send.send(Message::Point { index, energy }).is_err() {
                    return;
                }
                if index % check_every == check_every - 1 {
                    let end = evaluator.energy(&reference.coords);
                    let _ = send.send(Message::Drift { start, end });
                }
            }
            let end = evaluator.energy(&reference.coords);
            let _ = send.send(Message::Drift { start, end });
        };
        let mut run = Self::spawn(source, axes, force_field, work);
        run.scan.relaxed = true;
        Ok(run)
    }

    /// Start `work` on a thread for a scan over `axes`. `work` sends the messages; the
    /// finishing one is sent here.
    fn spawn(
        source: Source,
        axes: &[Axis],
        force_field: ForceField,
        work: impl FnOnce(&Source, &Sender<Message>, &AtomicBool) + Send + 'static,
    ) -> Run {
        let scan_axes = axes
            .iter()
            .map(|a| ScanAxis {
                param: a.param.clone(),
                label: a.param.label(design_of(&source), model_of(&source)),
                values: a.values(),
            })
            .collect();
        let scan = Scan::new(scan_axes, force_field, energy::unit(force_field));
        let source = Arc::new(source);
        let cancel = Arc::new(AtomicBool::new(false));
        let (send, receive) = mpsc::channel();
        let (thread_source, thread_cancel) = (source.clone(), cancel.clone());
        let spawned = std::thread::Builder::new()
            .name("param-scan".to_owned())
            .spawn(move || {
                work(&thread_source, &send, &thread_cancel);
                let _ = send.send(Message::Finished);
            });
        let mut run = Run {
            structures: vec![None; scan.total()],
            scan,
            source,
            atoms: None,
            error: None,
            receiver: Some(receive),
            cancel,
            started: Instant::now(),
            finished: None,
            setup: None,
            cancelled: false,
            drifted: false,
        };
        if let Err(e) = spawned {
            run.error = Some(t!("scan.error.start_failed", error = e));
            run.receiver = None;
            run.finished = Some(Instant::now());
        }
        run
    }

    /// A finished run holding `scan` (no thread): for tests and previews.
    pub fn finished(scan: Scan, source: Source) -> Run {
        let now = Instant::now();
        Run {
            structures: vec![None; scan.total()],
            scan,
            source: Arc::new(source),
            atoms: None,
            error: None,
            receiver: None,
            cancel: Arc::new(AtomicBool::new(false)),
            started: now,
            finished: Some(now),
            setup: None,
            cancelled: false,
            drifted: false,
        }
    }

    /// A finished relaxed run holding `scan` and each point's minimised structure (no
    /// thread): for tests and previews.
    pub fn finished_relaxed(
        mut scan: Scan,
        source: Source,
        mut structures: Vec<Option<Vec<Vec3>>>,
    ) -> Run {
        scan.relaxed = true;
        structures.resize(scan.total(), None);
        let mut run = Self::finished(scan, source);
        run.structures = structures;
        run
    }

    /// Point `index`'s minimised structure, in a relaxed scan once the point is in.
    pub fn structure(&self, index: usize) -> Option<&[Vec3]> {
        self.structures.get(index)?.as_deref()
    }

    pub fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn was_cancelled(&self) -> bool {
        self.cancelled
    }

    /// Whether the energies may not be comparable with one another: OpenBabel keeps one
    /// force-field setup per process, and setting it up for another molecule (another
    /// document's optimisation, a structure re-read at a point) can change the energy of this
    /// one by a small offset (MMFF94 charges, measured ~0.2–2 kcal/mol for butane). Detected
    /// by re-evaluating the starting structure as the scan goes (every point for up to
    /// [`SENTINEL_EVERY_POINT_ATOMS`] atoms, else every tenth, and at the end).
    pub fn drifted(&self) -> bool {
        self.drifted
    }

    /// Stop after the point being computed (a relaxed scan part-way through its minimisation,
    /// that point dropped); the points done so far stay.
    pub fn cancel(&mut self) {
        if self.is_running() {
            self.cancel.store(true, Ordering::Relaxed);
            self.cancelled = true;
        }
    }

    /// Take the energies that have arrived. Returns whether anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(receiver) = &self.receiver else {
            return false;
        };
        let mut changed = false;
        loop {
            match receiver.try_recv() {
                Ok(Message::Ready { atoms, setup }) => {
                    self.atoms = Some(atoms);
                    self.setup = Some(setup);
                }
                Ok(Message::Point { index, energy }) => {
                    if let Some(slot) = self.scan.energies.get_mut(index) {
                        *slot = Some(energy);
                    }
                }
                Ok(Message::Relaxed {
                    index,
                    coords,
                    converged,
                }) => {
                    if let Some(slot) = self.structures.get_mut(index) {
                        *slot = Some(coords);
                    }
                    if let Some(slot) = self.scan.unconverged.get_mut(index) {
                        *slot = !converged;
                    }
                }
                Ok(Message::Failed(e)) => self.error = Some(e),
                Ok(Message::Drift { start, end }) => {
                    // NaN at one end only (the energy became unobtainable) counts too.
                    let diff = (start - end).abs();
                    self.drifted |= !(start.is_nan() && end.is_nan())
                        && (diff.is_nan() || diff > 1e-6 * start.abs().max(1.0));
                }
                Ok(Message::Finished) | Err(TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.finished = Some(Instant::now());
                    return true;
                }
                Err(TryRecvError::Empty) => return changed,
            }
            changed = true;
        }
    }

    /// Wait for the thread to finish (tests, and closing).
    pub fn wait(&mut self) {
        while self.is_running() {
            self.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn progress(&self) -> Progress {
        let done = self.scan.done();
        let total = self.scan.total();
        let elapsed = self.finished.unwrap_or_else(Instant::now) - self.started;
        let setup = self.setup;
        let remaining = (done > 0 && self.is_running()).then(|| {
            let per_point = elapsed.saturating_sub(setup.unwrap_or_default()) / done as u32;
            per_point * (total - done) as u32
        });
        Progress {
            done,
            total,
            elapsed,
            remaining,
            setup,
        }
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        // A scan nobody will look at stops at the next point.
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Read the structure the scan starts from and set the force field up for it, telling the
/// window how long that took; on failure the window is told why and `None` returned.
fn set_up(
    source: &Source,
    force_field: ForceField,
    send: &Sender<Message>,
) -> Option<(Reference, Evaluator)> {
    let started = Instant::now();
    let set_up = source
        .reference()
        .and_then(|r| Evaluator::new(&r.molfile, force_field).map(|e| (r, e)));
    match set_up {
        Ok((reference, evaluator)) => {
            let _ = send.send(Message::Ready {
                atoms: evaluator.atoms(),
                setup: started.elapsed(),
            });
            Some((reference, evaluator))
        }
        Err(e) => {
            let _ = send.send(Message::Failed(e));
            None
        }
    }
}

/// How often the starting structure is re-evaluated as a drift sentinel: after every point for
/// a small structure (cheap), every tenth for a large one.
fn sentinel_every(reference: &Reference) -> usize {
    if reference.atoms <= SENTINEL_EVERY_POINT_ATOMS {
        1
    } else {
        10
    }
}

fn design_of(source: &Source) -> Option<&crate::assembly::Design> {
    match source {
        Source::Assembly { design, .. } => Some(design),
        Source::Model { .. } => None,
    }
}

fn model_of(source: &Source) -> Option<&crate::three_d::Model3d> {
    match source {
        Source::Model { model, .. } => Some(model),
        Source::Assembly { .. } => None,
    }
}

/// `d` as "1 min 05 s", "12.3 s" or "0.04 s" (in Japanese "1 分 05 秒", "12.3 秒", "0.04 秒").
pub fn format_duration(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 10.0 {
        t!("scan.duration.seconds", seconds = format!("{s:.2}"))
    } else if s < 60.0 {
        t!("scan.duration.seconds", seconds = format!("{s:.1}"))
    } else if s < 3600.0 {
        t!(
            "scan.duration.minutes",
            minutes = (s / 60.0) as u64,
            seconds = format!("{:02}", (s % 60.0) as u64)
        )
    } else {
        t!(
            "scan.duration.hours",
            hours = (s / 3600.0) as u64,
            minutes = format!("{:02}", ((s % 3600.0) / 60.0) as u64)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Param;
    use crate::three_d::{Atom3d, Bond3d, Model3d};

    /// A model for the plumbing tests (its energy is never asked of OpenBabel).
    fn chain_model() -> Model3d {
        let pos = [
            [0.0, 0.0, 0.0],
            [1.5, 0.0, 0.0],
            [2.0, 1.4, 0.0],
            [3.5, 1.4, 0.0],
        ];
        Model3d {
            atoms: pos
                .iter()
                .map(|&pos| Atom3d {
                    element: "C".into(),
                    pos,
                })
                .collect(),
            bonds: (0..3)
                .map(|i| Bond3d {
                    begin: i,
                    end: i + 1,
                    order: 1,
                })
                .collect(),
            force_field: ForceField::Uff,
            energy: 0.0,
            energy_unit: "kJ/mol".into(),
            formula: "C4".into(),
            molfile: String::new(),
        }
    }

    fn dihedral_axis(points: usize) -> Axis {
        Axis {
            param: Param::Dihedral([0, 1, 2, 3]),
            start: -180.0,
            end: 180.0,
            points,
        }
    }

    /// The work of a real scan, with a made-up energy: the dihedral's cosine, slowly.
    fn fake_work(
        delay: Duration,
    ) -> impl FnOnce(&Source, &Sender<Message>, &AtomicBool) + Send + 'static {
        move |source: &Source, send: &Sender<Message>, cancel: &AtomicBool| {
            let _ = send.send(Message::Ready {
                atoms: 4,
                setup: Duration::ZERO,
            });
            for index in 0.. {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(delay);
                let values = [-180.0 + 10.0 * index as f64];
                let energy = match source.at(&values) {
                    Ok(Point::Coords(c)) if index % 5 != 4 => source.params()[0]
                        .read_coords(&c)
                        .unwrap()
                        .to_radians()
                        .cos(),
                    _ => f64::NAN,
                };
                if send.send(Message::Point { index, energy }).is_err() || index >= 36 {
                    break;
                }
            }
        }
    }

    /// A scan whose reference energy differs at the end (OpenBabel set up for another
    /// molecule meanwhile) is marked; one whose does not, or that never got that far, is not.
    #[test]
    fn a_drifting_reference_energy_marks_the_scan() {
        for (start, end, drifted) in [
            (-5.075, -5.251, true),
            (-5.075, -5.075, false),
            (1000.0, 1000.0 + 1e-7, false),
            (f64::NAN, f64::NAN, false),
            (-5.0, f64::NAN, true),
        ] {
            let source = Source::model(&chain_model(), None, vec![Param::Dihedral([0, 1, 2, 3])]);
            let mut run = Run::spawn(
                source,
                &[dihedral_axis(3)],
                ForceField::Mmff94,
                move |_: &Source, send: &Sender<Message>, _: &AtomicBool| {
                    let _ = send.send(Message::Drift { start, end });
                },
            );
            run.wait();
            assert_eq!(run.drifted(), drifted, "{start} → {end}");
        }
    }

    #[test]
    fn points_stream_in_and_failures_stay_nan() {
        let source = Source::model(&chain_model(), None, vec![Param::Dihedral([0, 1, 2, 3])]);
        let mut run = Run::spawn(
            source,
            &[dihedral_axis(37)],
            ForceField::Uff,
            fake_work(Duration::ZERO),
        );
        run.wait();
        assert!(!run.is_running());
        assert_eq!(run.scan.done(), 37);
        assert_eq!(run.atoms, Some(4));
        let nan = run
            .scan
            .energies
            .iter()
            .filter(|e| e.unwrap().is_nan())
            .count();
        assert_eq!(nan, 7, "every fifth point failed");
        // cos(dihedral): the minimum at ±180°.
        let (at, _) = run.scan.minimum().unwrap();
        assert!(at == 0 || at == 36, "{at}");
        assert_eq!(run.progress().remaining, None, "done");
    }

    #[test]
    fn cancelling_keeps_the_points_done() {
        let source = Source::model(&chain_model(), None, vec![Param::Dihedral([0, 1, 2, 3])]);
        let mut run = Run::spawn(
            source,
            &[dihedral_axis(37)],
            ForceField::Uff,
            fake_work(Duration::from_millis(15)),
        );
        while run.scan.done() < 2 {
            run.poll();
            std::thread::sleep(Duration::from_millis(2));
        }
        let progress = run.progress();
        assert!(
            progress.remaining.is_some(),
            "an estimate once points are in"
        );
        run.cancel();
        run.wait();
        assert!(run.was_cancelled());
        let done = run.scan.done();
        assert!((2..37).contains(&done), "{done}");
        // The points done are the first ones, in order.
        assert!(run.scan.energies[..done].iter().all(Option::is_some));
        assert!(run.scan.energies[done..].iter().all(Option::is_none));
    }

    #[test]
    fn too_large_scans_are_refused_at_once() {
        let mut model = chain_model();
        let atom = model.atoms[0].clone();
        model.atoms.resize(MAX_ATOMS + 1, atom);
        let source = Source::model(&model, None, vec![Param::Dihedral([0, 1, 2, 3])]);
        let Err(message) = Run::start(source, &[dihedral_axis(3)], ForceField::Uff) else {
            panic!("a scan of too many atoms started");
        };
        assert!(message.contains("Too many atoms"), "{message}");
    }

    #[test]
    fn durations_read_in_english() {
        assert_eq!(format_duration(Duration::from_millis(12_340)), "12.3 s");
        assert_eq!(format_duration(Duration::from_millis(42)), "0.04 s");
        assert_eq!(format_duration(Duration::from_secs(65)), "1 min 05 s");
        assert_eq!(format_duration(Duration::from_secs(7_260)), "2 h 01 min");
    }

    #[test]
    fn durations_read_in_japanese() {
        let _lang = crate::i18n::ScopedLanguage::new(crate::i18n::Lang::Ja);
        assert_eq!(format_duration(Duration::from_millis(12_340)), "12.3 秒");
        assert_eq!(format_duration(Duration::from_millis(42)), "0.04 秒");
        assert_eq!(format_duration(Duration::from_secs(65)), "1 分 05 秒");
        assert_eq!(format_duration(Duration::from_secs(7_260)), "2 時間 01 分");
    }
}

/// Real scans through OpenBabel.
#[cfg(test)]
mod energy_tests {
    use super::*;
    use crate::assembly::model::{Component, Design, Step};
    use crate::scan::{MAX_POINTS, Param, StepField};
    use crate::three_d::Model3d;
    use chembuider_rs::BondOrder;

    fn generate(drawing: &chembuider_rs::Molecule, force_field: ForceField) -> Model3d {
        crate::three_d::generate(drawing, &chembuider_rs::Config::default(), force_field)
            .expect("generate")
    }

    /// Butane; its carbons are atoms 0–3, as drawn.
    pub(crate) fn butane(force_field: ForceField) -> Model3d {
        let mut drawing = chembuider_rs::Molecule::default();
        let mut prev = drawing.add_atom("C".to_owned(), [0.0, 0.0], 0);
        for i in 1..4 {
            let y = 0.75 * (i % 2) as f32;
            let next = drawing.add_atom("C".to_owned(), [1.3 * i as f32, y], 0);
            drawing.add_bond(prev, next, BondOrder::Single);
            prev = next;
        }
        generate(&drawing, force_field)
    }

    /// The butane torsion profile: anti (180°) lowest, gauche (±60°) a local minimum,
    /// eclipsed (±120°) above gauche and syn (0°) the highest — the shape, not the numbers.
    #[test]
    fn a_butane_torsion_scan_has_the_textbook_shape() {
        for force_field in [ForceField::Mmff94, ForceField::Uff] {
            let model = butane(force_field);
            let param = Param::Dihedral([0, 1, 2, 3]);
            let axis = Axis {
                param: param.clone(),
                start: -180.0,
                end: 180.0,
                points: 13,
            };
            let source = Source::model(&model, None, vec![param]);
            let started = Instant::now();
            let mut run =
                Run::start(source, std::slice::from_ref(&axis), force_field).expect("start");
            run.wait();
            let elapsed = started.elapsed();
            assert!(!run.drifted(), "{force_field}: drifted");
            assert_eq!(run.error, None);
            assert_eq!(run.scan.done(), 13);
            let e: Vec<f64> = run.scan.energies.iter().map(|e| e.unwrap()).collect();
            assert!(e.iter().all(|e| e.is_finite()), "{e:?}");
            // Values: -180, -150, …, 0 (index 6), …, 180.
            let at = |deg: i32| e[((deg + 180) / 30) as usize];
            let (min, _) = run.scan.minimum().unwrap();
            assert!(
                min == 0 || min == 12,
                "{force_field}: minimum at {min}: {e:?}"
            );
            assert!(
                (at(-180) - at(180)).abs() < 1e-3,
                "{force_field}: ±180° differ {e:?}"
            );
            assert!(
                at(0) > at(120) && at(0) > at(-120),
                "{force_field}: syn highest {e:?}"
            );
            assert!(
                at(120) > at(60) && at(-120) > at(-60),
                "{force_field}: {e:?}"
            );
            assert!(at(60) > at(180), "{force_field}: gauche above anti {e:?}");
            assert_eq!(run.scan.unit, model.energy_unit);
            eprintln!(
                "{force_field}: 13 points in {elapsed:?} (setup {:?})",
                run.progress().setup
            );
        }
    }

    fn benzene() -> Component {
        let mut drawing = chembuider_rs::Molecule::default();
        let ids: Vec<u32> = (0..6)
            .map(|i| {
                let t = std::f32::consts::PI / 3.0 * i as f32;
                drawing.add_atom("C".to_owned(), [1.5 * t.cos(), 1.5 * t.sin()], 0)
            })
            .collect();
        for i in 0..6 {
            let order = if i % 2 == 0 {
                BondOrder::Double
            } else {
                BondOrder::Single
            };
            drawing.add_bond(ids[i], ids[(i + 1) % 6], order);
        }
        Component::from_model("benzene", &generate(&drawing, ForceField::Mmff94))
    }

    /// A 2D scan of a stacked dimer: every point computed, the far stack nearly flat, and the
    /// contact minimum at a sensible separation.
    #[test]
    fn a_stack_rise_and_twist_scan_fills_its_grid() {
        let mut stack = Step::new_stack();
        if let Step::Stack(s) = &mut stack {
            s.n = 2;
        }
        let design = Design {
            components: vec![benzene()],
            steps: vec![stack],
            ..Default::default()
        };
        let rise = Param::Step {
            step: 0,
            field: StepField::StackRise,
        };
        let twist = Param::Step {
            step: 0,
            field: StepField::StackTwist,
        };
        let axes = [
            Axis {
                param: rise.clone(),
                start: 3.0,
                end: 6.0,
                points: 7,
            },
            Axis {
                param: twist.clone(),
                start: 0.0,
                end: 60.0,
                points: 5,
            },
        ];
        let source = Source::assembly(&design, vec![rise, twist]);
        let started = Instant::now();
        // UFF: van der Waals without the charges, whose quadrupole repulsion makes MMFF94's
        // face-to-face stack unbound.
        let mut run = Run::start(source, &axes, ForceField::Uff).expect("start");
        run.wait();
        eprintln!("{:?}", run.scan.energies);
        eprintln!("35 points of 24 atoms: {:?}", started.elapsed());
        assert_eq!(run.error, None);
        assert_eq!(run.scan.total(), 35);
        assert_eq!(run.scan.done(), 35);
        assert!(run.scan.energies.iter().all(|e| e.unwrap().is_finite()));
        let (min, _) = run.scan.minimum().unwrap();
        let rise_at_min = run.scan.values_at(min)[0];
        assert!(
            (3.0..=4.5).contains(&rise_at_min),
            "minimum at {rise_at_min} Å: {:?}",
            run.scan.energies
        );
        // At 6 Å the twist hardly matters.
        let far: Vec<f64> = (0..5)
            .map(|j| run.scan.energies[run.scan.index(6, j)].unwrap())
            .collect();
        let spread = far.iter().cloned().fold(f64::MIN, f64::max)
            - far.iter().cloned().fold(f64::MAX, f64::min);
        assert!(spread < 0.5, "{far:?}");
    }

    /// Timing of a larger assembly: a 40-layer stack of benzene (480 atoms), per point.
    #[test]
    fn a_larger_stack_is_scanned_by_moving_coordinates() {
        let mut stack = Step::new_stack();
        if let Step::Stack(s) = &mut stack {
            s.n = 40;
        }
        let design = Design {
            components: vec![benzene()],
            steps: vec![stack],
            ..Default::default()
        };
        let rise = Param::Step {
            step: 0,
            field: StepField::StackRise,
        };
        let axis = Axis {
            param: rise.clone(),
            start: 3.4,
            end: 4.0,
            points: 10,
        };
        let source = Source::assembly(&design, vec![rise]);
        // The moved structure keeps the reference's atoms: no re-parse per point.
        assert!(matches!(source.at(&[3.7]), Ok(Point::Coords(_))));
        let mut run = Run::start(source, &[axis], ForceField::Mmff94).expect("start");
        run.wait();
        let p = run.progress();
        eprintln!(
            "480 atoms: setup {:?}, 10 points in {:?}",
            p.setup,
            p.elapsed - p.setup.unwrap_or_default()
        );
        assert_eq!(run.scan.done(), 10);
        assert!(run.scan.energies.iter().all(|e| e.unwrap().is_finite()));
    }

    fn butane_dihedral_axis(points: usize) -> Axis {
        Axis {
            param: Param::Dihedral([0, 1, 2, 3]),
            start: -180.0,
            end: 180.0,
            points,
        }
    }

    /// The largest difference between each point's scanned values and those measured in its
    /// relaxed structure (degrees wrapped).
    fn worst_deviation(run: &Run) -> f64 {
        let params = run.source.params();
        (0..run.scan.total())
            .map(|i| {
                let coords = run.structure(i).expect("a relaxed structure");
                params
                    .iter()
                    .zip(run.scan.values_at(i))
                    .map(|(p, v)| {
                        let now = p.read_coords(coords).unwrap();
                        match p {
                            Param::Dihedral(_) => crate::three_d::geom::wrap_degrees(now - v),
                            _ => now - v,
                        }
                        .abs()
                    })
                    .fold(0.0, f64::max)
            })
            .fold(0.0, f64::max)
    }

    /// How closely OpenBabel's minimiser settles on a minimum (in the force field's unit). At
    /// anti the rigid structure is the model itself, minimised in several rounds as it was
    /// generated; even an unrestrained minimisation from the relaxed scan's anti structure
    /// stops ~0.002 kcal/mol (MMFF94) above it.
    const CONVERGED_WITHIN: f64 = 0.005;

    /// The butane torsion relaxed: at every point the dihedral stays at its value while the
    /// rest minimises, so the energy is at most the rigid scan's at the same point (the
    /// anti minimum is the same structure, the eclipsed barriers come down).
    #[test]
    fn a_relaxed_butane_torsion_scan_holds_the_dihedral_and_lies_below_the_rigid_one() {
        for force_field in [ForceField::Mmff94, ForceField::Uff] {
            let model = butane(force_field);
            let axis = butane_dihedral_axis(13);
            let params = vec![axis.param.clone()];
            let mut rigid = Run::start(
                Source::model(&model, None, params.clone()),
                std::slice::from_ref(&axis),
                force_field,
            )
            .expect("rigid");
            rigid.wait();
            let started = Instant::now();
            let mut relaxed = Run::start_relaxed(
                Source::model(&model, None, params),
                std::slice::from_ref(&axis),
                force_field,
                Relax::new(Vec::new()),
            )
            .expect("relaxed");
            relaxed.wait();
            let elapsed = started.elapsed();
            assert_eq!(relaxed.error, None);
            assert!(!relaxed.drifted(), "{force_field}: drifted");
            assert!(relaxed.scan.relaxed && !rigid.scan.relaxed);
            assert_eq!(relaxed.scan.done(), 13);
            assert!(rigid.scan.unconverged.iter().all(|u| !u));
            let deviation = worst_deviation(&relaxed);
            assert!(
                deviation < 0.5,
                "{force_field}: dihedral off by {deviation}°"
            );
            eprintln!("{force_field}: 13 relaxed points in {elapsed:?}, worst {deviation:.4}°");
            eprintln!(
                "{force_field} ({}) dihedral | rigid | relaxed",
                relaxed.scan.unit
            );
            for i in 0..13 {
                let (r, x) = (
                    rigid.scan.energies[i].unwrap(),
                    relaxed.scan.energies[i].unwrap(),
                );
                eprintln!(
                    "{:>6.0} | {r:>9.3} | {x:>9.3}{}",
                    relaxed.scan.values_at(i)[0],
                    if relaxed.scan.is_unconverged(i) {
                        " (unconverged)"
                    } else {
                        ""
                    }
                );
                assert!(x.is_finite(), "{force_field}: point {i} failed");
                assert!(
                    x <= r + CONVERGED_WITHIN,
                    "{force_field}: point {i}: relaxed {x} above rigid {r}"
                );
            }
            assert!(
                (0..13).all(|i| !relaxed.scan.is_unconverged(i)),
                "{force_field}: butane converges within the budget"
            );
            // The eclipsed barrier (syn, 0°) comes down when the angles may open.
            let syn = 6;
            assert!(
                relaxed.scan.energies[syn].unwrap() < rigid.scan.energies[syn].unwrap() - 0.5,
                "{force_field}: syn not relaxed"
            );
        }
    }

    /// A 2D relaxed scan holds both parameters at every point.
    #[test]
    fn a_2d_relaxed_scan_holds_both_parameters() {
        let model = butane(ForceField::Mmff94);
        let axes = [
            Axis {
                param: Param::Bond([1, 2]),
                start: 1.45,
                end: 1.65,
                points: 3,
            },
            butane_dihedral_axis(4),
        ];
        let params: Vec<Param> = axes.iter().map(|a| a.param.clone()).collect();
        let mut run = Run::start_relaxed(
            Source::model(&model, None, params),
            &axes,
            ForceField::Mmff94,
            Relax::new(Vec::new()),
        )
        .expect("start");
        run.wait();
        assert_eq!(run.error, None);
        assert_eq!(run.scan.done(), 12);
        assert!(run.scan.energies.iter().all(|e| e.unwrap().is_finite()));
        let deviation = worst_deviation(&run);
        assert!(deviation < 0.5, "off by {deviation}");
        // The bond length is held to a hundredth of an ångström.
        for i in 0..12 {
            let held = Param::Bond([1, 2])
                .read_coords(run.structure(i).unwrap())
                .unwrap();
            let want = run.scan.values_at(i)[0];
            assert!((held - want).abs() < 0.01, "point {i}: {held} for {want}");
        }
    }

    /// The restraints set in the 3D view hold at every point too: a fixed atom stays put.
    #[test]
    fn a_relaxed_scan_keeps_the_restraints_of_the_3d_view() {
        let model = butane(ForceField::Uff);
        let axis = butane_dihedral_axis(7);
        // Atom 0 is the anchor (the dihedral turns the other side) and fixed while relaxing.
        let fixed = model.atoms[0].pos;
        let mut run = Run::start_relaxed(
            Source::model(&model, Some(0), vec![axis.param.clone()]),
            std::slice::from_ref(&axis),
            ForceField::Uff,
            Relax::new(vec![
                Constraint::Fix(0),
                // Over the scanned atoms: gives way to the scanned value.
                Constraint::Torsion([3, 2, 1, 0], 60.0),
            ]),
        )
        .expect("start");
        run.wait();
        assert_eq!(run.scan.done(), 7);
        for i in 0..7 {
            let at = run.structure(i).unwrap()[0];
            assert!(
                crate::three_d::geom::distance(at, fixed) < 1e-3,
                "point {i}: the fixed atom moved to {at:?}"
            );
        }
        let deviation = worst_deviation(&run);
        assert!(deviation < 0.5, "off by {deviation}°");
    }

    /// A point whose minimisation runs out of steps still moving keeps its energy but is
    /// marked unconverged, unlike a failed one.
    #[test]
    fn points_out_of_minimiser_steps_are_marked_unconverged() {
        let model = butane(ForceField::Uff);
        let axis = butane_dihedral_axis(5);
        let relax = Relax {
            constraints: Vec::new(),
            steps: 2,
        };
        let mut run = Run::start_relaxed(
            Source::model(&model, None, vec![axis.param.clone()]),
            std::slice::from_ref(&axis),
            ForceField::Uff,
            relax,
        )
        .expect("start");
        run.wait();
        assert_eq!(run.scan.done(), 5);
        for i in 0..5 {
            assert!(run.scan.energies[i].unwrap().is_finite(), "point {i}");
            assert!(run.scan.is_unconverged(i), "point {i}");
        }
        let csv = run.scan.to_csv();
        assert!(
            csv.lines().next().unwrap().ends_with(",\"converged\""),
            "{csv}"
        );
        assert!(csv.lines().skip(1).all(|l| l.ends_with(",false")), "{csv}");
    }

    /// Cancelling a relaxed scan stops it part-way through, also inside a point's minimisation.
    #[test]
    fn a_relaxed_scan_can_be_cancelled() {
        let model = butane(ForceField::Mmff94);
        let axis = butane_dihedral_axis(MAX_POINTS);
        let mut run = Run::start_relaxed(
            Source::model(&model, None, vec![axis.param.clone()]),
            std::slice::from_ref(&axis),
            ForceField::Mmff94,
            Relax::new(Vec::new()),
        )
        .expect("start");
        while run.scan.done() < 1 && run.is_running() {
            run.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        run.cancel();
        run.wait();
        assert!(run.was_cancelled());
        let done = run.scan.done();
        assert!((1..MAX_POINTS).contains(&done), "{done}");
        assert!(run.scan.energies[..done].iter().all(Option::is_some));
        assert!(run.scan.energies[done..].iter().all(Option::is_none));
        assert!((0..done).all(|i| run.structure(i).is_some()));

        // Within a point: a raised flag stops the minimiser before its budget is spent.
        let source = Source::model(&model, None, vec![axis.param.clone()]);
        let reference = source.reference().unwrap();
        let mut evaluator = Evaluator::new(&reference.molfile, ForceField::Mmff94).unwrap();
        let cancel = AtomicBool::new(true);
        let relaxed = evaluator.relax(&reference.coords, &[], OPTIMIZE_STEPS, &cancel);
        assert_eq!(relaxed, Relaxation::Cancelled);
    }

    /// What OpenBabel cannot hold, or a relaxed scan cannot do, is refused before starting.
    #[test]
    fn relaxed_scans_it_cannot_do_are_refused_at_once() {
        let model = butane(ForceField::Uff);
        // OpenBabel drops torsion restraints next to an angle restraint.
        let axes = [
            Axis {
                param: Param::Angle([0, 1, 2]),
                start: 100.0,
                end: 120.0,
                points: 3,
            },
            butane_dihedral_axis(3),
        ];
        let params: Vec<Param> = axes.iter().map(|a| a.param.clone()).collect();
        let source = Source::model(&model, None, params);
        let Err(message) = Run::start_relaxed(source, &axes, ForceField::Uff, Relax::new(vec![]))
        else {
            panic!("an angle and a dihedral were relaxed together");
        };
        assert!(message.contains("angle"), "{message}");

        let design = Design {
            components: vec![benzene()],
            steps: vec![Step::new_stack()],
            ..Default::default()
        };
        let rise = Param::Step {
            step: 0,
            field: StepField::StackRise,
        };
        let axis = Axis {
            param: rise.clone(),
            start: 3.0,
            end: 4.0,
            points: 3,
        };
        let source = Source::assembly(&design, vec![rise]);
        assert!(
            Run::start_relaxed(source, &[axis], ForceField::Uff, Relax::new(vec![])).is_err(),
            "an assembly is not relaxed"
        );

        let mut big = model.clone();
        let atom = big.atoms[0].clone();
        big.atoms.resize(crate::scan::MAX_RELAX_ATOMS + 1, atom);
        let source = Source::model(&big, None, vec![Param::Dihedral([0, 1, 2, 3])]);
        let axis = butane_dihedral_axis(3);
        assert!(Run::start_relaxed(source, &[axis], ForceField::Uff, Relax::new(vec![])).is_err());
    }

    /// Per-point cost of replacing the coordinates against re-reading the molfile, for each
    /// force field (`cargo test --release -- --ignored --nocapture scan_timing`).
    #[test]
    #[ignore = "timing report"]
    fn scan_timing() {
        let mut stack = Step::new_stack();
        if let Step::Stack(s) = &mut stack {
            s.n = 40;
        }
        let design = Design {
            components: vec![benzene()],
            steps: vec![stack],
            ..Default::default()
        };
        let rise = Param::Step {
            step: 0,
            field: StepField::StackRise,
        };
        let source = Source::assembly(&design, vec![rise]);
        let reference = source.reference().unwrap();
        for ff in ForceField::ALL {
            let t = Instant::now();
            let mut ev = energy::Evaluator::new(&reference.molfile, ff).unwrap();
            let setup = t.elapsed();
            let t = Instant::now();
            for k in 0..10 {
                let Ok(Point::Coords(c)) = source.at(&[3.4 + 0.05 * k as f64]) else {
                    panic!()
                };
                assert!(ev.energy(&c).is_finite());
            }
            let in_place = t.elapsed() / 10;
            let t = Instant::now();
            for k in 0..10 {
                let Ok(Point::Coords(c)) = source.at(&[3.4 + 0.05 * k as f64]) else {
                    panic!()
                };
                let text = crate::three_d::edit::molfile_with_coords(&reference.molfile, &c);
                let mut fresh = energy::Evaluator::new(&text.unwrap(), ff).unwrap();
                assert!(fresh.energy(&c).is_finite());
            }
            let reparse = t.elapsed() / 10;
            eprintln!(
                "{ff} ({} atoms): setup {setup:?}, per point in place {in_place:?}, re-parsed {reparse:?}",
                reference.atoms
            );
        }
    }
}
