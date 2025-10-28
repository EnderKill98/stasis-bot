use crate::BotState;
use crate::module::Module;
use anyhow::{Context, anyhow};
use azalea::auth::game_profile::GameProfile;
use azalea::protocol::packets::game::ClientboundGamePacket;
use azalea::registry::EntityKind;
use azalea::world::MinecraftEntityId;
use azalea::{Client, Event};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct VisualRangeModule {
    visual_range_cache: Arc<Mutex<HashMap<MinecraftEntityId, GameProfile>>>,
    seen: Arc<Mutex<HashSet<Uuid>>>,
}

impl VisualRangeModule {
    pub fn seen_path() -> PathBuf {
        PathBuf::from("seen.json")
    }

    pub async fn load_seen(&self) -> anyhow::Result<()> {
        let remembered_trapdoor_positions_path = Self::seen_path();
        if remembered_trapdoor_positions_path.exists() && !remembered_trapdoor_positions_path.is_dir() {
            *self.seen.lock() = serde_json::from_str(
                &tokio::fs::read_to_string(remembered_trapdoor_positions_path)
                    .await
                    .context("Read seen uuids from file")?,
            )
            .context("Parsing stasis-config content")?;
            info!("Loaded seen uuids from file.");
        } else {
            *self.seen.lock() = Default::default();
            warn!("File for seen uuids doesn't exist, yet (saving default one).");
            self.save_config().await?;
        };

        Ok(())
    }

    pub async fn save_config(&self) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(&*self.seen.as_ref().lock()).context("Convert seen uuids to json")?;
        tokio::fs::write(Self::seen_path(), json).await.context("Save seen uuids to file")?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Module for VisualRangeModule {
    fn name(&self) -> &'static str {
        "VisualRange"
    }

    async fn handle(&self, bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Login => {
                self.visual_range_cache.lock().clear();

                info!("Loading seen uuids...");
                if let Err(err) = self.load_seen().await {
                    error!("Failed to load seen uuids: {err:?}");
                    std::fs::rename(
                        Self::seen_path(),
                        format!("{}.broken", Self::seen_path().as_os_str().to_str().ok_or(anyhow!("Path err"))?),
                    )?;
                    self.load_seen().await?;
                }
            }
            Event::Disconnect(_) => self.visual_range_cache.lock().clear(),
            Event::Packet(packet) => match packet.as_ref() {
                ClientboundGamePacket::Respawn(_) => self.visual_range_cache.lock().clear(),
                ClientboundGamePacket::AddEntity(packet) => {
                    if packet.entity_type == EntityKind::Player {
                        let mut seen = self.seen.lock();
                        let mut is_stranger = !seen.contains(&packet.uuid);
                        if is_stranger {
                            seen.insert(packet.uuid);
                            let self_clone = self.clone();
                            tokio::spawn(async move {
                                if let Err(err) = self_clone.save_config().await {
                                    error!("Failed to save seen uuids: {err:?}");
                                }
                            });
                        }

                        if is_stranger && let Some(stasis) = &bot_state.stasis {
                            if stasis
                                .config
                                .lock()
                                .chambers
                                .iter()
                                .flat_map(|chamber| &chamber.occupants)
                                .any(|occupants| occupants.player_uuid == Some(packet.uuid))
                            {
                                // Has a pearl
                                is_stranger = false;
                                info!(
                                    "Not considering player (uuid: {}) as a stranger because has at least one existing pearl!",
                                    packet.uuid
                                );
                            }
                        }

                        let webhook_message = match bot.tab_list().get(&packet.uuid) {
                            Some(player_info) => {
                                self.visual_range_cache.lock().insert(packet.id, player_info.profile.clone());
                                info!(
                                    "{} ({}) entered visual range{}",
                                    player_info.profile.name,
                                    player_info.uuid,
                                    if is_stranger { " for the first time!" } else { "." }
                                );
                                format!(
                                    "`🔷` [{}](<https://namemc.com/profile/{}>) entered visual range{}",
                                    player_info.profile.name,
                                    player_info.uuid,
                                    if is_stranger { " for the first time!" } else { "." }
                                )
                            }
                            None => {
                                info!(
                                    "An unknown player (id: {}, uuid: {}) entered visual range{}",
                                    packet.id,
                                    packet.uuid,
                                    if is_stranger { " **for the first time**!" } else { "." }
                                );
                                format!(
                                    "An unknown player (id: {}, uuid: [{}](<https://namemc.com/profile/{}>)) entered visual range{}",
                                    packet.id,
                                    packet.uuid,
                                    packet.uuid,
                                    if is_stranger { " **for the first time**!" } else { "." }
                                )
                            }
                        };
                        if is_stranger {
                            bot_state.webhook_alert(webhook_message);
                        } else {
                            bot_state.webhook(webhook_message);
                        }
                    }
                }
                ClientboundGamePacket::RemoveEntities(packet) => {
                    for entity_id in &packet.entity_ids {
                        // At this point the entity was already removed from the ecs world!
                        if let Some(profile) = self.visual_range_cache.lock().remove(entity_id) {
                            info!("{} ({}) left visual range.", profile.name, profile.uuid);
                            bot_state.webhook(format!(
                                "`🔶` [{}](<https://namemc.com/profile/{}>) left visual range.",
                                profile.name, profile.uuid
                            ));
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
