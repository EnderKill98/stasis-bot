#![feature(let_chains)]

pub mod commands;

#[macro_use]
extern crate tracing;

use anyhow::{Context, Result};
use azalea::app::PluginGroup;
use azalea::player::GameProfileComponent;
use azalea::swarm::DefaultSwarmPlugins;
use azalea::task_pool::{TaskPoolOptions, TaskPoolPlugin, TaskPoolThreadAssignmentPolicy};
use azalea::{
    entity::{metadata::Player, EyeHeight, Pose, Position},
    pathfinder::Pathfinder,
    prelude::*,
    registry::Item,
    swarm::{Swarm, SwarmEvent},
    world::MinecraftEntityId,
    DefaultBotPlugins, DefaultPlugins, JoinOpts, Vec3,
};
use bevy_log::LogPlugin;
use clap::Parser;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicU32;
use std::{
    collections::VecDeque,
    fmt::Debug,
    io::IsTerminal,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{Instrument, Level};
use tracing_subscriber::prelude::*;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[allow(dead_code)]
pub const EXITCODE_OTHER: i32 = 1; // Due to errors returned and propagated to main result
pub const EXITCODE_CONFLICTING_CLI_OPTS: i32 = 2;
pub const EXITCODE_AUTH_FAILED: i32 = 3;
pub const EXITCODE_NO_ACCESS_TOKEN: i32 = 4;

pub const EXITCODE_USER_REQUESTED_STOP: i32 = 20; // Using an error code to prevent automatic relaunching in some configurations or scripts

/// A simple stasis bot, using azalea!
#[derive(Parser)]
#[clap(author, version)]
struct Opts {
    /// What server ((and port) to connect to
    server_address: String,

    /// Player names who are considered more trustworthy for certain commands
    #[clap(short, long)]
    admin: Vec<String>,

    /// Workaround for crashes: Forbid the bot from sending any messages to players.
    #[clap(short = 'q', long)]
    quiet: bool,

    /// Specify a logfile to log everything into as well
    #[clap(short = 'l', long)]
    log_file: Option<PathBuf>,

    /// Disable color. Can fix some issues of still persistent escape codes in log files.
    #[clap(short = 'C', long)]
    no_color: bool,

    /// Allow chat messages to be signed.
    #[clap(short = 's', long)]
    sign_chat: bool,

    /// Use offline accounts with specified user name.
    #[clap(short = 'u', long)]
    offline_usernames: Vec<String>,

    /// Forbid pathfinding to mine blocks to reach its goal
    #[clap(short = 'M', long)]
    no_mining: bool,

    /// Enable looking at the closest player which is no more than N blocks away.
    #[clap(short = 'L', long)]
    look_at_players: Option<u32>,

    /// How many tokio worker threads to use. 0 = Automatic
    #[clap(short = 't', long, default_value = "0")]
    worker_threads: usize,

    /// How many async compute tasks to create (might be pathfinding). 0 = Automatic
    #[clap(short = 'c', long, default_value = "0")]
    compute_threads: usize,

    /// How many tasks to create for Bevys AsyncComputeTaskPool (might be pathfinding). 0 = Automatic
    #[clap(short = 'A', long, default_value = "0")]
    async_compute_threads: usize,

    /// How many tasks to create for IO. 0 = Automatic
    #[clap(short = 'i', long, default_value = "0")]
    io_threads: usize,
}

pub const FOOD_ITEMS: &[Item] = &[
    Item::Apple,
    Item::GoldenApple,
    Item::EnchantedGoldenApple,
    Item::Carrot,
    Item::GoldenCarrot,
    Item::MelonSlice,
    Item::SweetBerries,
    Item::GlowBerries,
    Item::Potato,
    Item::BakedPotato,
    Item::Beetroot,
    Item::DriedKelp,
    Item::Beef,
    Item::CookedBeef,
    Item::Porkchop,
    Item::CookedPorkchop,
    Item::Mutton,
    Item::CookedMutton,
    Item::Chicken,
    Item::CookedChicken,
    Item::Rabbit,
    Item::CookedRabbit,
    Item::Cod,
    Item::CookedCod,
    Item::Salmon,
    Item::CookedSalmon,
    Item::TropicalFish,
    Item::Bread,
    Item::Cookie,
    Item::PumpkinPie,
    Item::MushroomStew,
    Item::BeetrootSoup,
    Item::RabbitStew,
];

static OPTS: Lazy<Opts> = Lazy::new(|| Opts::parse());
static INPUTLINE_QUEUES: Mutex<Vec<Arc<Mutex<VecDeque<String>>>>> = Mutex::new(Vec::new());

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

fn main() -> Result<()> {
    // Parse cli args, handle --help, etc.
    let _ = *OPTS;

    // Setup logging
    if std::env::var("RUST_LOG").is_err() {
        unsafe {
            std::env::set_var("RUST_LOG", "info");
        }
    }

    /*
    let reg = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(!OPTS.no_color)
                .with_thread_names(true),
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
    }*/

    let worker_threads = if OPTS.worker_threads == 0 {
        (OPTS.offline_usernames.len() / 8).max(8)
    } else {
        OPTS.worker_threads
    };
    info!("Worker threads: {worker_threads}");

    let worker_thread_num = AtomicU32::new(1);
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .thread_name_fn(move || {
            format!(
                "Tokio Worker {:pad$}",
                worker_thread_num.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                pad = worker_threads.to_string().len(),
            )
        })
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main())
}

