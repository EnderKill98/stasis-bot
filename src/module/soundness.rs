use crate::BotState;
use crate::module::Module;
use crate::task::Task;
use crate::task::group::TaskGroup;
use azalea::core::game_type::GameMode;
use azalea::ecs::query::With;
use azalea::entity::EntityKind;
use azalea::entity::metadata::CustomName;
use azalea::protocol::packets::game::ClientboundGamePacket;
use azalea::protocol::packets::game::c_damage_event::OptionalEntityId;
use azalea::world::MinecraftEntityId;
use azalea::{Client, Event, GameProfileComponent, ResourceLocation};
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::Instant;

// Used when --use-hardcoded-damage-types is specified
#[rustfmt::skip]
const DAMAGE_TYPES: &[&'static str] = &[
    "arrow", "bad_respawn_point", "cactus", "campfire", "cramming", "dragon_breath", "drown", "dry_out", "ender_pearl", "explosion", "fall", "falling_anvil",
    "falling_block", "falling_stalactite", "fireball", "fireworks", "fly_into_wall", "freeze", "generic", "generic_kill", "hot_floor", "in_fire", "in_wall",
    "indirect_magic", "lava", "lightning_bolt", "mace_smash", "magic", "mob_attack", "mob_attack_no_aggro", "mob_projectile", "on_fire", "out_of_world",
    "outside_border", "player_attack", "player_explosion", "sonic_boom", "spit", "stalagmite", "starve", "sting", "sweet_berry_bush", "thorns", "thrown",
    "trident", "unattributed_fireball", "wind_charge", "wither", "wither_skull",
];

#[derive(Clone, Eq, PartialEq)]
pub enum InGameStatus {
    Disconnected,
    InLimbo { title: Option<String>, subtitle: Option<String> },
    InGame { when: Instant },
}

impl Default for InGameStatus {
    fn default() -> Self {
        Self::Disconnected
    }
}

#[derive(Clone, Default)]
pub struct SoundnessModule {
    pub status: Arc<Mutex<InGameStatus>>,
    pub last_damage_event: Arc<Mutex<Option<(Instant, String)>>>,
    pub interrupt_next_tick: Arc<AtomicBool>,
}

impl SoundnessModule {
    pub fn describe(bot: &mut Client, entity_id: i32) -> String {
        let entity = bot.entity_by::<With<EntityKind>, (&MinecraftEntityId,)>(|(id,): &(&MinecraftEntityId,)| id.0 == entity_id);
        if entity.is_none() {
            return format!("<Unknown entity {entity_id}>");
        }
        let entity = entity.unwrap();
        let kind = bot.entity_component::<EntityKind>(entity);
        let kind = kind.0.to_string().split(":").collect::<Vec<_>>()[1].to_owned();

        if let Some(profile) = bot.get_entity_component::<GameProfileComponent>(entity) {
            return format!("{} (uuid: {}, id: {entity_id})", profile.name, profile.uuid);
        }

        match bot.get_entity_component::<CustomName>(entity) {
            Some(CustomName(Some(custom_name))) => {
                format!("{custom_name} (type: {kind}, id: {entity_id})")
            }
            _ => format!("{kind} ({entity_id})"),
        }
    }

    fn update_status(&self, bot_state: &BotState, new_status: InGameStatus) {
        let mut old_status = self.status.lock();
        if *old_status != new_status {
            match new_status {
                InGameStatus::Disconnected => bot_state.webhook("`🔴` Disconnected!"),
                InGameStatus::InLimbo { ref title, ref subtitle } => {
                    if let Some(title) = title {
                        bot_state.webhook(format!("`🟣` In Limbo: {title}"))
                    } else if let Some(subtitle) = subtitle {
                        bot_state.webhook(format!("`🟣` In Limbo: {subtitle}"))
                    } else {
                        bot_state.webhook("`🟣` In Limbo!")
                    }
                }
                InGameStatus::InGame { .. } => bot_state.webhook("`🟢` In Game!"),
            }
            *old_status = new_status;
        }
    }

    fn interrupt(&self, bot: &mut Client, bot_state: &BotState) -> anyhow::Result<()> {
        info!("Interrupted. Cleaning all tasks!");
        bot_state.webhook("`✋` Interrupted. Cleaning all tasks...");
        let mut root_task_group = bot_state.root_task_group.lock();
        root_task_group.discard(bot.clone(), bot_state)?;
        *root_task_group = TaskGroup::new_root();
        Ok(())
    }

    pub fn is_ingame(&self) -> bool {
        matches!(*self.status.lock(), InGameStatus::InGame { .. })
    }

    pub fn is_ingame_for(&self, min_duration: Duration) -> bool {
        if let InGameStatus::InGame { when } = *self.status.lock() {
            when.elapsed() >= min_duration
        } else {
            false
        }
    }
}

