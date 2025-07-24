use crate::module::Module;
use crate::task::affect_block::AffectBlockTask;
use crate::task::delay_ticks::DelayTicksTask;
use crate::task::group::TaskGroup;
use crate::task::oncefunc::OnceFuncTask;
use crate::task::open_container_block::OpenContainerBlockTask;
use crate::task::pathfind;
use crate::task::pathfind::{BoxedPathfindGoal, PathfindTask};
use crate::task::wait_for_block_unpower::WaitForBlockUnpowerTask;
use crate::{BotState, OPTS};
use crate::{blockpos_string, commands};
use anyhow::{Context, bail};
use azalea::blocks::Block;
use azalea::blocks::properties::Open;
use azalea::ecs::entity::Entity;
use azalea::ecs::prelude::With;
use azalea::entity::metadata::{EnderPearl, Player};
use azalea::entity::{EntityUuid, Position};
use azalea::inventory::Inventory;
use azalea::packet::game::SendPacketEvent;
use azalea::pathfinder::goals;
use azalea::protocol::packets::game::{ClientboundGamePacket, ServerboundContainerButtonClick, ServerboundContainerClose, ServerboundGamePacket};
use azalea::registry::EntityKind;
use azalea::world::MinecraftEntityId;
use azalea::{BlockPos, Client, Event, GameProfileComponent, Vec3};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ops::Add;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct LecternRecoderTerminal {
    pub id: String,
    #[serde(with = "blockpos_string")]
    pub lectern: BlockPos,
    #[serde(with = "blockpos_string")]
    pub button: BlockPos,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct LecternRedcoderEndpoint {
    pub lectern_recoder_terminal_id: String,
    pub chamber_index: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
#[serde(tag = "type")]
pub enum StasisChamberDefinition {
    FlippableTrapdoor {
        #[serde(with = "blockpos_string")]
        trapdoor_pos: BlockPos,
    },
    SecuredFlippableTrapdoor {
        #[serde(with = "blockpos_string")]
        security_trapdoor_pos: BlockPos,
        #[serde(with = "blockpos_string")]
        trigger_trapdoor_pos: BlockPos,
    },
    RedcoderTrapdoor {
        #[serde(with = "blockpos_string")]
        trapdoor_pos: BlockPos,
        endpoint: LecternRedcoderEndpoint,
    },
    RedcoderShay {
        #[serde(with = "blockpos_string")]
        base_pos: BlockPos,
        endpoint: LecternRedcoderEndpoint,
    },
}

impl StasisChamberDefinition {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::FlippableTrapdoor { .. } => "FlippableTrapdoor",
            Self::SecuredFlippableTrapdoor { .. } => "SecuredFlippableTrapdoor",
            Self::RedcoderTrapdoor { .. } => "RedcoderTrapdoor",
            Self::RedcoderShay { .. } => "RedcoderShay",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ChamberOccupant {
    pub thrown_at: chrono::DateTime<chrono::Local>,
    pub player_uuid: Uuid,
    pub pearl_uuid: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct StasisChamberEntry {
    pub definition: StasisChamberDefinition,
    pub occupants: Vec<ChamberOccupant>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, Eq, PartialEq)]
pub struct StasisConfig {
    pub lectern_redcoder_terminals: Vec<LecternRecoderTerminal>,
    pub chambers: Vec<StasisChamberEntry>,
}

#[derive(Clone)]
pub struct StasisModule {
    pub do_reopen_trapdoor: bool,
    pub alternate_trapdoor_goal: bool,

    pub config: Arc<Mutex<StasisConfig>>,
    pub last_idle_pos: Arc<Mutex<Option<BlockPos>>>,
    spawned_pearl_uuids: Arc<Mutex<HashMap<MinecraftEntityId, EntityUuid>>>,
    //missing_pearl_counter: Arc<Mutex<HashMap<Uuid /*PearlUuid*/, usize>>>,
    expect_despawn_of: Arc<Mutex<HashSet<Uuid /*PearlUuid*/>>>,
}

impl StasisModule {
    pub fn new(do_reopen_trapdoor: bool, alternate_trapdoor_goal: bool) -> Self {
        Self {
            do_reopen_trapdoor,
            alternate_trapdoor_goal,

            config: Default::default(),
            last_idle_pos: Default::default(),
            spawned_pearl_uuids: Default::default(),
            //missing_pearl_counter: Default::default(),
            expect_despawn_of: Default::default(),
        }
    }

    pub fn config_path() -> PathBuf {
        PathBuf::from("stasis-config.json")
    }

    pub async fn load_config(&self) -> anyhow::Result<()> {
        let remembered_trapdoor_positions_path = Self::config_path();
        if remembered_trapdoor_positions_path.exists() && !remembered_trapdoor_positions_path.is_dir() {
            *self.config.lock() = serde_json::from_str(
                &tokio::fs::read_to_string(remembered_trapdoor_positions_path)
                    .await
                    .context("Read stasis-config file")?,
            )
            .context("Parsing stasis-config content")?;
            info!("Loaded stasis-config from file.");
        } else {
            *self.config.lock() = Default::default();
            warn!("File for stasis-config doesn't exist, yet (saving default one).");
            self.save_config().await?;
        };

        Ok(())
    }

    pub async fn save_config(&self) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(&*self.config.as_ref().lock()).context("Convert stasis-config to json")?;
        tokio::fs::write(Self::config_path(), json).await.context("Save stasis-config as file")?;
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

    pub fn chamber_for_pearl_pos<'a>(bot: &mut Client, config: &'a mut StasisConfig, pearl_pos: Vec3) -> Option<&'a mut StasisChamberEntry> {
        {
            let world = bot.world();
            let world = world.read();
            let base_pos = pearl_pos.to_block_pos_floor().add(BlockPos::new(0, 8, 0));
            #[derive(Eq, PartialEq, Copy, Clone)]
            enum BlockType {
                Unknown,
                Trapdoor,
                BubbleColumn,
                SoulSand,
            }
            let mut last_type = BlockType::Unknown;
            let mut trapdoor_1 = None;
            let mut trapdoor_2 = None;
            let mut found_soul_sand = false;
            let mut column_blocks = 0;
            for y_offset_abs in 0..32 {
                let block_pos = base_pos.add(BlockPos::new(0, -y_offset_abs, 0));
                if let Some(state) = world.get_block_state(&block_pos) {
                    let block = Box::<dyn Block>::from(state);
                    let block_type = if block.id().ends_with("_trapdoor") && !block.id().ends_with("iron_trapdoor") {
                        BlockType::Trapdoor
                    } else if block.id().ends_with("bubble_column") {
                        BlockType::BubbleColumn
                    } else if block.id().ends_with("soul_sand") {
                        BlockType::SoulSand
                    } else {
                        BlockType::Unknown
                    };

                    let mut reset = false;
                    match block_type {
                        BlockType::Trapdoor => {
                            if last_type == BlockType::Unknown {
                                trapdoor_1 = Some(block_pos);
                                column_blocks = 0;
                                found_soul_sand = false;
                            } else if last_type == BlockType::Trapdoor && trapdoor_1.is_some() {
                                trapdoor_2 = Some(block_pos);
                                column_blocks = 0;
                                found_soul_sand = false;
                            } else {
                                reset = true;
                            }
                        }
                        BlockType::BubbleColumn => {
                            if last_type == BlockType::BubbleColumn || last_type == BlockType::Trapdoor {
                                column_blocks += 1;
                            } else {
                                reset = true;
                            }
                        }
                        BlockType::SoulSand => {
                            if last_type == BlockType::BubbleColumn {
                                found_soul_sand = true;
                                break;
                            }
                        }
                        BlockType::Unknown => {
                            reset = true;
                        }
                    }

                    if reset {
                        trapdoor_1 = None;
                        trapdoor_2 = None;
                        column_blocks = 0;
                        found_soul_sand = false;
                    }
                    last_type = block_type;
                }
            }

            if trapdoor_1.is_some() && found_soul_sand && column_blocks >= 4 {
                info!(
                    "Found satisfactory stasis chamber setup at {} with {}, a {column_blocks} block long bubble column and soul sand",
                    trapdoor_1.unwrap(),
                    if trapdoor_1.is_some() && trapdoor_2.is_some() {
                        "two trapdoors"
                    } else {
                        "one trapdoor "
                    }
                );
                let definition = if let Some(security_trapdoor_pos) = trapdoor_1
                    && let Some(trigger_trapdoor_pos) = trapdoor_2
                {
                    Some(StasisChamberDefinition::SecuredFlippableTrapdoor {
                        security_trapdoor_pos,
                        trigger_trapdoor_pos,
                    })
                } else if let Some(trapdoor_pos) = trapdoor_1
                    && trapdoor_2.is_none()
                {
                    Some(StasisChamberDefinition::FlippableTrapdoor { trapdoor_pos })
                } else {
                    None
                };

                if let Some(definition) = definition {
                    let rough_pos = trapdoor_1.unwrap();
                    // Find close, but wrong definition

                    let mut remove_chambers_indices = Vec::new();
                    for (chamber_index, chamber) in config.chambers.iter().enumerate() {
                        match chamber.definition {
                            StasisChamberDefinition::RedcoderTrapdoor {
                                trapdoor_pos: existing_pos, ..
                            }
                            | StasisChamberDefinition::RedcoderShay { base_pos: existing_pos, .. }
                            | StasisChamberDefinition::FlippableTrapdoor {
                                trapdoor_pos: existing_pos, ..
                            }
                            | StasisChamberDefinition::SecuredFlippableTrapdoor {
                                trigger_trapdoor_pos: existing_pos,
                                ..
                            } => {
                                if chamber.definition != definition
                                    && existing_pos.x == rough_pos.x
                                    && existing_pos.z == rough_pos.z
                                    && (rough_pos.y - existing_pos.y).abs() <= 5
                                {
                                    warn!(
                                        "Detected an existing close chamber definition ({}) where new one ({}) is supposed to be at roughly {rough_pos}!",
                                        chamber.definition.type_name(),
                                        definition.type_name()
                                    );
                                    match chamber.definition {
                                        StasisChamberDefinition::FlippableTrapdoor { .. } | StasisChamberDefinition::SecuredFlippableTrapdoor { .. } => {
                                            warn!("Removing old definition (assuming trapdoors moved)!");
                                            remove_chambers_indices.push(chamber_index);
                                        }
                                        _ => {
                                            return None;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    for (chamber_index_index, chamber_index) in remove_chambers_indices.into_iter().enumerate() {
                        config.chambers.remove(chamber_index - chamber_index_index);
                    }

                    if let Some(chamber_index) = config.chambers.iter_mut().position(|chamber| chamber.definition == definition) {
                        let chamber = &mut config.chambers[chamber_index];
                        info!(
                            "Found existing, matching stasis chamber definition ({}) at {rough_pos}!",
                            chamber.definition.type_name()
                        );
                        return Some(chamber);
                    }

                    // Make new chamber
                    info!("Added new chamber definition: {definition:?}");
                    config.chambers.push(StasisChamberEntry { definition, occupants: vec![] });
                    let chamber_len = config.chambers.len();
                    return config.chambers.get_mut(chamber_len - 1);
                } else {
                    warn!("Failed to detect stasis definition near {pearl_pos:?} for some reason (trapdoor1: {trapdoor_1:?}, trapdoor2: {trapdoor_2:?})!");
                    return None;
                }
            }
        }

        // Find existing redcoder chamber
        let pearl_block_pos = pearl_pos.to_block_pos_floor();
        for chamber in config.chambers.iter_mut() {
            match chamber.definition {
                StasisChamberDefinition::RedcoderShay { base_pos: existing_pos, .. }
                | StasisChamberDefinition::RedcoderTrapdoor {
                    trapdoor_pos: existing_pos, ..
                } => {
                    if existing_pos.x == pearl_block_pos.x && existing_pos.z == pearl_block_pos.z && (pearl_block_pos.y - existing_pos.y).abs() <= 5 {
                        info!("Found existing chamber definition ({}) at {existing_pos}.", chamber.definition.type_name());
                        return Some(chamber);
                    }
                }
                _ => {}
            }
        }

        None
    }

    pub fn on_pearl_throw(
        &self,
        bot: &mut Client,
        _player: Entity,
        player_name: &str,
        player_uuid: Uuid,
        player_pos: Vec3,
        pearl_uuid: Uuid,
        pearl_pos: Vec3,
        _pearl_id: MinecraftEntityId,
    ) {
        for chamber in &self.config.lock().chambers {
            if chamber.occupants.iter().find(|occupant| occupant.pearl_uuid == Some(pearl_uuid)).is_some() {
                trace!("Pearl {pearl_uuid} is already noted as an occupant.");
                return;
            }
        }

        if player_pos.to_block_pos_floor().x != pearl_pos.to_block_pos_floor().x
            || player_pos.to_block_pos_floor().z != pearl_pos.to_block_pos_floor().z
            || (player_pos.y - pearl_pos.y).abs() >= 5.0
        {
            trace!("Pearl {pearl_uuid} is too far from player. Likely not a throw.");
            return;
        }

        let mut config = self.config.lock();
        let chamber = Self::chamber_for_pearl_pos(bot, &mut config, pearl_pos);
        if chamber.is_none() {
            trace!("No fitting chamber detected/created for pearl {pearl_uuid}");
            return;
        }
        let chamber = chamber.unwrap();
        let mut owned_by_other = false;
        let was_occupants_empty = chamber.occupants.is_empty();
        for occupant in &chamber.occupants {
            if occupant.player_uuid != player_uuid {
                warn!(
                    "Player {player_name} ({player_uuid}) threw a pearl into a chamber that had a pearl registed which was owned by another player with uuid {}! Clearing old occupants, but this is rude!",
                    occupant.player_uuid
                );
                owned_by_other = true;
            }
        }
        if owned_by_other {
            chamber.occupants.clear();
        }
        chamber.occupants.push(ChamberOccupant {
            player_uuid,
            pearl_uuid: Some(pearl_uuid),
            thrown_at: chrono::Local::now(),
        });
        let chamber_definition = chamber.definition.clone();
        if was_occupants_empty {
            commands::send_command(bot, format!("msg {player_name} You have thrown a pearl. Message me !tp to get back here."));
        }

        drop(config);
        let self_clone = self.clone();
        tokio::spawn(async move {
            if let Err(err) = self_clone.save_config().await {
                error!("Failed to save stasis-config: {err:?}");
            }
        });

        info!(
            "{player_name} threw an EnderPearl ({pearl_uuid}) at {player_pos} (chamber type: {}) and was saved to config!",
            chamber_definition.type_name()
        );
    }

    /*
    pub fn check_for_missing_pearls(&self, bot: &mut Client, bot_state: &BotState) {
        if let Some(server_tps) = &bot_state.server_tps
            && server_tps.is_server_likely_hanging()
        {
            trace!("Clearing missing_pearl_counters as server appears to be hanging!");
            self.missing_pearl_counter.lock().clear();
            return;
        }

        let own_pos = Vec3::from(&bot.component::<Position>());
        for chamber in &mut self.config.lock().chambers {
            let chamber_pos = match chamber.definition {
                StasisChamberDefinition::FlippableTrapdoor { trapdoor_pos: pos }
                | StasisChamberDefinition::SecuredFlippableTrapdoor { trigger_trapdoor_pos: pos, .. }
                | StasisChamberDefinition::RedcoderTrapdoor { trapdoor_pos: pos, .. }
                | StasisChamberDefinition::RedcoderShay { base_pos: pos, .. } => pos,
            };

            let mut remove_occupant_indices = Vec::new();
            for (occupant_index, occupant) in chamber.occupants.iter().enumerate() {
                if own_pos.horizontal_distance_squared_to(&chamber_pos.center()) > 56.0f64.powi(2) {
                    continue; // Too far to judge
                }

                if let Some(pearl_uuid) = occupant.pearl_uuid {
                    let pearl_entity = bot.entity_by::<With<EnderPearl>, (&EntityUuid,)>(|(entity_uuid,): &(&EntityUuid,)| ***entity_uuid == pearl_uuid);
                    if pearl_entity.is_none() {
                        let mut missing_pearl_counter = self.missing_pearl_counter.lock();
                        *missing_pearl_counter.entry(pearl_uuid).or_default() += 1;
                        if *missing_pearl_counter.entry(pearl_uuid).or_default() >= 10 {
                            warn!("Pearl {pearl_uuid} has not been found for an extensive amount of time. Removing it!");
                            remove_occupant_indices.push(occupant_index);
                        }
                    }
                }
            }
            for (occupant_index_index, occupant_index) in remove_occupant_indices.into_iter().enumerate() {
                chamber.occupants.remove(occupant_index - occupant_index_index);
            }
        }
    }*/

    pub fn pull_pearl<F: Fn(/*error*/ bool, &str) + Send + Sync + 'static>(
        &self,
        username: &str,
        bot: &Client,
        bot_state: &BotState,
        mut chamber_index: usize,
        feedback: F,
    ) -> anyhow::Result<()> {
        let username = username.to_owned();
        let uuid = bot.tab_list().iter().find(|(_, i)| i.profile.name == username).map(|(_, i)| i.profile.uuid);
        if uuid.is_none() {
            feedback(true, &format!("Could not find UUID for username {username}"));
            return Ok(());
        }
        let uuid = uuid.unwrap();

        let mut owned_chambers_with_times = HashMap::new();
        //let mut definition = None;
        //let mut definition_newest_time = None;
        // Find definition of chamber with newest thrown pearl
        for chamber in &self.config.lock().chambers {
            let mut newest_time_from_player = None;
            for occupant in &chamber.occupants {
                if occupant.player_uuid == uuid {
                    if newest_time_from_player
                        .as_ref()
                        .map(|other_time| &occupant.thrown_at > other_time)
                        .unwrap_or(true)
                    {
                        newest_time_from_player = Some(occupant.thrown_at);
                    }
                }
            }

            if let Some(newest_time_from_player) = newest_time_from_player {
                owned_chambers_with_times.insert(chamber.definition.clone(), newest_time_from_player);
            }
        }

        let mut chambers_ordered: Vec<StasisChamberDefinition> = owned_chambers_with_times.keys().map(|def| def.clone()).collect();
        chambers_ordered.sort_by(|a, b| owned_chambers_with_times[&b].cmp(&owned_chambers_with_times[&a]));

        if chambers_ordered.is_empty() {
            feedback(true, "No chamber found!");
            return Ok(());
        }

        if chamber_index >= chambers_ordered.len() {
            chamber_index = chambers_ordered.len() - 1;
        }
        let definition = chambers_ordered.remove(chamber_index);

        if bot_state.tasks() > 0 {
            feedback(false, "Hang on, will walk to your stasis chamber in due time...");
        } else {
            feedback(false, &format!("Pulling your stasis chamber... (you got {})", owned_chambers_with_times.len()));
        }

        self.pull_chamber(&username, definition, &bot, &bot_state, Arc::new(feedback))?;
        Ok(())
    }

    pub fn remove_occupants_with_no_pearl_uuid(&self, definition: &StasisChamberDefinition) {
        for chamber in &mut self.config.lock().chambers {
            if &chamber.definition != definition {
                continue;
            }

            let mut remove_occupant_indices = Vec::new();
            for (occupant_index, occupant) in &mut chamber.occupants.iter().enumerate() {
                if occupant.pearl_uuid.is_none() {
                    remove_occupant_indices.push(occupant_index);
                }
            }
            for (occupant_index_index, occupant_index) in remove_occupant_indices.into_iter().enumerate() {
                chamber.occupants.remove(occupant_index - occupant_index_index);
            }
        }
    }

    pub fn pull_chamber<F: Fn(/*error*/ bool, &str) + Send + Sync + 'static>(
        &self,
        occupant: &str,
        definition: StasisChamberDefinition,
        bot: &Client,
        bot_state: &BotState,
        feedback: Arc<F>,
    ) -> anyhow::Result<()> {
        info!("Pulling chamber {definition:?} for {occupant}...");

        let mut occupying_pearl_uuids = Vec::new();
        for chamber in &self.config.lock().chambers {
            if chamber.definition == definition {
                for occupant in &chamber.occupants {
                    if let Some(pearl_uuid) = occupant.pearl_uuid {
                        occupying_pearl_uuids.push(pearl_uuid);
                    }
                }
            }
        }

        match definition {
            StasisChamberDefinition::FlippableTrapdoor { trapdoor_pos }
            | StasisChamberDefinition::SecuredFlippableTrapdoor {
                trigger_trapdoor_pos: trapdoor_pos,
                ..
            } => {
                let trapdoor_goal: Arc<BoxedPathfindGoal> = if self.alternate_trapdoor_goal {
                    Arc::new(BoxedPathfindGoal::new(goals::RadiusGoal {
                        pos: trapdoor_pos.center(),
                        radius: 3.0,
                    }))
                } else {
                    Arc::new(BoxedPathfindGoal::new(goals::ReachBlockPosGoal {
                        pos: trapdoor_pos.to_owned(),
                        chunk_storage: bot.world().read().chunks.clone(),
                    }))
                };
                let return_goal = goals::BlockPosGoal(self.return_pos(&mut bot.clone()));
                let greeting = format!("Welcome back, {occupant}!");

                let mut group = TaskGroup::new(format!("Pull {occupant}'s chamber"));
                group = group.with(PathfindTask::new_concrete(!OPTS.no_mining, trapdoor_goal, format!("near {occupant}'s Pearl")));

                // Has closed security trapdoor: open it
                if let StasisChamberDefinition::SecuredFlippableTrapdoor { security_trapdoor_pos, .. } = definition
                    && let Some(state) = bot.world().read().get_block_state(&security_trapdoor_pos)
                    && !state.property::<Open>().unwrap_or(false)
                {
                    group.add(AffectBlockTask::new(security_trapdoor_pos)); // Open security trapdoor
                }

                let mut trigger_group = TaskGroup::new_with_hide_fail("Check and trigger", true);

                let occupying_pearl_uuids_clone = occupying_pearl_uuids.clone();
                let feedback_clone = feedback.clone();
                let (self_clone, self_clone_2, self_clone_3) = (self.clone(), self.clone(), self.clone());
                let definition_clone = definition.clone();
                trigger_group.add(OnceFuncTask::new("Check if any pearl exists", move |mut bot, bot_state| {
                    let any_pearl = bot
                        .entity_by::<With<EnderPearl>, (&EntityUuid,)>(|(entity_uuid,): &(&EntityUuid,)| occupying_pearl_uuids_clone.contains(entity_uuid))
                        .is_some();
                    let is_hanging = bot_state.server_tps.map(|server_tps| server_tps.is_server_likely_hanging()).unwrap_or(false);
                    if !any_pearl && !is_hanging {
                        for chamber in &mut self_clone_3.config.lock().chambers {
                            if chamber.definition == definition_clone {
                                chamber.occupants.clear();
                            }
                        }

                        tokio::spawn(async move {
                            if let Err(err) = self_clone_3.save_config().await {
                                error!("Failed to save stasis-config: {err:?}");
                            }
                        });
                        feedback_clone(
                            false,
                            "Sorry, but it seems this stasis chamber has no pearls in it! I removed it. Try again to pull the next if you got one.",
                        );
                        bail!("Chamber had no pearls!");
                    }
                    Ok(())
                }));
                trigger_group = trigger_group
                    .with(OnceFuncTask::new("Add expected despawning pearls", move |_bot, _bot_state| {
                        let mut expect_despawn_of = self_clone.expect_despawn_of.lock();
                        occupying_pearl_uuids.iter().for_each(|pearl_uuid| {
                            expect_despawn_of.insert(*pearl_uuid);
                        });
                        Ok(())
                    }))
                    .with(AffectBlockTask::new(trapdoor_pos))
                    .with(OnceFuncTask::new(format!("Greet {occupant}"), move |_bot, _bot_state| {
                        feedback(false, &greeting);
                        Ok(())
                    }))
                    .with(OnceFuncTask::new("Clean unknown pearls", move |_bot, _bot_state| {
                        self_clone_2.remove_occupants_with_no_pearl_uuid(&definition);
                        Ok(())
                    }));

                if self.do_reopen_trapdoor {
                    trigger_group = trigger_group
                        .with(DelayTicksTask::new(Self::recommended_closed_trapdoor_ticks(bot_state)))
                        .with(AffectBlockTask::new(trapdoor_pos))
                }

                group.add(trigger_group);
                group.add(PathfindTask::new(!OPTS.no_mining, return_goal, "old spot"));

                bot_state.add_task(group);
            }
            StasisChamberDefinition::RedcoderTrapdoor { ref endpoint, .. } | StasisChamberDefinition::RedcoderShay { ref endpoint, .. } => {
                let endpoint = endpoint.clone();
                let mut terminal = None;
                for lectern_redcoder_terminal in &self.config.lock().lectern_redcoder_terminals {
                    if lectern_redcoder_terminal.id == endpoint.lectern_recoder_terminal_id {
                        terminal = Some(lectern_redcoder_terminal.clone());
                    }
                }
                if terminal.is_none() {
                    feedback(
                        true,
                        &format!(
                            "Skill issue! I can't find details on LecternRedcoderTerminal \"{}\"!",
                            endpoint.lectern_recoder_terminal_id
                        ),
                    );
                    return Ok(());
                }
                let terminal = terminal.unwrap();

                let lectern_goal: Arc<BoxedPathfindGoal> = Arc::new(BoxedPathfindGoal::new(goals::ReachBlockPosGoal {
                    pos: terminal.lectern,
                    chunk_storage: bot.world().read().chunks.clone(),
                }));
                let button_goal: Arc<BoxedPathfindGoal> = Arc::new(BoxedPathfindGoal::new(goals::ReachBlockPosGoal {
                    pos: terminal.button,
                    chunk_storage: bot.world().read().chunks.clone(),
                }));
                let return_goal = goals::BlockPosGoal(self.return_pos(&mut bot.clone()));

                let greeting = format!("Welcome back, {occupant}!");

                let mut group = TaskGroup::new(format!("Pull {occupant}'s redcoder chamber ({}, idx: {})", terminal.id, endpoint.chamber_index));
                group = group
                    .with(PathfindTask::new_concrete(!OPTS.no_mining, lectern_goal, "Near Lectern"))
                    .with(OpenContainerBlockTask::new(terminal.lectern))
                    .with(OnceFuncTask::new("Select lectern page", move |bot, _bot_state| {
                        let inv = bot.component::<Inventory>();
                        if inv.id == 0 {
                            bail!("Expected to have lectern open!");
                        }
                        let mut ecs = bot.ecs.lock();
                        let sent_by = bot.entity;
                        ecs.send_event(SendPacketEvent {
                            sent_by,
                            packet: ServerboundGamePacket::ContainerButtonClick(ServerboundContainerButtonClick {
                                container_id: inv.id,
                                button_id: 101 + endpoint.chamber_index as u32,
                            }),
                        });
                        ecs.send_event(SendPacketEvent {
                            sent_by,
                            packet: ServerboundGamePacket::ContainerClose(ServerboundContainerClose { container_id: inv.id }),
                        });
                        Ok(())
                    }));

                let mut trigger_group = TaskGroup::new_with_hide_fail("Check and trigger", true);

                let occupying_pearl_uuids_clone = occupying_pearl_uuids.clone();
                let feedback_clone = feedback.clone();
                let (self_clone, self_clone_2, self_clone_3) = (self.clone(), self.clone(), self.clone());
                let definition_clone = definition.clone();
                trigger_group.add(OnceFuncTask::new("Check if any pearl exists", move |mut bot, bot_state| {
                    let any_pearl = bot
                        .entity_by::<With<EnderPearl>, (&EntityUuid,)>(|(entity_uuid,): &(&EntityUuid,)| occupying_pearl_uuids_clone.contains(entity_uuid))
                        .is_some();
                    let is_hanging = bot_state.server_tps.map(|server_tps| server_tps.is_server_likely_hanging()).unwrap_or(false);
                    if !any_pearl && !is_hanging {
                        for chamber in &mut self_clone_3.config.lock().chambers {
                            if chamber.definition == definition_clone {
                                chamber.occupants.clear();
                            }
                        }

                        tokio::spawn(async move {
                            if let Err(err) = self_clone_3.save_config().await {
                                error!("Failed to save stasis-config: {err:?}");
                            }
                        });
                        feedback_clone(
                            false,
                            "Sorry, but it seems this redcoder stasis chamber has no pearls in it! I removed it. Try again to pull the next if you got one.",
                        );
                        bail!("Chamber had no pearls!");
                    }
                    Ok(())
                }));
                trigger_group = trigger_group
                    .with(OnceFuncTask::new("Add expected despawning pearls", move |_bot, _bot_state| {
                        let mut expect_despawn_of = self_clone.expect_despawn_of.lock();
                        occupying_pearl_uuids.iter().for_each(|pearl_uuid| {
                            expect_despawn_of.insert(*pearl_uuid);
                        });
                        Ok(())
                    }))
                    .with(PathfindTask::new_concrete(!OPTS.no_mining, button_goal, "Near Button"))
                    .with(AffectBlockTask::new(terminal.button))
                    .with(WaitForBlockUnpowerTask::new(terminal.button))
                    .with(OnceFuncTask::new(format!("Greet {occupant}"), move |_bot, _bot_state| {
                        feedback(false, &greeting);
                        Ok(())
                    }))
                    .with(OnceFuncTask::new("Clean unknown pearls", move |_bot, _bot_state| {
                        self_clone_2.remove_occupants_with_no_pearl_uuid(&definition);
                        Ok(())
                    }));

                group.add(trigger_group);
                group.add(PathfindTask::new(!OPTS.no_mining, return_goal, "old spot"));

                bot_state.add_task(group);
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Module for StasisModule {
    fn name(&self) -> &'static str {
        "Stasis"
    }

    async fn handle(&self, mut bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Login => {
                info!("Loading remembered trapdoor positions...");
                self.load_config().await?;
                self.clear_idle_pos("Login-Event");
                self.spawned_pearl_uuids.lock().clear();
                self.expect_despawn_of.lock().clear();
            }
            Event::Disconnect(_) => {
                self.clear_idle_pos("Disconnect-Event");
                self.spawned_pearl_uuids.lock().clear();
                self.expect_despawn_of.lock().clear();
            }
            Event::Packet(packet) => match packet.as_ref() {
                ClientboundGamePacket::PlayerPosition(_) => {
                    self.clear_idle_pos("Got teleport-Packet");
                }
                ClientboundGamePacket::Respawn(_) => {
                    self.spawned_pearl_uuids.lock().clear();
                    self.expect_despawn_of.lock().clear();
                }
                ClientboundGamePacket::AddEntity(packet) => {
                    if packet.entity_type == EntityKind::EnderPearl {
                        self.spawned_pearl_uuids.lock().insert(packet.id, EntityUuid::new(packet.uuid));

                        let owning_player_entity_id = packet.data as i32;
                        let player_entity =
                            bot.entity_by::<With<Player>, (&MinecraftEntityId,)>(|(entity_id,): &(&MinecraftEntityId,)| entity_id.0 == owning_player_entity_id);

                        if let Some(player_entity) = player_entity {
                            let game_profile = bot.entity_component::<GameProfileComponent>(player_entity);
                            let player_pos = bot.entity_component::<Position>(player_entity);
                            self.on_pearl_throw(
                                &mut bot,
                                player_entity,
                                &game_profile.name,
                                game_profile.uuid,
                                Vec3::from(&player_pos),
                                packet.uuid,
                                packet.position,
                                packet.id,
                            );
                        }
                    }
                }
                ClientboundGamePacket::RemoveEntities(packet) => {
                    let mut spawned_pearl_uuids = self.spawned_pearl_uuids.lock();
                    let mut config_changed = false;
                    for id in &packet.entity_ids {
                        if let Some(pearl_uuid) = spawned_pearl_uuids.remove(id) {
                            let expected = self.expect_despawn_of.lock().remove(&pearl_uuid);
                            info!("Pearl {} despawned{}!", *pearl_uuid, if expected { " (expected)" } else { "" });

                            for chamber in &mut self.config.lock().chambers {
                                let chamber_pos = match chamber.definition {
                                    StasisChamberDefinition::FlippableTrapdoor { trapdoor_pos: pos }
                                    | StasisChamberDefinition::SecuredFlippableTrapdoor { trigger_trapdoor_pos: pos, .. }
                                    | StasisChamberDefinition::RedcoderTrapdoor { trapdoor_pos: pos, .. }
                                    | StasisChamberDefinition::RedcoderShay { base_pos: pos, .. } => pos,
                                };

                                let own_pos = Vec3::from(&bot.component::<Position>());
                                let mut remove_occupant_indices = Vec::new();
                                for (occupant_index, occupant) in chamber.occupants.iter().enumerate() {
                                    if occupant.pearl_uuid == Some(*pearl_uuid) {
                                        let player_entity = bot.entity_by::<With<Player>, (&EntityUuid,)>(|(entity_uuid,): &(&EntityUuid,)| {
                                            ***entity_uuid == occupant.player_uuid
                                        });
                                        let mut added = false;
                                        if let Some(player_entity) = player_entity {
                                            let player_pos = bot.entity_component::<Position>(player_entity);
                                            if player_pos.horizontal_distance_squared_to(&chamber_pos.center()) <= 2.0f64.powi(2)
                                                && own_pos.horizontal_distance_squared_to(&chamber_pos.center()) <= 56.0f64.powi(2)
                                            {
                                                info!(
                                                    "Pearl {} likely was removed when teleporting the owner ({}) to it. Removing it.",
                                                    *pearl_uuid, occupant.player_uuid
                                                );
                                                remove_occupant_indices.push(occupant_index);
                                                added = true;
                                            }
                                        }

                                        if !added && expected && own_pos.horizontal_distance_squared_to(&chamber_pos.center()) <= 56.0f64.powi(2) {
                                            info!(
                                                "Despawning of Pearl {} (owned by {}) was expected. Removing it.",
                                                *pearl_uuid, occupant.player_uuid
                                            );
                                            remove_occupant_indices.push(occupant_index);
                                        }
                                    }
                                }
                                for (occupant_index_index, occupant_index) in remove_occupant_indices.into_iter().enumerate() {
                                    chamber.occupants.remove(occupant_index - occupant_index_index);
                                    config_changed = true;
                                }
                            }
                        }
                    }

                    if config_changed {
                        let self_clone = self.clone();
                        tokio::spawn(async move {
                            if let Err(err) = self_clone.save_config().await {
                                error!("Failed to save stasis-config: {err:?}");
                            }
                        });
                    }
                }
                _ => {}
            },
            Event::Tick => {
                if bot_state.tasks() == 0 && !pathfind::is_pathfinding(&bot) {
                    self.update_idle_pos(&mut bot.clone());
                }
            }
            _ => {}
        }
        Ok(())
    }
}
