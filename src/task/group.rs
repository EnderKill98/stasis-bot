use crate::BotState;
use crate::task::{Task, TaskOutcome};
use anyhow::Context;
use azalea::{Client, Event};
use std::fmt::{Display, Formatter};

pub struct TaskGroup {
    pub name: Option<String>,
    pub subtasks: Vec<Box<dyn Task>>,
    subtask_index: usize,
    subtask_started: bool,
}

impl TaskGroup {
    pub fn new_root() -> Self {
        Self {
            name: None, // == Root
            subtasks: Vec::new(),
            subtask_index: 0,
            subtask_started: false,
        }
    }

    pub fn new(name: impl AsRef<str>) -> Self {
        Self {
            name: Some(name.as_ref().to_owned()),
            subtasks: Vec::new(),
            subtask_index: 0,
            subtask_started: false,
        }
    }

    pub fn name(&self) -> String {
        self.name.as_ref().map(|subtask| subtask.to_string()).unwrap_or_else(|| "<Root>".to_owned())
    }

    pub fn is_root(&self) -> bool {
        self.name.is_none()
    }

    pub fn with(mut self, subtask: impl Into<Box<dyn Task>>) -> Self {
        let subtask: Box<dyn Task> = subtask.into();
        debug!("TaskGroup \"{}\" with subtask: {subtask}", self.name());
        self.subtasks.push(subtask);
        self
    }

    pub fn add(&mut self, subtask: impl Into<Box<dyn Task>>) -> &mut Self {
        let subtask: Box<dyn Task> = subtask.into();
        debug!("TaskGroup \"{}\" add subtask: {subtask}", self.name());
        self.subtasks.push(subtask);
        self
    }

    pub fn add_now(&mut self, bot: Client, bot_state: &BotState, subtask: impl Into<Box<dyn Task>>) -> anyhow::Result<&mut Self> {
        let subtask: Box<dyn Task> = subtask.into();
        if self.subtask_index < self.subtasks.len() && self.subtask_started {
            let message = format!("Stopping current Task: {self}");
            warn!(message);
            self.subtasks.get_mut(self.subtask_index).unwrap().stop(bot, bot_state).context(message)?;
        }

        let subtask_tostring = subtask.to_string();
        self.subtasks.insert(self.subtask_index, subtask);
        self.subtask_started = false;
        if self.is_root() {
            info!("Added Task: {subtask_tostring} ({} remain)", self.remaining());
        }
        Ok(self)
    }

    pub fn remaining(&self) -> u64 {
        (self.subtasks.len() as i64 - self.subtask_index as i64).max(0) as u64
    }

    pub fn next_unchecked(&mut self) {
        if self.subtask_index >= self.subtasks.len() {
            return;
        }

        self.subtask_index += 1;
        self.subtask_started = false;
    }
}

impl Display for TaskGroup {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let subtask_name = self
            .subtasks
            .get(self.subtask_index)
            .map(|subtask| subtask.to_string())
            .unwrap_or_else(|| "<None>".to_owned());
        if let Some(name) = &self.name {
            if self.remaining() == 0 {
                write!(f, "{name}: Finished ({})", self.subtasks.len())
            } else {
                write!(
                    f,
                    "{name}: [{}/{}] {}{}",
                    self.subtask_index + 1,
                    self.subtasks.len(),
                    if self.subtask_started { "" } else { "..." },
                    subtask_name
                )
            }
        } else {
            // Root group
            let remaining = self.remaining();
            if remaining == 0 {
                write!(f, "Finished")
            } else {
                write!(f, "[..{remaining}] {subtask_name}")
            }
        }
    }
}

impl Task for TaskGroup {
    fn start(&mut self, _bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        self.subtask_started = false; // Possibly restart subtask
        // Delay start until handle on Tick event
        debug!("TaskGroup {} started.", self.name());
        Ok(())
    }

    fn handle(&mut self, bot: Client, bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        if let Event::Tick = event {
            // Cleanup if tasks pile up too much (e.g. in root node)
            if self.subtask_index >= 512 && self.subtasks.len() > 512 {
                // Remove 256 tasks
                warn!("Subtasks exceeded 512, removed first 256 tasks from list!");
                self.subtasks.drain(0..256);
                self.subtask_index -= 256;
            }
        }

        if self.subtask_index >= self.subtasks.len() {
            // Sanity check, can be triggered by self.next_unchecked()
            return Ok(TaskOutcome::Succeeded);
        }

        loop {
            trace!("TaskGroup {} Loop", self.name());

            let self_tostring = self.to_string();
            let self_name = self.name();
            let subtasks_len = self.subtasks.len();
            let is_root = self.is_root();
            let subtask = self.subtasks.get_mut(self.subtask_index);
            if subtask.is_none() {
                let reason = format!(
                    "TaskGroup \"{}\" failed, because expected to find a subtask at index {}, but only {} subtasks are present!",
                    self.name(),
                    self.subtask_index,
                    self.subtasks.len(),
                );
                return Ok(TaskOutcome::Failed { reason });
            }
            let subtask = subtask.unwrap();

            if !self.subtask_started {
                if let Event::Tick = &event {
                    if is_root {
                        info!("Starting Task: {subtask}");
                    } else {
                        info!("TaskGroup \"{}\" is starting subtask: {subtask}", self_name);
                    }
                    subtask.start(bot.clone(), bot_state).with_context(|| format!("Start {self_tostring}"))?;
                    self.subtask_started = true;
                } else {
                    // Wait until next tick to start next task
                    return Ok(TaskOutcome::Ongoing);
                }
            }

            match subtask.handle(bot.clone(), bot_state, event).with_context(|| format!("Handle {self_tostring}")) {
                Ok(TaskOutcome::Ongoing) => return Ok(TaskOutcome::Ongoing),
                Ok(TaskOutcome::Failed { reason }) => {
                    if is_root {
                        error!("{self} failed: {reason}");
                        info!("Trying next task...");
                        self.next_unchecked();
                        return Ok(TaskOutcome::Ongoing);
                    } else {
                        return Ok(TaskOutcome::Failed {
                            reason: format!("{self} failed: {reason}"),
                        });
                    }
                }
                Ok(TaskOutcome::Succeeded) => {
                    self.subtask_index += 1;
                    self.subtask_started = false;
                    if self.subtask_index == subtasks_len {
                        return Ok(TaskOutcome::Succeeded);
                    } else {
                        continue; // Init next task
                    }
                }
                Err(err) => {
                    let subtask_tostring = subtask.to_string();
                    error!("TaskGroup \"{}\" failed to handle subtask {subtask_tostring}", self.name());
                    self.stop(bot.clone(), bot_state)?;
                    return Ok(TaskOutcome::Failed {
                        reason: format!("Error when handling subtask {subtask_tostring}: {err:?}"),
                    });
                }
            }
        }
    }

    fn stop(&mut self, bot: Client, bot_state: &BotState) -> anyhow::Result<()> {
        if self.subtask_index < self.subtasks.len() && self.subtask_started {
            let subtask = self.subtasks.get_mut(self.subtask_index).unwrap();
            let subtask_tostring = subtask.to_string();
            subtask.stop(bot, bot_state).with_context(|| format!("Stop {subtask_tostring}"))?;
            self.subtask_started = false;
            warn!("Stopped Task: {subtask_tostring}");
            Ok(())
        } else {
            info!("Nothing to stop for TaskGroup {self}");
            Ok(())
        }
    }
}
