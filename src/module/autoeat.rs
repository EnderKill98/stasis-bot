use crate::BotState;
use crate::module::Module;
use crate::task::eat::EatTask;
use anyhow::Context;
use azalea::{Client, Event};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct AutoEatModule {
    pub last_eat_attempt_at: Arc<Mutex<Instant>>,
}

impl Default for AutoEatModule {
    fn default() -> Self {
        Self {
            last_eat_attempt_at: Arc::new(Mutex::new(Instant::now())),
        }
    }
}

impl AutoEatModule {
    fn has_eat_task(bot_state: &BotState) -> bool {
        for task in bot_state.root_task_group.lock().subtasks.iter() {
            if task.to_string().starts_with("Eat ") {
                return true;
            }
        }
        false
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
                    if EatTask::find_food_in_hotbar(&mut bot).is_some() && !Self::has_eat_task(bot_state) {
                        info!("Should and can eat. Adding EatTask to run now!");
                        bot_state.add_task_now(bot, bot_state, EatTask::default()).context("Add EatTask now")?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
