use crate::BotState;
use crate::module::Module;
use crate::module::stasis::{ChamberOccupant, StasisChamberDefinition, StasisChamberEntry};
use anyhow::Context;
use azalea::{BlockPos, Client, Event};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct LegacyStasisModule {
    pub remembered_trapdoor_positions: Arc<Mutex<HashMap<String, BlockPos>>>,
    pub last_idle_pos: Arc<Mutex<Option<BlockPos>>>,
}

impl LegacyStasisModule {
    pub fn remembered_trapdoor_positions_path() -> PathBuf {
        PathBuf::from("remembered-trapdoor-positions.json")
    }

    pub async fn load_stasis(&self) -> anyhow::Result<()> {
        let remembered_trapdoor_positions_path = Self::remembered_trapdoor_positions_path();
        if remembered_trapdoor_positions_path.exists() && !remembered_trapdoor_positions_path.is_dir() {
            *self.remembered_trapdoor_positions.lock() = serde_json::from_str(
                &tokio::fs::read_to_string(remembered_trapdoor_positions_path)
                    .await
                    .context("Read remembered_trapdoor_positions file")?,
            )
            .context("Parsing remembered_trapdoor_positions content")?;
            info!(
                "Loaded {} remembered trapdoor positions from file.",
                self.remembered_trapdoor_positions.lock().len()
            );
        } else {
            *self.remembered_trapdoor_positions.lock() = Default::default();
            warn!("File for remembered trapdoor positions doesn't exist, yet.");
        };

        Ok(())
    }

    pub async fn save_stasis(&self) -> anyhow::Result<()> {
        let json =
            serde_json::to_string_pretty(&*self.remembered_trapdoor_positions.as_ref().lock()).context("Convert remembered_trapdoor_positions to json")?;
        tokio::fs::write(Self::remembered_trapdoor_positions_path(), json)
            .await
            .context("Save remembered_trapdoor_positions as file")?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Module for LegacyStasisModule {
    fn name(&self) -> &'static str {
        "Stasis"
    }

    async fn handle(&self, _bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Login => {
                if Self::remembered_trapdoor_positions_path().exists() {
                    info!("Loading remembered trapdoor positions...");
                    self.load_stasis().await?;
                }
            }
            Event::AddPlayer(player_info) => {
                if let Some(trapdoor_pos) = self.remembered_trapdoor_positions.lock().remove(&player_info.profile.name) {
                    let stasis = bot_state.stasis.as_ref();
                    if stasis.is_none() {
                        return Ok(()); // Can't migrate
                    }
                    let stasis = stasis.unwrap();
                    let found_existing_trapdoor = stasis
                        .config
                        .lock()
                        .chambers
                        .iter()
                        .find(|chamber| match chamber.definition {
                            StasisChamberDefinition::FlippableTrapdoor { trapdoor_pos: pos }
                            | StasisChamberDefinition::SecuredFlippableTrapdoor { trigger_trapdoor_pos: pos, .. } => {
                                if pos == trapdoor_pos {
                                    true
                                } else {
                                    false
                                }
                            }
                            _ => false,
                        })
                        .is_some();

                    if !found_existing_trapdoor {
                        // Migrate
                        stasis.config.lock().chambers.push(StasisChamberEntry {
                            definition: StasisChamberDefinition::FlippableTrapdoor { trapdoor_pos },
                            occupants: vec![ChamberOccupant {
                                player_uuid: Some(player_info.profile.uuid),
                                // No clue, so filler values:
                                pearl_uuid: None,
                                thrown_at: chrono::DateTime::UNIX_EPOCH.into(),
                            }],
                        });
                        let stasis = stasis.clone();
                        tokio::spawn(async move {
                            if let Err(err) = stasis.save_config().await {
                                error!("Failed to save (new) stasis-config: {err:?}");
                            }
                        });
                        warn!("Migrating {}'s legacy trapdoor pos ({}) to new stasis!", player_info.profile.name, trapdoor_pos);
                    } else {
                        warn!(
                            "Not migrating {}'s legacy trapdoor pos ({}) as a new location already exists. Just removing it!",
                            player_info.profile.name, trapdoor_pos
                        );
                    }
                }
                if self.remembered_trapdoor_positions.lock().len() == 0 {
                    if Self::remembered_trapdoor_positions_path().exists() {
                        info!(
                            "Legacy trapdoor config is empty! Deleting file {:?}...",
                            Self::remembered_trapdoor_positions_path()
                        );
                        std::fs::remove_file(Self::remembered_trapdoor_positions_path()).ok();
                    }
                } else {
                    self.save_stasis().await?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