#[async_trait::async_trait]
impl Module for SoundnessModule {
    fn name(&self) -> &'static str {
        "Soundness"
    }

    async fn handle(&self, mut bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Init => bot_state.webhook("`👋` Initializing..."),
            Event::Login => bot_state.webhook("`🔵` Logged in!"),
            Event::Spawn => bot_state.webhook("`🎮`️ Spawned!"),
            Event::Disconnect(reason) => {
                if let Some(reason) = reason {
                    bot_state.webhook(format!("`👞` Kick: {reason}"));
                }
                if matches!(*self.status.lock(), InGameStatus::Disconnected) {
                    self.interrupt_next_tick.store(true, Ordering::Relaxed);
                    //self.interrupt(&mut bot, bot_state)?;
                }
                self.update_status(bot_state, InGameStatus::Disconnected);
            }
            Event::Packet(packet) => match packet.as_ref() {
                ClientboundGamePacket::Login(packet) => {
                    if packet.common.game_type == GameMode::Spectator {
                        if matches!(*self.status.lock(), InGameStatus::Disconnected) {
                            self.interrupt_next_tick.store(true, Ordering::Relaxed);
                            //self.interrupt(&mut bot, bot_state)?;
                        }
                        self.update_status(&bot_state, InGameStatus::InLimbo { title: None, subtitle: None });
                    } else {
                        self.update_status(&bot_state, InGameStatus::InGame { when: Instant::now() });
                    }
                }
                ClientboundGamePacket::SetTitleText(packet) => {
                    info!("Title Text: {}", packet.text);
                    let status = self.status.lock().clone();
                    if let InGameStatus::InLimbo { subtitle, .. } = status {
                        let title = packet.text.to_string();
                        self.update_status(
                            &bot_state,
                            InGameStatus::InLimbo {
                                title: if title.is_empty() { None } else { Some(title) },
                                subtitle,
                            },
                        );
                    }
                }
                ClientboundGamePacket::SetSubtitleText(packet) => {
                    info!("Subttitle Text: {}", packet.text);
                    let status = self.status.lock().clone();
                    if let InGameStatus::InLimbo { title, .. } = status {
                        let subtitle = packet.text.to_string();
                        self.update_status(
                            &bot_state,
                            InGameStatus::InLimbo {
                                title,
                                subtitle: if subtitle.is_empty() { None } else { Some(subtitle) },
                            },
                        );
                    }
                }
                ClientboundGamePacket::Respawn(packet) => {
                    if packet.common.game_type == GameMode::Spectator {
                        if matches!(*self.status.lock(), InGameStatus::Disconnected) {
                            self.interrupt_next_tick.store(true, Ordering::Relaxed);
                            //self.interrupt(&mut bot, bot_state)?;
                        }
                        self.update_status(&bot_state, InGameStatus::InLimbo { title: None, subtitle: None });
                    } else {
                        if !matches!(*self.status.lock(), InGameStatus::InGame { .. }) {
                            self.update_status(&bot_state, InGameStatus::InGame { when: Instant::now() });
                        }
                    }
                }
                ClientboundGamePacket::EntityEvent(packet) => {
                    let my_entity_id = bot.entity_component::<MinecraftEntityId>(bot.entity).0;
                    if packet.entity_id.0 == my_entity_id && packet.event_id == 35 {
                        info!("I popped a Totem!");
                    }
                }
                ClientboundGamePacket::SetHealth(packet) => {
                    info!(
                        "Health: {:.02}, Food: {:.02}, Saturation: {:.02}",
                        packet.health, packet.food, packet.saturation
                    );
                }
                ClientboundGamePacket::DamageEvent(packet) => {
                    let my_entity_id = bot.entity_component::<MinecraftEntityId>(bot.entity).0;
                    if packet.entity_id.0 == my_entity_id {
                        let attack_type_id = packet.source_type_id;
                        let cause_id = packet.source_cause_id.to_owned();
                        let mut direct_id = packet.source_direct_id.to_owned();
                        let maybe_attack_type_identifier = if crate::OPTS.use_hardcoded_damage_types {
                            if attack_type_id < DAMAGE_TYPES.len() as u32 {
                                Some(ResourceLocation {
                                    namespace: "minecraft".to_owned(),
                                    path: DAMAGE_TYPES[attack_type_id as usize].to_owned(),
                                })
                            } else {
                                None
                            }
                        } else {
                            bot.with_registry_holder(|holder| {
                                holder
                                    .map
                                    .get(&ResourceLocation::new("minecraft:damage_type"))
                                    .unwrap()
                                    .iter()
                                    .enumerate()
                                    .filter(|(index, (_identifier, _nbt_comp))| *index == attack_type_id as usize)
                                    .map(|(_index, (identifier, _nbt_comp))| identifier.to_owned())
                                    .find(|_| true)
                            })
                        };
                        let attack_type = if let Some(attack_type_identifier) = maybe_attack_type_identifier {
                            if attack_type_identifier.namespace == "minecraft" {
                                attack_type_identifier.path.to_string()
                            } else {
                                attack_type_identifier.to_string()
                            }
                        } else {
                            format!("TypeId {attack_type_id}")
                        };
                        if cause_id.0 == direct_id.0 {
                            direct_id = OptionalEntityId(None);
                        }

                        let message: String = match (cause_id, direct_id) {
                            (OptionalEntityId(Some(source_cause)), OptionalEntityId(Some(source_direct_id))) => {
                                format!(
                                    "Received {} damage by {} using {}",
                                    attack_type,
                                    Self::describe(&mut bot, source_cause as i32),
                                    Self::describe(&mut bot, source_direct_id as i32)
                                )
                            }
                            (OptionalEntityId(Some(source_cause)), OptionalEntityId(None)) => {
                                format!("Received {} damage by {}", attack_type, Self::describe(&mut bot, source_cause as i32))
                            }
                            _ => {
                                format!("Received {} damage", attack_type)
                            }
                        };
                        info!("{}", message);
                        *self.last_damage_event.lock() = Some((Instant::now(), message));
                    }
                }
                _ => {}
            },
            Event::Death(packet) => {
                if let Some(packet) = packet {
                    info!("You died: {}", packet.message);
                } else {
                    info!("You died!");
                }
            }
            Event::Tick => {
                if self.interrupt_next_tick.fetch_and(false, Ordering::Relaxed) {
                    self.interrupt(&mut bot, bot_state)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
