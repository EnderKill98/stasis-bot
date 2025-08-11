use crate::module::Module;
use crate::task::pathfind;
use crate::{BotState, util};
use anyhow::anyhow;
use azalea::entity::metadata::Player;
use azalea::entity::{EyeHeight, Pose, Position};
use azalea::inventory::Inventory;
use azalea::world::MinecraftEntityId;
use azalea::{Client, Event};

#[derive(Clone, Default)]
pub struct LookAtPlayersModule {
    max_distance: u32,
}

impl LookAtPlayersModule {
    pub fn new(max_distance: u32) -> Self {
        Self { max_distance }
    }
}

#[async_trait::async_trait]
impl Module for LookAtPlayersModule {
    fn name(&self) -> &'static str {
        "LookAtPlayers"
    }

    async fn handle(&self, mut bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Tick => {
                // Look at players
                if !pathfind::is_pathfinding(&bot) && bot_state.tasks() == 0 && bot.get_component::<Inventory>().map(|inv| inv.id == 0).unwrap_or(true) {
                    let my_entity_id = bot.entity_component::<MinecraftEntityId>(bot.entity).0;
                    let my_eye_pos = match util::own_eye_pos(&bot) {
                        Some(pos) => pos,
                        None => return Ok(()),
                    };

                    let mut closest_eye_pos = None;
                    let mut closest_dist_sqrt = f64::MAX;
                    let mut query = bot.ecs.lock().query::<(&Player, &Position, &EyeHeight, &Pose, &MinecraftEntityId)>();
                    for (_player, pos, eye_height, pose, entity_id) in query.iter(&bot.ecs.lock()) {
                        if entity_id.0 == my_entity_id {
                            continue;
                        }

                        let eye_pos = util::calculate_player_eye_pos(pos, pose, eye_height);
                        let dist_sqrt = my_eye_pos.distance_squared_to(pos);
                        if (closest_eye_pos.is_none() || dist_sqrt < closest_dist_sqrt) && dist_sqrt <= (self.max_distance * self.max_distance) as f64 {
                            closest_eye_pos = Some(eye_pos);
                            closest_dist_sqrt = dist_sqrt;
                        }
                    }

                    if let Some(eye_pos) = closest_eye_pos {
                        //bot.look_at(eye_pos);
                        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? =
                            util::fix_look_direction(azalea::direction_looking_at(&my_eye_pos, &eye_pos));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