async fn async_main() -> Result<()> {
    if OPTS.no_color {
        info!("Will not use colored chat messages and disabled ansi formatting in console.");
    }

    if OPTS.quiet {
        info!("Will not automatically send any chat commands (workaround for getting kicked because of broken ChatCommand packet).");
    }

    info!("Admins: {}", OPTS.admin.join(", "));
    info!("Logging in...");

    let accounts: Vec<Account> = OPTS
        .offline_usernames
        .iter()
        .map(|username| Account::offline(username))
        .collect();

    // Read input and put in queue
    if std::io::stdin().is_terminal() {
        std::thread::spawn(|| loop {
            let mut line = String::new();
            if let Err(err) = std::io::stdin().read_line(&mut line) {
                warn!("Not accepting any input anymore because reading failed: Err: {err}");
                return;
            }
            let line: &str = line.trim();

            let inputline_queues = INPUTLINE_QUEUES.lock();
            for inputline_queue in inputline_queues.iter() {
                inputline_queue.lock().push_back(line.to_owned());
            }
        });
    }

    let task_opts = TaskPoolOptions {
        // By default, use however many cores are available on the system
        min_total_threads: 1,
        max_total_threads: usize::MAX,

        // Use 25% of cores for IO, at least 1, no more than 4
        io: TaskPoolThreadAssignmentPolicy {
            min_threads: if OPTS.io_threads == 0 {
                (OPTS.offline_usernames.len() / 4).max(4)
            } else {
                OPTS.io_threads
            },
            max_threads: if OPTS.io_threads == 0 {
                (OPTS.offline_usernames.len() / 4).max(4)
            } else {
                OPTS.io_threads
            },
            percent: 0.25,
        },

        // Use 25% of cores for async compute, at least 1, no more than 4
        async_compute: TaskPoolThreadAssignmentPolicy {
            min_threads: if OPTS.async_compute_threads == 0 {
                (OPTS.offline_usernames.len() / 4).max(4)
            } else {
                OPTS.async_compute_threads
            },
            max_threads: if OPTS.async_compute_threads == 0 {
                (OPTS.offline_usernames.len() / 4).max(4)
            } else {
                OPTS.async_compute_threads
            },
            percent: 0.25,
        },

        // Use all remaining cores for compute (at least 1)
        compute: TaskPoolThreadAssignmentPolicy {
            min_threads: if OPTS.compute_threads == 0 {
                (OPTS.offline_usernames.len() / 4).max(4)
            } else {
                OPTS.compute_threads
            },
            max_threads: if OPTS.compute_threads == 0 {
                (OPTS.offline_usernames.len() / 4).max(4)
            } else {
                OPTS.compute_threads
            },
            percent: 1.0, // This 1.0 here means "whatever is left over"
        },
    };
    let builder = azalea::swarm::SwarmBuilder::new_without_plugins()
        .add_plugins(DefaultPlugins.build().disable::<TaskPoolPlugin>())
        .add_plugins(DefaultBotPlugins)
        .add_plugins(DefaultSwarmPlugins)
        .add_plugins(TaskPoolPlugin {
            task_pool_options: task_opts,
        })
        .set_handler(handle_outer)
        .set_swarm_handler(swarm_handle)
        .add_accounts(accounts);
    // if OPTS.no_fall:
    // builder = builder.add_plugins(plugins::nofall::NoFallPlugin::default());
    builder
        .start(OPTS.server_address.as_str())
        .await
        .context("Running bot")?
}

