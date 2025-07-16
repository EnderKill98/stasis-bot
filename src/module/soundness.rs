use crate::BotState;
use crate::module::Module;
use azalea::ecs::query::With;
use azalea::entity::EntityKind;
use azalea::entity::metadata::CustomName;
use azalea::protocol::packets::game::ClientboundGamePacket;
use azalea::protocol::packets::game::c_damage_event::OptionalEntityId;
use azalea::world::MinecraftEntityId;
use azalea::{Client, Event, GameProfileComponent, ResourceLocation};

#[derive(Clone, Default)]
pub struct SoundnessModule {}

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
}

#[async_trait::async_trait]
impl Module for SoundnessModule {
    fn name(&self) -> &'static str {
        "Soundness"
    }

    async fn handle(&self, mut bot: Client, event: &Event, _bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Packet(packet) => match packet.as_ref() {
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
                        let maybe_attack_type_identifier = bot.with_registry_holder(|holder| {
                            holder
                                .map
                                .get(&ResourceLocation::new("minecraft:damage_type"))
                                .unwrap()
                                .iter()
                                .enumerate()
                                .filter(|(index, (_identifier, _nbt_comp))| *index == attack_type_id as usize)
                                .map(|(_index, (identifier, _nbt_comp))| identifier.to_owned())
                                .find(|_| true)
                        });
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

                        match (cause_id, direct_id) {
                            (OptionalEntityId(Some(source_cause)), OptionalEntityId(Some(source_direct_id))) => {
                                info!(
                                    "Received {} damage by {} using {}",
                                    attack_type,
                                    Self::describe(&mut bot, source_cause as i32),
                                    Self::describe(&mut bot, source_direct_id as i32)
                                );
                            }
                            (OptionalEntityId(Some(source_cause)), OptionalEntityId(None)) => {
                                info!("Received {} damage by {}", attack_type, Self::describe(&mut bot, source_cause as i32));
                            }
                            _ => {
                                info!("Received {} damage", attack_type);
                            }
                        }
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
            _ => {}
        }
        Ok(())
    }
}
