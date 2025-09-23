use crate::BotState;
use crate::task::{Task, TaskOutcome};
use azalea::{Client, Event};
use parking_lot::lock_api::MutexGuard;
use parking_lot::{MappedMutexGuard, Mutex};
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct TrackedAnyhowError {
    error: Arc<anyhow::Error>,
}

impl Error for TrackedAnyhowError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.error.source()
    }
    #[allow(deprecated)]
    fn description(&self) -> &str {
        self.error.description()
    }
    /*fn provide<'a>(&'a self, request: &mut Request<'a>) {
        self.error.provide(request)
    }*/
    #[allow(deprecated)]
    fn cause(&self) -> Option<&dyn Error> {
        self.error.cause()
    }
}

impl Display for TrackedAnyhowError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)
    }
}

#[derive(Debug, Clone)]
pub enum TrackedTaskStatus {
    NotStartedYet,
    Running,
    Concluded { outcome: TaskOutcome },
    Errored { error: TrackedAnyhowError },
    Discarded,
    Interrupted,
}

impl TrackedTaskStatus {
    pub fn outcome(&self) -> Option<&TaskOutcome> {
        if let Self::Concluded { outcome } = self { Some(outcome) } else { None }
    }

    pub fn error(&self) -> Option<&TrackedAnyhowError> {
        if let Self::Errored { error } = self { Some(error) } else { None }
    }

    pub fn is_interrupted(&self) -> bool {
        matches!(self, Self::Interrupted)
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::NotStartedYet | Self::Running)
    }

    pub fn is_not_started_yet(&self) -> bool {
        matches!(self, Self::Interrupted | Self::NotStartedYet)
    }

    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Concluded { .. } | Self::Errored { .. } | Self::Discarded)
    }

    pub fn is_abandoned(&self) -> bool {
        matches!(self, Self::Discarded)
    }
}

pub struct TrackedTask<T: Task> {
    data: Arc<Mutex<(T, TrackedTaskStatus)>>,
}

impl<T: Task> TrackedTask<T> {
    pub fn new(task: T) -> Self {
        Self {
            data: Arc::new(Mutex::new((task, TrackedTaskStatus::NotStartedYet))),
        }
    }

    pub fn status(&self) -> TrackedTaskStatus {
        self.data.lock().1.clone()
    }

    pub fn status_mut(&self) -> MappedMutexGuard<'_, TrackedTaskStatus> {
        MutexGuard::map(self.data.lock(), |t| &mut t.1)
    }

    pub fn task_mut(&self) -> MappedMutexGuard<'_, T> {
        MutexGuard::map(self.data.lock(), |t| &mut t.0)
    }
}

impl<T: Task> Display for TrackedTask<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "T: {}", self.task_mut())
    }
}

impl<T: Task> Clone for TrackedTask<T> {
    fn clone(&self) -> Self {
        TrackedTask { data: self.data.clone() }
    }
}

impl<T: Task> Task for TrackedTask<T> {
    fn start(&mut self, bot: Client, bot_state: &BotState) -> anyhow::Result<()> {
        let (task, status) = &mut *self.data.lock();
        match task.start(bot, bot_state) {
            Ok(_) => {
                *status = TrackedTaskStatus::Running;
                Ok(())
            }
            Err(error) => {
                let tracked_error = TrackedAnyhowError { error: Arc::new(error) };
                *status = TrackedTaskStatus::Errored { error: tracked_error.clone() };
                Err(tracked_error.into())
            }
        }
    }

    fn handle(&mut self, bot: Client, bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        let (task, status) = &mut *self.data.lock();
        match task.handle(bot, bot_state, event) {
            Ok(outcome) => {
                if !matches!(outcome, TaskOutcome::Ongoing) {
                    *status = TrackedTaskStatus::Concluded { outcome: outcome.clone() };
                }
                Ok(outcome)
            }
            Err(error) => {
                let tracked_error = TrackedAnyhowError { error: Arc::new(error) };
                *status = TrackedTaskStatus::Errored { error: tracked_error.clone() };
                Err(tracked_error.into())
            }
        }
    }

    fn stop(&mut self, bot: Client, bot_state: &BotState) -> anyhow::Result<()> {
        let (task, status) = &mut *self.data.lock();
        match task.stop(bot, bot_state) {
            Ok(_) => {
                let allow_interrupt_change = match status {
                    TrackedTaskStatus::Concluded { outcome } => match outcome {
                        TaskOutcome::Failed { .. } | TaskOutcome::Succeeded => false,
                        _ => true,
                    },
                    _ => true,
                };
                if allow_interrupt_change {
                    *status = TrackedTaskStatus::Interrupted;
                }
                Ok(())
            }
            Err(error) => {
                let tracked_error = TrackedAnyhowError { error: Arc::new(error) };
                *status = TrackedTaskStatus::Errored { error: tracked_error.clone() };
                Err(tracked_error.into())
            }
        }
    }

    fn discard(&mut self, bot: Client, bot_state: &BotState) -> anyhow::Result<()> {
        let (task, status) = &mut *self.data.lock();
        match task.discard(bot, bot_state) {
            Ok(_) => {
                if !matches!(status, TrackedTaskStatus::Concluded { .. } | TrackedTaskStatus::Interrupted) {
                    *status = TrackedTaskStatus::Discarded;
                }
                Ok(())
            }
            Err(error) => {
                let tracked_error = TrackedAnyhowError { error: Arc::new(error) };
                *status = TrackedTaskStatus::Errored { error: tracked_error.clone() };
                Err(tracked_error.into())
            }
        }
    }

    fn new_task_waiting(&mut self, bot: Client, bot_state: &BotState) -> anyhow::Result<()> {
        let (task, status) = &mut *self.data.lock();
        match task.new_task_waiting(bot, bot_state) {
            Ok(_) => Ok(()),
            Err(error) => {
                let tracked_error = TrackedAnyhowError { error: Arc::new(error) };
                *status = TrackedTaskStatus::Errored { error: tracked_error.clone() };
                Err(tracked_error.into())
            }
        }
    }
}
