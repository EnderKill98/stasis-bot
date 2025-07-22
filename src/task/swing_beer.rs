use crate::BotState;
use crate::task::sip_beer::SipBeerTask;
use crate::task::{Task, TaskOutcome};
use azalea::inventory::{Inventory, SetSelectedHotbarSlotEvent};
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::s_interact::InteractionHand;
use azalea::protocol::packets::game::{ServerboundGamePacket, ServerboundSwing};
use azalea::{Client, Event};
use std::fmt::{Display, Formatter};
use std::time::Instant;

pub struct SwingBeerTask {
    // State
    started_at: Instant,
    orig_held_slot: u8,
}

impl Default for SwingBeerTask {
    fn default() -> Self {
        Self {
            // Doesn't matter
            orig_held_slot: 0,
            started_at: Instant::now(),
        }
    }
}

impl Display for SwingBeerTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "SwingBeer")
    }
}

impl Task for SwingBeerTask {
    fn start(&mut self, mut bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        self.started_at = Instant::now();
        self.orig_held_slot = bot.component::<Inventory>().selected_hotbar_slot;
        let beer_in_hotbar = SipBeerTask::find_beer_in_hotbar(&mut bot);
        if beer_in_hotbar.is_none() {
            warn!("No beer in hotbar to swing with!");
            return Ok(());
        }

        let beer_in_hotbar = beer_in_hotbar.unwrap();
        info!(
            "Swinging with beer in hotbar slot index {} ({}x {})",
            beer_in_hotbar.0, beer_in_hotbar.1.count, beer_in_hotbar.1.kind
        );
        let mut ecs = bot.ecs.lock();
        let entity = bot.entity;
        ecs.send_event(SetSelectedHotbarSlotEvent {
            entity,
            slot: beer_in_hotbar.0,
        });
        ecs.send_event(SendPacketEvent {
            sent_by: entity,
            packet: ServerboundGamePacket::Swing(ServerboundSwing {
                hand: InteractionHand::MainHand,
            }),
        });
        Ok(())
    }

    fn handle(&mut self, bot: Client, bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        if let Event::Tick = event {
            if (self.started_at.elapsed().as_millis() / 50) as u32 >= 12 {
                self.stop(bot, bot_state)?;
                Ok(TaskOutcome::Succeeded)
            } else {
                Ok(TaskOutcome::Ongoing)
            }
        } else {
            Ok(TaskOutcome::Ongoing)
        }
    }

    fn stop(&mut self, bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        bot.ecs.lock().send_event(SetSelectedHotbarSlotEvent {
            entity: bot.entity,
            slot: self.orig_held_slot,
        });
        Ok(())
    }
}
