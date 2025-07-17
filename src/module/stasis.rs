use crate::module::Module;
use crate::task::delay_ticks::DelayTicksTask;
use crate::task::group::TaskGroup;
use crate::task::oncefunc::OnceFuncTask;
use crate::task::pathfind;
use crate::task::pathfind::PathfindTask;
use crate::{BotState, OPTS};
use anyhow::Context;
use azalea::blocks::Block;
use azalea::core::direction::Direction;
use azalea::ecs::prelude::With;
use azalea::entity::Position;
use azalea::entity::metadata::Player;
use azalea::packet::game::SendPacketEvent;
use azalea::pathfinder::goals::{BlockPosGoal, ReachBlockPosGoal};
use azalea::protocol::packets::game::s_interact::InteractionHand;
use azalea::protocol::packets::game::s_use_item_on::BlockHit;
use azalea::protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket, ServerboundUseItemOn};
use azalea::registry::EntityKind;
use azalea::world::MinecraftEntityId;
use azalea::{BlockPos, Client, Event, GameProfileComponent, Vec3};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct StasisModule {
    pub do_reopen_trapdoor: bool,

    pub remembered_trapdoor_positions: Arc<Mutex<HashMap<String, BlockPos>>>,
    pub last_idle_pos: Arc<Mutex<Option<BlockPos>>>,
}

impl StasisModule {
    pub fn new(do_reopen_trapdoor: bool) -> Self {
        Self {
            do_reopen_trapdoor,

            remembered_trapdoor_positions: Default::default(),
            last_idle_pos: Default::default(),
        }
    }

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
            warn!("File for rememembered trapdoor positions doesn't exist, yet.");
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

    pub fn recommended_closed_trapdoor_ticks(bot_state: &BotState) -> u32 {
        if let Some(server_tps) = &bot_state.server_tps {
            // These values worked well with fabric Based Bot (PearlButler on Simpcraft)
            let tps = server_tps.current_tps().unwrap_or(20.0);
            let mut tick_delay = 12;
            if server_tps.is_server_likely_hanging() {
                tick_delay += 60;
            }
            if tps <= 15.0 {
                tick_delay += 5;
            }
            if tps <= 10.0 {
                tick_delay += 10;
            }
            if tps <= 5.0 {
                tick_delay += 10;
            }
            tick_delay
        } else {
            30
        }
    }

    fn current_block_pos(bot: &mut Client) -> BlockPos {
        let pos = bot.entity_component::<Position>(bot.entity);
        BlockPos {
            x: pos.x.floor() as i32,
            y: pos.y.floor() as i32,
            z: pos.z.floor() as i32,
        }
    }

    pub fn return_pos(&self, bot: &mut Client) -> BlockPos {
        if let Some(last_idle_pos) = self.last_idle_pos.lock().as_ref() {
            last_idle_pos.clone()
        } else {
            Self::current_block_pos(bot)
        }
    }

    pub fn clear_idle_pos(&self, reason: impl AsRef<str>) {
        *self.last_idle_pos.lock() = None;
        debug!("Cleared idle pos: {}", reason.as_ref());
    }

    pub fn update_idle_pos(&self, bot: &mut Client) {
        *self.last_idle_pos.lock() = Some(Self::current_block_pos(bot));
    }

