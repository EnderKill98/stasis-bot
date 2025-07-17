use crate::BotState;
use crate::task::{Task, TaskOutcome};
use azalea::{Client, Event};
use std::fmt::{Display, Formatter};

pub struct FuncTask<B, H, E> {
    display: String,
    start_func: Option<Box<B>>,
    handle_func: Box<H>,
    stop_func: Option<Box<E>>,
}

impl<B, H, E> FuncTask<B, H, E>
where
    B: (Fn(Client, BotState) -> anyhow::Result<()>) + Send + Sync,
    H: (Fn(Client, BotState, Event) -> anyhow::Result<TaskOutcome>) + Send + Sync,
    E: (Fn(Client, BotState) -> anyhow::Result<()>) + Send + Sync,
{
    pub fn new_handle(name: impl AsRef<str>, handle_func: H) -> Self {
        Self {
            display: name.as_ref().to_string(),
            start_func: None,
            handle_func: Box::new(handle_func),
            stop_func: None,
        }
    }

    pub fn new_all(name: impl AsRef<str>, start_func: B, handle_func: H, end_func: E) -> Self {
        Self {
            display: name.as_ref().to_string(),
            start_func: Some(Box::new(start_func)),
            handle_func: Box::new(handle_func),
            stop_func: Some(Box::new(end_func)),
        }
    }
}

impl<B, H, E> Display for FuncTask<B, H, E>
where
    B: (Fn(Client, BotState) -> anyhow::Result<()>) + Send + Sync,
    H: (Fn(Client, BotState, Event) -> anyhow::Result<TaskOutcome>) + Send + Sync,
    E: (Fn(Client, BotState) -> anyhow::Result<()>) + Send + Sync,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display)
    }
}

impl<B, H, E> Task for FuncTask<B, H, E>
where
    B: (Fn(Client, BotState) -> anyhow::Result<()>) + Send + Sync,
    H: (Fn(Client, BotState, Event) -> anyhow::Result<TaskOutcome>) + Send + Sync,
    E: (Fn(Client, BotState) -> anyhow::Result<()>) + Send + Sync,
{
    fn start(&mut self, bot: Client, bot_state: &BotState) -> anyhow::Result<()> {
        if let Some(start_func) = self.start_func.as_ref() {
            start_func(bot.clone(), bot_state.to_owned())?;
        }
        Ok(())
    }

    fn handle(&mut self, bot: Client, bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        self.handle_func.as_ref()(bot, bot_state.to_owned(), event.to_owned())
    }

    fn stop(&mut self, bot: Client, bot_state: &BotState) -> anyhow::Result<()> {
        if let Some(stop_func) = self.stop_func.as_ref() {
            stop_func(bot.clone(), bot_state.to_owned())?;
        }
        Ok(())
    }
}
