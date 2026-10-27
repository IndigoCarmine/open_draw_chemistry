//! A document's 3D model and the background work that produces it.
//!
//! Generating or minimising takes from a fraction of a second to many seconds, so it runs on
//! its own thread and the UI polls for the result each frame ([`Job::poll`]). Work only starts
//! when asked for: the model is not rebuilt as the drawing changes, it is marked stale
//! ([`Job::is_stale`]) until the user regenerates it.
//!
//! The model can also be edited by hand ([`Job::edit`], see [`super::edit`]); after an edit the
//! energy of the new geometry is recomputed in the background too.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use chembuider_rs::{Config, Molecule};

use super::{Edit3d, Error, ForceField, Model3d};

/// What a [`Job`] is busy with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    /// Building a model from the drawing.
    Generate,
    /// Minimising the current model further.
    Optimize,
}

impl Task {
    /// The task in progress, for the UI.
    pub fn running_label(self) -> &'static str {
        match self {
            Task::Generate => "3D モデルを生成しています…",
            Task::Optimize => "構造を最適化しています…",
        }
    }
}

#[derive(Debug, Default)]
pub enum JobState {
    /// Nothing generated yet.
    #[default]
    Idle,
    /// Work in progress on a background thread.
    Running(Task),
    Done(Model3d),
    /// The last attempt failed, with a message for the user.
    Failed(String),
}

/// A document's 3D state: the model (or the work towards one) and the force field chosen.
#[derive(Debug, Default)]
pub struct Job {
    pub state: JobState,
    /// The force field the next generation or optimisation uses.
    pub force_field: ForceField,
    /// Hand edits of the model: selection, restraints and the undo history.
    pub edit: Edit3d,
    /// The drawing the model was generated from, to tell when it has since changed.
    source: Option<Molecule>,
    /// Where the running work's result arrives.
    result: Option<Receiver<Result<Model3d, Error>>>,
    /// The model being optimised, still shown (and restored if the optimisation fails).
    previous: Option<Model3d>,
    /// The energy being computed for an edited geometry, with the edit revision it is for.
    energy: Option<(u64, Receiver<Result<f64, Error>>)>,
}

impl Job {
    /// A job already holding `model` (read from a file rather than generated) as the model of
    /// `drawing`, so it is not stale until the drawing is edited.
    pub fn with_model(model: Model3d, drawing: &Molecule) -> Self {
        Self {
            force_field: model.force_field,
            state: JobState::Done(model),
            source: Some(drawing.clone()),
            result: None,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, JobState::Running(_))
    }

    pub fn model(&self) -> Option<&Model3d> {
        match &self.state {
            JobState::Done(model) => Some(model),
            _ => None,
        }
    }

    pub fn model_mut(&mut self) -> Option<&mut Model3d> {
        match &mut self.state {
            JobState::Done(model) => Some(model),
            _ => None,
        }
    }

    /// The model to show: the finished one, or the one being optimised.
    pub fn shown_model(&self) -> Option<&Model3d> {
        self.model().or(self.previous.as_ref())
    }

    /// The model and its edit state together, for applying an edit.
    pub fn model_and_edit(&mut self) -> Option<(&mut Model3d, &mut Edit3d)> {
        match &mut self.state {
            JobState::Done(model) => Some((model, &mut self.edit)),
            _ => None,
        }
    }

    /// Whether there is a model and the drawing has changed since it was generated.
    pub fn is_stale(&self, drawing: &Molecule) -> bool {
        self.model().is_some() && self.source.as_ref() != Some(drawing)
    }

    /// Start generating a model of `drawing` in the background, replacing the current one.
    /// Conversion problems (unknown labels, an empty drawing) fail at once, without a thread.
    pub fn generate(&mut self, drawing: &Molecule, config: &Config) {
        if self.is_running() {
            return;
        }
        if !super::AVAILABLE {
            self.state = JobState::Failed(Error::Unavailable.to_string());
            return;
        }
        self.source = Some(drawing.clone());
        let molfile = match super::molfile::to_molfile(drawing, config) {
            Ok(molfile) => molfile,
            Err(e) => {
                self.state = JobState::Failed(e.to_string());
                return;
            }
        };
        let force_field = self.force_field;
        self.previous = None;
        self.spawn(Task::Generate, move || {
            super::generate_from_molfile(&molfile, force_field)
        });
    }

