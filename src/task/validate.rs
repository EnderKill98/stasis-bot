use crate::BotState;
use crate::task::{Task, TaskOutcome};
use anyhow::Context;
use azalea::{Client, Event};
use std::fmt::{Display, Formatter};

pub struct ValidateTask<F> {
    expectation: String,
    once_func: Option<Box<F>>,
}

impl<F> ValidateTask<F>
where
    F: (FnOnce(Client, BotState) -> anyhow::Result<bool>) + Send + Sync,
{
    pub fn new(name: impl AsRef<str>, once_func: F) -> Self {
        Self {
            expectation: name.as_ref().to_string(),
            once_func: Some(Box::new(once_func)),
        }
    }
}

impl<F> Display for ValidateTask<F>
where
    F: (FnOnce(Client, BotState) -> anyhow::Result<bool>) + Send + Sync,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Validate: {}", self.expectation)
    }
}

impl<F> Task for ValidateTask<F>
where
    F: (FnOnce(Client, BotState) -> anyhow::Result<bool>) + Send + Sync,
{
    fn handle(&mut self, bot: Client, bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        if let Event::Tick = event
            && let Some(once_func) = self.once_func.take()
        {
            let self_tostring = self.to_string();
            if once_func(bot, bot_state.to_owned()).with_context(|| format!("Running func of ({self_tostring})"))? {
                Ok(TaskOutcome::Succeeded)
            } else {
                Ok(TaskOutcome::Failed {
                    reason: format!("Failed to validate expectation: {}", self.expectation),
                })
            }
        } else if self.once_func.is_none() {
            Ok(TaskOutcome::Succeeded)
        } else {
            // Wait for tick event
            Ok(TaskOutcome::Ongoing)
        }
    }
}
