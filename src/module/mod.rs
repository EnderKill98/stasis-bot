use crate::BotState;
use azalea::{Client, Event};

pub mod autoeat;
pub mod chat;
pub mod devnet_handler;
pub mod disc_jockey;
pub mod emergency_quit;
pub mod legacy_stasis;
pub mod look_at_players;
pub mod periodic_swing;
pub mod server_tps;
pub mod shulker_counter;
pub mod soundness;
pub mod stasis;
pub mod visual_range;
pub mod webhook;

#[async_trait::async_trait]
pub trait Module {
    fn name(&self) -> &'static str;

    fn status(&self) -> Option<String> {
        None
    }

    async fn handle(&self, bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()>;
}
