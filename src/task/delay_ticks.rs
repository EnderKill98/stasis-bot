use crate::BotState;
use crate::task::{Task, TaskOutcome};
use azalea::{Client, Event};
use std::fmt::{Display, Formatter};

pub struct DelayTicksTask {
    pub ticks: u32,
    pub elapsed: u32,
}

impl DelayTicksTask {
    pub fn new(ticks: u32) -> Self {
        Self {
            ticks,
            elapsed: 0, /*Doesn't matter*/
        }
    }
}

impl Display for DelayTicksTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DelayTicks ({})", self.ticks)
    }
}

impl Task for DelayTicksTask {
    fn start(&mut self, _bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        self.elapsed = 0;
        Ok(())
    }

    fn handle(&mut self, _bot: Client, _bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        if let Event::Tick = event {
            self.elapsed += 1;
            if self.elapsed >= self.ticks {
                return Ok(TaskOutcome::Succeeded);
            }
        }
        Ok(TaskOutcome::Ongoing)
    }
}
