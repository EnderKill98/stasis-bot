use crate::module::Module;
use crate::{BotState, FOOD_ITEMS};
use azalea::entity::LookDirection;
use azalea::entity::metadata::{AbstractLivingUsingItem, Health};
use azalea::inventory::components::Food;
use azalea::inventory::{Inventory, ItemStackData, SetSelectedHotbarSlotEvent};
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::s_interact::InteractionHand;
use azalea::protocol::packets::game::{
    ServerboundGamePacket, ServerboundSetCarriedItem, ServerboundUseItem,
};
use azalea::{Client, Event, Hunger};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Copy, Clone)]
pub enum EatingProgress {
    NotEating,
    StartedEating,
    IsEating,
}

#[derive(Clone)]
pub struct AutoEatModule {
    pub eating_progress: Arc<Mutex<(Instant /* Last updated at*/, EatingProgress)>>,
}

impl Default for AutoEatModule {
    fn default() -> Self {
        Self {
            eating_progress: Arc::new(Mutex::new((Instant::now(), EatingProgress::NotEating))),
        }
    }
}

impl AutoEatModule {
    pub fn is_interacting(&self, bot: &mut Client) -> bool {
        bot.component::<AbstractLivingUsingItem>().0 // (part of AbstractLivingMetadataBundle)
    }

    /*
    fn next_sequence(&self, bot: &mut Client) -> u32 {
        /*let sequence = bot.entity_component::<&mut CurrentSequenceNumber>(bot.entity);
         **sequence += 1;
         *sequence as u32*/
        let mut ecs = bot.ecs.lock();
        let mut state = ecs.query::<&QueryData<(&mut CurrentSequenceNumber)>>();
        let query_result = state.get(&ecs, bot.entity);
        match query_result {
            Ok(query_result) => {
                let seq = **query_result;
            }
            Err(err) => {
                error!("Expected bot to have component CurrentSequenceNumber: {err:?}");
            }
        }
        //let seq = *current_sequence;

        todo!()
    }*/

    pub fn find_food_in_hotbar(&self, bot: &mut Client) -> Option<(u8, ItemStackData)> {
        let mut eat_item = None;

        // Find first food item in hotbar
        let inv = bot.entity_component::<Inventory>(bot.entity);
        let inv_menu = inv.inventory_menu;
        for (hotbar_slot, slot) in inv_menu.hotbar_slots_range().enumerate() {
            if let Some(item_stack_data) = inv_menu.slot(slot).and_then(|stack| stack.as_present())
            {
                if item_stack_data.components.get::<Food>().is_some()
                    || FOOD_ITEMS.contains(&item_stack_data.kind)
                {
                    eat_item = Some((hotbar_slot as u8, item_stack_data.to_owned()));
                    break;
                }
            }
        }

        eat_item
    }

    pub fn attempt_eat(&self, bot: &mut Client) -> bool {
        if let Some((eat_hotbar_slot, eat_item_stack_data)) = self.find_food_in_hotbar(bot) {
            // Switch to slot and start eating
            let look_direction = bot.component::<LookDirection>();
            let mut ecs = bot.ecs.lock();
            let entity = bot.entity;

            ecs.send_event(SetSelectedHotbarSlotEvent {
                entity,
                slot: eat_hotbar_slot,
            });
            ecs.send_event(SendPacketEvent {
                sent_by: entity,
                packet: ServerboundGamePacket::SetCarriedItem(ServerboundSetCarriedItem {
                    slot: eat_hotbar_slot as u16,
                }),
            });

            ecs.send_event(SendPacketEvent {
                sent_by: entity,
                packet: ServerboundGamePacket::UseItem(ServerboundUseItem {
                    hand: InteractionHand::MainHand,
                    sequence: 0,
                    yaw: look_direction.y_rot,
                    pitch: look_direction.x_rot,
                }),
            });

            info!(
                "Starting to eat item in hotbar slot {slot} ({count}x {kind})...",
                kind = eat_item_stack_data.kind,
                count = eat_item_stack_data.count,
                slot = eat_hotbar_slot,
            );
            true
        } else {
            false
        }
    }

    pub fn eat_tick(&self, bot: &mut Client) {
        let mut eating_progress = self.eating_progress.lock();

        match eating_progress.1.clone() {
            EatingProgress::NotEating => {
                let health = bot.component::<Health>();
                let hunger = bot.component::<Hunger>();

                let should_eat = (hunger.food <= 20 - (3 * 2)
                    || (health.0 < 20f32 && hunger.food < 20))
                    && hunger.saturation <= 0.0;

                if should_eat && eating_progress.0.elapsed() > Duration::from_millis(500) {
                    if self.attempt_eat(bot) {
                        *eating_progress = (Instant::now(), EatingProgress::StartedEating);
                    }
                }
            }
            EatingProgress::StartedEating => {
                if eating_progress.0.elapsed() > Duration::from_secs(3) {
                    warn!(
                        "Attempted to eat, but it failed (no interacting detected more than 3s later)!"
                    );
                    *eating_progress = (Instant::now(), EatingProgress::NotEating);
                } else if self.is_interacting(bot) {
                    *eating_progress = (Instant::now(), EatingProgress::IsEating);
                    info!("Eating in progress...");
                }
            }
            EatingProgress::IsEating => {
                if eating_progress.0.elapsed() > Duration::from_secs(15) {
                    warn!(
                        "Eating took too long! Perhaps interaction got confused with another action, the server is seriously lagging or eating a modified food item that takes forever (in which case ignore this)!"
                    );
                    *eating_progress = (Instant::now(), EatingProgress::NotEating);
                } else if !self.is_interacting(bot) {
                    *eating_progress = (Instant::now(), EatingProgress::NotEating);
                    info!("Successfully finished eating!");
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Module for AutoEatModule {
    fn name(&self) -> &'static str {
        "AutoEat"
    }

    async fn handle(
        &self,
        mut bot: Client,
        event: &Event,
        _bot_state: &BotState,
    ) -> anyhow::Result<()> {
        match event {
            Event::Tick => self.eat_tick(&mut bot),
            _ => {}
        }
        Ok(())
    }
}
