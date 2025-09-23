use crate::module::disc_jockey::DiscJockeyModule;
use crate::nbs_format::{NbsNote, NbsSong};
use crate::task::center::CenterTask;
use crate::task::disc_jockey::ratelimit::SongRateLimiter;
use crate::task::disc_jockey::tune::Tuner;
use crate::task::pathfind::PathfindTask;
use crate::task::{Task, TaskOutcome};
use crate::{BotState, util};
use anyhow::{Context, anyhow, bail, ensure};
use azalea::core::direction::Direction;
use azalea::core::game_type::GameMode;
use azalea::entity::Position;
use azalea::packet::game::SendPacketEvent;
use azalea::pathfinder::goals::BlockPosGoal;
use azalea::protocol::packets::game::s_interact::InteractionHand;
use azalea::protocol::packets::game::s_player_action::Action;
use azalea::protocol::packets::game::{ServerboundGamePacket, ServerboundPlayerAction, ServerboundSwing};
use azalea::{BlockPos, Client, Event, Vec3};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::Instant;

pub mod tune {
    use crate::nbs_format::{NbsInstrument, NbsNote, NbsPitch};
    use crate::util;
    use anyhow::{Context, anyhow, bail};
    use azalea::entity::Position;
    use azalea::packet::game::SendPacketEvent;
    use azalea::protocol::packets::game::s_interact::InteractionHand;
    use azalea::protocol::packets::game::s_use_item_on::BlockHit;
    use azalea::protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket, ServerboundPingRequest, ServerboundSwing, ServerboundUseItemOn};
    use azalea::world::Instance;
    use azalea::{BlockPos, Client, Event, Vec3};
    use rand::Rng;
    use std::collections::{HashMap, HashSet};
    use std::time::{Duration, Instant};

    struct TuneJob {
        pos: BlockPos,
        instrument: NbsInstrument,
        wanted_pitch: NbsPitch,
        predicted: Option<(NbsPitch, Instant /*Until*/)>,
    }

    enum PingState {
        NotStarted,
        Pending(i64, Instant),
        Responded(Instant, Duration),
    }

    pub struct Tuner {
        pub wanted: HashSet<NbsNote>,
        jobs: Vec<TuneJob>,

        ping: PingState,
        wait_for_last_ping: bool,
        waiting_block_hit: Option<BlockHit>,
    }

    impl Tuner {
        pub fn new(wanted: HashSet<NbsNote>) -> Self {
            Self {
                wanted,
                jobs: Default::default(),
                ping: PingState::NotStarted,
                wait_for_last_ping: true,
                waiting_block_hit: None,
            }
        }

        pub fn can_reach(own_eye_pos: Vec3, pos: BlockPos) -> bool {
            if util::squared_magnitude(util::aabb_from_blockpos(&pos), own_eye_pos) > 5.5 * 5.5 {
                false // Too far (default survival interact range for MC 1.20.5+)
            } else if pos.center().distance_squared_to(&own_eye_pos) > 6.0 * 6.0 {
                false // Too far (until MC 1.20.4)
            } else {
                true
            }
        }

        pub fn is_occluded(ecs: &Instance, pos: &BlockPos, instrument: &NbsInstrument) -> bool {
            if instrument.is_instrument_block_below()
                && let Some(state) = ecs.get_block_state(&pos.up(1))
            {
                !state.is_air()
            } else {
                false
            }
        }

        pub fn tick_ping(&mut self, bot: &mut Client) {
            match self.ping {
                PingState::NotStarted => {
                    let ping_id = i64::MIN + rand::rng().random_range(0..i64::MAX / 4);
                    bot.ecs.lock().send_event(SendPacketEvent {
                        sent_by: bot.entity,
                        packet: ServerboundGamePacket::PingRequest(ServerboundPingRequest { time: ping_id as u64 }),
                    });
                    trace!("Sending ping with id {ping_id}");
                    self.ping = PingState::Pending(ping_id, Instant::now());
                }
                PingState::Pending(_, _) | PingState::Responded(_, _) => {}
            };
        }

        pub fn search_notes(bot: &mut Client) -> anyhow::Result<HashMap<NbsInstrument, HashMap<BlockPos, NbsPitch>>> {
            let own_pos = Vec3::from(&bot.component::<Position>());
            let own_eye_pos = util::own_eye_pos(bot).ok_or(anyhow!("No eyepos"))?;
            let own_block_pos = own_pos.to_block_pos_floor();

            let world = bot.world();
            let world = world.read();

            let mut notes = HashMap::<NbsInstrument, HashMap<BlockPos, NbsPitch>>::new();

            // Find reachable blocks
            for x_offset in [0, -1, 1, -2, 2, -3, 3, -4, 4, -5, 5, -6, 6, -7, 7] {
                for z_offset in [0, -1, 1, -2, 2, -3, 3, -4, 4, -5, 5, -6, 6, -7, 7] {
                    for y_offset in [0, -1, 1, -2, 2, -3, 3, -4, 4, -5, 5, -6, 6, -7, 7] {
                        let pos = BlockPos::new(own_block_pos.x + x_offset, own_block_pos.y + y_offset, own_block_pos.z + z_offset);
                        if !Self::can_reach(own_eye_pos, pos) {
                            continue; // Too far
                        }

                        let note = world.get_block_state(&pos).map_or(None, |state| NbsNote::try_from(state).ok());
                        if let Some(note) = note {
                            notes.entry(note.instrument).or_default().try_insert(pos, note.pitch).ok();
                        }
                    }
                }
            }

            Ok(notes)
        }

        pub fn create_jobs(&mut self, bot: &mut Client) -> anyhow::Result<HashSet<NbsNote>> {
            self.jobs.clear();
            let mut wanted = HashMap::<NbsInstrument, HashSet<NbsPitch>>::new();
            let mut wanted_count = 0;
            for note in &self.wanted {
                wanted.entry(note.instrument).or_default().insert(note.pitch);
                wanted_count += 1;
            }
            debug!(
                "Create Jobs: {} unique instruments and {}/{} notes wanted.",
                wanted.len(),
                wanted_count,
                self.wanted.len()
            );

            let mut missing = HashSet::<NbsNote>::new();
            let mut notes = Self::search_notes(bot)?;
            for (instrument, pitches) in notes.iter_mut() {
                if let Some(wanted_pitches) = wanted.get(instrument) {
                    for wanted_pitch in wanted_pitches {
                        let mut best: Option<(BlockPos, NbsPitch, u8)> = None;
                        for (pos, pitch) in pitches.iter() {
                            let right_clicks = pitch.right_clicks_for(wanted_pitch);
                            if let Some((_best_pos, _best_pitch, best_right_clicks)) = best
                                && best_right_clicks > right_clicks
                            {
                                best = Some((*pos, *pitch, right_clicks));
                            } else if best.is_none() {
                                best = Some((*pos, *pitch, right_clicks));
                            }
                        }

                        if let Some((best_pos, _best_pitch, _best_right_clicks)) = best {
                            self.jobs.push(TuneJob {
                                instrument: *instrument,
                                pos: best_pos,
                                wanted_pitch: *wanted_pitch,
                                //last_pitch: best_pitch,
                                predicted: None,
                            });
                            //trace!("New Job ({}): {:?} {:?} at {}", self.jobs.len(), instrument, wanted_pitch, best_pos);
                            pitches.remove(&best_pos); // Consumed
                        } else {
                            missing.insert(NbsNote {
                                instrument: *instrument,
                                pitch: *wanted_pitch,
                            });
                        }
                    }
                }
            }

            // Find missing instruments
            for (wanted_instrument, wanted_pitches) in wanted {
                if !notes.contains_key(&wanted_instrument) {
                    for wanted_pitch in wanted_pitches {
                        missing.insert(NbsNote {
                            instrument: wanted_instrument,
                            pitch: wanted_pitch,
                        });
                    }
                }
            }

            debug!("Create Jobs: Created {} Jobs ({} missing).", self.jobs.len(), missing.len());

            Ok(missing)
        }

        pub fn reset(&mut self) {
            self.ping = PingState::NotStarted;
            self.jobs.clear();
        }

        pub fn format_missing_block_list(notes: &HashSet<NbsNote>) -> String {
            let mut instrument_counts = HashMap::new();
            for notes in notes {
                *instrument_counts.entry(notes.instrument).or_insert(0) += 1;
            }
            let mut blocks = vec![];
            for (instrument, count) in instrument_counts.iter() {
                blocks.push((instrument.example_instrument_block_name(), *count));
            }
            blocks.sort_by_key(|(_, count)| *count);
            blocks.reverse();
            blocks
                .into_iter()
                .map(|(block_name, count)| format!("{count}x {block_name}"))
                .collect::<Vec<_>>()
                .join(", ")
        }

        /// True if done, false if ongoing
        pub fn progress(&mut self, bot: &mut Client, event: &Event) -> anyhow::Result<bool> {
            match event {
                Event::Tick => {
                    if let Some(block_hit) = self.waiting_block_hit.take() {
                        let mut ecs = bot.ecs.lock();
                        ecs.send_event(SendPacketEvent {
                            sent_by: bot.entity,
                            packet: ServerboundGamePacket::UseItemOn(ServerboundUseItemOn {
                                block_hit,
                                hand: InteractionHand::MainHand,
                                sequence: 0,
                            }),
                        });
                        ecs.send_event(SendPacketEvent {
                            sent_by: bot.entity,
                            packet: ServerboundGamePacket::Swing(ServerboundSwing {
                                hand: InteractionHand::MainHand,
                            }),
                        });
                    }

                    if self.wanted.is_empty() {
                        return Ok(true);
                    } else if self.wanted.len() != self.jobs.len() {
                        self.wait_for_last_ping = false;
                        let missing_notes = self.create_jobs(bot)?;
                        if !missing_notes.is_empty() {
                            bail!("Missing instruments: {}", Self::format_missing_block_list(&missing_notes));
                        }
                    }

                    self.tick_ping(bot);
                    let safe_delay = if let PingState::Responded(_when, duration) = &self.ping {
                        Duration::from_millis(100) + *duration.min(&Duration::from_millis(50)) * 2
                    } else {
                        Duration::from_millis(150 + 200)
                    };

                    // Get current pitches and Find lowest note to tune next
                    let world = bot.world();
                    let world = world.read();
                    // Prefer job with lowest wanted pitch
                    let mut best_job: Option<(usize /*JobIndex*/, NbsPitch /*Wanted*/, NbsPitch /*Actual*/)> = None;
                    let mut pending_predictions = 0usize;
                    let now = Instant::now();
                    for (index, job) in self.jobs.iter_mut().enumerate() {
                        let current_pitch = if let Some((pitch, until)) = &job.predicted
                            && *until >= now
                        {
                            pending_predictions += 1;
                            *pitch // Prediction
                        } else if let Some(note) = world.get_block_state(&job.pos).map_or(None, |state| NbsNote::try_from(state).ok()) {
                            if note.instrument != job.instrument {
                                warn!(
                                    "Expected instrument {:?} at pos {} but found {:?} instead. Re-starting tune...",
                                    job.instrument, job.pos, note.instrument
                                );
                                self.jobs.clear();
                                return Ok(false); // Re-populate next tick
                            }

                            note.pitch
                        } else {
                            warn!("Failed to read noteblock states at pos {}. Re-starting tune...", job.pos);
                            self.jobs.clear();
                            return Ok(false); // Re-populate next tick
                        };

                        if current_pitch != job.wanted_pitch {
                            if let Some((_best_job_index, best_pitch, _)) = &best_job
                                && best_pitch.ordinal() > job.wanted_pitch.ordinal()
                            {
                                best_job = Some((index, job.wanted_pitch, current_pitch));
                            } else if best_job.is_none() {
                                best_job = Some((index, job.wanted_pitch, current_pitch));
                            }
                        }
                    }

                    if let Some((best_job_index, _wanted_pitch, actual_pitch)) = best_job
                        && let Some(own_eye_pos) = util::own_eye_pos(bot)
                    {
                        let job = &mut self.jobs[best_job_index];

                        let (look_dir, block_hit) = util::nice_blockhit(&own_eye_pos, &job.pos).context("Calculate block hit")?;
                        debug!("BlockHit: {block_hit:?}");
                        let mut ecs = bot.ecs.lock();
                        *ecs.get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = look_dir;

                        self.waiting_block_hit = Some(block_hit); // Grim expects the interact to happen after the look but before it (next tick). So delaying it.
                        job.predicted = Some((actual_pitch.next(), Instant::now() + safe_delay));
                        self.wait_for_last_ping = false;
                    } else if best_job.is_none() && pending_predictions == 0 {
                        // Seemingly done
                        if !self.wait_for_last_ping {
                            debug!("Waiting for last ping...");
                            self.ping = PingState::NotStarted;
                            self.tick_ping(bot);
                            self.wait_for_last_ping = true;
                        } else {
                            if let PingState::Responded(when, _duration) = self.ping
                                && when.elapsed() >= Duration::from_millis(100)
                            {
                                return Ok(true);
                            }
                        }
                    } else {
                        if best_job.is_none() && pending_predictions > 0 {
                            trace!("No best_job but {pending_predictions} pending_predictions.")
                        }
                    }
                }
                Event::Packet(packet) => match packet.as_ref() {
                    ClientboundGamePacket::PongResponse(packet) => {
                        if let PingState::Pending(ping_id, started) = &self.ping
                            && packet.time as i64 == *ping_id
                        {
                            let duration = started.elapsed();
                            trace!("Got response to ping with id {ping_id}. Ping is {duration:?}");
                            self.ping = PingState::Responded(Instant::now(), duration);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
            Ok(false)
        }
    }
}

pub mod ratelimit {
    use std::time::{Duration, Instant};

    #[derive(Debug, Clone, Default)]
    pub struct SongRateLimiter {
        last_100_ms_span_at: Option<Instant>,
        last_100_ms_span_estimated_packets: u32,
        reduce_packets_until: Option<Instant>,
        stop_packets_until: Option<Instant>,
        last_look_sent_at: Option<Instant>,
        last_swing_sent_at: Option<Instant>,
    }

    impl SongRateLimiter {
        pub fn reset(&mut self) {
            let def = Self::default();
            self.last_100_ms_span_at = def.last_100_ms_span_at;
            self.last_100_ms_span_estimated_packets = def.last_100_ms_span_estimated_packets;
            self.reduce_packets_until = def.reduce_packets_until;
            self.stop_packets_until = def.stop_packets_until;
            self.last_look_sent_at = def.last_look_sent_at;
            self.last_swing_sent_at = def.last_swing_sent_at;
        }

        pub fn max_cosmetic_packets_per_100_ms() -> u32 {
            200 / 10
        }

        pub fn max_packets_per_100_ms() -> u32 {
            250 / 10
        }

        /// Should run each tick, but does not need to.
        /// Just run before running a lot of rate limit checks / on_*_packet()
        pub fn tick(&mut self) {
            let now = Instant::now();
            if self
                .last_100_ms_span_at
                .map(|last_100_ms_span_at| (now - last_100_ms_span_at) >= Duration::from_millis(100))
                .unwrap_or(true)
            {
                self.last_100_ms_span_estimated_packets = 0;
                self.last_100_ms_span_at = Some(now);
            }
        }

        pub fn on_packet_sent(&mut self) {
            self.last_100_ms_span_estimated_packets += 1;
            self.check_limits();
        }

        pub fn on_look_packet_sent(&mut self) {
            self.last_look_sent_at = Some(Instant::now());
            self.on_packet_sent();
        }

        pub fn on_swing_packet_sent(&mut self) {
            self.last_swing_sent_at = Some(Instant::now());
            self.on_packet_sent();
        }

        pub fn check_limits(&mut self) {
            if self.last_100_ms_span_estimated_packets >= Self::max_cosmetic_packets_per_100_ms() {
                let new_reduce_until = Instant::now() + Duration::from_millis(500);
                self.reduce_packets_until = Some(self.reduce_packets_until.map(|old| old.max(new_reduce_until)).unwrap_or(new_reduce_until));
            }
            if self.last_100_ms_span_estimated_packets >= Self::max_packets_per_100_ms() {
                warn!("Stopping all song-related packets for a bit.");
                let now = Instant::now();
                let new_stop_until = now + Duration::from_millis(250);
                self.stop_packets_until = Some(self.stop_packets_until.map(|old| old.max(new_stop_until)).unwrap_or(new_stop_until));
                let new_reduce_until = now + Duration::from_secs(10);
                self.reduce_packets_until = Some(self.reduce_packets_until.map(|old| old.max(new_reduce_until)).unwrap_or(new_reduce_until));
            }
        }

        pub fn can_send_cosmetic_packet(&self) -> bool {
            if let Some(reduce_packets_until) = self.reduce_packets_until
                && reduce_packets_until >= Instant::now()
            {
                false
            } else {
                self.last_100_ms_span_estimated_packets < Self::max_cosmetic_packets_per_100_ms()
            }
        }

        pub fn can_send_packet(&self) -> bool {
            if let Some(stop_packets_until) = self.stop_packets_until
                && stop_packets_until >= Instant::now()
            {
                false
            } else {
                self.last_100_ms_span_estimated_packets < Self::max_cosmetic_packets_per_100_ms()
            }
        }

        pub fn can_send_look_packet(&self) -> bool {
            if let Some(last_look_sent_at) = self.last_look_sent_at
                && last_look_sent_at.elapsed() < Duration::from_millis(100)
            {
                false
            } else {
                self.can_send_cosmetic_packet()
            }
        }

        pub fn can_send_swing_packet(&self) -> bool {
            if let Some(last_swing_sent_at) = self.last_swing_sent_at
                && last_swing_sent_at.elapsed() < Duration::from_millis(150)
            {
                false
            } else {
                self.can_send_cosmetic_packet()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlaybackState {
    pub song: Option<Arc<NbsSong>>,
    pub speed: f64,

    pub tick: f64,
    pub actual_status: ActualPlaybackStatus,
    pub desired_status: DesiredPlaybackStatus,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            song: None,
            speed: 1.0,
            tick: 0.0,
            actual_status: ActualPlaybackStatus::Unknown,
            desired_status: DesiredPlaybackStatus::Stopped,
        }
    }
}

impl PlaybackState {
    pub fn formatted_state(&self) -> String {
        let desired = format!("{:?}", self.actual_status);
        let actual = format!("{:?}", self.desired_status);
        let mut result = String::with_capacity(64);
        if desired != actual {
            result.push_str(&format!("{actual} » {desired}"));
        } else {
            result.push_str(&actual);
        }
        if let Some(song) = &self.song {
            if matches!(self.actual_status, ActualPlaybackStatus::Playing | ActualPlaybackStatus::Paused) {
                result.push_str(&format!(
                    ": {} [{}/{}]",
                    song.friendly_name(),
                    DiscJockeyModule::format_timestamp(song.ticks_to_millis(self.tick).floor() as u64, false),
                    DiscJockeyModule::format_timestamp(song.ticks_to_millis(song.length_ticks as f64).floor() as u64, false),
                ));
            } else {
                result.push_str(&format!(
                    ": {} [{}]",
                    song.friendly_name(),
                    DiscJockeyModule::format_timestamp(song.ticks_to_millis(self.tick).floor() as u64, false),
                ));
            }
        }
        result
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ActualPlaybackStatus {
    Unknown,
    Positioning,
    WaitingForSong,
    Tuning,
    Playing,
    Paused,
    Stopped,
    Finished,
    Interrupted,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DesiredPlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

/// Specific states for this task
pub enum SongPlaybackPhase {
    Initializing,
    Pathfinding {
        subtask: PathfindTask,
    },
    Centering {
        subtask: CenterTask,
    },
    WaitForSongAndPlayingDesire {
        since: Instant,
    },
    Tuning {
        song: Arc<NbsSong>,
        tuner: Tuner,
    },
    Playing {
        song: Arc<NbsSong>,
        positions: HashMap<NbsNote, BlockPos>,
        index_for_tick: Option<(f64, usize)>,
        last_playback_ticked: Option<Instant>,
    },
}

pub struct DiscJockeyTask {
    state: Arc<Mutex<PlaybackState>>,
    phase: SongPlaybackPhase,
    rate_limiter: SongRateLimiter,
    another_task_added: bool, // Set if another task was added (so this task stops)
}

impl DiscJockeyTask {
    pub fn new(state: Arc<Mutex<PlaybackState>>) -> Self {
        Self {
            state,
            phase: SongPlaybackPhase::Initializing,
            rate_limiter: Default::default(),
            another_task_added: false,
        }
    }

    pub fn tick_playing(
        bot: &mut Client,
        rate_limiter: &mut SongRateLimiter,
        state: &mut PlaybackState,
        song: &Arc<NbsSong>,
        positions: &HashMap<NbsNote, BlockPos>,
        index_for_tick: &mut Option<(f64, usize)>,
        last_playback_ticked: &mut Option<Instant>,
    ) -> anyhow::Result<()> {
        ensure!(state.song.is_some(), "State should have a song!");

        let (last_tick, index) = if let Some((expected_tick, _last_index)) = index_for_tick
            && state.tick == *expected_tick
        {
            &mut index_for_tick.as_mut().unwrap()
        } else {
            let mut index: usize = song.notes.len();
            for (i, detailed_note) in song.notes.iter().enumerate() {
                if detailed_note.tick as f64 >= state.tick {
                    index = i;
                    break;
                }
            }
            info!("Computed tick {:.02} to be note index {} of song...", state.tick, index);
            *index_for_tick = Some((state.tick, index));
            &mut index_for_tick.as_mut().unwrap()
        };

        let now = Instant::now();
        if let Some(last_ticked_at) = last_playback_ticked {
            let elapsed = now - *last_ticked_at;
            state.tick += song.millis_to_ticks(elapsed.as_millis() as i64) * state.speed;
        }
        *last_playback_ticked = Some(now);
        *last_tick = state.tick;

        let own_gamemode = bot.tab_list().get(&bot.uuid()).ok_or(anyhow!("No own tab entry"))?.gamemode;
        if own_gamemode != GameMode::Survival {
            bail!("Expected GameMode Survival, but got {own_gamemode:?}!")
        }

        rate_limiter.tick();

        let own_eye_pos = util::own_eye_pos(&bot).ok_or(anyhow!("No own eyepos!"))?;
        let mut last_pos = None;
        let mut ecs = bot.ecs.lock();
        loop {
            if *index >= song.notes.len() {
                state.actual_status = ActualPlaybackStatus::Finished;
                break; // Out-of-bounds
            }
            let detailed_note = song.notes[*index];
            if detailed_note.tick as f64 > state.tick.floor() {
                break; // In future, wait
            }
            *index += 1;

            let pos = positions.get(&detailed_note.note);
            if pos.is_none() {
                bail!("Failed to get position for Note {:?}!", detailed_note.note)
            }
            let pos = pos.unwrap();
            if !Tuner::can_reach(own_eye_pos, *pos) {
                bail!("Went out of range for a block!")
            }

            if rate_limiter.can_send_packet() {
                // Set proper direction to satisfy Grim 2's PositionBreakA check:
                let direction = Direction::nearest(pos.center() - own_eye_pos).opposite();
                ecs.send_event(SendPacketEvent {
                    sent_by: bot.entity,
                    packet: ServerboundGamePacket::PlayerAction(ServerboundPlayerAction {
                        action: Action::StartDestroyBlock,
                        direction,
                        sequence: 0,
                        pos: *pos,
                    }),
                });
                rate_limiter.on_packet_sent();
            }
            last_pos = Some(*pos);
        }

        if let Some(last_pos) = last_pos {
            if rate_limiter.can_send_look_packet() {
                *ecs.get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? =
                    util::fix_look_direction(azalea::direction_looking_at(&own_eye_pos, &last_pos.center()));
                rate_limiter.on_look_packet_sent();
            }
            if rate_limiter.can_send_cosmetic_packet() {
                ecs.send_event(SendPacketEvent {
                    sent_by: bot.entity,
                    packet: ServerboundGamePacket::PlayerAction(ServerboundPlayerAction {
                        action: Action::AbortDestroyBlock,
                        direction: Direction::Down,
                        sequence: 0,
                        pos: last_pos,
                    }),
                });
                rate_limiter.on_packet_sent();
            }
            if rate_limiter.can_send_swing_packet() {
                ecs.send_event(SendPacketEvent {
                    sent_by: bot.entity,
                    packet: ServerboundGamePacket::Swing(ServerboundSwing {
                        hand: InteractionHand::MainHand,
                    }),
                });
                rate_limiter.on_swing_packet_sent();
            }
        } else {
            rate_limiter.reset();
        }
        Ok(())
    }
}

impl Display for DiscJockeyTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.phase {
            SongPlaybackPhase::Initializing => write!(f, "SongPlayTask (Initializing)"),
            SongPlaybackPhase::Pathfinding { subtask: task } => write!(f, "SongPlayTask (Pathfinding: {task})"),
            SongPlaybackPhase::Centering { subtask: task } => write!(f, "SongPlayTask (Centering: {task})"),
            SongPlaybackPhase::WaitForSongAndPlayingDesire { .. } => write!(f, "SongPlayTask (WaitForSongAndPlayingDesire)"),
            SongPlaybackPhase::Tuning { song, .. } => write!(f, "SongPlayTask (Tuning {:?})", song.friendly_name()),
            SongPlaybackPhase::Playing { song, .. } => write!(f, "SongPlayTask (Playing {:?})", song.friendly_name()),
        }
    }
}

impl Task for DiscJockeyTask {
    fn start(&mut self, _bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        self.state.lock().actual_status = ActualPlaybackStatus::Unknown;
        self.phase = SongPlaybackPhase::Initializing;
        Ok(())
    }

    fn handle(&mut self, mut bot: Client, bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        if self.another_task_added {
            info!("Stopping task voluntarily due to another task being added.");
            self.stop(bot, bot_state)?;
            return Ok(TaskOutcome::Succeeded);
        }

        // Phase: Initialize
        if let SongPlaybackPhase::Initializing = self.phase {
            self.state.lock().actual_status = ActualPlaybackStatus::Unknown;
            if let Some(ref pos) = crate::OPTS.dj_pos {
                // Pathfind first, center later
                let own_pos = bot.component::<Position>();
                if own_pos.horizontal_distance_squared_to(&pos.center()) >= 3000.0 * 3000.0 {
                    return Ok(TaskOutcome::Failed {
                        reason: "DJ-Pos is very far away!".to_owned(),
                    });
                }
                let mut subtask = PathfindTask::new(false, BlockPosGoal(**pos), "Go to noteblock sphere");
                subtask.start(bot.clone(), bot_state).context("Start Pathfind Subtask")?;
                self.phase = SongPlaybackPhase::Pathfinding { subtask };
            } else {
                // Center only
                let pos = bot.component::<Position>().to_block_pos_floor();
                let mut subtask = CenterTask::new(pos);
                subtask.start(bot.clone(), bot_state).context("Start Center Subtask")?;
                self.phase = SongPlaybackPhase::Centering { subtask };
            }
        }

        // Phase: Pathfinding
        if let SongPlaybackPhase::Pathfinding { subtask } = &mut self.phase {
            self.state.lock().actual_status = ActualPlaybackStatus::Positioning;
            match subtask.handle(bot.clone(), bot_state, event).context("Handle Pathfind Subtask")? {
                TaskOutcome::Succeeded => {
                    let own_block_pos = bot.component::<Position>().to_block_pos_floor();
                    self.phase = SongPlaybackPhase::Centering {
                        subtask: CenterTask::new(own_block_pos),
                    };
                }
                TaskOutcome::Failed { reason } => {
                    return Ok(TaskOutcome::Failed {
                        reason: format!("Pathfind Subtask failed: {reason}"),
                    });
                }
                TaskOutcome::Ongoing => {}
            }
        }

        // Phase: Centering
        if let SongPlaybackPhase::Centering { subtask } = &mut self.phase {
            self.state.lock().actual_status = ActualPlaybackStatus::Positioning;
            match subtask.handle(bot.clone(), bot_state, event).context("Handle Center subtask")? {
                TaskOutcome::Succeeded => {
                    self.phase = SongPlaybackPhase::WaitForSongAndPlayingDesire { since: Instant::now() };
                }
                TaskOutcome::Failed { reason } => {
                    return Ok(TaskOutcome::Failed {
                        reason: format!("Center subtask failed: {reason}"),
                    });
                }
                TaskOutcome::Ongoing => {}
            }
        }

        // Phase: WaitForSongAndPlayingDesire
        if let SongPlaybackPhase::WaitForSongAndPlayingDesire { since } = self.phase {
            let mut state = self.state.lock();
            if !matches!(
                state.actual_status,
                ActualPlaybackStatus::Paused | ActualPlaybackStatus::Stopped | ActualPlaybackStatus::Finished | ActualPlaybackStatus::WaitingForSong
            ) {
                state.actual_status = ActualPlaybackStatus::WaitingForSong;
                info!(
                    "Actual status in phase WaitForSongAndPlayingDesire was {:?}, changed to WaitingForSong",
                    state.actual_status
                );
            }
            let song = state.song.clone();
            if let Some(song) = song
                && let DesiredPlaybackStatus::Playing = state.desired_status
            {
                let tuner = Tuner::new(song.unique.clone());
                self.phase = SongPlaybackPhase::Tuning { tuner, song };
            } else {
                // Waiting for a song...
                if since.elapsed().as_secs() >= 15 {
                    warn!("Waited over 15s for a song and/or desire to play, but never got one! Ending task in success...");
                    return Ok(TaskOutcome::Succeeded);
                }
            }
        }

        // Phase: Tuning
        if let SongPlaybackPhase::Tuning { song, tuner } = &mut self.phase {
            let mut state = self.state.lock();
            state.actual_status = ActualPlaybackStatus::Tuning;
            let current_song = &state.song;
            if let Some(current_song) = current_song {
                if current_song != song {
                    info!(
                        "Song changed from {:?} to {:?} during tuning. Re-tuning...",
                        song.friendly_name(),
                        current_song.friendly_name(),
                    );
                    *song = current_song.clone();
                    tuner.wanted = song.unique.clone();
                    tuner.reset();
                }
            } else {
                info!("Song was removed during tuning {:?}. Going back to WaitingForSong...", song.friendly_name());
                self.phase = SongPlaybackPhase::WaitForSongAndPlayingDesire { since: Instant::now() };
                self.state.lock().actual_status = ActualPlaybackStatus::WaitingForSong;
                return Ok(TaskOutcome::Ongoing);
            }

            match tuner.progress(&mut bot, event) {
                Ok(true) => {
                    debug!("Finished tuning. Finding noteblocks with which to play {:?}...", song.friendly_name());

                    let mut positions = HashMap::new();
                    for (instrument, pitches) in Tuner::search_notes(&mut bot)? {
                        for (pos, pitch) in pitches {
                            let note = NbsNote { instrument, pitch };
                            if song.unique.contains(&note) && !positions.contains_key(&note) {
                                positions.insert(note, pos);
                            }
                        }
                    }

                    if positions.len() != song.unique.len() {
                        bail!(
                            "Only found {} out of {} required unique notes nearby! Was tuning done?",
                            positions.len(),
                            song.unique.len()
                        );
                    }
                    /*
                    if let PlaybackState::Interrupted { prev_state } = &*state {
                        *state = prev_state.as_ref().clone();
                        info!("Restored state (previously interrupted): {state:?}");
                    }*/

                    info!("Found {} relevant noteblocks. Playing {:?}", positions.len(), song.friendly_name());
                    self.phase = SongPlaybackPhase::Playing {
                        song: song.clone(),
                        positions,
                        last_playback_ticked: None,
                        index_for_tick: None,
                    };
                }
                Ok(false) => {} // Still tuning...
                Err(err) => {
                    return Ok(TaskOutcome::Failed {
                        reason: format!("Tuning failed: {err}"),
                    });
                }
            }
        }

        // Phase: Playing
        if let SongPlaybackPhase::Playing {
            song,
            positions,
            index_for_tick,
            last_playback_ticked,
        } = &mut self.phase
            && let Event::Tick = event
        {
            let mut state = self.state.lock();
            if state.desired_status != DesiredPlaybackStatus::Playing {
                if state.desired_status == DesiredPlaybackStatus::Stopped {
                    // Just in case
                    state.tick = 0.0;
                }
                info!("Desired status is {:?}. Going back to waiting...", state.desired_status);
                self.phase = SongPlaybackPhase::WaitForSongAndPlayingDesire { since: Instant::now() };
                state.actual_status = match state.desired_status {
                    DesiredPlaybackStatus::Paused => ActualPlaybackStatus::Paused,
                    DesiredPlaybackStatus::Stopped => ActualPlaybackStatus::Stopped,
                    _ => {
                        return Ok(TaskOutcome::Failed {
                            reason: format!(
                                "UNEXPECTED: Ticking Playing phase in DiscJockey task expected desired_status Paused or Stopped but got {:?} instead!",
                                state.desired_status
                            ),
                        });
                    }
                };
                return Ok(TaskOutcome::Ongoing);
            }

            let current_song = state.song.clone();
            if current_song.is_none() {
                info!("No more current song found. Waiting...");
                self.phase = SongPlaybackPhase::WaitForSongAndPlayingDesire { since: Instant::now() };
                state.actual_status = ActualPlaybackStatus::WaitingForSong; // Change early, since otherwise only changed next time task is handled
                return Ok(TaskOutcome::Ongoing);
            }
            let current_song = current_song.unwrap();

            if let Some(pos) = bot.get_component::<Position>() {
                let center_pos = Vec3::new(pos.x.floor() + 0.5, pos.y, pos.z.floor() + 0.5);
                if pos.horizontal_distance_squared_to(&center_pos) >= 0.15 * 0.15 {
                    info!("Bot is no longer centered. Positioning again...");
                    self.phase = SongPlaybackPhase::Initializing;
                    state.actual_status = ActualPlaybackStatus::Positioning; // Change early, since otherwise only changed next time task is handled
                    return Ok(TaskOutcome::Ongoing);
                } else if let Some(dj_pos) = &crate::OPTS.dj_pos
                    && **dj_pos != pos.to_block_pos_floor()
                {
                    info!("Bot is no longer on DJ Goal pos! Positioning again...");
                    self.phase = SongPlaybackPhase::Initializing;
                    state.actual_status = ActualPlaybackStatus::Positioning; // Change early, since otherwise only changed next time task is handled
                    return Ok(TaskOutcome::Ongoing);
                }
            }

            if *song != current_song {
                info!(
                    "Song changed from {:?} to {} during playback. Tuning again...",
                    song.friendly_name(),
                    current_song.friendly_name()
                );
                let tuner = Tuner::new(current_song.unique.clone());
                self.phase = SongPlaybackPhase::Tuning { song: current_song, tuner };
                return Ok(TaskOutcome::Ongoing);
            }

            if !matches!(state.actual_status, ActualPlaybackStatus::Finished | ActualPlaybackStatus::Stopped) {
                state.actual_status = ActualPlaybackStatus::Playing;
                if let Err(err) = Self::tick_playing(
                    &mut bot,
                    &mut self.rate_limiter,
                    &mut *state,
                    song,
                    positions,
                    index_for_tick,
                    last_playback_ticked,
                ) {
                    return Ok(TaskOutcome::Failed {
                        reason: format!("Playback failed: {err}"),
                    });
                }
            }

            // Might have changed
            if matches!(state.actual_status, ActualPlaybackStatus::Finished | ActualPlaybackStatus::Stopped) {
                //info!("Ending task because actual_status is {:?}", state.actual_status);
                //return Ok(TaskOutcome::Succeeded);
                info!("Clearing song and waiting because actual_status is: {:?}", state.actual_status);
                state.song = None;
                self.phase = SongPlaybackPhase::WaitForSongAndPlayingDesire { since: Instant::now() };
                return Ok(TaskOutcome::Ongoing);
            }
        }

        Ok(TaskOutcome::Ongoing)
    }

    fn stop(&mut self, bot: Client, bot_state: &BotState) -> anyhow::Result<()> {
        {
            let mut state = self.state.lock();
            info!("Got interrupted.");
            state.actual_status = ActualPlaybackStatus::Interrupted;
        }
        match self.phase {
            SongPlaybackPhase::Pathfinding { ref mut subtask } => {
                subtask.stop(bot.clone(), bot_state)?;
            }
            SongPlaybackPhase::Centering { ref mut subtask } => {
                subtask.stop(bot.clone(), bot_state)?;
            }
            _ => {}
        }
        self.phase = SongPlaybackPhase::Initializing;
        Ok(())
    }

    fn new_task_waiting(&mut self, _bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        info!("NEW TASK");
        self.another_task_added = true;
        Ok(())
    }

    fn discard(&mut self, bot: Client, bot_state: &BotState) -> anyhow::Result<()> {
        match self.phase {
            SongPlaybackPhase::Pathfinding { ref mut subtask } => {
                subtask.discard(bot.clone(), bot_state)?;
            }
            SongPlaybackPhase::Centering { ref mut subtask } => {
                subtask.discard(bot.clone(), bot_state)?;
            }
            _ => {}
        }
        Ok(())
    }
}
