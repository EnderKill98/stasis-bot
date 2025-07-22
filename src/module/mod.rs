use crate::BotState;
use azalea::{Client, Event};

pub mod autoeat;
pub mod beertender;
pub mod devnet_handler;
pub mod emergency_quit;
pub mod look_at_players;
pub mod periodic_swing;
pub mod server_tps;
pub mod soundness;
pub mod stasis;
pub mod visual_range;

#[async_trait::async_trait]
pub trait Module {
    fn name(&self) -> &'static str;

    fn status(&self) -> Option<String> {
        None
    }

    async fn handle(&self, bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()>;
}
