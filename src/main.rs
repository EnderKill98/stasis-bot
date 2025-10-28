#![feature(error_generic_member_access)]
#![feature(map_try_insert)]

mod blockpos_string;
pub mod commands;
pub mod devnet;
pub mod module;
pub mod nbs_format;
pub mod task;
pub mod util;

#[macro_use]
extern crate tracing;

use crate::blockpos_string::BlockPosString;
use crate::module::Module;
use crate::module::autoeat::AutoEatModule;
use crate::module::chat::ChatModule;
use crate::module::devnet_handler::DevNetIntegrationModule;
use crate::module::disc_jockey::DiscJockeyModule;
use crate::module::emergency_quit::EmergencyQuitModule;
use crate::module::legacy_stasis::LegacyStasisModule;
use crate::module::look_at_players::LookAtPlayersModule;
use crate::module::periodic_swing::PeriodicSwingModule;
use crate::module::server_tps::ServerTpsModule;
use crate::module::soundness::SoundnessModule;
use crate::module::stasis::StasisModule;
use crate::module::visual_range::VisualRangeModule;
use crate::module::webhook::WebhookModule;
use crate::task::group::TaskGroup;
use crate::task::{Task, TaskOutcome};
use anyhow::{Context, Result};
use azalea::app::PluginGroup;
use azalea::entity::Position;
use azalea::swarm::DefaultSwarmPlugins;
use azalea::task_pool::{TaskPoolOptions, TaskPoolPlugin, TaskPoolThreadAssignmentPolicy};
use azalea::{
    DefaultBotPlugins, DefaultPlugins, JoinOpts,
    auth::AuthResult,
    prelude::*,
    swarm::{Swarm, SwarmEvent},
};
use bevy_log::LogPlugin;
use clap::Parser;
use logroller::{Compression, LogRollerBuilder, Rotation, RotationAge, TimeZone};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{
    collections::VecDeque,
    io::IsTerminal,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use strip_ansi_escapes::Writer as StripAnsiWriter;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

#[allow(dead_code)]
pub const EXITCODE_OTHER: i32 = 1; // Due to errors returned and propagated to main result
pub const EXITCODE_CONFLICTING_CLI_OPTS: i32 = 2;
pub const EXITCODE_AUTH_FAILED: i32 = 3;
pub const EXITCODE_NO_ACCESS_TOKEN: i32 = 4;
pub const EXITCODE_PATHFINDER_DEADLOCKED: i32 = 5;
pub const EXITCODE_USER_REQUESTED_STOP: i32 = 20; // Using an error code to prevent automatic relaunching in some configurations or scripts
pub const EXITCODE_LOW_HEALTH_OR_TOTEM_POP: i32 = 69;

/// A simple stasis bot, using azalea!
#[derive(Parser)]
#[clap(author, version)]
struct Opts {
    /// What server (and port) to connect to
    server_address: String,

    /// Player names who are considered more trustworthy for certain commands
    #[clap(short, long)]
    admin: Vec<String>,

    /// Automatically logout/quit, once getting to this HP or lower (or popping a totem)
    #[clap(short = 'H', long, alias = "autolog-hp")]
    emergency_quit: Option<f32>,

    /// Workaround for crashes: Forbid the bot from sending any messages to players.
    #[clap(short = 'q', long)]
    quiet: bool,

    /// Make the stasis bot, not do any stasis-related duties. So you can abuse him easier as an afk bot.
    #[clap(short = 'S', long)]
    no_stasis: bool,

    /// Specify a simple, single logfile to log everything into as well
    #[clap(short = 'l', long)]
    log_file: Option<PathBuf>,

    /// Specify a directory to put automatically rotated and compressed logfiles into
    #[clap(long)]
    log_directory: Option<PathBuf>,

    /// Use a different filter in log files (instead of RUST_LOG, but same syntax)
    #[clap(long)]
    log_filter: Option<String>,

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

    /// Which key to use within the auth-file
    #[clap(long, default_value = "default")]
    auth_file_key: String,

    /// Only print access token, then quit. Fancy account refresher for something else.
    #[clap(long)]
    just_print_access_token: bool,

    /// 2b Anti AFK
    #[clap(long)]
    periodic_swing: bool,

    /*
    /// How many tokio worker threads to use. 0 = Automatic
    #[clap(short = 't', long, default_value = "0")]
    worker_threads: usize,
    */
    /// How many async compute tasks to create (might be pathfinding). 0 = Automatic
    #[clap(short = 'c', long, default_value = "0")]
    compute_threads: usize,

    /// How many tasks to create for Bevys AsyncComputeTaskPool (might be pathfinding). 0 = Automatic
    #[clap(short = 'A', long, default_value = "0")]
    async_compute_threads: usize,

    /// How many tasks to create for IO. 0 = Automatic
    #[clap(short = 'i', long, default_value = "0")]
    io_threads: usize,

    /// Base URL to devnet API
    #[clap(long)]
    devnet_url: Option<String>,

    /// Token to access devnet API (as destination)
    #[clap(long)]
    devnet_access_token: Option<String>,

    /// Don't re-open trapdoor after teleport. Use if teleports fail or you're paranoid.
    #[clap(long)]
    no_trapdoor_reopen: bool,

    /// Allow users to only set a certain amount of pearls
    #[clap(long)]
    max_pearls: Option<u32>,

    #[clap(long, default_value = "/msg")]
    message_command: String,

    #[clap(long)]
    reply_command: Option<String>,

    /// Attempt to bypass anti-spam measures
    #[clap(long)]
    anti_anti_spam: bool,

    /// If a pearl spawns below this position, it will get ignored (X,Y,Z)
    #[clap(long)]
    pearls_min_pos: Option<BlockPosString>,

    /// If a pearl spawns above this position, it will get ignored (X,Y,Z)
    #[clap(long)]
    pearls_max_pos: Option<BlockPosString>,

    /// Path to folder containing *.nbs song files (enables DiscJockey)
    #[clap(long)]
    songs: Option<PathBuf>,

    /// BlockPos of the noteblocks are to play at (for !dj)
    #[clap(long)]
    dj_pos: Option<blockpos_string::BlockPosString>,

    /// Specifiy to only allow any dj commands from admins (otherwise anyone in render distance can use it)
    #[clap(long)]
    dj_admin_only: bool,

    /// Automatically resume DJ playback when Idle for X seconds (no tasks), goes back if --dj-pos set
    #[clap(long)]
    dj_resume_after_idle_for: Option<usize>,

    #[clap(long)]
    webhook_url: Option<String>,

    #[clap(long)]
    webhook_alert_role_id: Option<u64>,

    #[clap(long)]
    /// Seems that azalea currently has issues getting the correct damage types.
    /// Use this as a workaround on 1.21.4
    use_hardcoded_damage_types: bool,
}

static OPTS: Lazy<Opts> = Lazy::new(|| Opts::parse());
static INPUTLINE_QUEUE: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static DEVNET_RX_QUEUE: Mutex<VecDeque<devnet::Message>> = Mutex::new(VecDeque::new());
static DEVNET_TX: Mutex<Option<tokio::sync::mpsc::Sender<devnet::Message>>> = Mutex::new(None);

fn main() -> Result<()> {
    // Parse cli args, handle --help, etc.
    let _ = *OPTS;

    // Setup logging
    if std::env::var("RUST_LOG").is_err() {
        unsafe {
            std::env::set_var("RUST_LOG", "info,azalea::pathfinder=warn");
        }
    }

    let (non_blocking_stdout, _stdout_guard) = tracing_appender::non_blocking(std::io::stdout());
    let reg = tracing_subscriber::registry().with(
        // Default layer outputting to console/tty
        tracing_subscriber::fmt::layer()
            .with_ansi(!OPTS.no_color)
            .with_thread_names(true)
            .with_writer(non_blocking_stdout)
            .with_filter(EnvFilter::from_default_env()),
    );

    let _file_guard: WorkerGuard; // Extend lifetime of workerguard or logs will be empty due to it being dropped early

    // Log either directly to log file or rotated using logrotate into a directory
    // The filter may be different from default, if --log-filter was specified
    match (&OPTS.log_file, &OPTS.log_directory) {
        (Some(_log_file_path), Some(_log_directory_path)) => {
            eprintln!("Fatal: --log-file and --log-directory are not supported together. Choose one!");
            std::process::exit(EXITCODE_CONFLICTING_CLI_OPTS);
        }
        (None, Some(log_directory_path)) => {
            let appender = LogRollerBuilder::new(log_directory_path, &PathBuf::from("stasis-bot"))
                .rotation(Rotation::AgeBased(RotationAge::Daily))
                .time_zone(TimeZone::UTC)
                .compression(Compression::Gzip)
                .graceful_shutdown(true)
                .suffix("log".to_owned())
                .build()?;

            let (non_blocking, worker_guard) = tracing_appender::non_blocking(StripAnsiWriter::new(appender));
            reg.with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_thread_names(true)
                    .with_writer(non_blocking)
                    .with_filter(if let Some(filter) = &OPTS.log_filter {
                        filter.parse::<EnvFilter>().expect("Parsing --log-filter")
                    } else {
                        EnvFilter::from_default_env()
                    }),
            )
            .init();
            _file_guard = worker_guard;
        }
        (Some(log_file_path), None) => {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_file_path)
                .context("Open logfile for appending")?;
            let (non_blocking, worker_guard) = tracing_appender::non_blocking(StripAnsiWriter::new(file));

            reg.with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_thread_names(true)
                    .with_writer(non_blocking)
                    .with_filter(if let Some(filter) = &OPTS.log_filter {
                        filter.parse::<EnvFilter>().expect("Parsing --log-filter")
                    } else {
                        EnvFilter::from_default_env()
                    }),
            )
            .init();
            _file_guard = worker_guard;
        }
        (None, None) => {
            reg.init();
        }
    }

    //let worker_threads = if OPTS.worker_threads == 0 { 4 } else { OPTS.worker_threads };
    //info!("Worker threads: {worker_threads}");

    let worker_thread_num = AtomicU32::new(1);
    tokio::runtime::Builder::new_current_thread() // https://github.com/azalea-rs/azalea/tree/main/azalea#using-a-single-threaded-tokio-runtime
        //.worker_threads(worker_threads) // Does nothing rn
        .thread_name_fn(move || format!("Tokio Worker {}", worker_thread_num.fetch_add(1, std::sync::atomic::Ordering::Relaxed),))
        .enable_all()
        .build()?
        .block_on(async_main())
}

