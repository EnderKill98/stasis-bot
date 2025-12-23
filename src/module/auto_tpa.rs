use crate::module::Module;
use crate::{BotState, OPTS};
use anyhow::{Context, anyhow};
use azalea::{Client, Event};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct AutoTpaModule {
    pub trusted: Arc<Mutex<HashSet<Uuid>>>,
}

impl AutoTpaModule {
    pub fn trusted_path() -> PathBuf {
        PathBuf::from("autotpa-trusted.json")
    }

    pub async fn load_trusted(&self) -> anyhow::Result<()> {
        let remembered_trapdoor_positions_path = Self::trusted_path();
        if remembered_trapdoor_positions_path.exists() && !remembered_trapdoor_positions_path.is_dir() {
            *self.trusted.lock() = serde_json::from_str(
                &tokio::fs::read_to_string(remembered_trapdoor_positions_path)
                    .await
                    .context("Read trusted uuids from file")?,
            )
            .context("Parsing trusted uuids content")?;
            info!("Loaded trusted uuids from file.");
        } else {
            *self.trusted.lock() = Default::default();
            warn!("File for trusted uuids doesn't exist, yet (saving default one).");
            self.save_trusted().await?;
        };

        Ok(())
    }

    pub async fn save_trusted(&self) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(&*self.trusted.as_ref().lock()).context("Convert trusted uuids to json")?;
        tokio::fs::write(Self::trusted_path(), json).await.context("Save trusted uuids to file")?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Module for AutoTpaModule {
    fn name(&self) -> &'static str {
        "AutoTpa"
    }

    async fn handle(&self, bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Login => {
                info!("Loading trusted uuids...");
                if let Err(err) = self.load_trusted().await {
                    error!("Failed to load trusted uuids: {err:?}");
                    std::fs::rename(
                        Self::trusted_path(),
                        format!("{}.broken", Self::trusted_path().as_os_str().to_str().ok_or(anyhow!("Path err"))?),
                    )?;
                    self.load_trusted().await?;
                }
            }
            Event::Chat(packet) => {
                let message = packet.message().to_string();

                let mut requested_by: Option<String> = None;
                for ending in [" has requested to teleport to you.", " requested to teleport to you."] {
                    if message.ends_with(ending) {
                        requested_by = Some(message[0..message.len() - ending.len()].split(" ").into_iter().last().unwrap().to_owned());
                        break;
                    }
                }
                if requested_by.is_none() && message.starts_with("Received teleport request from ") {
                    let words = message.split(" ").collect::<Vec<_>>();
                    if words.len() >= 5 {
                        let name = words[4];
                        let name = &name[..name.len() - 1];
                        requested_by = Some(name.to_owned());
                    }
                }

                if let Some(requested_by) = requested_by {
                    let uuid = bot
                        .tab_list()
                        .iter()
                        .find(|(_, e)| e.profile.name.eq_ignore_ascii_case(&requested_by))
                        .map(|(_, e)| e.profile.uuid);
                    let trusted = uuid.map(|uuid| self.trusted.lock().contains(&uuid)).unwrap_or(false);
                    if let Some(chat) = bot_state.chat.as_ref() {
                        if trusted {
                            info!("Received TPA from {requested_by}, who is trusted. Accepting tpa...");
                            chat.cmd(OPTS.tpa_accept_command.replace("{NAME}", &requested_by)); // Some do allow specifying the senders name, most seem to not mind it being extra
                        } else {
                            info!("Received TPA from {requested_by}, who is not trusted. Denying tpa...");
                            chat.cmd(OPTS.tpa_deny_command.replace("{NAME}", &requested_by));
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
