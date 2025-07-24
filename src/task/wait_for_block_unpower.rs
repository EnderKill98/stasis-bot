use crate::BotState;
use crate::task::{Task, TaskOutcome};
use azalea::blocks::properties::Powered;
use azalea::{BlockPos, Client, Event};
use std::fmt::{Display, Formatter};
use std::time::Duration;
use tokio::time::Instant;

pub struct WaitForBlockUnpowerTask {
    pub block_pos: BlockPos,

    started_at: Instant,
}

impl WaitForBlockUnpowerTask {
    pub fn new(block_pos: BlockPos) -> Self {
        Self {
            block_pos,

            // Doesn't matter:
            started_at: Instant::now(),
        }
    }
}

impl Display for WaitForBlockUnpowerTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "WaitForBlockUnpower")
    }
}

impl Task for WaitForBlockUnpowerTask {
    fn start(&mut self, _bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        self.started_at = Instant::now();
        Ok(())
    }

    fn handle(&mut self, bot: Client, _bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        if let Event::Tick = event {
            if let Some(state) = bot.world().read().get_block_state(&self.block_pos)
                && !state.property::<Powered>().unwrap_or(true)
            {
                return Ok(TaskOutcome::Succeeded);
            } else if self.started_at.elapsed() >= Duration::from_secs(5) {
                return Ok(TaskOutcome::Failed {
                    reason: "Block not result unpower 5s after starting WaitForBlockUnpowerTask!".to_owned(),
                });
            }
        }
        Ok(TaskOutcome::Ongoing)
    }
}
