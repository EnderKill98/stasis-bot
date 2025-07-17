use crate::BotState;
use crate::task::{Task, TaskOutcome};
use anyhow::Context;
use azalea::core::direction::Direction;
use azalea::entity::LookDirection;
use azalea::entity::metadata::{AbstractLivingUsingItem, Health};
use azalea::inventory::components::Food;
use azalea::inventory::{Inventory, ItemStackData, SetSelectedHotbarSlotEvent};
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::s_interact::InteractionHand;
use azalea::protocol::packets::game::s_player_action::Action;
use azalea::protocol::packets::game::{ServerboundGamePacket, ServerboundPlayerAction, ServerboundSetCarriedItem, ServerboundUseItem};
use azalea::registry::Item;
use azalea::{BlockPos, Client, Event, Hunger};
use std::fmt::{Display, Formatter};
use std::time::{Duration, Instant};

pub const FOOD_ITEMS: &[Item] = &[
    Item::Apple,
    Item::GoldenApple,
    Item::EnchantedGoldenApple,
    Item::Carrot,
    Item::GoldenCarrot,
    Item::MelonSlice,
    Item::SweetBerries,
    Item::GlowBerries,
    Item::Potato,
    Item::BakedPotato,
    Item::Beetroot,
    Item::DriedKelp,
    Item::Beef,
    Item::CookedBeef,
    Item::Porkchop,
    Item::CookedPorkchop,
    Item::Mutton,
    Item::CookedMutton,
    Item::Chicken,
    Item::CookedChicken,
    Item::Rabbit,
    Item::CookedRabbit,
    Item::Cod,
    Item::CookedCod,
    Item::Salmon,
    Item::CookedSalmon,
    Item::TropicalFish,
    Item::Bread,
    Item::Cookie,
    Item::PumpkinPie,
    Item::MushroomStew,
    Item::BeetrootSoup,
    Item::RabbitStew,
];

#[derive(Debug, Copy, Clone)]
pub enum EatingProgress {
    NotEating { reason: &'static str },
    StartedEating,
    IsEating,
}

impl Display for EatingProgress {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            EatingProgress::NotEating { reason } => write!(f, "NotEating ({reason})"),
            EatingProgress::StartedEating => write!(f, "StartedEating"),
            EatingProgress::IsEating => write!(f, "IsEating"),
        }
    }
}

pub struct EatTask {
    last_updated_at: Instant,
    eating_progress: EatingProgress,
}

impl Default for EatTask {
    fn default() -> Self {
        Self {
            last_updated_at: Instant::now(),
            eating_progress: EatingProgress::NotEating { reason: "Not started" },
        }
    }
}

impl EatTask {
    pub fn is_interacting(bot: &mut Client) -> bool {
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

    pub fn find_food_in_hotbar(bot: &mut Client) -> Option<(u8, ItemStackData)> {
        let mut eat_item = None;

        // Find first food item in hotbar
        let inv = bot.entity_component::<Inventory>(bot.entity);
        let inv_menu = inv.inventory_menu;
        for (hotbar_slot, slot) in inv_menu.hotbar_slots_range().enumerate() {
            if let Some(item_stack_data) = inv_menu.slot(slot).and_then(|stack| stack.as_present()) {
                if item_stack_data.components.get::<Food>().is_some() || FOOD_ITEMS.contains(&item_stack_data.kind) {
                    eat_item = Some((hotbar_slot as u8, item_stack_data.to_owned()));
                    break;
                }
            }
        }

        eat_item
    }

    pub fn attempt_eat(bot: &mut Client) -> bool {
        if let Some((eat_hotbar_slot, eat_item_stack_data)) = Self::find_food_in_hotbar(bot) {
            // Switch to slot and start eating
            let look_direction = bot.component::<LookDirection>();
            let mut ecs = bot.ecs.lock();
            let entity = bot.entity;

            ecs.send_event(SetSelectedHotbarSlotEvent { entity, slot: eat_hotbar_slot });
            ecs.send_event(SendPacketEvent {
                sent_by: entity,
                packet: ServerboundGamePacket::SetCarriedItem(ServerboundSetCarriedItem { slot: eat_hotbar_slot as u16 }),
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

    pub fn stop_interacting(bot: &Client) {
        bot.ecs.lock().send_event(SendPacketEvent {
            sent_by: bot.entity,
            packet: ServerboundGamePacket::PlayerAction(ServerboundPlayerAction {
                action: Action::ReleaseUseItem,
                direction: Direction::Down,
                sequence: 0,
                pos: BlockPos::new(0, 0, 0),
            }),
        });
    }

    pub fn should_eat(bot: &mut Client) -> bool {
        let health = bot.component::<Health>();
        let hunger = bot.component::<Hunger>();

        (hunger.food <= 20 - (3 * 2) || (health.0 < 20f32 && hunger.food < 20)) && hunger.saturation <= 0.0
    }
}

impl Display for EatTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // Changing this can break AutoEatModule::has_eat_task()!
        write!(f, "Eat ({})", self.eating_progress)
    }
}

impl Task for EatTask {
    fn start(&mut self, mut bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        if Self::attempt_eat(&mut bot) {
            self.last_updated_at = Instant::now();
            self.eating_progress = EatingProgress::StartedEating;
        }
        Ok(())
    }

    fn handle(&mut self, mut bot: Client, bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        if let Event::Tick = event {
            match self.eating_progress {
                EatingProgress::NotEating { reason } => Ok(TaskOutcome::Failed { reason: reason.to_string() }),
                EatingProgress::StartedEating => {
                    if self.last_updated_at.elapsed() > Duration::from_secs(3) {
                        self.last_updated_at = Instant::now();
                        self.eating_progress = EatingProgress::NotEating {
                            reason: "Starting to eat timed out (>3s)!",
                        };
                        Ok(TaskOutcome::Failed {
                            reason: "Attempted to eat, but it failed (no interacting detected more than 3s later)!".to_owned(),
                        })
                    } else if Self::is_interacting(&mut bot) {
                        self.last_updated_at = Instant::now();
                        self.eating_progress = EatingProgress::IsEating;
                        Ok(TaskOutcome::Ongoing)
                    } else {
                        Ok(TaskOutcome::Ongoing)
                    }
                }
                EatingProgress::IsEating => {
                    if self.last_updated_at.elapsed() > Duration::from_secs(15) {
                        self.stop(bot, &bot_state).context("Stop eating task")?;
                        self.last_updated_at = Instant::now();
                        self.eating_progress = EatingProgress::NotEating {
                            reason: "Eating took too long!",
                        };
                        Ok(TaskOutcome::Failed {
                            reason: "Eating took too long! Perhaps interaction got confused with another action, the server is seriously lagging or eating a modified food item that takes forever (in which case ignore this)!".to_owned(),
                        })
                    } else if !Self::is_interacting(&mut bot) {
                        self.last_updated_at = Instant::now();
                        self.eating_progress = EatingProgress::NotEating { reason: "Already done" };
                        info!("Successfully finished eating!");
                        Ok(TaskOutcome::Succeeded)
                    } else {
                        Ok(TaskOutcome::Ongoing)
                    }
                }
            }
        } else {
            Ok(TaskOutcome::Ongoing)
        }
    }

    fn stop(&mut self, bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        match self.eating_progress {
            EatingProgress::StartedEating | EatingProgress::IsEating => {
                info!("Stopping to interact.");
                Self::stop_interacting(&bot);
            }
            _ => {}
        }
        Ok(())
    }
}
