use crate::BotState;
use crate::module::Module;
use azalea::entity::metadata::Player;
use azalea::entity::{EyeHeight, Pose, Position};
use azalea::pathfinder::Pathfinder;
use azalea::world::MinecraftEntityId;
use azalea::{BotClientExt, Client, Event, Vec3};

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

    async fn handle(&self, mut bot: Client, event: &Event, _bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Tick => {
                // Look at players
                let is_pathfinding = {
                    let mut ecs = bot.ecs.lock();
                    let pathfinder: &Pathfinder = ecs.query::<&Pathfinder>().get_mut(&mut *ecs, bot.entity).unwrap();
                    pathfinder.goal.is_some()
                };

                if !is_pathfinding {
                    let my_entity_id = bot.entity_component::<MinecraftEntityId>(bot.entity).0;
                    let my_pos = bot.entity_component::<Position>(bot.entity);
                    let my_eye_height = *bot.entity_component::<EyeHeight>(bot.entity) as f64;
                    let my_eye_pos = *my_pos + Vec3::new(0f64, my_eye_height, 0f64);

                    let mut closest_eye_pos = None;
                    let mut closest_dist_sqrt = f64::MAX;
                    let mut query = bot.ecs.lock().query::<(&Player, &Position, &EyeHeight, &Pose, &MinecraftEntityId)>();
                    for (_player, pos, eye_height, pose, entity_id) in query.iter(&bot.ecs.lock()) {
                        if entity_id.0 == my_entity_id {
                            continue;
                        }

                        let y_offset = match pose {
                            Pose::FallFlying | Pose::Swimming | Pose::SpinAttack => 0.5f64,
                            Pose::Sleeping => 0.25f64,
                            Pose::Sneaking => (**eye_height as f64) * 0.85,
                            _ => **eye_height as f64,
                        };
                        let eye_pos = **pos + Vec3::new(0f64, y_offset, 0f64);
                        let dist_sqrt = my_eye_pos.distance_squared_to(pos);
                        if (closest_eye_pos.is_none() || dist_sqrt < closest_dist_sqrt) && dist_sqrt <= (self.max_distance * self.max_distance) as f64 {
                            closest_eye_pos = Some(eye_pos);
                            closest_dist_sqrt = dist_sqrt;
                        }
                    }

                    if let Some(eye_pos) = closest_eye_pos {
                        bot.look_at(eye_pos);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