#[derive(Clone, Component)]
pub struct BotState {
    last_dm_handled_at: Arc<Mutex<Option<Instant>>>,
    ticks_since_message: u32,
    message_count: u32,
    inputline_queue: Arc<Mutex<VecDeque<String>>>,
    own_username: Arc<Mutex<Option<String>>>,
}

impl Default for BotState {
    fn default() -> Self {
        let inputline_queue = Arc::new(Mutex::new(VecDeque::<String>::new()));
        INPUTLINE_QUEUES.lock().push(inputline_queue.clone());
        Self {
            last_dm_handled_at: Default::default(),
            ticks_since_message: Default::default(),
            message_count: Default::default(),
            own_username: Arc::new(Mutex::new(None)),
            inputline_queue,
        }
    }
}

async fn handle_outer(bot: Client, event: Event, bot_state: BotState) -> anyhow::Result<()> {
    // Sometimes the component GameProfileComponent is not available early on, causing worker threads to crash
    // Also caching the value for hopefully some performance (not sure, maybe it's worse)
    let username = if let Some(ref username) = *bot_state.own_username.lock() {
        username.clone()
    } else {
        if let Some(username) = bot
            .get_component::<GameProfileComponent>()
            .map(|profile| profile.name.clone())
        {
            *bot_state.own_username.lock() = Some(username.clone());
            username
        } else {
            "<Unknown>".to_owned()
        }
    };

    let span = tracing::info_span!("handle", user = username);
    handle(bot, event, bot_state).instrument(span).await
}

