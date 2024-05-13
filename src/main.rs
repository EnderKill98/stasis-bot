//! A bot that logs chat messages sent in the server to the console.

#[macro_use]
extern crate log;

use anyhow::{Context, Result};
use azalea::{
    blocks::Block,
    core::direction::Direction,
    ecs::query::With,
    entity::{metadata::Player, Position},
    packet_handling::game::SendPacketEvent,
    pathfinder::{
        goals::{BlockPosGoal, ReachBlockPosGoal},
        Pathfinder,
    },
    prelude::*,
    protocol::packets::game::{
        serverbound_interact_packet::InteractionHand,
        serverbound_use_item_on_packet::{BlockHit, ServerboundUseItemOnPacket},
        ClientboundGamePacket, ServerboundGamePacket,
    },
    registry::EntityKind,
    world::MinecraftEntityId,
    GameProfileComponent, Vec3,
};
use clap::Parser;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

/// A simple stasis bot, using azalea!
#[derive(Parser)]
#[clap(author, version)]
struct Opts {
    // What server ((and port) to connect to
    server_address: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
struct BlockPos {
    x: i32,
    y: i32,
    z: i32,
}

impl From<azalea::BlockPos> for BlockPos {
    fn from(value: azalea::BlockPos) -> Self {
        Self {
            x: value.x,
            y: value.y,
            z: value.z,
        }
    }
}

impl From<BlockPos> for azalea::BlockPos {
    fn from(value: BlockPos) -> Self {
        Self {
            x: value.x,
            y: value.y,
            z: value.z,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();

    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }

    let account = Account::offline("unnamed_bot");
    //let account = Account::microsoft("example@example.com").await.unwrap();
    /*let auth_result = azalea::auth::auth(
        "default",
        azalea::auth::AuthOpts {
            cache_file: Some(PathBuf::from("login-secrets.json")),
            ..Default::default()
        },
    )
    .await?;
    let account = azalea::Account {
        username: auth_result.profile.name,
        access_token: Some(Arc::new(Mutex::new(auth_result.access_token))),
        uuid: Some(auth_result.profile.id),
        account_opts: azalea::AccountOpts::Microsoft {
            email: "default".to_owned(),
        },
        // we don't do chat signing by default unless the user asks for it
        certs: None,
    };*/

    ClientBuilder::new()
        .set_handler(handle)
        .start(account, opts.server_address.as_str())
        .await
        .context("Running bot")?;
}

#[derive(Default, Clone, Component)]
pub struct BotState {
    remembered_trapdoor_positions: Arc<Mutex<HashMap<String, BlockPos>>>,
    pathfinding_requested_by: Arc<Mutex<Option<String>>>,
    return_to_after_pulled: Arc<Mutex<Option<azalea::Vec3>>>,
    last_dm_handled_at: Arc<Mutex<Option<Instant>>>,
}

impl BotState {
    pub fn remembered_trapdoor_positions_path() -> PathBuf {
        PathBuf::from("remembered-trapdoor-positions.json")
    }

