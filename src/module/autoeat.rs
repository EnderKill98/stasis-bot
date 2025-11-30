use crate::BotState;
use crate::module::Module;
use crate::task::eat::EatTask;
use crate::task::tracked::TrackedTask;
use anyhow::Context;
use azalea::{Client, Event};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct AutoEatModule {
    pub last_eat_attempt_at: Arc<Mutex<Instant>>,
    pub eating_task: Arc<Mutex<Option<TrackedTask<EatTask>>>>,
}

impl Default for AutoEatModule {
    fn default() -> Self {
        Self {
            last_eat_attempt_at: Arc::new(Mutex::new(Instant::now())),
            eating_task: Default::default(),
        }
    }
}

impl AutoEatModule {
    pub fn has_eat_task(&self) -> bool {
        /*if let Some(eat_task) = self.eating_task.lock().as_ref() {
            debug!("Eat Task: {:?}", eat_task.status());
        } else {
            debug!("Eat Task: None!!");
        }*/
        self.eating_task.lock().as_ref().map(|t| !t.status().is_finished()).unwrap_or(false)
    }
}

#[async_trait::async_trait]
impl Module for AutoEatModule {
    fn name(&self) -> &'static str {
        "AutoEat"
    }

    async fn handle(&self, mut bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Tick => {
                if EatTask::should_eat(&mut bot) && self.last_eat_attempt_at.lock().elapsed() > Duration::from_millis(500) {
                    *self.last_eat_attempt_at.lock() = Instant::now();
                    if EatTask::find_food_in_hotbar(&mut bot).is_some() && !self.has_eat_task() {
                        info!("Should and can eat. Adding EatTask to run now!");
                        let task = TrackedTask::new(EatTask::default());
                        *self.eating_task.lock() = Some(task.clone());
                        bot_state.add_task_now(bot, bot_state, task).context("Add EatTask now")?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
