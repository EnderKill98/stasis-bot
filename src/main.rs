#[macro_use]
extern crate tracing;

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
    swarm::{Swarm, SwarmEvent},
    world::MinecraftEntityId,
    GameProfileComponent, JoinOpts, Vec3,
};
use clap::Parser;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt::Debug,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing_subscriber::prelude::*;

/// A simple stasis bot, using azalea!
#[derive(Parser)]
#[clap(author, version)]
struct Opts {
    /// What server ((and port) to connect to
    server_address: String,

    /// Player names who are considered more trustworthy for certain commands
    #[clap(short, long)]
    admin: Vec<String>,

    /// Use ViaProxy to translate the protocol to the given minecraft version.
    #[clap(short, long)]
    via: Option<String>,

    /// Automatically log out, once getting to this HP or lower (or a totem pops)
    #[clap(short = 'h', long)]
    autolog_hp: Option<f32>,

    /// Workaround for crashes: Forbid the bot from sending any messages to players.
    #[clap(short = 'q', long)]
    quiet: bool,

    /// Make the stasis bot, not do any stasis-related duties. So you can abuse him easier as an afk bot.
    #[clap(short = 'S', long)]
    no_stasis: bool,

    /// Specify a logfile to log everything into as well
    #[clap(short = 'l', long)]
    log_file: Option<PathBuf>,

    /// Disable color. Can fix some issues of still persistent escape codes in log files.
    #[clap(short = 'C', long)]
    no_color: bool,
}

