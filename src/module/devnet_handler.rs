use crate::module::Module;
use crate::{BotState, DEVNET_RX_QUEUE, commands};
use azalea::{Client, Event};

#[derive(Clone, Default)]
pub struct DevNetIntegrationModule {}

#[async_trait::async_trait]
impl Module for DevNetIntegrationModule {
    fn name(&self) -> &'static str {
        "DevNetIntegration"
    }

    async fn handle(
        &self,
        mut bot: Client,
        event: &Event,
        bot_state: &BotState,
    ) -> anyhow::Result<()> {
        match event {
            Event::Tick => loop {
                let next_devnet_message = DEVNET_RX_QUEUE.lock().pop_front();
                if let Some(message) = next_devnet_message {
                    commands::handle_devnet_message(&mut bot, &bot_state, message).await?;
                } else {
                    break;
                }
            },
            _ => {}
        }
        Ok(())
    }
}