    pub async fn load(&mut self) -> Result<()> {
        let remembered_trapdoor_positions_path = Self::remembered_trapdoor_positions_path();
        if remembered_trapdoor_positions_path.exists()
            && !remembered_trapdoor_positions_path.is_dir()
        {
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

    pub async fn save(&self) -> Result<()> {
        let json =
            serde_json::to_string_pretty(&*self.remembered_trapdoor_positions.as_ref().lock())
                .context("Convert remembered_trapdoor_positions to json")?;
        tokio::fs::write(Self::remembered_trapdoor_positions_path(), json)
            .await
            .context("Save remembered_trapdoor_positions as file")?;
        Ok(())
    }
}

async fn handle(mut bot: Client, event: Event, mut bot_state: BotState) -> anyhow::Result<()> {
    match event {
        Event::Login => {
            info!("Loading remembered trapdoor positions...");
            bot_state.load().await?;
        }
        Event::Chat(packet) => {
            info!("[CHAT] {}", packet.message().to_ansi());
            let message = packet.message().to_string();

            // Very security and sane way to find out, if message was a dm to self.
            // Surely no way to cheese it..
            let mut dm = None;
            if message.starts_with('[') && message.contains("-> me] ") {
                dm = Some((
                    message.split(" ").next().unwrap()[1..].to_owned(),
                    message.split("-> me] ").collect::<Vec<_>>()[1].to_owned(),
                ));
            } else if message.contains(" whispers to you: ") {
                dm = Some((
                    message.split(" ").next().unwrap().to_owned(),
                    message.split("whispers to you: ").collect::<Vec<_>>()[1].to_owned(),
                ));
            }

            if let Some((sender, mut content)) = dm {
                if content.starts_with('!') {
                    content.remove(0);
                }
                let command = if content.contains(' ') {
                    content.to_lowercase().split(' ').next().unwrap().to_owned()
                } else {
                    content.to_lowercase()
                };

                if bot_state
                    .last_dm_handled_at
                    .lock()
                    .map(|at| at.elapsed() > Duration::from_secs(1))
                    .unwrap_or(true)
                {
                    match command.as_str() {
                        "help" => {
                            *bot_state.last_dm_handled_at.lock() = Some(Instant::now());
                            bot.send_command_packet(&format!(
                                "msg {sender} Commands: !help, !about, !tp"
                            ));
                        }
                        "about" => {
                            *bot_state.last_dm_handled_at.lock() = Some(Instant::now());
                            bot.send_command_packet(&format!("msg {sender} Hi, I'm running EnderKill98's azalea-based stasis-bot! Find me on GitHub!"));
                        }
                        "tp" => {
                            *bot_state.last_dm_handled_at.lock() = Some(Instant::now());
                            let remembered_trapdoor_positions =
                                bot_state.remembered_trapdoor_positions.lock();
                            if let Some(trapdoor_pos) = remembered_trapdoor_positions.get(&sender) {
                                if bot_state.pathfinding_requested_by.lock().is_some() {
                                    bot.send_command_packet(&format!(
                                "msg {sender} Please ask again in a bit. I'm currently already going somewhere..."
                            ));
                                } else {
                                    bot.send_command_packet(&format!(
                                        "msg {sender} Walking to your stasis chamber..."
                                    ));

                                    *bot_state.return_to_after_pulled.lock() = Some(Vec3::from(
                                        &bot.entity_component::<Position>(bot.entity),
                                    ));

                                    info!("Walking to {trapdoor_pos:?}...");
                                    bot.goto(ReachBlockPosGoal {
                                        pos: azalea::BlockPos::from(*trapdoor_pos),
                                        chunk_storage: bot.world().read().chunks.clone(),
                                    });
                                    *bot_state.pathfinding_requested_by.lock() =
                                        Some(sender.clone());
                                }
                            } else {
                                bot.send_command_packet(&format!(
                                "msg {sender} I'm not aware whether you have a pearl here. Sorry!"
                            ));
                            }
                        }
                        _ => {} // Do nothing if unrecognized command
                    }
                }
            }
        }
        Event::Packet(packet) => match packet.as_ref() {
            ClientboundGamePacket::AddEntity(packet) => {
                if packet.entity_type == EntityKind::EnderPearl {
                    let owning_player_entity_id = packet.data;
                    let mut bot = bot.clone();
                    let entity = bot.entity_by::<With<Player>, (&MinecraftEntityId,)>(
                        |(entity_id,): &(&MinecraftEntityId,)| {
                            entity_id.0 as i32 == owning_player_entity_id
                        },
                    );
                    if let Some(entity) = entity {
                        let game_profile = bot.entity_component::<GameProfileComponent>(entity);
                        info!(
                            "{} threw an EnderPearl at {}",
                            game_profile.name, packet.position
                        );
                        let mut found_trapdoor = None;

                        {
                            let world = bot.world();
                            let world = world.read();
                            for y_offset in [0, -1, 1, -2, 2, -3, 3, -4, 4, -5, 5] {
                                let azalea_block_pos = azalea::BlockPos::new(
                                    packet.position.x.floor() as i32,
                                    packet.position.y.floor() as i32 + y_offset,
                                    packet.position.z.floor() as i32,
                                );
                                if let Some(state) = world.get_block_state(&azalea_block_pos) {
                                    let block = Box::<dyn Block>::from(state);
                                    if block.id().ends_with("_trapdoor") {
                                        info!(
                                            "Detected trapdoor at {} for pearl thrown by {}",
                                            azalea_block_pos, game_profile.name
                                        );
                                        found_trapdoor = Some(BlockPos::from(azalea_block_pos));
                                        break;
                                    }
                                }
                            }
                        }

                        if let Some(block_pos) = found_trapdoor {
                            if bot_state
                                .remembered_trapdoor_positions
                                .lock()
                                .get(&game_profile.name)
                                != Some(&block_pos)
                            {
                                let mut remembered_trapdoor_positions =
                                    bot_state.remembered_trapdoor_positions.lock();
                                // Remove postions at same trapdoor
                                for playername in remembered_trapdoor_positions
                                    .keys()
                                    .map(|p| p.to_owned())
                                    .collect::<Vec<_>>()
                                {
                                    if remembered_trapdoor_positions.get(&playername)
                                        == Some(&block_pos)
                                    {
                                        info!("Found that {playername} is already using that trapdoor position. Removed that player!");
                                        remembered_trapdoor_positions.remove(&playername);
                                    }
                                }
                                remembered_trapdoor_positions
                                    .insert(game_profile.name.clone(), block_pos);

                                bot.send_command_packet(&format!("msg {} You have thrown a pearl. Message me \"tp\" to get back here.", game_profile.name));
                                let bot_state = bot_state.clone();
                                tokio::spawn(async move {
                                    match bot_state
                                    .save()
                                    .await {
                                        Ok(_) => info!("Saved remembered trapdoor positions to file."),
                                        Err(err) => error!("Failed to save remembered trapdoor positions to file: {err:?}"),
                                    }
                                });
                            }
                        }
                    }
                }
                if packet.entity_type == EntityKind::Player {
                    info!(
                        "A player appeared at {} with entity id {}",
                        packet.position, packet.id
                    );
                }
            }
            _ => {}
        },
        Event::Tick => {
            let mut pathfinding_requested_by = bot_state.pathfinding_requested_by.lock();
            if let Some(ref requesting_player) = *pathfinding_requested_by {
                let mut ecs = bot.ecs.lock();
                let pathfinder: &Pathfinder = ecs
                    .query::<&Pathfinder>()
                    .get_mut(&mut *ecs, bot.entity)
                    .unwrap();

                if !pathfinder.is_calculating && pathfinder.goal.is_none() {
                    drop(ecs);

                    if let Some(trapdoor_pos) = bot_state
                        .remembered_trapdoor_positions
                        .lock()
                        .remove(requesting_player)
                    {
                        bot.send_command_packet(&format!(
                            "msg {requesting_player} Welcome back, {requesting_player}!"
                        ));

                        bot.ecs.lock().send_event(SendPacketEvent {
                            entity: bot.entity,
                            packet: ServerboundGamePacket::UseItemOn(ServerboundUseItemOnPacket {
                                block_hit: BlockHit {
                                    block_pos: azalea::BlockPos::from(trapdoor_pos),
                                    direction: Direction::Down,
                                    location: azalea::Vec3 {
                                        x: trapdoor_pos.x as f64 + 0.5,
                                        y: trapdoor_pos.y as f64 + 0.5,
                                        z: trapdoor_pos.z as f64 + 0.5,
                                    },
                                    inside: true,
                                },
                                hand: InteractionHand::MainHand,
                                sequence: 0,
                            }),
                        });

                        *pathfinding_requested_by = None;
                        if let Some(return_to_after_pulled) =
                            bot_state.return_to_after_pulled.lock().take()
                        {
                            info!("Returning to {return_to_after_pulled}...");
                            bot.goto(BlockPosGoal(azalea::BlockPos {
                                x: return_to_after_pulled.x.floor() as i32,
                                y: return_to_after_pulled.y.floor() as i32,
                                z: return_to_after_pulled.z.floor() as i32,
                            }));
                        }

                        let bot_state = bot_state.clone();
                        tokio::spawn(async move {
                            match bot_state.save().await {
                                Ok(_) => info!("Saved remembered trapdoor positions to file."),
                                Err(err) => error!(
                                    "Failed to save remembered trapdoor positions to file: {err:?}"
                                ),
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
