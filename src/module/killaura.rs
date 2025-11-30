use crate::module::Module;
use crate::task::attack::AttackTask;
use crate::task::tracked::TrackedTask;
use crate::{BotState, entity_util, util};
use anyhow::Context;
use azalea::ecs::entity::Entity;
use azalea::ecs::world::World;
use azalea::entity::metadata::*;
use azalea::entity::{Dead, Physics, Position};
use azalea::world::MinecraftEntityId;
use azalea::{Client, Event, Vec3};
use parking_lot::Mutex;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct KillauraModule {
    pub attack_task: Arc<Mutex<Option<TrackedTask<AttackTask>>>>,
}

impl KillauraModule {
    fn is_entity_likely_to_hurt_someone(ecs: &World, entity: Entity) -> bool {
        if ecs
            .get::<CustomName>(entity)
            .map(|name| name.0.as_ref().map(|name| !name.to_string().is_empty()).unwrap_or(false))
            .unwrap_or(false)
        {
            // Never attack anything with custom names!
            return false;
        }
        if ecs.get::<Dead>(entity).is_some() {
            return false; // Dead men don't hurt anyone
        }

        if ecs.get::<ShulkerBullet>(entity).is_some() || ecs.get::<Fireball>(entity).is_some() || ecs.get::<DragonFireball>(entity).is_some() {
            // Repellable Projectiles
            return true;
        }

        let is_attacking = ecs.get::<Aggressive>(entity).map(|aggressive| aggressive.0).unwrap_or(false);
        if ecs.get::<Bee>(entity).is_some()
            || ecs.get::<Wolf>(entity).is_some()
            || ecs.get::<Axolotl>(entity).is_some()
            || ecs.get::<Goat>(entity).is_some()
            || ecs.get::<IronGolem>(entity).is_some()
            || ecs.get::<Llama>(entity).is_some()
            || ecs.get::<TraderLlama>(entity).is_some()
            || ecs.get::<PolarBear>(entity).is_some()
            || ecs.get::<Panda>(entity).is_some()
            || ecs.get::<Dolphin>(entity).is_some()
        {
            // Entities that can become aggressive
            if is_attacking && !ecs.get::<Tamed>(entity).map(|tamed| tamed.0).unwrap_or(false) {
                // Entity is angry and not tamed
                return true;
            }
        }

        if !ecs.get::<AbstractMonster>(entity).is_some() {
            return false; // Not a monster
        }
        if ecs.get::<Enderman>(entity).is_some()
            && !ecs.get::<Creepy>(entity).map(|m| m.0).unwrap_or(false)
            && !ecs.get::<StaredAt>(entity).map(|m| m.0).unwrap_or(false)
        {
            return false; // Non-Angry enderman
        }
        if ecs.get::<Piglin>(entity).is_some() && !ecs.get::<PiglinIsChargingCrossbow>(entity).map(|m| m.0).unwrap_or(false) && !is_attacking {
            return false; // Piglin in non-aggressive stance
        }
        if ecs.get::<ZombifiedPiglin>(entity).is_some() && !is_attacking {
            return false; // Pigman in non-aggressive stance
        }

        // Any other monster should be attacked (Zombies, Creepers, etc.)
        true
    }

    pub fn has_attack_task(&self) -> bool {
        self.attack_task.lock().as_ref().map(|t| !t.status().is_finished()).unwrap_or(false)
    }
}

#[async_trait::async_trait]
impl Module for KillauraModule {
    fn name(&self) -> &'static str {
        "KillAura"
    }

    async fn handle(&self, bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Tick => {
                if let Some(auto_eat) = &bot_state.auto_eat
                    && auto_eat.has_eat_task()
                {
                    // Do not interrupt eating
                    return Ok(());
                }

                if self.has_attack_task() {
                    return Ok(()); // Already attacking
                }

                let (own_entity_id, own_eye_pos) = match (bot.get_component::<MinecraftEntityId>(), util::own_eye_pos(&bot)) {
                    (Some(id), Some(pos)) => (id, pos),
                    _ => return Ok(()),
                };
                let own_pos = Vec3::from(&bot.component::<Position>());
                let own_interaction_range = entity_util::own_entity_interaction_range(&bot);

                let mut target_entity = None;
                let mut target_dist = None;
                let mut ecs = bot.ecs.lock();
                let mut query = ecs.query::<(Entity, &Physics, &MinecraftEntityId)>();
                for (entity, physics, entity_id) in query.iter(&ecs) {
                    if entity_id == &own_entity_id {
                        continue; // Us
                    }
                    if !entity_util::could_interact_with_entity_box(own_eye_pos, own_interaction_range, physics.bounding_box, 0.0) {
                        continue; // Too far
                    }
                    if !Self::is_entity_likely_to_hurt_someone(&*ecs, entity) {
                        continue; // Not dangerous
                    }
                    let dist = util::squared_magnitude(physics.bounding_box, own_pos).min(util::squared_magnitude(physics.bounding_box, own_eye_pos));

                    if target_entity.is_some()
                        && let Some(act_target_dist) = target_dist
                    {
                        if dist < act_target_dist {
                            target_entity = Some(entity);
                            target_dist = Some(dist);
                        }
                    } else {
                        target_entity = Some(entity);
                        target_dist = Some(dist);
                    }
                }
                drop(ecs); // Stopping another task due to prio task would cause deadlock on ecs

                if let Some(target_entity) = target_entity {
                    let task = TrackedTask::new(AttackTask::new(target_entity, 10));
                    *self.attack_task.lock() = Some(task.clone());
                    bot_state.add_task_now(bot.clone(), bot_state, task).context("Add AttackTask now")?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