async fn async_main() -> Result<()> {
    if !OPTS.just_print_access_token {
        if OPTS.offline_username.is_none() {
            info!(
                "File used to store authentication information: {:?} (key: {:?})",
                OPTS.auth_file, OPTS.auth_file_key
            );
        }

        if OPTS.no_color {
            info!("Will not use colored chat messages and disabled ansi formatting in console.");
        }

        if OPTS.quiet {
            info!("Will not automatically send any chat commands (workaround for getting kicked because of broken ChatCommand packet).");
        }

        if let Some(emergency_quit) = OPTS.emergency_quit {
            info!("Will automatically logout and quit, when reaching {emergency_quit} HP or popping a totem.");
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

        if OPTS.no_stasis && OPTS.pearls_min_pos.is_some() {
            error!("--pearls-min-pos has no use when --no-stasis is specified!");
            std::process::exit(EXITCODE_CONFLICTING_CLI_OPTS);
        }

        if OPTS.no_stasis && OPTS.pearls_max_pos.is_some() {
            error!("--pearls-max-pos has no use when --no-stasis is specified!");
            std::process::exit(EXITCODE_CONFLICTING_CLI_OPTS);
        }

        if let Some(pearls_min_pos) = &OPTS.pearls_min_pos {
            info!("Ignoring any pearls that spawn at a block pos below: {pearls_min_pos}");
        }

        if let Some(pearls_max_pos) = &OPTS.pearls_max_pos {
            info!("Ignoring any pearls that spawn at a block pos above: {pearls_max_pos}");
        }

        if OPTS.devnet_url.is_some() != OPTS.devnet_access_token.is_some() {
            error!("--devnet-url and --devnet-access-token must both be specified or omitted!");
            std::process::exit(EXITCODE_CONFLICTING_CLI_OPTS);
        }

        info!("Admins: {}", OPTS.admin.join(", "));
        info!("Logging in...");
    }

    // parking_lot deadlock detection:
    // Create a background thread which checks for deadlocks every 10s
    std::thread::Builder::new().name("Deadlock-Checker".to_owned()).spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(30));
            let deadlocks = parking_lot::deadlock::check_deadlock();
            if deadlocks.is_empty() {
                continue;
            }

            error!("{} DEADLOCKS DETECTED!!!", deadlocks.len());
            for (i, threads) in deadlocks.iter().enumerate() {
                error!("Deadlock #{}", i);
                for t in threads {
                    error!("Thread Id {:#?}\n{:#?}", t.thread_id(), t.backtrace());
                }
            }
        }
    })?;

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
        Account {
            username: auth_result.profile.name,
            access_token: Some(Arc::new(Mutex::new(auth_result.access_token))),
            uuid: Some(auth_result.profile.id),
            account_opts: azalea::AccountOpts::Microsoft {
                email: OPTS.auth_file_key.to_owned(),
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
        account.request_certs().await.context("Request certs for chat signing")?;
        info!("Chat signing is enabled. Retreived certs for it.");
    }

    info!("Logged in as {}. Connecting to \"{}\"...", account.username, OPTS.server_address);

    if let Some(ref url) = OPTS.devnet_url
        && let Some(ref access_token) = OPTS.devnet_access_token
    {
        let url = url.to_owned();
        let access_token = access_token.to_owned();
        let (msg_tx_tx, msg_tx_rx) = tokio::sync::mpsc::channel(512);
        //msg_tx_tx.send(devnet::Message::DestinationsRequest).await?;
        *DEVNET_TX.lock() = Some(msg_tx_tx);

        let (msg_rx_tx, mut msg_rx_rx) = tokio::sync::mpsc::channel(512);
        tokio::spawn(async move {
            while let Some(message) = msg_rx_rx.recv().await {
                DEVNET_RX_QUEUE.lock().push_back(message);
            }
            error!("Receiver for devnet messages ended unexpectedly!");
        });
        tokio::spawn(devnet::run_forever(url, access_token, msg_tx_rx, msg_rx_tx));
    }

    // Read input and put in queue
    if std::io::stdin().is_terminal() {
        std::thread::spawn(|| {
            loop {
                let mut line = String::new();
                if let Err(err) = std::io::stdin().read_line(&mut line) {
                    warn!("Not accepting any input anymore because reading failed: Err: {err}");
                    return;
                }
                let line: &str = line.trim();

                INPUTLINE_QUEUE.lock().push_back(line.to_owned());
            }
        });
    }

    let account_count = 1;
    let task_opts = TaskPoolOptions {
        // By default, use however many cores are available on the system
        min_total_threads: 1,
        max_total_threads: usize::MAX,

        // Use 25% of cores for IO, at least 1, no more than 4
        io: TaskPoolThreadAssignmentPolicy {
            min_threads: if OPTS.io_threads == 0 { (account_count / 12).max(2) } else { OPTS.io_threads },
            max_threads: if OPTS.io_threads == 0 { (account_count / 12).max(2) } else { OPTS.io_threads },
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
        .add_plugins(DefaultPlugins.build().disable::<TaskPoolPlugin>().disable::<LogPlugin>())
        .add_plugins(DefaultBotPlugins)
        .add_plugins(DefaultSwarmPlugins)
        .add_plugins(TaskPoolPlugin { task_pool_options: task_opts })
        .set_handler(handle)
        .set_swarm_handler(swarm_handle)
        .add_account(account.clone());
    builder.start(OPTS.server_address.as_str()).await.context("Running swarm")?
}

async fn auth() -> Result<AuthResult> {
    Ok(azalea::auth::auth(
        &OPTS.auth_file_key,
        azalea::auth::AuthOpts {
            cache_file: Some(OPTS.auth_file.clone()),
            ..Default::default()
        },
    )
    .await?)
}

#[derive(Clone, Component)]
pub struct BotState {
    last_dm_handled_at: Arc<Mutex<Option<Instant>>>,
    root_task_group: Arc<Mutex<TaskGroup>>,
    root_task_last_display: Arc<Mutex<String>>,

    auto_eat: Option<AutoEatModule>,
    periodic_swing: Option<PeriodicSwingModule>,
    legacy_stasis: Option<LegacyStasisModule>,
    visual_range: Option<VisualRangeModule>,
    look_at_players: Option<LookAtPlayersModule>,
    soundness: Option<SoundnessModule>,
    emergency_quit: Option<EmergencyQuitModule>,
    devnet_integration: Option<DevNetIntegrationModule>,
    server_tps: Option<ServerTpsModule>,
    stasis: Option<StasisModule>,
    chat: Option<ChatModule>,
    disc_jockey: Option<DiscJockeyModule>,
    webhook: Option<WebhookModule>,
}

fn default_if<T: Default>(enabled: bool) -> Option<T> {
    if enabled { Some(Default::default()) } else { None }
}

impl Default for BotState {
    fn default() -> Self {
        Self {
            last_dm_handled_at: Default::default(),
            root_task_group: Arc::new(Mutex::new(TaskGroup::new_root())),
            root_task_last_display: Default::default(),

            auto_eat: default_if(OPTS.auto_eat),
            periodic_swing: default_if(OPTS.periodic_swing),
            legacy_stasis: default_if(!OPTS.no_stasis), // Just left to migrate locations, does not do anything anymore
            visual_range: Some(Default::default()),
            look_at_players: OPTS.look_at_players.map(|dist| LookAtPlayersModule::new(dist)),
            soundness: Some(Default::default()),
            emergency_quit: OPTS.emergency_quit.map(|hp| EmergencyQuitModule::new(hp)),
            devnet_integration: default_if(OPTS.devnet_url.is_some() && OPTS.devnet_access_token.is_some()),
            server_tps: Some(Default::default()),
            stasis: if !OPTS.no_stasis {
                Some(StasisModule::new(!OPTS.no_trapdoor_reopen, OPTS.max_pearls))
            } else {
                None
            },
            chat: Some(ChatModule::default()),
            disc_jockey: if let Some(songs) = &OPTS.songs {
                Some(DiscJockeyModule::new(songs))
            } else {
                None
            },
            webhook: if let Some(url) = &OPTS.webhook_url {
                Some(WebhookModule::new(url, OPTS.webhook_alert_role_id))
            } else {
                None
            },
        }
    }
}

impl BotState {
    pub fn modules(&self) -> Vec<&dyn Module> {
        let mut modules: Vec<&dyn Module> = vec![];
        if let Some(module) = &self.auto_eat {
            modules.push(module);
        };
        if let Some(module) = &self.webhook {
            modules.push(module);
        };
        if let Some(module) = &self.periodic_swing {
            modules.push(module);
        };
        if let Some(module) = &self.legacy_stasis {
            modules.push(module);
        };
        if let Some(module) = &self.visual_range {
            modules.push(module);
        };
        if let Some(module) = &self.look_at_players {
            modules.push(module);
        };
        if let Some(module) = &self.soundness {
            modules.push(module);
        };
        if let Some(module) = &self.emergency_quit {
            modules.push(module);
        };
        if let Some(module) = &self.devnet_integration {
            modules.push(module);
        };
        if let Some(module) = &self.server_tps {
            modules.push(module);
        };
        if let Some(module) = &self.stasis {
            modules.push(module);
        };
        if let Some(module) = &self.chat {
            modules.push(module);
        };
        if let Some(module) = &self.disc_jockey {
            modules.push(module);
        };
        modules
    }

    pub fn add_task(&self, task: impl Into<Box<dyn Task>>) {
        self.root_task_group.lock().add(task);
    }

    pub fn add_task_now(&self, bot: Client, bot_state: &BotState, task: impl Into<Box<dyn Task>>) -> Result<()> {
        self.root_task_group.lock().add_now(bot.clone(), bot_state, task)?;
        Ok(())
    }

    pub fn tasks(&self) -> u64 {
        self.root_task_group.lock().remaining()
    }

    pub fn webhook_alert(&self, message: impl AsRef<str>) {
        if let Some(webhook) = &self.webhook {
            webhook.webhook_alert(message);
        } else {
            info!("Webhook module not active. Message: {}", message.as_ref());
        }
    }

    pub fn webhook_silent(&self, message: impl AsRef<str>) {
        if let Some(webhook) = &self.webhook {
            webhook.webhook_silent(message);
        } else {
            info!("Webhook module not active. Message: {}", message.as_ref());
        }
    }

    pub fn webhook(&self, message: impl AsRef<str>) {
        if let Some(webhook) = &self.webhook {
            webhook.webhook(message);
        } else {
            info!("Webhook-Message (not enabled): {}", message.as_ref());
        }
    }

    pub fn wait_on_webhooks_and_exit(&self, exit_code: i32) {
        if let Some(webhook) = self.webhook.clone() {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                webhook.queue_open.store(false, Ordering::Relaxed);
                let started = tokio::time::Instant::now();
                loop {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    if webhook.queue.lock().is_empty() && !webhook.sending_message.load(Ordering::Relaxed) {
                        info!("Webhook is done sending messages. Quitting now!");
                        std::process::exit(exit_code);
                    }
                    if started.elapsed() > Duration::from_secs(3) {
                        warn!("Waited too long on webhook. Quitting now!");
                        std::process::exit(exit_code);
                    }
                }
            });
        } else {
            std::process::exit(exit_code);
        }
    }
}

