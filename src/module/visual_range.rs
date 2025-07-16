use crate::BotState;
use crate::module::Module;
use azalea::auth::game_profile::GameProfile;
use azalea::protocol::packets::game::ClientboundGamePacket;
use azalea::registry::EntityKind;
use azalea::world::MinecraftEntityId;
use azalea::{Client, Event};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct VisualRangeModule {
    visual_range_cache: Arc<parking_lot::Mutex<HashMap<MinecraftEntityId, GameProfile>>>,
}

#[async_trait::async_trait]
impl Module for VisualRangeModule {
    fn name(&self) -> &'static str {
        "VisualRange"
    }

    async fn handle(
        &self,
        bot: Client,
        event: &Event,
        _bot_state: &BotState,
    ) -> anyhow::Result<()> {
        match event {
            Event::Packet(packet) => match packet.as_ref() {
                ClientboundGamePacket::AddEntity(packet) => {
                    if packet.entity_type == EntityKind::Player {
                        match bot.tab_list().get(&packet.uuid) {
                            Some(player_info) => {
                                info!(
                                    "{} ({}) entered visual range!",
                                    player_info.profile.name, player_info.uuid
                                );
                                self.visual_range_cache
                                    .lock()
                                    .insert(packet.id, player_info.profile.clone());
                            }
                            None => {
                                warn!(
                                    "An unknown player (id: {}, uuid: {}) entered visual range!",
                                    packet.id, packet.uuid
                                );
                            }
                        }
                    }
                }
                ClientboundGamePacket::RemoveEntities(packet) => {
                    for entity_id in &packet.entity_ids {
                        // At this point the entity was already removed from the ecs world!
                        if let Some(profile) = self.visual_range_cache.lock().remove(entity_id) {
                            info!("{} ({}) left visual range!", profile.name, profile.uuid)
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }
}