async fn handle(mut bot: Client, event: Event, mut bot_state: BotState) -> anyhow::Result<()> {
    match event {
        Event::Login => {
            info!("Logged in");
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
            } else if message.contains(" whispers: ") {
                let sender = message.split(" ").next().unwrap().to_owned();
                let mut message = message.split(" whispers: ").collect::<Vec<_>>()[1].to_owned();
                if message.ends_with(&sender) {
                    message = message[..(message.len() - sender.len())].to_owned();
                }
                dm = Some((sender, message));
            } else if message.starts_with("<") && message.contains("> ") {
                let sender = message[1..].split("> ").next().unwrap().to_owned();
                let mut message = message.split("> ").collect::<Vec<_>>()[1].to_owned();
                if message.contains(&format!("<{sender}> ")) {
                    message = message
                        .split(&format!("<{sender}> "))
                        .next()
                        .unwrap()
                        .to_owned();
                }
                if message.starts_with('!') {
                    dm = Some((sender, message));
                }
            } else if message.starts_with("[") && message.contains("] ") {
                let sender = message[1..].split("] ").next().unwrap().to_owned();
                let mut message = message.split("] ").collect::<Vec<_>>()[1].to_owned();
                if message.contains(&format!("[{sender}] ")) {
                    message = message
                        .split(&format!("[{sender}] "))
                        .next()
                        .unwrap()
                        .to_owned();
                }
                if message.starts_with('!') {
                    dm = Some((sender, message));
                }
            }

            if let Some((sender, content)) = dm {
                let (command, args) = if content.contains(' ') {
                    let mut all_args: Vec<_> = content.split(' ').map(|s| s.to_owned()).collect();
                    let command = all_args.remove(0);
                    (command, all_args)
                } else {
                    (content, vec![])
                };

                if bot_state
                    .last_dm_handled_at
                    .lock()
                    .map(|at| at.elapsed() > Duration::from_secs(1))
                    .unwrap_or(true)
                {
                    info!("Executing command {command:?} sent by {sender:?} with args {args:?}");
                    if commands::execute(&mut bot, &bot_state, sender, command, args)
                        .context("Executing command")?
                    {
                        *bot_state.last_dm_handled_at.lock() = Some(Instant::now());
                    } else {
                        warn!("Command was not executed. Most likely an unknown command.");
                    }
                } else {
                    warn!("Last command was handled less than a second ago. Ignoring command from {sender:?} to avoid getting spam kicked.");
                }
            }
        }
        Event::Tick => {
            // Chat message test
            bot_state.ticks_since_message += 1;
            if bot_state.ticks_since_message >= 40 {
                bot_state.ticks_since_message = 0;
                bot.send_chat_packet(&format!("Hello {}!", bot_state.message_count));
                bot_state.message_count += 1;
                info!("Sent message");
            }

            // Execute commands from input queue
            {
                let mut queue = bot_state.inputline_queue.lock();
                while let Some(line) = queue.pop_front() {
                    if line.starts_with("/") {
                        info!("Sending command: {line}");
                        bot.send_command_packet(&format!("{}", &line[1..]));
                    } else if line.starts_with("!") {
                        let mut args = line.split(" ").map(|a| a.to_owned()).collect::<Vec<_>>();
                        let command = args.remove(0);
                        if let Err(err) = commands::execute(
                            &mut bot,
                            &bot_state,
                            "<Console>".to_owned(),
                            command,
                            args,
                        ) {
                            error!("Failed to execute command from console: {err}");
                        }
                    } else {
                        info!("Sending chat message: {line}");
                        bot.send_chat_packet(&line);
                    }
                }
            }

            // Look at players
            if let Some(max_dist) = OPTS.look_at_players {
                let is_pathfinding = {
                    let mut ecs = bot.ecs.lock();
                    let pathfinder: &Pathfinder = ecs
                        .query::<&Pathfinder>()
                        .get_mut(&mut *ecs, bot.entity)
                        .unwrap();
                    pathfinder.goal.is_some()
                };

                if !is_pathfinding {
                    let my_entity_id = bot.entity_component::<MinecraftEntityId>(bot.entity).0;
                    let my_pos = bot.entity_component::<Position>(bot.entity);
                    let my_eye_height = *bot.entity_component::<EyeHeight>(bot.entity) as f64;
                    let my_eye_pos = *my_pos + Vec3::new(0f64, my_eye_height, 0f64);

                    let mut closest_eye_pos = None;
                    let mut closest_dist_sqrt = f64::MAX;
                    let mut query =
                        bot.ecs
                            .lock()
                            .query::<(&Player, &Position, &EyeHeight, &Pose, &MinecraftEntityId)>();
                    for (_player, pos, eye_height, pose, entity_id) in query.iter(&bot.ecs.lock()) {
                        if entity_id.0 == my_entity_id {
                            continue;
                        }

                        let y_offset = match pose {
                            Pose::FallFlying | Pose::Swimming | Pose::SpinAttack => 0.5f64,
                            Pose::Sleeping => 0.25f64,
                            Pose::Sneaking => (**eye_height as f64) * 0.85,
                            _ => **eye_height as f64,
                        };
                        let eye_pos = **pos + Vec3::new(0f64, y_offset, 0f64);
                        let dist_sqrt = my_eye_pos.distance_squared_to(Vec3::from(pos));
                        if (closest_eye_pos.is_none() || dist_sqrt < closest_dist_sqrt)
                            && dist_sqrt <= (max_dist * max_dist) as f64
                        {
                            closest_eye_pos = Some(eye_pos);
                            closest_dist_sqrt = dist_sqrt;
                        }
                    }

                    if let Some(eye_pos) = closest_eye_pos {
                        bot.look_at(eye_pos);
                    }
                }
            }

            /*
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
                            let goal = BlockPosGoal(azalea::BlockPos {
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

                        let bot_state = bot_state.clone();
                        tokio::spawn(async move {
                            match bot_state.save_stasis().await {
                                Ok(_) => info!("Saved remembered trapdoor positions to file."),
                                Err(err) => error!(
                                    "Failed to save remembered trapdoor positions to file: {err:?}"
                                ),
                            }
                        });
                    }
                }
            }*/
        }
        _ => {}
    }

    Ok(())
}

#[derive(Clone, Component, Resource)]
pub struct SwarmState {}

impl Default for SwarmState {
    fn default() -> Self {
        Self {}
    }
}

async fn swarm_rejoin(swarm: Swarm, state: SwarmState, account: Account, join_opts: JoinOpts) {
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
            tokio::spawn(
                swarm_rejoin(
                    swarm.clone(),
                    state.clone(),
                    (*account).clone(),
                    join_opts.clone(),
                )
                .instrument(span!(
                    Level::INFO,
                    "swarm_rejoin",
                    user = &account.username
                )),
            );
        }
        _ => {}
    }

    Ok(())
}
