use crate::BotState;
use crate::task::delay_ticks::DelayTicksTask;
use crate::task::group::TaskGroup;
use crate::task::{Task, TaskOutcome};
use anyhow::anyhow;
use azalea::core::direction::Direction;
use azalea::entity::LookDirection;
use azalea::inventory::{Inventory, ItemStackData, SetSelectedHotbarSlotEvent};
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::s_interact::InteractionHand;
use azalea::protocol::packets::game::s_player_action::Action;
use azalea::protocol::packets::game::{ServerboundGamePacket, ServerboundPlayerAction, ServerboundUseItem};
use azalea::registry::Item;
use azalea::{BlockPos, Client, Event};
use rand::Rng;
use std::fmt::{Display, Formatter};
use std::time::Instant;

pub struct SipBeerTask {
    // Config
    look_away: bool,
    ticks: u32,
    pitch: f32,

    // State
    started_at: Instant,
    orig_held_slot: u8,
    orig_look_direction: LookDirection,
    repeats: u32,
}

impl Default for SipBeerTask {
    fn default() -> Self {
        Self::new_random(0)
    }
}

impl SipBeerTask {
    fn new(look_away: bool, pitch: f32, ticks: u32, repeats: u32) -> Self {
        Self {
            look_away,
            pitch,
            ticks,

            // Doesn't matter, yet
            started_at: Instant::now(),
            orig_held_slot: 0,
            orig_look_direction: LookDirection::new(0.0, 0.0),
            repeats,
        }
    }

    fn new_random(repeats: u32) -> Self {
        let look_away = rand::rng().random_range(0..10) >= 8;
        let pitch = if look_away { -65.0 } else { -20.0 };
        let ticks = rand::rng().random_range(15..32);
        Self::new(look_away, pitch, ticks, repeats)
    }

    pub fn find_beer_in_hotbar(bot: &mut Client) -> Option<(u8, ItemStackData)> {
        // Find first food item in hotbar
        let inv = bot.entity_component::<Inventory>(bot.entity);
        let inv_menu = inv.inventory_menu;
        for (hotbar_slot, slot) in inv_menu.hotbar_slots_range().enumerate() {
            if let Some(item_stack_data) = inv_menu.slot(slot).and_then(|stack| stack.as_present()) {
                if Item::HoneyBottle == item_stack_data.kind {
                    return Some((hotbar_slot as u8, item_stack_data.to_owned()));
                }
            }
        }

        None
    }
}

impl Display for SipBeerTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SipBeer ({} ticks{}{})",
            self.ticks,
            if self.look_away { ", look away" } else { "" },
            if self.repeats > 0 {
                format!(", repeats: {}", self.repeats)
            } else {
                "".to_owned()
            }
        )
    }
}

impl Task for SipBeerTask {
    fn start(&mut self, mut bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        self.started_at = Instant::now();
        self.orig_held_slot = bot.component::<Inventory>().selected_hotbar_slot;
        self.orig_look_direction = bot.component();
        let beer_in_hotbar = Self::find_beer_in_hotbar(&mut bot);
        if beer_in_hotbar.is_none() {
            warn!("No beer in hotbar to sip on!");
            return Ok(());
        }

        let beer_in_hotbar = beer_in_hotbar.unwrap();
        info!(
            "Sipping on beer in hotbar slot index {} ({}x {})",
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
            packet: ServerboundGamePacket::UseItem(ServerboundUseItem {
                hand: InteractionHand::MainHand,
                sequence: 0,
                yaw: self.orig_look_direction.y_rot,
                pitch: self.orig_look_direction.x_rot,
            }),
        });
        Ok(())
    }

    fn handle(&mut self, mut bot: Client, bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        if let Event::Tick = event {
            if Self::find_beer_in_hotbar(&mut bot).is_none() {
                return Ok(TaskOutcome::Failed {
                    reason: "No beer in hotbar to sip on".to_string(),
                });
            }

            // Drinking head animation
            let yaw = if self.look_away {
                (self.orig_look_direction.y_rot + 180.0) % 360.0
            } else {
                self.orig_look_direction.y_rot
            };
            let pitch = (self.pitch - rand::rng().random_range(0..25) as f32).max(-90.0);
            *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = LookDirection::new(yaw, pitch);

            if (self.started_at.elapsed().as_millis() / 50) as u32 >= self.ticks {
                self.stop(bot, bot_state)?;
                // Random chance
                let rand_float = rand::rng().random_range(0.0..1.0);
                let chance = 0.5 / (self.repeats as f64 + 1.0);
                if rand_float <= chance {
                    // Repeat sip in a bit again (chance exponentially decreases)
                    // Spawn as task to prevent deadlock
                    let (look_away, pitch, ticks, repeats) = (self.look_away, self.pitch, self.ticks, self.repeats + 1);
                    let bot_state = bot_state.clone();
                    tokio::spawn(async move {
                        bot_state.add_task(
                            TaskGroup::new("Repeat sipping (auto)")
                                .with(DelayTicksTask::new(4))
                                .with(SipBeerTask::new(look_away, pitch, ticks, repeats)),
                        );
                    });
                }
                Ok(TaskOutcome::Succeeded)
            } else {
                Ok(TaskOutcome::Ongoing)
            }
        } else {
            Ok(TaskOutcome::Ongoing)
        }
    }

    fn stop(&mut self, bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        let mut ecs = bot.ecs.lock();
        let entity = bot.entity;
        ecs.send_event(SendPacketEvent {
            sent_by: bot.entity,
            packet: ServerboundGamePacket::PlayerAction(ServerboundPlayerAction {
                action: Action::ReleaseUseItem,
                direction: Direction::Down,
                sequence: 0,
                pos: BlockPos::new(0, 0, 0),
            }),
        });
        ecs.send_event(SetSelectedHotbarSlotEvent {
            entity,
            slot: self.orig_held_slot,
        });
        *ecs.get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = self.orig_look_direction;
        Ok(())
    }
}
