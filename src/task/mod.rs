pub mod delay_duration;
pub mod delay_ticks;
pub mod eat;
pub mod func;
pub mod group;
pub mod oncefunc;
pub mod pathfind;
pub mod sip_beer;
pub mod swing_beer;
pub mod validate;

use crate::BotState;
use azalea::{Client, Event};
use std::fmt::Display;

#[derive(Debug)]
pub enum TaskOutcome {
    Ongoing,
    Succeeded,
    Failed { reason: String },
}

impl TaskOutcome {
    pub fn finished(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed { .. })
    }
}

pub trait Task: Display + Send + Sync {
    #[allow(unused_variables)]
    fn start(&mut self, bot: Client, bot_state: &BotState) -> anyhow::Result<()> {
        Ok(())
    }

    fn handle(&mut self, bot: Client, bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome>;

    #[allow(unused_variables)]
    fn stop(&mut self, bot: Client, bot_state: &BotState) -> anyhow::Result<()> {
        Ok(())
    }
}

impl<T: Task + 'static> From<T> for Box<dyn Task> {
    fn from(task: T) -> Self {
        Box::new(task)
    }
}