    pub fn pull_pearl<F: Fn(/*error*/ bool, &str) + Send + Sync + 'static>(
        &self,
        username: &str,
        bot: &Client,
        bot_state: &BotState,
        feedback: F,
    ) -> anyhow::Result<()> {
        let username = username.to_owned();
        let remembered_trapdoor_positions = self.remembered_trapdoor_positions.lock();
        let trapdoor_pos = remembered_trapdoor_positions.get(&username);
        if trapdoor_pos.is_none() {
            feedback(true, "I'm not aware whether you have a pearl here. Sorry!");
            return Ok(());
        }
        let trapdoor_pos = trapdoor_pos.unwrap();

        if bot_state.tasks() > 0 {
            feedback(false, "Hang on, will walk to your pearl in due time...");
        } else {
            feedback(false, "Walking to your pearl...");
        }
        info!("Walking to {trapdoor_pos:?}...");

        let trapdoor_goal = ReachBlockPosGoal {
            pos: trapdoor_pos.to_owned(),
            chunk_storage: bot.world().read().chunks.clone(),
        };
        let return_goal = BlockPosGoal(self.return_pos(&mut bot.clone()));

        let greeting = format!("Welcome back, {username}!");

        let interact_event = SendPacketEvent {
            sent_by: bot.entity,
            packet: ServerboundGamePacket::UseItemOn(ServerboundUseItemOn {
                block_hit: BlockHit {
                    block_pos: trapdoor_pos.clone(),
                    direction: Direction::Down,
                    location: Vec3 {
                        x: trapdoor_pos.x as f64 + 0.5,
                        y: trapdoor_pos.y as f64 + 0.5,
                        z: trapdoor_pos.z as f64 + 0.5,
                    },
                    inside: true,
                    world_border: false,
                },
                hand: InteractionHand::MainHand,
                sequence: 0,
            }),
        };

        let interact_event_clone = interact_event.clone();
        let do_reopen_trapdoor = self.do_reopen_trapdoor;
        let username_clone = username.to_owned();

        let remembered_trapdoor_positions = self.remembered_trapdoor_positions.clone();
        bot_state.add_task(
            TaskGroup::new(format!("Pull {username}'s Pearl"))
                .with(PathfindTask::new(!OPTS.no_mining, trapdoor_goal, format!("near {username}'s Pearl")))
                .with(OnceFuncTask::new(format!("Close {username}'s Trapdoor and Greet"), move |bot, _bot_state| {
                    bot.ecs.lock().send_event(interact_event);
                    remembered_trapdoor_positions.lock().remove(&username_clone);
                    feedback(false, &greeting);
                    Ok(())
                }))
                .with(DelayTicksTask::new(Self::recommended_closed_trapdoor_ticks(bot_state)))
                .with(OnceFuncTask::new(format!("Re-Open {username}'s Trapdoor"), move |bot, _bot_state| {
                    if do_reopen_trapdoor {
                        bot.ecs.lock().send_event(interact_event_clone);
                    }
                    Ok(())
                }))
                .with(PathfindTask::new(!OPTS.no_mining, return_goal, "old spot")),
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl Module for StasisModule {
    fn name(&self) -> &'static str {
        "Stasis"
    }

    async fn handle(&self, bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Login => {
                info!("Loading remembered trapdoor positions...");
                self.load_stasis().await?;
                self.clear_idle_pos("Login-Event")
            }
            Event::Disconnect(_) => {
                self.clear_idle_pos("Disconnect-Event");
            }
            Event::Packet(packet) => match packet.as_ref() {
                ClientboundGamePacket::PlayerPosition(_) => {
                    self.clear_idle_pos("Got teleport-Packet");
                }
                ClientboundGamePacket::AddEntity(packet) => {
                    if packet.entity_type == EntityKind::EnderPearl {
                        let owning_player_entity_id = packet.data as i32;
                        let mut bot = bot.clone();
                        let entity =
                            bot.entity_by::<With<Player>, (&MinecraftEntityId,)>(|(entity_id,): &(&MinecraftEntityId,)| entity_id.0 == owning_player_entity_id);
                        if let Some(entity) = entity {
                            let game_profile = bot.entity_component::<GameProfileComponent>(entity);
                            info!("{} threw an EnderPearl at {}", game_profile.name, packet.position);
                            let mut found_trapdoor = None;

                            {
                                let world = bot.world();
                                let world = world.read();
                                for y_offset_abs in 0..16 {
                                    for y_offset_mult in [1, -1] {
                                        let y_offset = y_offset_abs * y_offset_mult;
                                        let azalea_block_pos = azalea::BlockPos::new(
                                            packet.position.x.floor() as i32,
                                            (packet.position.y.floor() + 1.0) as i32 + y_offset,
                                            packet.position.z.floor() as i32,
                                        );
                                        if let Some(state) = world.get_block_state(&azalea_block_pos) {
                                            let block = Box::<dyn Block>::from(state);
                                            if block.id().ends_with("_trapdoor") {
                                                info!("Detected trapdoor at {} for pearl thrown by {}", azalea_block_pos, game_profile.name);
                                                found_trapdoor = Some(BlockPos::from(azalea_block_pos));
                                                break;
                                            }
                                        }
                                    }
                                }
                            }

                            if let Some(block_pos) = found_trapdoor {
                                if self.remembered_trapdoor_positions.lock().get(&game_profile.name) != Some(&block_pos) {
                                    let mut remembered_trapdoor_positions = self.remembered_trapdoor_positions.lock();
                                    // Remove postions at same trapdoor
                                    for playername in remembered_trapdoor_positions.keys().map(|p| p.to_owned()).collect::<Vec<_>>() {
                                        if remembered_trapdoor_positions.get(&playername) == Some(&block_pos) {
                                            info!("Found that {playername} is already using that trapdoor position. Removed that player!");
                                            remembered_trapdoor_positions.remove(&playername);
                                        }
                                    }
                                    remembered_trapdoor_positions.insert(game_profile.name.clone(), block_pos);

                                    if !OPTS.quiet {
                                        bot.send_command_packet(&format!(
                                            "msg {} You have thrown a pearl. Message me \"tp\" to get back here.",
                                            game_profile.name
                                        ));
                                    }
                                    let self_clone = self.clone();
                                    tokio::spawn(async move {
                                        match self_clone.save_stasis().await {
                                            Ok(_) => {
                                                info!("Saved remembered trapdoor positions to file.")
                                            }
                                            Err(err) => error!("Failed to save remembered trapdoor positions to file: {err:?}"),
                                        }
                                    });
                                }
                            }
                        }
                    }
                }
                _ => {}
            },
            Event::Tick => {
                if bot_state.tasks() == 0 && pathfind::is_pathfinding(&bot) {
                    self.update_idle_pos(&mut bot.clone());
                }
            }
            _ => {}
        }
        Ok(())
    }
}