async fn handle(bot: Client, event: Event, bot_state: BotState) -> Result<()> {
    if let Some(ref module) = bot_state.auto_eat {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.webhook {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.periodic_swing {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.legacy_stasis {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.visual_range {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.look_at_players {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.soundness {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.emergency_quit {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.devnet_integration {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.server_tps {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.stasis {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.chat {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.disc_jockey {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }

    // Process task(s)
    {
        let mut task_root = bot_state.root_task_group.lock();
        if task_root.remaining() > 0 {
            if bot.get_component::<Position>().is_none() {
                warn!("Bot has no position associated anymore. Canceling all tasks just in case!");
                task_root.discard(bot.clone(), &bot_state)?;
                *task_root = TaskGroup::new_root();
            } else {
                let mut do_cleanup = false;
                match task_root.handle(bot.clone(), &bot_state, &event).context("Handling root TaskGroup") {
                    Ok(TaskOutcome::Ongoing) => {}
                    Ok(TaskOutcome::Succeeded) => {
                        do_cleanup = true;
                    }
                    Ok(TaskOutcome::Failed { reason }) => {
                        do_cleanup = true;
                        error!("Task Fail: {reason}");
                    }
                    Err(err) => {
                        error!("Got error while handling task {task_root}: {err:?}");
                        bot_state.root_task_last_display.lock().clear();
                        *task_root = TaskGroup::new_root();
                    }
                }

                if do_cleanup {
                    info!("All Tasks done ({}).", task_root.subtasks.len());
                    bot_state.root_task_last_display.lock().clear();
                    *task_root = TaskGroup::new_root();
                } else {
                    let mut last_task_display = bot_state.root_task_last_display.lock();
                    let current_task_display = task_root.to_string();
                    if current_task_display != *last_task_display {
                        info!("Task Status: {current_task_display}");
                        *last_task_display = current_task_display;
                    }
                }
            }
        }
    }

    match event {
        Event::Tick => {
            // Execute commands from input queue
            {
                let mut queue = INPUTLINE_QUEUE.lock();
                while let Some(line) = queue.pop_front() {
                    if let Some(chat) = bot_state.chat.as_ref() {
                        chat.chat(line);
                    } else {
                        if line.starts_with("/") {
                            info!("Sending command: {line}");
                            bot.send_command_packet(&format!("{}", &line[1..]));
                        } else {
                            info!("Sending chat message: {line}");
                            bot.send_chat_packet(&line);
                        }
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
        match swarm.add_with_opts(&account, state.clone(), &join_opts).await {
            Ok(_) => return,
            Err(join_err) => error!("Failed to rejoin: {join_err}"), // Keep rejoining
        }
    }
}

async fn swarm_handle(swarm: Swarm, event: SwarmEvent, state: SwarmState) -> anyhow::Result<()> {
    match event {
        SwarmEvent::Disconnect(account, join_opts) => {
            tokio::spawn(swarm_rejoin(swarm.clone(), state.clone(), (*account).clone(), join_opts.clone()));
        }
        _ => {}
    }

    Ok(())
}
