use crate::task::{Task, TaskOutcome};
use crate::{BotState, entity_util, util};
use anyhow::{Result, anyhow};
use azalea::attack::AttackEvent;
use azalea::ecs::entity::Entity;
use azalea::entity::{Dead, LookDirection, Physics};
use azalea::movement::LastSentLookDirection;
use azalea::prelude::*;
use azalea::world::MinecraftEntityId;
use azalea::{Client, Event};
use std::fmt::{Display, Formatter};
use std::time::Instant;

#[derive(Clone, Component)]
pub struct LastAttack {
    when: Instant,
}

pub struct AttackTask {
    started_at: Instant,
    target: Entity,
    target_name: String,
    attack_delay_ticks: usize,
}

impl AttackTask {
    pub fn new(target: Entity, attack_delay_ticks: usize) -> Self {
        Self {
            started_at: Instant::now(),       // Doesn't matter
            target_name: "<...>".to_string(), // Evaluate on start

            target,
            attack_delay_ticks,
        }
    }

    pub fn look_at_closest_point(&self, bot: &mut Client) -> Result<()> {
        if let Some(own_eye_pos) = util::own_eye_pos(bot) {
            let good_look_dir = azalea::direction_looking_at(
                &own_eye_pos,
                &util::closest_aabb_pos_towards(own_eye_pos, entity_util::entity_aabb(bot, self.target), 0.05),
            );
            *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = util::fix_look_direction(good_look_dir);
        }
        Ok(())
    }
}

impl Display for AttackTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Attack ({})", self.target_name)
    }
}

impl Task for AttackTask {
    fn start(&mut self, mut bot: Client, _bot_state: &BotState) -> Result<()> {
        self.started_at = Instant::now();
        self.target_name = entity_util::describe(&mut bot, self.target)?;
        self.look_at_closest_point(&mut bot)?;
        Ok(())
    }

    fn handle(&mut self, mut bot: Client, _bot_state: &BotState, event: &Event) -> Result<TaskOutcome> {
        if let Event::Tick = event {
            if bot.get_entity_component::<Dead>(self.target).is_some() {
                return Ok(TaskOutcome::Failed {
                    reason: format!("Target is dead: {}", self.target_name),
                });
            }
            if !bot.get_entity_component::<Physics>(self.target).is_some() {
                return Ok(TaskOutcome::Failed {
                    reason: format!("Target has no physics: {}", self.target_name),
                });
            }
            if !bot.get_entity_component::<Physics>(self.target).is_some() {
                return Ok(TaskOutcome::Failed {
                    reason: format!("Target has no physics: {}", self.target_name),
                });
            }

            let entity_interaction_range = entity_util::own_entity_interaction_range(&bot);
            // Fail when out of reach
            if let Some(own_eye_pos) = util::own_eye_pos(&bot) {
                if !entity_util::could_interact_with_entity_box(own_eye_pos, entity_interaction_range, entity_util::entity_aabb(&mut bot, self.target), 0.05) {
                    return Ok(TaskOutcome::Failed {
                        reason: format!("Target is no longer reachable: {}", self.target_name),
                    });
                }
            }

            // Look at good point on target hitbox
            self.look_at_closest_point(&mut bot)?;

            if let Some(last_attack) = bot.get_component::<LastAttack>() {
                if last_attack.when.elapsed().as_millis() as u64 / 50 < self.attack_delay_ticks as u64 {
                    // Too soon
                    return Ok(TaskOutcome::Ongoing);
                }
            }

            // Attack, if looking at
            if let Some(own_eye_pos) = util::own_eye_pos(&bot)
                && let Some(last_sent_look_dir) = bot.get_component::<LastSentLookDirection>()
            {
                let entity_interaction_range = entity_util::own_entity_interaction_range(&bot);
                if entity_util::can_interact_with_entity_box(
                    own_eye_pos,
                    LookDirection::new(last_sent_look_dir.y_rot, last_sent_look_dir.x_rot),
                    entity_interaction_range,
                    entity_util::entity_aabb(&mut bot, self.target),
                ) {
                    // Attack
                    let target_id = bot.entity_component::<MinecraftEntityId>(self.target);
                    let mut ecs = bot.ecs.lock();
                    /*ecs.send_event(SendPacketEvent {
                        sent_by: bot.entity,
                        packet: ServerboundGamePacket::Interact(ServerboundInteract {
                            entity_id: target_id,
                            action: ActionType::Attack,
                            using_secondary_action: false, // Sneak
                        }),
                    });
                    ecs.send_event(SwingArmEvent { entity: bot.entity });*/

                    // May be later, but does not flag grim (PacketOrderB, post-attack)
                    ecs.send_event(AttackEvent {
                        entity: bot.entity,
                        target: target_id,
                    });
                    ecs.entity_mut(bot.entity).insert(LastAttack { when: Instant::now() });
                    return Ok(TaskOutcome::Succeeded);
                } else {
                    // Not intersecting with hitbox rn
                }
            }
            Ok(TaskOutcome::Ongoing)
        } else {
            Ok(TaskOutcome::Ongoing)
        }
    }

    fn stop(&mut self, _bot: Client, _bot_state: &BotState) -> Result<()> {
        Ok(())
    }
}
