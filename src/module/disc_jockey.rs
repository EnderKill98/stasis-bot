use crate::BotState;
use crate::module::Module;
use crate::nbs_format::NbsSong;
use crate::task::TaskOutcome;
use crate::task::disc_jockey::{ActualPlaybackStatus, DesiredPlaybackStatus, DiscJockeyTask, PlaybackState};
use crate::task::tracked::{TrackedTask, TrackedTaskStatus};
use anyhow::{Context, bail};
use azalea::ecs::prelude::With;
use azalea::entity::metadata::Player;
use azalea::{Client, Event, GameProfileComponent};
use parking_lot::Mutex;
use rand::Rng;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Queue {
    songs: Vec<Arc<NbsSong>>,
    current: Option<usize>,
    repeat: bool,
    shuffle: bool,
}

impl Queue {
    pub fn current_song(&self) -> Option<Arc<NbsSong>> {
        self.current.and_then(|current| self.songs.get(current).cloned())
    }

    pub fn clear(&mut self) {
        self.songs.clear();
        self.current = None;
    }

    pub fn next(&mut self) {
        if self.songs.is_empty() {
            self.current = None;
            return;
        }

        if let Some(current) = self.current {
            match (self.repeat, self.shuffle) {
                (false, false) => {
                    if current == self.songs.len() - 1 {
                        self.current = None; // Reached end
                    } else {
                        self.current = Some(current + 1);
                    }
                }
                (true, false) => {
                    self.current = Some((current + 1) % self.songs.len());
                }
                (_, true) => {
                    if self.songs.len() == 1 {
                        self.current = Some(0);
                        return; // Can't change song
                    }
                    // Choose new one at random, except for the current one
                    let mut next_current = rand::rng().random_range(0..=self.songs.len() - 1);
                    if next_current >= current {
                        next_current += 1;
                    }
                    self.current = Some(next_current);
                }
            }
        }
    }