    /// Start minimising the current model further with the chosen force field, holding the
    /// edit's restraints when they are switched on. Does nothing without a model.
    pub fn optimize(&mut self) {
        if self.is_running() {
            return;
        }
        let Some(model) = self.model().cloned() else {
            return;
        };
        let force_field = self.force_field;
        let constraints = self.edit.active_constraints(model.atoms.len());
        self.previous = Some(model.clone());
        self.spawn(Task::Optimize, move || {
            super::optimize_with(&model, force_field, &constraints)
        });
    }

    fn spawn(
        &mut self,
        task: Task,
        work: impl FnOnce() -> Result<Model3d, Error> + Send + 'static,
    ) {
        let (send, receive) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("3d-model".to_owned())
            // A closed document drops the receiver; the result then has nowhere to go.
            .spawn(move || send.send(work()).ok());
        match spawned {
            Ok(_) => {
                self.state = JobState::Running(task);
                self.result = Some(receive);
            }
            Err(e) => {
                self.state = match self.previous.take() {
                    Some(model) => JobState::Done(model),
                    None => JobState::Failed(format!("処理を開始できませんでした: {e}")),
                };
            }
        }
    }

    /// Whether the energy of an edited geometry is still being worked out.
    pub fn energy_pending(&self) -> bool {
        self.edit.energy_stale && self.model().is_some() && super::AVAILABLE
    }

    /// Take the background work's result if it has arrived, returning a message for the status
    /// bar when it has. Also keeps the energy of an edited model up to date.
    pub fn poll(&mut self) -> Option<String> {
        self.poll_energy();
        let JobState::Running(task) = self.state else {
            return None;
        };
        let received = self.result.as_ref()?.try_recv();
        let outcome = match received {
            Err(TryRecvError::Empty) => return None,
            Ok(outcome) => outcome,
            // The thread ended without sending: it panicked.
            Err(TryRecvError::Disconnected) => Err(Error::GenerationFailed),
        };
        self.result = None;
        let previous = self.previous.take();
        let message = match (&outcome, task) {
            (Ok(model), Task::Generate) => format!(
                "3D モデルを生成しました（{}、{} 原子）",
                model.formula,
                model.atoms.len()
            ),
            (Ok(model), Task::Optimize) => format!(
                "構造を最適化しました（E = {:.1} {}）",
                model.energy, model.energy_unit
            ),
            (Err(e), _) => e.to_string(),
        };
        self.state = match (outcome, task, previous) {
            (Ok(model), Task::Optimize, Some(before)) => {
                self.edit.record_replacement(&before, &model, "構造最適化");
                JobState::Done(model)
            }
            (Ok(model), _, _) => {
                self.edit.reset();
                JobState::Done(model)
            }
            // A failed optimisation leaves the model as it was.
            (Err(_), Task::Optimize, Some(before)) => JobState::Done(before),
            (Err(e), _, _) => JobState::Failed(e.to_string()),
        };
        Some(message)
    }

