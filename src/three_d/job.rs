//! A document's 3D model and the background work that produces it.
//!
//! Generating or minimising takes from a fraction of a second to many seconds, so it runs on
//! its own thread and the UI polls for the result each frame ([`Job::poll`]). Work only starts
//! when asked for: the model is not rebuilt as the drawing changes, it is marked stale
//! ([`Job::is_stale`]) until the user regenerates it.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use chembuider_rs::{Config, Molecule};

use super::{Error, ForceField, Model3d};

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
    /// The drawing the model was generated from, to tell when it has since changed.
    source: Option<Molecule>,
    /// Where the running work's result arrives.
    result: Option<Receiver<Result<Model3d, Error>>>,
}

impl Job {
    pub fn is_running(&self) -> bool {
        matches!(self.state, JobState::Running(_))
    }

    pub fn model(&self) -> Option<&Model3d> {
        match &self.state {
            JobState::Done(model) => Some(model),
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
        self.spawn(Task::Generate, move || {
            super::generate_from_molfile(&molfile, force_field)
        });
    }

    /// Start minimising the current model further with the chosen force field. Does nothing
    /// without a model.
    pub fn optimize(&mut self) {
        if self.is_running() {
            return;
        }
        let Some(model) = self.model().cloned() else {
            return;
        };
        let force_field = self.force_field;
        self.spawn(Task::Optimize, move || super::optimize(&model, force_field));
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
            Err(e) => self.state = JobState::Failed(format!("処理を開始できませんでした: {e}")),
        }
    }

    /// Take the background work's result if it has arrived, returning a message for the status
    /// bar when it has.
    pub fn poll(&mut self) -> Option<String> {
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
        self.state = match outcome {
            Ok(model) => JobState::Done(model),
            Err(e) => JobState::Failed(e.to_string()),
        };
        Some(message)
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
    #[test]
    fn a_generated_model_goes_stale_when_the_drawing_changes() {
        let mut drawing = Molecule::default();
        let c = drawing.add_atom("C".to_owned(), [0.0, 0.0], 0);
        let o = drawing.add_atom("O".to_owned(), [1.5, 0.0], 0);
        drawing.add_bond(c, o, chembuider_rs::BondOrder::Single);

        let mut job = Job::default();
        job.generate(&drawing, &Config::default());
        assert!(job.is_running());
        let message = loop {
            if let Some(message) = job.poll() {
                break message;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let model = job.model().unwrap_or_else(|| panic!("{message}"));
        assert_eq!(model.atoms.len(), 6, "CH3OH");
        assert!(!job.is_stale(&drawing));

        drawing.add_atom("N".to_owned(), [3.0, 0.0], 0);
        assert!(job.is_stale(&drawing));
    }
}
