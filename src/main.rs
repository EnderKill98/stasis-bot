#![feature(let_chains)]

pub mod commands;

#[macro_use]
extern crate tracing;

use anyhow::{Context, Result};
use azalea::app::PluginGroup;
use azalea::auth::game_profile::GameProfile;
use azalea::entity::metadata::{AbstractLivingUsingItem, Health};
use azalea::entity::LookDirection;
use azalea::inventory::components::Food;
use azalea::inventory::{Inventory, ItemStackData};
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::s_interact::InteractionHand;
use azalea::protocol::packets::game::s_use_item_on::BlockHit;
use azalea::protocol::packets::game::{
    ClientboundGamePacket, ServerboundGamePacket, ServerboundSetCarriedItem, ServerboundSwing,
    ServerboundUseItem, ServerboundUseItemOn,
};
use azalea::registry::EntityKind;
use azalea::swarm::DefaultSwarmPlugins;
use azalea::task_pool::{TaskPoolOptions, TaskPoolPlugin, TaskPoolThreadAssignmentPolicy};
use azalea::{
    auth::AuthResult,
    blocks::Block,
    core::direction::Direction,
    ecs::query::With,
    entity::{metadata::Player, EyeHeight, Pose, Position},
    inventory::SetSelectedHotbarSlotEvent,
    pathfinder::{goals::BlockPosGoal, Pathfinder},
    prelude::*,
    registry::Item,
    swarm::{Swarm, SwarmEvent},
    world::MinecraftEntityId,
    DefaultBotPlugins, DefaultPlugins, GameProfileComponent, Hunger, JoinOpts, Vec3,
};
use bevy_log::LogPlugin;
use clap::Parser;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::{
    collections::{HashMap, VecDeque},
    fmt::Debug,
    io::IsTerminal,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing_subscriber::prelude::*;

#[allow(dead_code)]
pub const EXITCODE_OTHER: i32 = 1; // Due to errors returned and propagated to main result
pub const EXITCODE_CONFLICTING_CLI_OPTS: i32 = 2;
pub const EXITCODE_AUTH_FAILED: i32 = 3;
pub const EXITCODE_NO_ACCESS_TOKEN: i32 = 4;

pub const EXITCODE_USER_REQUESTED_STOP: i32 = 20; // Using an error code to prevent automatic relaunching in some configurations or scripts
pub const EXITCODE_LOW_HEALTH_OR_TOTEM_POP: i32 = 69;

/// A simple stasis bot, using azalea!
#[derive(Parser)]
#[clap(author, version)]
struct Opts {
    /// What server ((and port) to connect to
    server_address: String,

    /// Player names who are considered more trustworthy for certain commands
    #[clap(short, long)]
    admin: Vec<String>,

    /// Automatically log out, once getting to this HP or lower (or a totem pops)
    #[clap(short = 'H', long)]
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

    /// Allow chat messages to be signed.
    #[clap(short = 's', long)]
    sign_chat: bool,

    /// Use an offline account with specified user name.
    #[clap(long)]
    offline_username: Option<String>,

    /// Forbid pathfinding to mine blocks to reach its goal
    #[clap(short = 'M', long)]
    no_mining: bool,

    /// Enable looking at the closest player which is no more than N blocks away.
    #[clap(short = 'L', long)]
    look_at_players: Option<u32>,

    /// Enable a command, that allows admins to get the position of the bot. Might be dangerous!
    #[clap(long)]
    enable_pos_command: bool,

    /// Enables Automatic Eating food items in hotbar, when appropriate
    #[clap(long)]
    auto_eat: bool,

    /// File, used to store authentication information in. Ignored if --offline-username is used.
    #[clap(long, default_value = "login-secrets.json")]
    auth_file: PathBuf,

    /// Only print access token, then quit. Fancy account refresher for something else.
    #[clap(long)]
    just_print_access_token: bool,

    /// 2b Anti AFK
    #[clap(long)]
    periodic_swing: bool,

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
static INPUTLINE_QUEUE: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

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
            std::env::set_var("RUST_LOG", "info,azalea::pathfinder=warn");
        }
    }

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
    }

    let worker_threads = if OPTS.worker_threads == 0 {
        4
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
    if !OPTS.just_print_access_token {
        if OPTS.offline_username.is_none() {
            info!(
                "File used to store Authentication information: {:?}",
                OPTS.auth_file
            );
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

        if OPTS.no_stasis {
            info!("Will not perform any stasis duties!");
        }

        if OPTS.enable_pos_command {
            info!("The command !pos has been enabled for admins!");
        }

        if OPTS.auto_eat {
            info!("Automatic Eating is enabled.");
        }

        info!("Admins: {}", OPTS.admin.join(", "));
        info!("Logging in...");
    }

    let mut account = if let Some(offline_username) = &OPTS.offline_username {
        if OPTS.just_print_access_token {
            error!("Can't print an access token for an offline account!");
            std::process::exit(EXITCODE_CONFLICTING_CLI_OPTS);
        }
        info!("Using an offline account with username {offline_username:?}!");
        Account::offline(&offline_username)
    } else {
        let auth_result = match auth().await {
            Ok(result) => result,
            Err(err) => {
                error!("Quiting because failed to authenticate: {err:?}");
                std::process::exit(EXITCODE_AUTH_FAILED);
            }
        };
        azalea::Account {
            username: auth_result.profile.name,
            access_token: Some(Arc::new(Mutex::new(auth_result.access_token))),
            uuid: Some(auth_result.profile.id),
            account_opts: azalea::AccountOpts::Microsoft {
                email: "default".to_owned(),
            },
            // we don't do chat signing by default unless the user asks for it
            certs: None,
        }
    };

    // Print access token and exit, if requested
    if OPTS.just_print_access_token {
        if let Some(access_token) = account.access_token {
            println!("{}", access_token.lock());
            std::process::exit(0);
        } else {
            error!("Failed to find access token!");
            std::process::exit(EXITCODE_NO_ACCESS_TOKEN);
        }
    }

    if OPTS.sign_chat {
        account
            .request_certs()
            .await
            .context("Request certs for chat signing")?;
        info!("Chat signing is enabled. Retreived certs for it.");
    }

    info!(
        "Logged in as {}. Connecting to \"{}\"...",
        account.username, OPTS.server_address
    );

    // Read input and put in queue
    if std::io::stdin().is_terminal() {
        std::thread::spawn(|| loop {
            let mut line = String::new();
            if let Err(err) = std::io::stdin().read_line(&mut line) {
                warn!("Not accepting any input anymore because reading failed: Err: {err}");
                return;
            }
            let line: &str = line.trim();

            INPUTLINE_QUEUE.lock().push_back(line.to_owned());
        });
    }

    let account_count = 1;
    let task_opts = TaskPoolOptions {
        // By default, use however many cores are available on the system
        min_total_threads: 1,
        max_total_threads: usize::MAX,

        // Use 25% of cores for IO, at least 1, no more than 4
        io: TaskPoolThreadAssignmentPolicy {
            min_threads: if OPTS.io_threads == 0 {
                (account_count / 12).max(2)
            } else {
                OPTS.io_threads
            },
            max_threads: if OPTS.io_threads == 0 {
                (account_count / 12).max(2)
            } else {
                OPTS.io_threads
            },
            percent: 0.25,
        },

        // Use 25% of cores for async compute, at least 1, no more than 4
        async_compute: TaskPoolThreadAssignmentPolicy {
            min_threads: if OPTS.async_compute_threads == 0 {
                account_count.max(4)
            } else {
                OPTS.async_compute_threads
            },
            max_threads: if OPTS.async_compute_threads == 0 {
                account_count.max(4)
            } else {
                OPTS.async_compute_threads
            },
            percent: 0.25,
        },

        // Use all remaining cores for compute (at least 1)
        compute: TaskPoolThreadAssignmentPolicy {
            min_threads: if OPTS.compute_threads == 0 {
                (account_count / 12).max(2)
            } else {
                OPTS.compute_threads
            },
            max_threads: if OPTS.compute_threads == 0 {
                (account_count / 12).max(2)
            } else {
                OPTS.compute_threads
            },
            percent: 1.0, // This 1.0 here means "whatever is left over"
        },
    };
    let builder = azalea::swarm::SwarmBuilder::new_without_plugins()
        .add_plugins(
            DefaultPlugins
                .build()
                .disable::<TaskPoolPlugin>()
                .disable::<LogPlugin>(),
        )
        .add_plugins(DefaultBotPlugins)
        .add_plugins(DefaultSwarmPlugins)
        .add_plugins(TaskPoolPlugin {
            task_pool_options: task_opts,
        })
        .set_handler(handle)
        .set_swarm_handler(swarm_handle)
        .add_account(account.clone());
    builder
        .start(OPTS.server_address.as_str())
        .await
        .context("Running swarm")?
}

async fn auth() -> Result<AuthResult> {
    Ok(azalea::auth::auth(
        "default",
        azalea::auth::AuthOpts {
            cache_file: Some(OPTS.auth_file.clone()),
            ..Default::default()
        },
    )
    .await?)
}

#[derive(Debug, Copy, Clone)]
enum EatingProgress {
    NotEating,
    StartedEating,
    IsEating,
}

#[derive(Clone, Component)]
pub struct BotState {
    remembered_trapdoor_positions: Arc<Mutex<HashMap<String, BlockPos>>>,
    pathfinding_requested_by: Arc<Mutex<Option<String>>>,
    return_to_after_pulled: Arc<Mutex<Option<azalea::Vec3>>>,
    last_dm_handled_at: Arc<Mutex<Option<Instant>>>,
    eating_progress: Arc<Mutex<(Instant /* Last updated at*/, EatingProgress)>>,
    ticks_since_last_swing: Arc<AtomicU32>,
    visual_range_cache: Arc<Mutex<HashMap<MinecraftEntityId, GameProfile>>>,
}

impl Default for BotState {
    fn default() -> Self {
        Self {
            remembered_trapdoor_positions: Default::default(),
            pathfinding_requested_by: Default::default(),
            return_to_after_pulled: Default::default(),
            last_dm_handled_at: Default::default(),
            eating_progress: Arc::new(Mutex::new((Instant::now(), EatingProgress::NotEating))),
            ticks_since_last_swing: Default::default(),
            visual_range_cache: Default::default(),
        }
    }
}

impl BotState {
    pub fn remembered_trapdoor_positions_path() -> PathBuf {
        PathBuf::from("remembered-trapdoor-positions.json")
    }

    pub async fn load_stasis(&mut self) -> Result<()> {
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

    pub async fn save_stasis(&self) -> Result<()> {
        let json =
            serde_json::to_string_pretty(&*self.remembered_trapdoor_positions.as_ref().lock())
                .context("Convert remembered_trapdoor_positions to json")?;
        tokio::fs::write(Self::remembered_trapdoor_positions_path(), json)
            .await
            .context("Save remembered_trapdoor_positions as file")?;
        Ok(())
    }

    pub fn find_food_in_hotbar(&self, bot: &mut Client) -> Option<(u8, ItemStackData)> {
        let mut eat_item = None;

        // Find first food item in hotbar
        let inv = bot.entity_component::<Inventory>(bot.entity);
        let inv_menu = inv.inventory_menu;
        for (hotbar_slot, slot) in inv_menu.hotbar_slots_range().enumerate() {
            if let Some(item_stack_data) = inv_menu.slot(slot).and_then(|stack| stack.as_present())
            {
                if item_stack_data.components.get::<Food>().is_some()
                    || FOOD_ITEMS.contains(&item_stack_data.kind)
                {
                    eat_item = Some((hotbar_slot as u8, item_stack_data.to_owned()));
                    break;
                }
            }
        }

        eat_item
    }

    pub fn is_interacting(&self, bot: &mut Client) -> bool {
        bot.component::<AbstractLivingUsingItem>().0 // (part of AbstractLivingMetadataBundle)
    }

    /*
    fn next_sequence(&self, bot: &mut Client) -> u32 {
        /*let sequence = bot.entity_component::<&mut CurrentSequenceNumber>(bot.entity);
         **sequence += 1;
         *sequence as u32*/
        let mut ecs = bot.ecs.lock();
        let mut state = ecs.query::<&QueryData<(&mut CurrentSequenceNumber)>>();
        let query_result = state.get(&ecs, bot.entity);
        match query_result {
            Ok(query_result) => {
                let seq = **query_result;
            }
            Err(err) => {
                error!("Expected bot to have component CurrentSequenceNumber: {err:?}");
            }
        }
        //let seq = *current_sequence;

        todo!()
    }*/

    pub fn attempt_eat(&self, bot: &mut Client) -> bool {
        if let Some((eat_hotbar_slot, eat_item_stack_data)) = self.find_food_in_hotbar(bot) {
            // Switch to slot and start eating
            let look_direction = bot.component::<LookDirection>();
            let mut ecs = bot.ecs.lock();
            let entity = bot.entity;

            ecs.send_event(SetSelectedHotbarSlotEvent {
                entity,
                slot: eat_hotbar_slot,
            });
            ecs.send_event(SendPacketEvent {
                sent_by: entity,
                packet: ServerboundGamePacket::SetCarriedItem(ServerboundSetCarriedItem {
                    slot: eat_hotbar_slot as u16,
                }),
            });

            ecs.send_event(SendPacketEvent {
                sent_by: entity,
                packet: ServerboundGamePacket::UseItem(ServerboundUseItem {
                    hand: InteractionHand::MainHand,
                    sequence: 0,
                    yaw: look_direction.y_rot,
                    pitch: look_direction.x_rot,
                }),
            });

            info!(
                "Starting to eat item in hotbar slot {slot} ({count}x {kind})...",
                kind = eat_item_stack_data.kind,
                count = eat_item_stack_data.count,
                slot = eat_hotbar_slot,
            );
            true
        } else {
            false
        }
    }

    pub fn eat_tick(&self, bot: &mut Client) {
        let mut eating_progress = self.eating_progress.lock();

        match eating_progress.1.clone() {
            EatingProgress::NotEating => {
                let health = bot.component::<Health>();
                let hunger = bot.component::<Hunger>();

                let should_eat = (hunger.food <= 20 - (3 * 2)
                    || (health.0 < 20f32 && hunger.food < 20))
                    && hunger.saturation <= 0.0;

                if OPTS.auto_eat
                    && should_eat
                    && eating_progress.0.elapsed() > Duration::from_millis(500)
                {
                    if self.attempt_eat(bot) {
                        *eating_progress = (Instant::now(), EatingProgress::StartedEating);
                    }
                }
            }
            EatingProgress::StartedEating => {
                if eating_progress.0.elapsed() > Duration::from_secs(3) {
                    warn!("Attempted to eat, but it failed (no interacting detected more than 3s later)!");
                    *eating_progress = (Instant::now(), EatingProgress::NotEating);
                } else if self.is_interacting(bot) {
                    *eating_progress = (Instant::now(), EatingProgress::IsEating);
                    info!("Eating in progress...");
                }
            }
            EatingProgress::IsEating => {
                if eating_progress.0.elapsed() > Duration::from_secs(15) {
                    warn!("Eating took too long! Perhaps interaction got confused with another action, the server is seriously lagging or eating a modified food item that takes forever (in which case ignore this)!");
                    *eating_progress = (Instant::now(), EatingProgress::NotEating);
                } else if !self.is_interacting(bot) {
                    *eating_progress = (Instant::now(), EatingProgress::NotEating);
                    info!("Successfully finished eating!");
                }
            }
        }
    }
}

async fn handle(mut bot: Client, event: Event, mut bot_state: BotState) -> anyhow::Result<()> {
    match event {
        Event::Login => {
            if !OPTS.no_stasis {
                info!("Loading remembered trapdoor positions...");
                bot_state.load_stasis().await?;
            }
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
                // Common format used by Essentials and other custom plugins: [Someone -> me] Message
                dm = Some((
                    message.split(" ").next().unwrap()[1..].to_owned(),
                    message.split("-> me] ").collect::<Vec<_>>()[1].to_owned(),
                ));
            } else if message.contains(" whispers to you: ") {
                // Vanilla minecraft: Someone whispers to you: Message
                dm = Some((
                    message.split(" ").next().unwrap().to_owned(),
                    message.split("whispers to you: ").collect::<Vec<_>>()[1].to_owned(),
                ));
            } else if message.contains(" whispers: ") {
                // Used on 2b2t: Someone whispers: Message
                let sender = message.split(" ").next().unwrap().to_owned();
                let mut message = message.split(" whispers: ").collect::<Vec<_>>()[1].to_owned();
                if message.ends_with(&sender) {
                    message = message[..(message.len() - sender.len())].to_owned();
                }
                dm = Some((sender, message));
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
                    match commands::execute(&mut bot, &bot_state, sender.clone(), command, args) {
                        Ok(executed) => {
                            if executed {
                                *bot_state.last_dm_handled_at.lock() = Some(Instant::now());
                            } else {
                                warn!("Command was not executed. Most likely an unknown command.");
                            }
                        }
                        Err(err) => {
                            commands::send_command(&mut bot, &format!("msg {sender} Oops: {err}"));
                        }
                    }
                } else {
                    warn!("Last command was handled less than a second ago. Ignoring command from {sender:?} to avoid getting spam kicked.");
                }
            }
        }
        Event::Packet(packet) => match packet.as_ref() {
            ClientboundGamePacket::AddEntity(packet) => {
                if !OPTS.no_stasis && packet.entity_type == EntityKind::EnderPearl {
                    let owning_player_entity_id = packet.data as i32;
                    let mut bot = bot.clone();
                    let entity = bot.entity_by::<With<Player>, (&MinecraftEntityId,)>(
                        |(entity_id,): &(&MinecraftEntityId,)| {
                            entity_id.0 == owning_player_entity_id
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
                                        .save_stasis()
                                        .await {
                                        Ok(_) => info!("Saved remembered trapdoor positions to file."),
                                        Err(err) => error!("Failed to save remembered trapdoor positions to file: {err:?}"),
                                    }
                                });
                            }
                        }
                    }
                } else if packet.entity_type == EntityKind::Player {
                    match bot.tab_list().get(&packet.uuid) {
                        Some(player_info) => {
                            info!(
                                "{} ({}) entered visual range!",
                                player_info.profile.name, player_info.uuid
                            );
                            bot_state
                                .visual_range_cache
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
                    if let Some(profile) = bot_state.visual_range_cache.lock().remove(entity_id) {
                        info!("{} ({}) left visual range!", profile.name, profile.uuid)
                    } else {
                        warn!(
                            "A player with no known GameProfile (id: {}) left visual range!",
                            entity_id.0
                        )
                    }
                }
            }
            ClientboundGamePacket::EntityEvent(packet) => {
                let my_entity_id = bot.entity_component::<MinecraftEntityId>(bot.entity).0;
                if packet.entity_id.0 == my_entity_id && packet.event_id == 35 {
                    // Totem popped!
                    info!("I popped a Totem!");
                    if OPTS.autolog_hp.is_some() {
                        warn!("Disconnecting and quitting because --autolog-hp is enabled...");
                        bot.disconnect();
                        std::process::exit(EXITCODE_LOW_HEALTH_OR_TOTEM_POP);
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
                        std::process::exit(EXITCODE_LOW_HEALTH_OR_TOTEM_POP);
                    }
                }
            }
            _ => {}
        },
        Event::Tick => {
            bot_state.eat_tick(&mut bot);

            // 2b2t Anti AFK
            if OPTS.periodic_swing {
                let mut ticks = bot_state.ticks_since_last_swing.load(Ordering::Relaxed);
                ticks += 1;
                if ticks > 20 * 30 {
                    ticks = 0;
                    bot.ecs.lock().send_event(SendPacketEvent {
                        sent_by: bot.entity,
                        packet: ServerboundGamePacket::Swing(ServerboundSwing {
                            hand: InteractionHand::MainHand,
                        }),
                    });
                }
                bot_state
                    .ticks_since_last_swing
                    .store(ticks, Ordering::Relaxed);
            }

            // Execute commands from input queue
            {
                let mut queue = INPUTLINE_QUEUE.lock();
                while let Some(line) = queue.pop_front() {
                    if line.starts_with("/") {
                        info!("Sending command: {line}");
                        bot.send_command_packet(&format!("{}", &line[1..]));
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
                        let dist_sqrt = my_eye_pos.distance_squared_to(pos);
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
            }
        }
        _ => {}
    }

    Ok(())
}

#[derive(Clone, Component, Resource)]
pub struct SwarmState {
    last_account_refresh: Arc<Mutex<Instant>>,
}

impl Default for SwarmState {
    fn default() -> Self {
        Self {
            last_account_refresh: Arc::new(Mutex::new(Instant::now())),
        }
    }
}

async fn swarm_rejoin(swarm: Swarm, state: SwarmState, account: Account, join_opts: JoinOpts) {
    let mut reconnect_after_secs = 5;
    loop {
        let last_refreshed = state.last_account_refresh.lock().elapsed();
        if last_refreshed > Duration::from_secs(/*3h*/ 60 * 60 * 3)
            && let Some(access_token) = account.access_token.clone()
        {
            info!("This account's access token is more than hours old. Refreshing it!");
            let auth_result = auth().await;
            match auth_result {
                Ok(result) => {
                    info!("Got new access token!");
                    *access_token.lock() = result.access_token;
                    *state.last_account_refresh.lock() = Instant::now();
                }
                Err(err) => {
                    error!("Quitting, because could not get new access token: {err:?}");
                    std::process::exit(EXITCODE_AUTH_FAILED);
                }
            }
        }

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