    pub fn previous(&mut self) {
        if self.songs.is_empty() {
            self.current = None;
            return;
        }

        if let Some(current) = self.current {
            match (self.repeat, self.shuffle) {
                (false, false) => {
                    if current == 0 {
                        self.current = None; // Reached start
                    } else {
                        self.current = Some(current - 1);
                    }
                }
                (true, false) => {
                    if current == 0 {
                        self.current = Some(self.songs.len() - 1);
                    } else {
                        self.current = Some(current - 1);
                    }
                }
                (_, true) => {
                    self.next(); // Same behavior as next for now
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct DiscJockeyModule {
    song_directory: Arc<PathBuf>,
    initial_load_started: Arc<AtomicBool>,
    is_loading: Arc<AtomicBool>,
    pub songs: Arc<Mutex<Vec<Arc<NbsSong>>>>,
    pub state: Arc<Mutex<PlaybackState>>,
    tracked_task: Arc<Mutex<Option<TrackedTask<DiscJockeyTask>>>>,
    tracked_send_fail_to: Arc<Mutex<Option<String>>>,
    last_task_seen_at: Arc<Mutex<Instant>>,

    queue: Arc<Mutex<Queue>>,
}

impl DiscJockeyModule {
    pub fn new(song_directory: impl AsRef<Path>) -> Self {
        Self {
            song_directory: Arc::new(song_directory.as_ref().to_path_buf()),
            initial_load_started: Arc::new(AtomicBool::new(false)),
            is_loading: Arc::new(AtomicBool::new(false)),
            songs: Arc::new(Mutex::new(Vec::new())),
            state: Arc::new(Mutex::new(PlaybackState::default())),
            tracked_task: Arc::new(Mutex::new(None)),
            tracked_send_fail_to: Arc::new(Mutex::new(None)),
            last_task_seen_at: Arc::new(Mutex::new(Instant::now())),
            queue: Arc::new(Mutex::new(Queue::default())),
        }
    }

    pub fn load_songs(&self) -> anyhow::Result<()> {
        if !self.initial_load_started.load(Ordering::Relaxed) {
            self.initial_load_started.store(true, Ordering::Relaxed);
        }
        if self.is_loading.load(Ordering::Relaxed) {
            bail!("Already loading!")
        }
        let start = Instant::now();
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&*self.song_directory).context("List files")? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                continue;
            }

            files.push(path.to_path_buf());
        }
        let self_clone = self.clone();
        self.is_loading.store(true, Ordering::Relaxed);
        *self.songs.lock() = vec![];
        tokio::spawn(async move {
            let mut succeeded = 0usize;
            let mut failed = 0usize;
            for file in files {
                match NbsSong::from_path_async(&file).await {
                    Ok(song) => {
                        debug!(
                            "Loaded {:?} (unique: {}, notes: {}, duration: {:.02}s)",
                            song.friendly_name(),
                            song.unique.len(),
                            song.notes.len(),
                            song.length_in_seconds()
                        );
                        self_clone.songs.lock().push(Arc::new(song));
                        succeeded += 1;
                    }
                    Err(err) => {
                        warn!("Could not load {file:?}: {err:?}");
                        failed += 1;
                    }
                }
            }

            self_clone.is_loading.store(false, Ordering::Relaxed);
            info!("Loaded {succeeded} NBS Songs ({failed} failed) in {:?}", start.elapsed());
        });
        Ok(())
    }

    pub fn play(&self, bot: &mut Client, bot_state: &BotState, sender: impl AsRef<str>, song: Arc<NbsSong>) {
        {
            let mut queue = self.queue.lock();
            queue.songs.clear();
            queue.songs.push(song.clone());
            queue.current = Some(0);
        }
        self.restart_current_song_in_queue(bot, bot_state, Some(sender));
    }

    pub fn ensure_queue_is_active(&self) {
        let mut queue = self.queue.lock();
        if queue.current.is_none() && !queue.songs.is_empty() {
            queue.current = Some(0);
        }
    }

    pub fn restart_current_song_in_queue(&self, _bot: &mut Client, bot_state: &BotState, sender: Option<impl AsRef<str>>) -> bool {
        {
            let queue = self.queue.lock();
            let mut state = self.state.lock();

            state.desired_status = DesiredPlaybackStatus::Playing;
            state.tick = 0.0;
            state.song = queue.current_song();

            if state.song.is_none() {
                return false;
            }
        }
        self.ensure_task_running(bot_state, sender);
        true
    }

    pub fn ensure_task_running(&self, bot_state: &BotState, triggered_by: Option<impl AsRef<str>>) -> bool {
        *self.tracked_send_fail_to.lock() = triggered_by.map(|str| str.as_ref().to_string());

        let mut tracked_task = self.tracked_task.lock();
        if let Some(task) = tracked_task.as_ref()
            && task.status().is_running()
        {
            info!("Tracked DJ-Task still appears to be running ({:?}). Not restarting it.", task.status());
            false
        } else {
            info!("Tracked DJ-Task is either missing or no longer running. Creating new one.");
            let task = TrackedTask::new(DiscJockeyTask::new(self.state.clone()));
            bot_state.add_task(task.clone());
            *tracked_task = Some(task);
            true
        }
    }

    pub fn format_timestamp(millis: u64, include_fraction: bool) -> String {
        if include_fraction {
            format!("{:02}:{:02}.{}", millis / (1000 * 60), (millis % (1000 * 60)) / 1000, (millis % 1000) / 10)
        } else {
            format!("{:02}:{:02}", millis / (1000 * 60), (millis % (1000 * 60)) / 1000)
        }
    }

    pub fn search(&self, search_term: impl AsRef<str>, in_queue: bool) -> Vec<Arc<NbsSong>> {
        let search_term = search_term.as_ref();

        let all_songs = self.songs.lock();
        let queue = self.queue.lock();
        let songs = if in_queue { &queue.songs } else { &all_songs };
        let mut matches: Vec<Arc<NbsSong>> = vec![];

        let normalize = |str: &str| str.to_lowercase().replace(&[' ', '_', '-', '.', '(', ')', '\'', '"'], "");

        // Exact matches
        for song in songs.iter() {
            if song.friendly_name() == search_term {
                matches.push(song.clone());
            }
        }

        // Exact matches (case-insensitive)
        for song in songs.iter() {
            if song.friendly_name().eq_ignore_ascii_case(search_term) && !matches.contains(song) {
                matches.push(song.clone());
            }
        }

        let ref search_term_lc = search_term.to_lowercase();
        // StartsWith matches
        for song in songs.iter() {
            if song.friendly_name().to_lowercase().starts_with(search_term_lc) && !matches.contains(song) {
                matches.push(song.clone());
            }
        }

        // Contains matches
        for song in songs.iter() {
            if song.friendly_name().to_lowercase().contains(search_term_lc) && !matches.contains(song) {
                matches.push(song.clone());
            }
        }

        let ref search_term_norm = normalize(search_term);
        // StartsWith normalized matches
        for song in songs.iter() {
            if normalize(&song.friendly_name()).starts_with(search_term_norm) && !matches.contains(song) {
                matches.push(song.clone());
            }
        }

        // Contains normalized matches
        for song in songs.iter() {
            if normalize(&song.friendly_name()).contains(search_term_norm) && !matches.contains(song) {
                matches.push(song.clone());
            }
        }

        matches
    }
}

#[async_trait::async_trait]
impl Module for DiscJockeyModule {
    fn name(&self) -> &'static str {
        "DiscJockey"
    }

    async fn handle(&self, mut bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Init => {
                if !self.initial_load_started.load(Ordering::Relaxed) {
                    if let Err(err) = self.load_songs() {
                        error!("Failed to start song loading: {err:?}");
                    }
                }
            }
            Event::Tick => {
                {
                    let mut tracked_task = self.tracked_task.lock();
                    if let Some(play_task) = tracked_task.as_ref() {
                        if let TrackedTaskStatus::Errored { error } = play_task.status()
                            && let Some(send_fail_to) = self.tracked_send_fail_to.lock().take()
                        {
                            if let Some(chat) = &bot_state.chat {
                                info!("Sending tracked task error to {send_fail_to}: {error}");
                                chat.msg(send_fail_to, format!("DJ-Task errored: {error}"));
                            }
                            *tracked_task = None;
                        } else if let TrackedTaskStatus::Concluded { outcome } = play_task.status()
                            && let TaskOutcome::Failed { reason } = outcome
                            && let Some(send_fail_to) = self.tracked_send_fail_to.lock().take()
                        {
                            if let Some(chat) = &bot_state.chat {
                                info!("Sending tracked task fail to {send_fail_to}: {reason}");
                                chat.msg(send_fail_to, format!("DJ-Task failed: {reason}"));
                            }
                            *tracked_task = None;
                        }
                    }
                }

                if self.state.lock().actual_status == ActualPlaybackStatus::Finished && self.queue.lock().current.is_some() {
                    info!("Song finished. Attempting to play next one...");
                    self.queue.lock().next();
                    self.restart_current_song_in_queue(&mut bot, bot_state, None::<&str>);
                }

                if bot_state.tasks() > 0 {
                    *self.last_task_seen_at.lock() = Instant::now();
                }

                if let Some(dj_resume_after_idle_for) = crate::OPTS.dj_resume_after_idle_for
                    && self.state.lock().actual_status == ActualPlaybackStatus::Interrupted
                    && self.state.lock().desired_status == DesiredPlaybackStatus::Playing
                    && self.last_task_seen_at.lock().elapsed() >= Duration::from_secs(dj_resume_after_idle_for as u64)
                {
                    info!("Automatically resuming DJ, because no Task detected for {dj_resume_after_idle_for}s and actual state is interrupted.");
                    self.ensure_task_running(bot_state, None::<&str>);
                    *self.last_task_seen_at.lock() = Instant::now();
                }
            }
            _ => {}
        }
        Ok(())
    }
}

