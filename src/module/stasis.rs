use crate::module::Module;
use crate::{BotState, OPTS};
use anyhow::Context;
use azalea::blocks::Block;
use azalea::core::direction::Direction;
use azalea::ecs::prelude::With;
use azalea::entity::metadata::Player;
use azalea::packet::game::SendPacketEvent;
use azalea::pathfinder::goals::BlockPosGoal;
use azalea::pathfinder::{Pathfinder, PathfinderClientExt};
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

#[derive(Clone, Default)]
pub struct StasisModule {
    pub remembered_trapdoor_positions: Arc<Mutex<HashMap<String, BlockPos>>>,
    pub pathfinding_requested_by: Arc<Mutex<Option<String>>>,
    pub return_to_after_pulled: Arc<Mutex<Option<azalea::Vec3>>>,
}

impl StasisModule {
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
}

#[async_trait::async_trait]
impl Module for StasisModule {
    fn name(&self) -> &'static str {
        "Stasis"
    }

    async fn handle(&self, bot: Client, event: &Event, _bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Login => {
                info!("Loading remembered trapdoor positions...");
                self.load_stasis().await?;
            }
            Event::Packet(packet) => match packet.as_ref() {
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
                let mut pathfinding_requested_by = self.pathfinding_requested_by.lock();
                if let Some(ref requesting_player) = *pathfinding_requested_by {
                    let mut ecs = bot.ecs.lock();
                    let pathfinder: &Pathfinder = ecs.query::<&Pathfinder>().get_mut(&mut *ecs, bot.entity).unwrap();

                    if !pathfinder.is_calculating && pathfinder.goal.is_none() {
                        drop(ecs);

                        if let Some(trapdoor_pos) = self.remembered_trapdoor_positions.lock().remove(requesting_player) {
                            if !OPTS.quiet {
                                bot.send_command_packet(&format!("msg {requesting_player} Welcome back, {requesting_player}!"));
                            }
                            bot.ecs.lock().send_event(SendPacketEvent {
                                sent_by: bot.entity,
                                packet: ServerboundGamePacket::UseItemOn(ServerboundUseItemOn {
                                    block_hit: BlockHit {
                                        block_pos: azalea::BlockPos::from(trapdoor_pos),
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
                            });

                            *pathfinding_requested_by = None;
                            if let Some(return_to_after_pulled) = self.return_to_after_pulled.lock().take() {
                                info!("Returning to {return_to_after_pulled}...");
                                let goal = BlockPosGoal(BlockPos {
                                    x: return_to_after_pulled.x.floor() as i32,
                                    y: return_to_after_pulled.y.floor() as i32,
                                    z: return_to_after_pulled.z.floor() as i32,
                                });
                                if OPTS.no_mining {
                                    bot.goto_without_mining(goal);
                                } else {
                                    bot.goto(goal);
                                }
                            }

                            let self_clone = self.clone();
                            tokio::spawn(async move {
                                match self_clone.save_stasis().await {
                                    Ok(_) => info!("Saved remembered trapdoor positions to file."),
                                    Err(err) => error!("Failed to save remembered trapdoor positions to file: {err:?}"),
                                }
                            });
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
