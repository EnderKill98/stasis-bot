use crate::BotState;
use crate::task::{Task, TaskOutcome};
use anyhow::Context;
use azalea::{Client, Event};
use std::fmt::{Display, Formatter};

pub struct OnceFuncTask<F> {
    display: String,
    once_func: Option<Box<F>>,
}

impl<F> OnceFuncTask<F>
where
    F: (FnOnce(Client, BotState) -> anyhow::Result<()>) + Send + Sync,
{
    pub fn new(name: impl AsRef<str>, once_func: F) -> Self {
        Self {
            display: name.as_ref().to_string(),
            once_func: Some(Box::new(once_func)),
        }
    }
}

impl<F> Display for OnceFuncTask<F>
where
    F: (FnOnce(Client, BotState) -> anyhow::Result<()>) + Send + Sync,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display)
    }
}

impl<F> Task for OnceFuncTask<F>
where
    F: (FnOnce(Client, BotState) -> anyhow::Result<()>) + Send + Sync,
{
    fn handle(&mut self, bot: Client, bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        //trace!("OnceFunc: {}", if self.once_func.is_some() { "Some" } else { "None" });
        if let Event::Tick = event
            && let Some(once_func) = self.once_func.take()
        {
            let display = self.display.to_owned();
            if let Err(err) = once_func(bot, bot_state.to_owned()).with_context(|| format!("Running OnceFunc ({display})")) {
                Ok(TaskOutcome::Failed {
                    reason: format!("OnceFunc error: {err}"),
                })
            } else {
                Ok(TaskOutcome::Succeeded)
            }
        } else if self.once_func.is_none() {
            Ok(TaskOutcome::Succeeded)
        } else {
            // Wait for tick event
            Ok(TaskOutcome::Ongoing)
        }
    }
}