    /// Collect a finished energy evaluation, and start one for an edited geometry that has
    /// none yet. One runs at a time; a result for a geometry since edited again is dropped and
    /// the next poll asks again.
    fn poll_energy(&mut self) {
        if let Some((revision, receiver)) = &self.energy {
            match receiver.try_recv() {
                Err(TryRecvError::Empty) => return,
                Ok(result) => {
                    let revision = *revision;
                    if let (Ok(energy), JobState::Done(model)) = (result, &mut self.state) {
                        self.edit.energy_updated(model, revision, energy);
                    }
                    self.energy = None;
                }
                Err(TryRecvError::Disconnected) => self.energy = None,
            }
        }
        if !self.energy_pending() || self.energy.is_some() {
            return;
        }
        let Some(model) = self.model().cloned() else {
            return;
        };
        let revision = self.edit.revision();
        let force_field = model.force_field;
        let (send, receive) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("3d-energy".to_owned())
            .spawn(move || send.send(super::energy(&model, force_field)).ok());
        if spawned.is_ok() {
            self.energy = Some((revision, receive));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unconvertible_drawing_fails_without_a_thread() {
        let mut job = Job::default();
        job.generate(&Molecule::default(), &Config::default());
        let JobState::Failed(message) = &job.state else {
            panic!("expected a failure, got {:?}", job.state);
        };
        let expected = if crate::three_d::AVAILABLE {
            Error::Empty
        } else {
            Error::Unavailable
        };
        assert_eq!(message, &expected.to_string());
        assert!(job.poll().is_none());
    }

    #[test]
    fn nothing_is_stale_without_a_model() {
        let job = Job::default();
        assert!(!job.is_stale(&Molecule::default()));
    }

    #[cfg(feature = "openbabel")]
    fn finish(job: &mut Job) -> String {
        loop {
            if let Some(message) = job.poll() {
                return message;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[cfg(feature = "openbabel")]
    #[test]
    fn a_generated_model_goes_stale_when_the_drawing_changes() {
        let mut drawing = Molecule::default();
        let c = drawing.add_atom("C".to_owned(), [0.0, 0.0], 0);
        let o = drawing.add_atom("O".to_owned(), [1.5, 0.0], 0);
        drawing.add_bond(c, o, chembuider_rs::BondOrder::Single);

        let mut job = Job::default();
        job.generate(&drawing, &Config::default());
        assert!(job.is_running());
        let message = finish(&mut job);
        let model = job.model().unwrap_or_else(|| panic!("{message}"));
        assert_eq!(model.atoms.len(), 6, "CH3OH");
        assert!(!job.is_stale(&drawing));

        drawing.add_atom("N".to_owned(), [3.0, 0.0], 0);
        assert!(job.is_stale(&drawing));
    }

    /// Edit → the energy follows; optimise under a restraint → it holds, the model stays on
    /// screen meanwhile, and the optimisation can be undone.
    #[cfg(feature = "openbabel")]
    #[test]
    fn an_edited_model_is_re_evaluated_and_optimised_under_its_restraints() {
        use crate::three_d::{edit, geom};

        let mut drawing = Molecule::default();
        let mut prev = drawing.add_atom("C".to_owned(), [0.0, 0.0], 0);
        for i in 1..4 {
            let next = drawing.add_atom("C".to_owned(), [1.3 * i as f32, 0.0], 0);
            drawing.add_bond(prev, next, chembuider_rs::BondOrder::Single);
            prev = next;
        }
        let mut job = Job {
            force_field: ForceField::Uff,
            ..Default::default()
        };
        job.generate(&drawing, &Config::default());
        let message = finish(&mut job);
        assert!(job.model().is_some(), "{message}");

        // The butane carbons are the first four atoms, as drawn.
        let chain = [0, 1, 2, 3];
        let (model, edit3d) = job.model_and_edit().unwrap();
        let energy_before = model.energy;
        let g = edit::graph(model);
        let coords = geom::set_dihedral(&edit::coords(model), &g, chain, 60.0, None).unwrap();
        edit3d.apply(model, coords, "二面角");
        edit3d.constrain(&chain, 60.0);
        assert!(job.energy_pending());
        while job.energy_pending() {
            job.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let energy_edited = job.model().unwrap().energy;
        assert_ne!(energy_edited, energy_before, "the energy follows the edit");

        job.optimize();
        assert!(job.shown_model().is_some(), "still shown while optimising");
        let message = finish(&mut job);
        let model = job.model().unwrap_or_else(|| panic!("{message}"));
        let c = edit::coords(model);
        let torsion = geom::dihedral(c[0], c[1], c[2], c[3]);
        assert!(
            (torsion - 60.0).abs() < 3.0,
            "restrained torsion: {torsion}"
        );

        assert!(job.edit.can_undo());
        let (model, edit3d) = job.model_and_edit().unwrap();
        assert_eq!(edit3d.undo(model).as_deref(), Some("構造最適化"));
        assert_eq!(model.energy, energy_edited);
    }
}