static OPTS: Lazy<Opts> = Lazy::new(|| Opts::parse());

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
    // Parse cli args, handle --help, etc.
    let _ = *OPTS;

    // Setup logging
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    let reg = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(
            tracing_subscriber::fmt::layer()
                .compact()
                .with_ansi(!OPTS.no_color),
        );
    if let Some(logfile_path) = &OPTS.log_file {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(logfile_path)
            .context("Open logfile for appending")?;
        reg.with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file),
        )
        .init();
    } else {
        reg.init();
    }

    if OPTS.no_color {
        info!("Will not use colored chat messages and disabled ansi formatting in console.");
    }

    if OPTS.quiet {
        info!("Will not automatically send any chat commands (workaround for getting kicked because of broken ChatCommand packet).");
    }

    if let Some(autolog_hp) = OPTS.autolog_hp {
        info!("Will automatically logout and quit, when getting to or below {autolog_hp} HP or popping a totem.");
    }

    info!("Admins: {}", OPTS.admin.join(", "));

    info!("Logging in...");
    //let account = Account::offline("unnamed_bot");
    //let account = Account::microsoft("example@example.com").await.unwrap();
    let auth_result = azalea::auth::auth(
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
    };

    info!(
        "Logged in as {}. Connecting to \"{}\"...",
        account.username, OPTS.server_address
    );

    let mut builder = azalea::swarm::SwarmBuilder::new()
        .set_handler(handle)
        .set_swarm_handler(swarm_handle)
        .add_account(account.clone());
    if let Some(via) = &OPTS.via {
        info!("Using ViaProxy to translate the protocol to minecraft version {via}...");
        builder = builder.add_plugins(azalea_viaversion::ViaVersionPlugin::start(via).await);
    }
    builder
        .start(OPTS.server_address.as_str())
        .await
        .context("Running bot")?
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
            info!(
                "CHAT: {}",
                if OPTS.no_color {
                    packet.message().to_string()
                } else {
                    packet.message().to_ansi()
                }
            );
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
                let (command, args) = if content.contains(' ') {
                    let mut all_args: Vec<_> = content
                        .to_lowercase()
                        .split(' ')
                        .map(|s| s.to_owned())
                        .collect();
                    let command = all_args.remove(0);
                    (command, all_args)
                } else {
                    (content.to_lowercase(), vec![])
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
                            if !OPTS.quiet {
                                let mut commands =
                                    vec!["!help", "!about", "!comehere", "!admins", "!say"];
                                if !OPTS.no_stasis {
                                    commands.push("!tp");
                                }
                                commands.sort();

                                bot.send_command_packet(&format!(
                                    "msg {sender} Commands: {}",
                                    commands.join(", ")
                                ));
                            }
                        }
                        "about" => {
                            *bot_state.last_dm_handled_at.lock() = Some(Instant::now());
                            if !OPTS.quiet {
                                bot.send_command_packet(&format!("msg {sender} Hi, I'm running EnderKill98's azalea-based stasis-bot: github.com/EnderKill98/stasis-bot"));
                            }
                        }
                        "tp" => {
                            *bot_state.last_dm_handled_at.lock() = Some(Instant::now());
                            let remembered_trapdoor_positions =
                                bot_state.remembered_trapdoor_positions.lock();
                            if OPTS.no_stasis {
                                if !OPTS.quiet {
                                    bot.send_command_packet(&format!(
                                        "msg {sender} I'm not allowed to do pearl duties :(..."
                                    ));
                                }
                            } else {
                                if let Some(trapdoor_pos) =
                                    remembered_trapdoor_positions.get(&sender)
                                {
                                    if bot_state.pathfinding_requested_by.lock().is_some() {
                                        if !OPTS.quiet {
                                            bot.send_command_packet(&format!(
                                                "msg {sender} Please ask again in a bit. I'm currently already going somewhere..."
                                            ));
                                        }
                                    } else {
                                        if !OPTS.quiet {
                                            bot.send_command_packet(&format!(
                                                "msg {sender} Walking to your stasis chamber..."
                                            ));
                                        }

                                        *bot_state.return_to_after_pulled.lock() =
                                            Some(Vec3::from(
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
                                    if !OPTS.quiet {
                                        bot.send_command_packet(&format!(
                                "msg {sender} I'm not aware whether you have a pearl here. Sorry!"
                            ));
                                    }
                                }
                            }
                        }
                        "comehere" => {
                            *bot_state.last_dm_handled_at.lock() = Some(Instant::now());
                            if OPTS.admin.contains(&sender) {
                                let sender_entity = bot
                                    .entity_by::<With<Player>, (&GameProfileComponent,)>(
                                        |(profile,): &(&GameProfileComponent,)| {
                                            profile.name == sender
                                        },
                                    );
                                if let Some(sender_entity) = sender_entity {
                                    let position = bot.entity_component::<Position>(sender_entity);
                                    bot.goto(BlockPosGoal(azalea::BlockPos {
                                        x: position.x.floor() as i32,
                                        y: position.y.floor() as i32,
                                        z: position.z.floor() as i32,
                                    }));
                                    if !OPTS.quiet {
                                        bot.send_command_packet(&format!(
                                            "msg {sender} Walking to your block position..."
                                        ));
                                    }
                                } else {
                                    if !OPTS.quiet {
                                        bot.send_command_packet(&format!("msg {sender} I could not find you in my render distance!"));
                                    }
                                }
                            } else {
                                if !OPTS.quiet {
                                    bot.send_command_packet(&format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"));
                                }
                            }
                        }
                        "admins" => {
                            *bot_state.last_dm_handled_at.lock() = Some(Instant::now());
                            if !OPTS.quiet {
                                bot.send_command_packet(&format!(
                                    "msg {sender} Admins: {}",
                                    OPTS.admin.join(", ")
                                ));
                            }
                        }
                        "say" => {
                            *bot_state.last_dm_handled_at.lock() = Some(Instant::now());
                            if OPTS.admin.contains(&sender) {
                                let command_or_chat = args.join(" ");
                                if command_or_chat.starts_with("/") {
                                    info!("Sending command: {command_or_chat}");
                                    bot.send_command_packet(&format!("{}", &command_or_chat[1..]));
                                } else {
                                    info!("Sending chat: {command_or_chat}");
                                    bot.send_chat_packet(&command_or_chat);
                                }
                            } else {
                                if !OPTS.quiet {
                                    bot.send_command_packet(&format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"));
                                }
                            }
                        }

                        _ => {} // Do nothing if unrecognized command
                    }
                }
            }
        }
        Event::Packet(packet) => match packet.as_ref() {
            ClientboundGamePacket::AddEntity(packet) => {
                if !OPTS.no_stasis && packet.entity_type == EntityKind::EnderPearl {
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

                                if !OPTS.quiet {
                                    bot.send_command_packet(&format!("msg {} You have thrown a pearl. Message me \"tp\" to get back here.", game_profile.name));
                                }
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
            ClientboundGamePacket::EntityEvent(packet) => {
                let my_entity_id = bot.entity_component::<MinecraftEntityId>(bot.entity).0;
                if packet.entity_id == my_entity_id && packet.event_id == 35 {
                    // Totem popped!
                    info!("I popped a Totem!");
                    if OPTS.autolog_hp.is_some() {
                        warn!("Disconnecting and quitting because --autolog-hp is enabled...");
                        bot.disconnect();
                        std::process::exit(1);
                    }
                }
            }
            ClientboundGamePacket::SetHealth(packet) => {
                info!(
                    "Health: {:.02}, Food: {:.02}, Saturation: {:.02}",
                    packet.health, packet.food, packet.saturation
                );
                if let Some(hp) = OPTS.autolog_hp {
                    if packet.health <= hp {
                        warn!("My Health got below {hp:.02}! Disconnecting and quitting...");
                        bot.disconnect();
                        std::process::exit(1);
                    }
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
                        if !OPTS.quiet {
                            bot.send_command_packet(&format!(
                                "msg {requesting_player} Welcome back, {requesting_player}!"
                            ));
                        }
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

#[derive(Default, Clone, Component, Resource)]
pub struct SwarmState {}

async fn swarm_rejoin(mut swarm: Swarm, state: SwarmState, account: Account, join_opts: JoinOpts) {
    let mut reconnect_after_secs = 5;
    loop {
        info!("Reconnecting after {} seconds...", reconnect_after_secs);

        tokio::time::sleep(Duration::from_secs(reconnect_after_secs)).await;
        reconnect_after_secs = (reconnect_after_secs * 2).min(60 * 30); // 2x or max 30 minutes

        info!("Joining again...");
        match swarm
            .add_with_opts(&account, state.clone(), &join_opts)
            .await
        {
            Ok(_) => return,
            Err(join_err) => error!("Failed to rejoin: {join_err}"), // Keep rejoining
        }
    }
}

async fn swarm_handle(swarm: Swarm, event: SwarmEvent, state: SwarmState) -> anyhow::Result<()> {
    match event {
        SwarmEvent::Disconnect(account, join_opts) => {
            tokio::spawn(swarm_rejoin(
                swarm.clone(),
                state.clone(),
                (*account).clone(),
                join_opts.clone(),
            ));
        }
        _ => {}
    }

    Ok(())
}