pub async fn execute_dj_command<F: Fn(&str) + Send + Sync + 'static>(
    bot: &mut Client,
    bot_state: &BotState,
    sender: String,
    _command: String,
    args: Vec<String>,
    feedback: F,
    dj: &DiscJockeyModule,
    sender_is_admin: bool,
) -> anyhow::Result<()> {
    let can_see = bot
        .entity_by::<With<Player>, (&GameProfileComponent,)>(|(profile,): &(&GameProfileComponent,)| profile.name == sender)
        .is_some();
    if crate::OPTS.dj_admin_only && !sender_is_admin {
        feedback("You need to be specified as an admin to use this command!");
        return Ok(());
    } else if !can_see && !sender_is_admin {
        feedback("This command can only be used by people in render distance or admins.");
        return Ok(());
    }

    let subcommand_lc = if args.is_empty() { "".to_owned() } else { args[0].to_lowercase() };
    match subcommand_lc.as_str() {
        "play" => {
            if args.len() == 1 {
                feedback("Please provide a song to play. It will behave like a search and play the first result.");
                return Ok(());
            }

            let search_term = args.iter().skip(1).map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
            let matches = dj.search(search_term, false);
            if matches.is_empty() {
                feedback("No song found that matches your term.");
                return Ok(());
            }

            let song = matches[0].clone();
            feedback(&format!(
                "Playing: {:?} [{}]",
                song.friendly_name(),
                DiscJockeyModule::format_timestamp(song.ticks_to_millis(song.length_ticks as f64).floor() as u64, false)
            ));
            dj.play(bot, bot_state, &sender, song.clone());
            return Ok(());
        }
        "playrandom" => {
            let songs = dj.songs.lock();
            if songs.is_empty() {
                feedback("No songs :(");
                return Ok(());
            }

            let song = &songs[rand::rng().random_range(0..songs.len())];
            feedback(&format!(
                "Playing: {:?} [{}]",
                song.friendly_name(),
                DiscJockeyModule::format_timestamp(song.ticks_to_millis(song.length_ticks as f64).floor() as u64, false)
            ));
            dj.play(bot, bot_state, &sender, song.clone());
            return Ok(());
        }
        "search" => {
            if args.len() == 1 {
                feedback("Please provide a search term.");
                return Ok(());
            }

            let search_term = args.iter().skip(1).map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
            let matches = dj.search(search_term, false);
            if matches.is_empty() {
                feedback("No matching songs found. Maybe try to generalize your term a bit more.");
                return Ok(());
            }
            feedback(&format!(
                "{} results: {}",
                matches.len(),
                matches.iter().map(|s| s.friendly_name()).collect::<Vec<_>>().join(", ")
            ));
            return Ok(());
        }
        "add" => {
            if args.len() == 1 {
                feedback("Please provide a song to add. It will behave like a search and play this song after all the others.");
                return Ok(());
            }

            let search_term = args.iter().skip(1).map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
            let matches = dj.search(search_term, false);
            if matches.is_empty() {
                feedback("No song found that matches your term.");
                return Ok(());
            }

            let song = matches[0].clone();
            dj.queue.lock().songs.push(song.clone());
            if dj.queue.lock().songs.len() == 1 {
                info!("First song added to queue. Starting it...");
                dj.ensure_queue_is_active();
                dj.restart_current_song_in_queue(bot, bot_state, Some(sender));
            }
            feedback(&format!(
                "Added to queue: {:?} [{}]",
                song.friendly_name(),
                DiscJockeyModule::format_timestamp(song.ticks_to_millis(song.length_ticks as f64).floor() as u64, false)
            ));
            return Ok(());
        }
        "addrandom" => {
            let songs = dj.songs.lock();
            if songs.is_empty() {
                feedback("No songs :(");
                return Ok(());
            }

            let song = &songs[rand::rng().random_range(0..songs.len())];
            dj.queue.lock().songs.push(song.clone());
            if dj.queue.lock().songs.len() == 1 {
                info!("First song added to queue. Starting it...");
                dj.ensure_queue_is_active();
                dj.restart_current_song_in_queue(bot, bot_state, Some(sender));
            }
            feedback(&format!(
                "Added to queue: {:?} [{}]",
                song.friendly_name(),
                DiscJockeyModule::format_timestamp(song.ticks_to_millis(song.length_ticks as f64).floor() as u64, false)
            ));
        }
        "rm" | "remove" => {
            if args.len() == 1 {
                feedback("Please provide a song to add. It will behave like a search and play this song after all the others.");
                return Ok(());
            }

            let search_term = args.iter().skip(1).map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
            let matches = dj.search(search_term, true);
            if matches.is_empty() {
                feedback("No song in queue found that matches your term.");
                return Ok(());
            }

            let mut restart_current_song = false;
            let song = matches[0].clone();
            {
                let mut queue = dj.queue.lock();
                let index = queue.songs.iter().position(|s| *s == song);
                if let Some(index) = index {
                    queue.songs.remove(index);
                    if let Some(current) = queue.current {
                        if index < current {
                            queue.current = Some(current - 1);
                        } else if current == index {
                            queue.next();
                            restart_current_song = true;
                        }
                    }
                } else {
                    bail!("UNEXPECTED: Failed to find song in queue!")
                }
            }
            if restart_current_song {
                dj.restart_current_song_in_queue(bot, bot_state, Some(sender));
            }

            feedback(&format!(
                "Removed from queue: {:?} [{}]",
                song.friendly_name(),
                DiscJockeyModule::format_timestamp(song.ticks_to_millis(song.length_ticks as f64).floor() as u64, false)
            ));
            return Ok(());
        }
        "next" => {
            dj.queue.lock().next();
            if !dj.restart_current_song_in_queue(bot, bot_state, Some(sender)) {
                feedback("Queue ended.");
            } else {
                if let Some(song) = dj.queue.lock().current_song() {
                    feedback(&format!(
                        "Playing next song: {:?} [{}]",
                        song.friendly_name(),
                        DiscJockeyModule::format_timestamp(song.ticks_to_millis(song.length_ticks as f64).floor() as u64, false),
                    ));
                } else {
                    feedback("No next song found!");
                }
            }
            return Ok(());
        }
        "prev" | "previous" => {
            dj.queue.lock().previous();
            if !dj.restart_current_song_in_queue(bot, bot_state, Some(sender)) {
                feedback("Queue ended.");
            } else {
                if let Some(song) = dj.queue.lock().current_song() {
                    feedback(&format!(
                        "Playing previous song: {:?} [{}]",
                        song.friendly_name(),
                        DiscJockeyModule::format_timestamp(song.ticks_to_millis(song.length_ticks as f64).floor() as u64, false)
                    ));
                } else {
                    feedback("No previous song found!");
                }
            }
            return Ok(());
        }
        "shuffle" => {
            let mut queue = dj.queue.lock();
            if args.len() >= 2 {
                if args[1].eq_ignore_ascii_case("on") {
                    queue.shuffle = true;
                } else if args[1].eq_ignore_ascii_case("off") {
                    queue.shuffle = false;
                } else {
                    feedback("Please specify either \"on\" or \"off\" or nothing to see whether shuffle is currently on.");
                    return Ok(());
                }
            }
            feedback(&format!("Shuffle: {}", if queue.shuffle { "On" } else { "Off" }));
            return Ok(());
        }
        "repeat" => {
            let mut queue = dj.queue.lock();
            if args.len() >= 2 {
                if args[1].eq_ignore_ascii_case("on") {
                    queue.repeat = true;
                } else if args[1].eq_ignore_ascii_case("off") {
                    queue.repeat = false;
                } else {
                    feedback("Please specify either \"on\" or \"off\" or nothing to see whether repeat is currently on.");
                    return Ok(());
                }
            }
            feedback(&format!("Repeat: {}", if queue.repeat { "On" } else { "Off" }));
            return Ok(());
        }
        "clear" => {
            dj.state.lock().desired_status = DesiredPlaybackStatus::Stopped;
            dj.queue.lock().clear();
            dj.restart_current_song_in_queue(bot, &bot_state, Some(sender));
            feedback("Cleared queue");
            return Ok(());
        }
        "restart" => {
            dj.ensure_queue_is_active();
            if dj.restart_current_song_in_queue(bot, bot_state, Some(sender)) {
                feedback("Restarted song");
            } else {
                feedback("No song to restart");
            }
            return Ok(());
        }
        "info" => {
            let prefix = match dj.state.lock().actual_status {
                ActualPlaybackStatus::Finished
                | ActualPlaybackStatus::Paused
                | ActualPlaybackStatus::Playing
                | ActualPlaybackStatus::Stopped
                | ActualPlaybackStatus::Tuning
                | ActualPlaybackStatus::Positioning => {
                    let queue = dj.queue.lock();
                    if queue.songs.len() > 1 {
                        if let Some(current) = &queue.current {
                            format!("({}/{}) ", current + 1, queue.songs.len())
                        } else {
                            format!("(?/{}) ", queue.songs.len())
                        }
                    } else {
                        "".to_owned()
                    }
                }
                _ => "".to_owned(),
            };
            feedback(&format!("{}{}", prefix, &dj.state.lock().formatted_state()));
            return Ok(());
        }
        "stop" => {
            let mut state = dj.state.lock();
            state.tick = 0.0;
            let mut queue = dj.queue.lock();
            queue.clear();

            if state.desired_status == DesiredPlaybackStatus::Stopped {
                feedback("Already stopped!");
                return Ok(());
            }
            state.desired_status = DesiredPlaybackStatus::Stopped;
            if !matches!(
                state.actual_status,
                ActualPlaybackStatus::Playing | ActualPlaybackStatus::Paused | ActualPlaybackStatus::Tuning
            ) {
                feedback(&format!(
                    "Stopped, but might not have taken affect as current status is {:?}",
                    state.actual_status
                ));
            } else {
                feedback("Stopped song.");
            }
        }
        "speed" => {
            let state = &mut *dj.state.lock();
            if args.len() >= 2 {
                state.speed = args[1].parse::<f64>().context("Parse speed")?;
            }
            feedback(&format!("Speed: {:.03}", state.speed));
        }
        "pause" => {
            let state = &mut *dj.state.lock();
            state.desired_status = DesiredPlaybackStatus::Paused;
            if !matches!(
                state.actual_status,
                ActualPlaybackStatus::Playing | ActualPlaybackStatus::Paused | ActualPlaybackStatus::Tuning
            ) {
                feedback(&format!(
                    "Paused, but might not have taken affect as current status is {:?}",
                    state.actual_status
                ));
            } else {
                feedback("Paused song.");
            }
        }
        "resume" => {
            dj.ensure_task_running(bot_state, Some(&sender));
            let state = &mut *dj.state.lock();
            if state.song.is_none() {
                feedback("There is no song to resume!");
                return Ok(());
            }

            if state.actual_status == ActualPlaybackStatus::Playing {
                state.desired_status = DesiredPlaybackStatus::Playing;
                feedback("Already playing. Perhaps busy with other tasks.");
                return Ok(());
            }
            if state.desired_status == DesiredPlaybackStatus::Playing {
                    feedback("Started DJ-Task. Song should resume soon.");
                } else {
                    feedback("Already wanting to play!");
                }
                return Ok(());
            }
            state.desired_status = DesiredPlaybackStatus::Playing;
            if !matches!(state.actual_status, ActualPlaybackStatus::Paused) {
                feedback(&format!(
                    "Resumed, but might not have taken affect as current status is {:?}",
                    state.actual_status
                ));
            } else {
                feedback("Resumed song.");
            }
        }
        "reload" => {
            if !sender_is_admin {
                feedback("Only admins are allowed to reload the song list!");
                return Ok(());
            }

            if let Err(err) = dj.load_songs() {
                error!("Reload failed: {err:?}");
                feedback(&format!("Reload failed: {err}"));
            } else {
                feedback("Reloading songs.");
            }
        }
        _ => {
            if sender_is_admin {
                feedback("Usage: !dj <play/add/playRandom/addRandom/rm/next/prev/pause/resume/stop/clear/shuffle/repeat/info/speed/reload>");
            } else {
                feedback("Usage: !dj <play/add/playRandom/addRandom/rm/next/prev/pause/resume/stop/clear/shuffle/repeat/info/speed>");
            }
            return Ok(());
        }
    }

    Ok(())
}
