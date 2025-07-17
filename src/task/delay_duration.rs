use crate::BotState;
use crate::task::{Task, TaskOutcome};
use azalea::{Client, Event};
use std::fmt::{Display, Formatter};
use std::time::{Duration, Instant};

pub struct DelayDurationTask {
    pub duration: Duration,
    pub started_at: Instant,
}

impl DelayDurationTask {
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            started_at: Instant::now(),
        }
    }
}

impl Display for DelayDurationTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut total_millis = self.duration.as_millis();
        let mut mins = 0;
        if total_millis >= 1000 * 60 {
            mins = total_millis / (1000 * 60);
            total_millis %= 1000 * 60;
        }
        let mut secs = 0;
        if total_millis >= 1000 {
            secs = total_millis / 1000;
            total_millis %= 1000;
        }
        let mut components = Vec::<String>::new();
        if mins > 0 {
            components.push(format!("{mins:02}m"))
        }
        if secs > 0 {
            components.push(format!("{secs:02}s"))
        }
        if total_millis > 0 {
            components.push(format!("{total_millis:03}ms"))
        }

        write!(f, "DelayDuration ({})", components.join(" "))
    }
}

impl Task for DelayDurationTask {
    fn start(&mut self, _bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        self.started_at = Instant::now();
        Ok(())
    }

    fn handle(&mut self, _bot: Client, _bot_state: &BotState, _event: &Event) -> anyhow::Result<TaskOutcome> {
        if self.started_at.elapsed() >= self.duration {
            Ok(TaskOutcome::Succeeded)
        } else {
            Ok(TaskOutcome::Ongoing)
        }
    }
}
