use crate::blockpos_string;
use crate::module::Module;
use crate::task::affect_block::AffectBlockTask;
use crate::task::close_inventory_and_sync::CloseInventoryAndSyncTask;
use crate::task::delay_ticks::DelayTicksTask;
use crate::task::group::TaskGroup;
use crate::task::oncefunc::OnceFuncTask;
use crate::task::open_container_block::OpenContainerBlockTask;
use crate::task::pathfind::{BoxedPathfindGoal, PathfindTask};
use crate::task::wait_for_block_unpower::WaitForBlockUnpowerTask;
use crate::util::InteractableGoal;
use crate::{BotState, OPTS};
use anyhow::{Context, anyhow, bail};
use azalea::blocks::Block;
use azalea::blocks::properties::{FacingCubic, Open};
use azalea::core::direction::Direction;
use azalea::core::position::ChunkPos;
use azalea::ecs::entity::Entity;
use azalea::ecs::prelude::With;
use azalea::entity::metadata::{EnderPearl, Player};
use azalea::entity::{EntityDataValue, EntityUuid, Position};
use azalea::inventory::Inventory;
use azalea::packet::game::SendPacketEvent;
use azalea::pathfinder::goals;
use azalea::protocol::packets::game::c_animate::AnimationAction;
use azalea::protocol::packets::game::{ClientboundGamePacket, ServerboundContainerButtonClick, ServerboundGamePacket};
use azalea::registry::{BlockEntityKind, EntityKind};
use azalea::world::{Instance, MinecraftEntityId};
use azalea::{BlockPos, Client, Event, FormattedText, GameProfileComponent, Vec3};
use parking_lot::{Mutex, RwLockReadGuard};
use serde::{Deserialize, Serialize};
use simdnbt::owned::{Nbt, NbtCompound};
use std::collections::{HashMap, HashSet};
use std::ops::{Add, Sub};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct LecternRedcoderTerminal {
    pub id: String,
    #[serde(with = "blockpos_string")]
    pub lectern: BlockPos,
    #[serde(with = "blockpos_string")]
    pub button: BlockPos,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct LecternRedcoderEndpoint {
    #[serde(alias = "lectern_recoder_terminal_id")]
    pub lectern_redcoder_terminal_id: String,
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
    RedstoneSingleTrigger {
        #[serde(with = "blockpos_string")]
        base_pos: BlockPos,
        #[serde(with = "blockpos_string")]
        trigger_pos: BlockPos,
    },
    RedstoneDoubleTrigger {
        #[serde(with = "blockpos_string")]
        base_pos: BlockPos,
        #[serde(with = "blockpos_string")]
        trigger_pos: BlockPos,
        delay_ticks: u32,
    },
    MultiPullPickles {
        #[serde(with = "blockpos_string")]
        throw_pos: BlockPos,
        #[serde(with = "blockpos_string")]
        pickles_pos: BlockPos,
        #[serde(with = "blockpos_string")]
        pull_trigger_pos: BlockPos,
        #[serde(with = "blockpos_string")]
        align_trigger_pos: BlockPos,
    },
}

impl StasisChamberDefinition {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::FlippableTrapdoor { .. } => "FlippableTrapdoor",
            Self::SecuredFlippableTrapdoor { .. } => "SecuredFlippableTrapdoor",
            Self::RedcoderTrapdoor { .. } => "RedcoderTrapdoor",
            Self::RedcoderShay { .. } => "RedcoderShay",
            Self::RedstoneSingleTrigger { .. } => "RedstoneSingleTrigger",
            Self::RedstoneDoubleTrigger { .. } => "RedstoneDoubleTrigger",
            Self::MultiPullPickles { .. } => "MultiPearlPickles",
        }
    }

    pub fn rough_pos(&self) -> BlockPos {
        match self {
            StasisChamberDefinition::RedcoderTrapdoor { trapdoor_pos: rough_pos, .. }
            | StasisChamberDefinition::RedcoderShay { base_pos: rough_pos, .. }
            | StasisChamberDefinition::FlippableTrapdoor { trapdoor_pos: rough_pos, .. }
            | StasisChamberDefinition::SecuredFlippableTrapdoor {
                trigger_trapdoor_pos: rough_pos,
                ..
            }
            | StasisChamberDefinition::RedstoneSingleTrigger { base_pos: rough_pos, .. }
            | StasisChamberDefinition::RedstoneDoubleTrigger { base_pos: rough_pos, .. } => *rough_pos,
            StasisChamberDefinition::MultiPullPickles { throw_pos: rough_pos, .. } => *rough_pos,
        }
    }

    pub fn can_be_pulled_multiple_times(&self) -> bool {
        matches!(self, Self::MultiPullPickles { .. })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct ChamberOccupant {
    pub thrown_at: chrono::DateTime<chrono::Local>,
    pub player_uuid: Option<Uuid>,
    pub pearl_uuid: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct StasisChamberEntry {
    pub definition: StasisChamberDefinition,
    pub occupants: Vec<ChamberOccupant>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, Eq, PartialEq)]
pub struct StasisConfig {
    pub lectern_redcoder_terminals: Vec<LecternRedcoderTerminal>,
    pub chambers: Vec<StasisChamberEntry>,
}

#[derive(Debug, Clone)]
pub struct SignSide {
    #[allow(dead_code)]
    has_glowing_text: bool,
    #[allow(dead_code)]
    color: String,
    /// Should always be 4, even if just empty!
    lines: Vec<FormattedText>,
}

impl SignSide {
    pub fn from_nbt(nbt: &NbtCompound) -> anyhow::Result<SignSide> {
        let has_glowing_text = nbt.get("has_glowing_text").ok_or(anyhow!("Missing has_glowing_text"))?;
        let color = nbt.get("color").ok_or(anyhow!("Missing color"))?;
        let messages = nbt.get("messages").ok_or(anyhow!("Missing messages"))?;

        let has_glowing_text = has_glowing_text.byte().ok_or(anyhow!("Not a byte: has_glowing_text"))? != 0;
        let color = color.string().ok_or(anyhow!("Not a string: color"))?.to_string();
        let messages = messages
            .list()
            .ok_or(anyhow!("Not a list: messages"))?
            .strings()
            .ok_or(anyhow!("Not a List of strings: messages"))?
            .iter()
            .map(|s| serde_json::from_slice(s.as_bytes()))
            .collect::<Result<Vec<FormattedText>, serde_json::Error>>()
            .context("Parse messages as formatted text")?;

        Ok(SignSide {
            has_glowing_text,
            color,
            lines: messages,
        })
    }
}

#[derive(Debug, Clone)]
pub struct SignBlockEntity {
    front: SignSide,
    back: SignSide,
    #[allow(dead_code)]
    is_waxed: bool,
}

impl SignBlockEntity {
    pub fn from_nbt(nbt: &NbtCompound) -> anyhow::Result<SignBlockEntity> {
        let is_waxed = nbt.get("is_waxed").ok_or(anyhow!("Missing is_waxed"))?;
        let front_text = nbt.get("front_text").ok_or(anyhow!("Missing front_text"))?;
        let back_text = nbt.get("back_text").ok_or(anyhow!("Missing back_text"))?;

        let is_waxed = is_waxed.byte().ok_or(anyhow!("Not a byte: is_waxed"))? != 0;
        let front_text = SignSide::from_nbt(front_text.compound().ok_or(anyhow!("Not a compound: front_text"))?).context("Parse front_text")?;
        let back_text = SignSide::from_nbt(back_text.compound().ok_or(anyhow!("Not a compound: back_text"))?).context("Parse back_text")?;
        Ok(SignBlockEntity {
            is_waxed,
            front: front_text,
            back: back_text,
        })
    }
}

#[derive(Clone)]
pub struct StasisModule {
    pub do_reopen_trapdoor: bool,
    pub max_pearls: Option<u32>,

    pub config: Arc<Mutex<StasisConfig>>,
    pub last_idle_pos: Arc<Mutex<Option<BlockPos>>>,
    spawned_pearl_uuids: Arc<Mutex<HashMap<MinecraftEntityId, EntityUuid>>>,
    //missing_pearl_counter: Arc<Mutex<HashMap<Uuid /*PearlUuid*/, usize>>>,
    expect_despawn_of: Arc<Mutex<HashSet<Uuid /*PearlUuid*/>>>,
    remove_occupant_if_player_gets_near: Arc<
        Mutex<
            Vec<(
                Uuid,     /*PearlUuid*/
                BlockPos, /*ChamberPos*/
                Uuid,     /*Player*/
                Instant,  /*Until*/
            )>,
        >,
    >,
    pub mass_adding: Arc<Mutex<HashMap<Uuid /*PlayerUuid*/, (String /*LecternRedcoderTerminalId*/, bool /*IsShay*/, usize /*Next index*/)>>>,
    last_task_seen_at: Arc<Mutex<Instant>>,
    pub sign_cache: Arc<Mutex<HashMap<ChunkPos, HashMap<BlockPos, SignBlockEntity>>>>,
}

impl StasisModule {
    pub fn new(do_reopen_trapdoor: bool, max_pearls: Option<u32>) -> Self {
        Self {
            do_reopen_trapdoor,
            max_pearls,

            config: Default::default(),
            last_idle_pos: Default::default(),
            spawned_pearl_uuids: Default::default(),
            //missing_pearl_counter: Default::default(),
            expect_despawn_of: Default::default(),
            remove_occupant_if_player_gets_near: Default::default(),
            mass_adding: Default::default(),
            last_task_seen_at: Arc::new(Mutex::new(Instant::now())),
            sign_cache: Default::default(),
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

    fn current_block_pos(bot: &mut Client) -> Option<BlockPos> {
        bot.get_entity_component::<Position>(bot.entity).map(|pos| pos.to_block_pos_floor())
    }

    pub fn return_pos(&self, bot: &mut Client) -> Option<BlockPos> {
        let own_block_pos = Self::current_block_pos(bot);
        if let Some(last_idle_pos) = self.last_idle_pos.lock().as_ref()
            && own_block_pos
                .as_ref()
                .map(|pos| last_idle_pos.horizontal_distance_squared_to(pos) <= 3500 * 3500)
                .unwrap_or(false)
        {
            Some(last_idle_pos.clone())
        } else {
            own_block_pos
        }
    }

    pub fn clear_idle_pos(&self, reason: impl AsRef<str>) {
        *self.last_idle_pos.lock() = None;
        debug!("Cleared idle pos: {}", reason.as_ref());
    }

    pub fn update_idle_pos(&self, bot: &mut Client) {
        *self.last_idle_pos.lock() = Self::current_block_pos(bot);
    }

    fn find_observer_trigger_for_block(world: &RwLockReadGuard<Instance>, target_block: BlockPos) -> Option<BlockPos> {
        // TODO: Account for observers strong powering through ordinary, full blocks, pisitons and nother madness.
        // Tbh i might never care, this is madness enough.

        let mut current_pos = target_block;
        let mut previous_pos = target_block;
        let mut at_least_one_observer = false;
        'search_loop: loop {
            if let Some(state) = world.get_block_state(&current_pos) {
                let block = Box::<dyn Block>::from(state);
                let block_id = block.id();
                //info!("Current block: {}", block_id);
                if at_least_one_observer && block_id.ends_with("lever")
                    || (block_id.ends_with("_trapdoor") && !block_id.ends_with("iron_trapdoor"))
                    || block_id.ends_with("_button")
                    || block_id.ends_with("note_block")
                    || block_id.ends_with("_fence_gate")
                {
                    // Some right-clickable blocks that can trigger observers (reasonable and that I could think of)
                    return Some(current_pos);
                } else if current_pos == target_block
                    || block_id.ends_with("redstone_lamp")
                    || block_id.ends_with("observer")
                    || block_id.ends_with("_trapdoor")
                    || block_id.ends_with("note_block")
                    || block_id.ends_with("fence_gate")
                    || block_id.ends_with("powered_rail")
                    || block_id.ends_with("activator_rail")
                    || block_id.ends_with("redstone")
                {
                    // Target block or some that observers can trigger through

                    for dir in [
                        Direction::Up,
                        Direction::Down,
                        Direction::East,
                        Direction::South,
                        Direction::West,
                        Direction::North,
                    ] {
                        let maybe_observer_pos = current_pos.offset_with_direction(dir);
                        if maybe_observer_pos == previous_pos {
                            continue; // Don't return
                        }
                        if let Some(state) = world.get_block_state(&maybe_observer_pos) {
                            let block = Box::<dyn Block>::from(state);
                            let facing = state.property::<FacingCubic>();
                            if block.id().ends_with("observer")
                                && let Some(facing) = facing
                            {
                                let observer_observing = match facing {
                                    FacingCubic::East => Direction::East,
                                    FacingCubic::West => Direction::West,
                                    FacingCubic::South => Direction::South,
                                    FacingCubic::North => Direction::North,
                                    FacingCubic::Up => Direction::Up,
                                    FacingCubic::Down => Direction::Down,
                                };
                                if observer_observing == dir {
                                    // Observer must be observing from same direction to power this block
                                    at_least_one_observer = true;
                                    current_pos = maybe_observer_pos.offset_with_direction(dir);
                                    previous_pos = maybe_observer_pos;
                                    continue 'search_loop;
                                }
                            }
                        }
                    }
                    return None; // No observer with FacingCubic property found
                } else {
                    return None; // Unknown block
                }
            } else {
                return None; // Error: No state
            }
        }
    }

    pub fn chamber_for_pearl_pos<'a>(bot: &mut Client, config: &'a mut StasisConfig, pearl_pos: Vec3) -> Option<&'a mut StasisChamberEntry> {
        // Find existing redcoder chamber or others
        let mut use_existing_chamber_index = None;
        {
            let pearl_block_pos = pearl_pos.to_block_pos_floor();
            for (chamber_index, chamber) in config.chambers.iter().enumerate() {
                match chamber.definition {
                    StasisChamberDefinition::RedcoderShay { base_pos: existing_pos, .. }
                    | StasisChamberDefinition::RedcoderTrapdoor {
                        trapdoor_pos: existing_pos, ..
                    }
                    | StasisChamberDefinition::RedstoneSingleTrigger { base_pos: existing_pos, .. }
                    | StasisChamberDefinition::RedstoneDoubleTrigger { base_pos: existing_pos, .. } => {
                        if existing_pos.x == pearl_block_pos.x && existing_pos.z == pearl_block_pos.z && (pearl_block_pos.y - existing_pos.y).abs() <= 8 {
                            info!("Found existing chamber definition ({}) at {existing_pos}.", chamber.definition.type_name());
                            use_existing_chamber_index = Some(chamber_index);
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }

        if use_existing_chamber_index.is_none() {
            let world = bot.world();
            let world = world.read();
            let base_pos = pearl_pos.to_block_pos_floor().add(BlockPos::new(0, 8, 0));
            #[derive(Eq, PartialEq, Copy, Clone, Debug)]
            enum BlockType {
                Unknown,
                Trapdoor,
                BubbleColumn,
                SoulSand,
                WaterStill,
                WaterFlowing,
            }
            let mut last_type = BlockType::Unknown;
            // For simple or secured trapdoor setup:
            let mut trapdoor_1 = None;
            let mut trapdoor_2 = None;

            // For multipull pickles setup:
            let mut water_still = None; // Highest block
            let mut water_flowing = None; // One below where pearls will rest/touch (be technically on pickles block after moving)
            let mut pickles_pos = None; // Where pearls get aligned against
            let mut align_trigger_pos = None; // Block to click so a piston aligns pearls to allow the next one to pull user
            let mut pull_trigger_pos = None; // Block to click so a piston moves a pearl to pull user

            // Both types:
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
                    } else if block.id().ends_with("water")
                        && let Some(fluid_state) = world.get_fluid_state(&block_pos)
                    {
                        if fluid_state.amount == 8 {
                            BlockType::WaterStill
                        } else {
                            BlockType::WaterFlowing
                        }
                    } else {
                        BlockType::Unknown
                    };

                    let mut reset = false;
                    match block_type {
                        BlockType::WaterStill => {
                            column_blocks = 0;
                            if last_type == BlockType::Unknown && trapdoor_1.is_none() && trapdoor_2.is_none() {
                                water_still = Some(block_pos);
                            } else if last_type == BlockType::WaterFlowing || last_type == BlockType::WaterStill {
                                reset = true;
                            }
                        }
                        BlockType::WaterFlowing => {
                            column_blocks = 0;
                            if last_type == BlockType::WaterStill && water_still.is_some() && trapdoor_1.is_none() && trapdoor_2.is_none() {
                                // A lot of checks here, as this block height should have the pickle and 2 pistons

                                // Try find nearby pickle
                                water_flowing = Some(block_pos);
                                pickles_pos = None;
                                let mut expect_align_piston_at = None;
                                pull_trigger_pos = None;

                                // Find pickles
                                for dir in [Direction::East, Direction::South, Direction::West, Direction::North] {
                                    let nearby_pos = block_pos.offset_with_direction(dir);
                                    if let Some(state) = world.get_block_state(&nearby_pos) {
                                        let block = Box::<dyn Block>::from(state);
                                        if block.id().ends_with("sea_pickle") {
                                            pickles_pos = Some(nearby_pos);
                                            expect_align_piston_at = Some(block_pos.offset_with_direction(dir.opposite()));
                                            break;
                                        }
                                    }
                                }

                                if let Some(pickles_pos) = pickles_pos
                                    && let Some(expect_align_piston_at) = expect_align_piston_at
                                {
                                    // Find piston that can move pearls to pickles
                                    if let Some(state) = world.get_block_state(&expect_align_piston_at) {
                                        let block = Box::<dyn Block>::from(state);
                                        if block.id().ends_with("piston") && !block.id().ends_with("sticky_piston") {
                                            // Found a piston
                                            let maybe_trigger_pos = Self::find_observer_trigger_for_block(&world, expect_align_piston_at);
                                            if let Some(found_trigger_pos) = maybe_trigger_pos {
                                                // Found block to trigger for this piston
                                                align_trigger_pos = Some(found_trigger_pos);
                                            }
                                        } else {
                                            reset = true; // Expected a piston opposite of pickles!
                                        }
                                    } else {
                                        reset = true; // Failed to get block state
                                    }

                                    if align_trigger_pos.is_some() {
                                        // Find piston that could push near pickles (pull pearl)
                                        for dir in [Direction::East, Direction::South, Direction::West, Direction::North] {
                                            let nearby_pos = block_pos.offset_with_direction(dir);
                                            if let Some(state) = world.get_block_state(&nearby_pos) {
                                                let block = Box::<dyn Block>::from(state);
                                                if block.id().ends_with("piston") && !block.id().ends_with("sticky_piston") {
                                                    // Found a piston
                                                    let delta = pickles_pos.sub(nearby_pos);
                                                    if delta.x.abs() == 1 && delta.y == 0 && delta.z.abs() == 1 {
                                                        // Position makes sense for pickles (essentially is a diagonal to pickles block at same y)
                                                        // Could also check facing, but lazy

                                                        let maybe_trigger_pos = Self::find_observer_trigger_for_block(&world, nearby_pos);
                                                        if let Some(found_trigger_pos) = maybe_trigger_pos {
                                                            // Found block to trigger for this piston
                                                            pull_trigger_pos = Some(found_trigger_pos);
                                                        }
                                                    }
                                                }
                                            } else {
                                                reset = true; // Failed to get block state
                                            }
                                        }
                                    }
                                }

                                if pickles_pos.is_none() || pull_trigger_pos.is_none() || align_trigger_pos.is_none() {
                                    reset = true;
                                }
                            } else if last_type == BlockType::WaterFlowing || last_type == BlockType::WaterStill {
                                reset = true;
                            }
                        }
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
                            if last_type == BlockType::BubbleColumn
                                || last_type == BlockType::Trapdoor
                                || (last_type == BlockType::WaterFlowing && pickles_pos.is_some() && pull_trigger_pos.is_some())
                            {
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

                    last_type = block_type;
                    if reset {
                        trapdoor_1 = None;
                        trapdoor_2 = None;
                        water_still = None;
                        water_flowing = None;
                        pickles_pos = None;
                        column_blocks = 0;
                        found_soul_sand = false;
                        last_type = BlockType::Unknown;
                    }
                }
            }

            if trapdoor_1.is_some()
                && found_soul_sand
                && column_blocks >= 4
                && water_flowing.is_none()
                && water_still.is_none()
                && pickles_pos.is_none()
                && pull_trigger_pos.is_none()
                && align_trigger_pos.is_none()
            {
                info!(
                    "Found satisfactory trapdoor stasis chamber setup at {} with {}, a {column_blocks} block long bubble column and soul sand",
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
                        let existing_rough_pos = chamber.definition.rough_pos();
                        if chamber.definition != definition
                            && rough_pos.x == existing_rough_pos.x
                            && rough_pos.z == existing_rough_pos.z
                            && (rough_pos.y - existing_rough_pos.y).abs() <= 5
                        {
                            warn!(
                                "Detected an existing close chamber definition ({}) where new one ({}) is supposed to be at roughly {rough_pos}!",
                                chamber.definition.type_name(),
                                definition.type_name()
                            );
                            match chamber.definition {
                                StasisChamberDefinition::FlippableTrapdoor { .. }
                                | StasisChamberDefinition::SecuredFlippableTrapdoor { .. }
                                | StasisChamberDefinition::MultiPullPickles { .. } => {
                                    warn!("Removing old definition (assuming trapdoors moved)!");
                                    remove_chambers_indices.push(chamber_index);
                                }
                                _ => {
                                    return None;
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
            } else if water_still.is_some()
                && water_flowing.is_some()
                && pickles_pos.is_some()
                && pull_trigger_pos.is_some()
                && align_trigger_pos.is_some()
                && found_soul_sand
                && column_blocks >= 4
                && trapdoor_1.is_none()
                && trapdoor_2.is_none()
            {
                info!(
                    "Found satisfactory multipull pickles stasis chamber setup at {} with pickles at {}, pull trigger at {}, align trigger at {}, a {column_blocks} block long bubble column and soul sand",
                    water_still.unwrap(),
                    pickles_pos.unwrap(),
                    pull_trigger_pos.unwrap(),
                    align_trigger_pos.unwrap(),
                );
                let definition = StasisChamberDefinition::MultiPullPickles {
                    throw_pos: water_still.unwrap(),
                    pull_trigger_pos: pull_trigger_pos.unwrap(),
                    align_trigger_pos: align_trigger_pos.unwrap(),
                    pickles_pos: pickles_pos.unwrap(),
                };

                let rough_pos = water_still.unwrap();
                // Find close, but wrong definition

                let mut remove_chambers_indices = Vec::new();
                for (chamber_index, chamber) in config.chambers.iter().enumerate() {
                    let existing_rough_pos = chamber.definition.rough_pos();
                    if chamber.definition != definition
                        && rough_pos.x == existing_rough_pos.x
                        && rough_pos.z == existing_rough_pos.z
                        && (rough_pos.y - existing_rough_pos.y).abs() <= 5
                    {
                        warn!(
                            "Detected an existing close chamber definition ({}) where new one ({}) is supposed to be at roughly {rough_pos}!",
                            chamber.definition.type_name(),
                            definition.type_name()
                        );
                        match chamber.definition {
                            StasisChamberDefinition::FlippableTrapdoor { .. }
                            | StasisChamberDefinition::SecuredFlippableTrapdoor { .. }
                            | StasisChamberDefinition::MultiPullPickles { .. } => {
                                warn!("Removing old definition (assuming trapdoors moved)!");
                                remove_chambers_indices.push(chamber_index);
                            }
                            _ => {
                                return None;
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
                        "Found existing, matching pickles stasis chamber definition ({}) at {rough_pos}!",
                        chamber.definition.type_name()
                    );
                    return Some(chamber);
                }

                // Make new chamber
                info!("Added new chamber definition: {definition:?}");
                config.chambers.push(StasisChamberEntry { definition, occupants: vec![] });
                let chamber_len = config.chambers.len();
                return config.chambers.get_mut(chamber_len - 1);
            }
        }

        // As discovered at the beginning (weird this way because of stupid borrow checker, 11 PM me says...)
        if let Some(index) = use_existing_chamber_index {
            return config.chambers.get_mut(index);
        }
        None
    }

    pub fn on_pearl_throw(
        &self,
        bot: &mut Client,
        bot_state: &BotState,
        _player: Entity,
        player_name: &str,
        player_uuid: Uuid,
        player_pos: Vec3,
        pearl_uuid: Uuid,
        pearl_pos: Vec3,
        _pearl_id: MinecraftEntityId,
    ) {
        for chamber in &self.config.lock().chambers {
            if chamber.occupants.iter().any(|occupant| occupant.pearl_uuid == Some(pearl_uuid)) {
                trace!("Pearl {pearl_uuid} (at {pearl_pos}) is already noted as an occupant.");
                return;
            }
        }

        /*if player_pos.to_block_pos_floor().x != pearl_pos.to_block_pos_floor().x
            || player_pos.to_block_pos_floor().z != pearl_pos.to_block_pos_floor().z
            || (player_pos.y - pearl_pos.y).abs() >= 5.0
        {
            trace!("Pearl {pearl_uuid} is too far from player. Likely not a throw.");
            return;
        }*/

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
            if let Some(oc_player_uuid) = occupant.player_uuid
                && oc_player_uuid != player_uuid
            {
                warn!(
                    "Player {player_name} ({player_uuid}) threw a pearl into a chamber that had a pearl registed which was owned by another player with uuid {oc_player_uuid}! Clearing old occupants, but this is rude!",
                );
                owned_by_other = true;
            }
        }
        if owned_by_other {
            chamber.occupants.clear();
        }
        chamber.occupants.push(ChamberOccupant {
            player_uuid: Some(player_uuid),
            pearl_uuid: Some(pearl_uuid),
            thrown_at: chrono::Local::now(),
        });
        let chamber_definition = chamber.definition.clone();
        drop(config);
        let chambers_ordered = self.get_ordered_chambers(bot, player_name, player_uuid);
        if was_occupants_empty {
            if let Some(chat) = &bot_state.chat {
                if let Some(max_pearls) = self.max_pearls {
                    let further_allowed = max_pearls as i32 - chambers_ordered.len() as i32;
                    if further_allowed == 1 {
                        chat.msg(
                            player_name,
                            "You have thrown a pearl. Message me !tp to get back here (you are allowed to throw ONE MORE PEARL).",
                        );
                    } else if further_allowed == 0 {
                        chat.msg(
                            player_name,
                            format!(
                                "You have thrown a pearl. Message me !tp to get back here. You used up all your existing chambers ({max_pearls}). Please don't throw any further pearls!",

                            ),
                        );
                    } else {
                        chat.msg(
                            player_name,
                            format!("You have thrown a pearl. Message me !tp to get back here (you are allowed to throw {further_allowed} more pearls)."),
                        );
                    }
                } else {
                    chat.msg(player_name, "You have thrown a pearl. Message me !tp to get back here.");
                }
            }
        }

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

        // Check if player has too many pearls
        if let Some(max_pearls) = self.max_pearls {
            if chambers_ordered.len() > max_pearls as usize {
                info!(
                    "Detected that {player_name} (uuid: {player_uuid} has too many pearls ({} > {max_pearls}). Pulling some...",
                    chambers_ordered.len(),
                );
                if let Some(chat) = &bot_state.chat {
                    chat.msg(
                        player_name,
                        format!(
                            "Sorry, but your amount of chambers ({}) exceeds the allowed limit ({max_pearls}). Pulling your old pearls, hang on...",
                            chambers_ordered.len(),
                        ),
                    );
                    let player_name_clone = player_name.to_owned();
                    let feedback = Arc::new(move |_error: bool, message: &str| debug!("Ignored feedback to {player_name_clone}: {message}"));
                    for def in chambers_ordered[max_pearls as usize..].iter().rev() {
                        if let Err(err) = self.pull_chamber(player_name, def.clone(), bot, bot_state, feedback.clone()) {
                            error!("Could not pull a chamber of {player_name}: {err:?}");
                        }
                    }
                }
            }
        }
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

    pub fn get_ordered_chambers(&self, bot: &mut Client, player_name: &str, player_uuid: Uuid) -> Vec<StasisChamberDefinition> {
        let mut owned_chambers_with_times = HashMap::new();
        // Find definition of chamber with newest thrown pearl
        for chamber in &self.config.lock().chambers {
            let mut newest_time_from_player = None;
            let mut owned_by_other = false;
            for occupant in &chamber.occupants {
                if occupant.player_uuid == Some(player_uuid) {
                    if newest_time_from_player
                        .as_ref()
                        .map(|other_time| &occupant.thrown_at > other_time)
                        .unwrap_or(true)
                    {
                        newest_time_from_player = Some(occupant.thrown_at);
                    }
                } else if occupant.player_uuid.is_some() && occupant.pearl_uuid.is_some() {
                    owned_by_other = true;
                }
            }

            // Find chambers with no clear owner and a labeled sign
            if OPTS.chambers_use_sign_fallback && newest_time_from_player.is_none() && !owned_by_other && !chamber.occupants.is_empty() {
                let chamber_pos = match chamber.definition {
                    StasisChamberDefinition::FlippableTrapdoor { trapdoor_pos: pos }
                    | StasisChamberDefinition::SecuredFlippableTrapdoor { trigger_trapdoor_pos: pos, .. }
                    | StasisChamberDefinition::RedcoderTrapdoor { trapdoor_pos: pos, .. }
                    | StasisChamberDefinition::RedcoderShay { base_pos: pos, .. } => pos,
                    StasisChamberDefinition::RedstoneSingleTrigger { base_pos: pos, .. } => pos,
                    StasisChamberDefinition::RedstoneDoubleTrigger { base_pos: pos, .. } => pos,
                    StasisChamberDefinition::MultiPullPickles { pickles_pos: pos, .. } => pos,
                };

                let chunk_pos = ChunkPos::new(chamber_pos.x >> 4, chamber_pos.z >> 4);
                let world = bot.world();
                let world = world.read();
                if let Some(signs) = self.sign_cache.lock().get(&chunk_pos) {
                    'offsets: for y_offset in [0, -1, 1, -2, 2, -3, 3, -4, 4] {
                        let maybe_sign_pos = BlockPos::new(chamber_pos.x, chamber_pos.y + y_offset, chamber_pos.z);
                        if let Some(state) = world.get_block_state(&maybe_sign_pos) {
                            let block = Box::<dyn Block>::from(state);
                            if !block.id().ends_with("_sign") {
                                continue; // Not a sign anymore
                            }
                        } else {
                            continue; // No state here
                        }

                        if let Some(sign_data) = signs.get(&maybe_sign_pos) {
                            for side in vec![&sign_data.front, &sign_data.back] {
                                for line in &side.lines {
                                    if line.to_string().eq_ignore_ascii_case(player_name) {
                                        // Found name on sign! Use newest pearl time or just now:
                                        for occupant in &chamber.occupants {
                                            if newest_time_from_player
                                                .as_ref()
                                                .map(|other_time| &occupant.thrown_at > other_time)
                                                .unwrap_or(true)
                                            {
                                                newest_time_from_player = Some(occupant.thrown_at);
                                            }
                                        }
                                        info!(
                                            "Assuming chamber at {chamber_pos} ({}) is owned by {player_name} ({player_uuid}) because sign at {maybe_sign_pos} contained their name!",
                                            chamber.definition.type_name(),
                                        );
                                        /*if newest_time_from_player.is_none() {
                                            // Placeholder because we're clueless
                                            newest_time_from_player = Some(chrono::Local::now());
                                        }*/
                                        break 'offsets;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if let Some(newest_time_from_player) = newest_time_from_player {
                owned_chambers_with_times.insert(chamber.definition.clone(), newest_time_from_player);
            }
        }

        let mut chambers_ordered: Vec<StasisChamberDefinition> = owned_chambers_with_times.keys().map(|def| def.clone()).collect();
        chambers_ordered.sort_by(|a, b| owned_chambers_with_times[&b].cmp(&owned_chambers_with_times[&a]));
        chambers_ordered
    }

    pub fn pull_pearl<F: Fn(/*error*/ bool, &str) + Send + Sync + 'static>(
        &self,
        username: &str,
        bot: &mut Client,
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
        let chambers_ordered = self.get_ordered_chambers(bot, &username, uuid);

        if chambers_ordered.is_empty() {
            feedback(true, "No chamber found!");
            return Ok(());
        }

        if chamber_index >= chambers_ordered.len() {
            chamber_index = chambers_ordered.len() - 1;
        }
        let definition = chambers_ordered.get(chamber_index).unwrap().clone();

        if bot_state.tasks() > 0 {
            feedback(false, "Hang on, will walk to your stasis chamber in due time...");
        } else {
            feedback(false, &format!("Pulling your stasis chamber... (you got {})", chambers_ordered.len()));
        }

        self.pull_chamber(&username, definition, bot, &bot_state, Arc::new(feedback))?;
        Ok(())
    }

    pub fn remove_uncertain_occupants(
        &self,
        bot: &mut Client,
        definition: &StasisChamberDefinition,
        with_no_pearl_uuid: bool,
        with_no_player_uuid: bool,
        pearl_not_found: bool,
    ) {
        let mut config_changed = false;
        for chamber in &mut self.config.lock().chambers {
            if &chamber.definition != definition {
                continue;
            }

            let mut remove_occupant_indices = Vec::new();
            for (occupant_index, occupant) in &mut chamber.occupants.iter().enumerate() {
                if with_no_pearl_uuid && occupant.pearl_uuid.is_none() {
                    remove_occupant_indices.push(occupant_index);
                    warn!("Removing uncertain occupant preemptively (no pearl uuid, likely migrated from legacy chamber): {occupant:?}");
                } else if with_no_player_uuid && occupant.player_uuid.is_none() {
                    remove_occupant_indices.push(occupant_index);
                    warn!("Remove uncertain occupant preemptively (no player uuid, likely thrown while offline): {occupant:?}");
                } else if pearl_not_found && let Some(pearl_uuid) = occupant.pearl_uuid
                    && bot.world().read().chunks.get(&ChunkPos::new(definition.rough_pos().x >> 4, definition.rough_pos().z >> 4)).is_some()
                    && definition.rough_pos().center().horizontal_distance_squared_to(&Vec3::from(&bot.component::<Position>())) <= 58f64.powi(2) /*In range by ~6 blocks */
                    && bot.entity_by::<With<EnderPearl>, (&EntityUuid,)>(|(uuid,): &(&EntityUuid,)| pearl_uuid == ***uuid).is_none()
                {
                    warn!("Remove uncertain occupant preemptively (pearl not found by uuid, likely despawned while offline): {occupant:?}");
                    remove_occupant_indices.push(occupant_index);
                }
            }
            for (occupant_index_index, occupant_index) in remove_occupant_indices.into_iter().enumerate() {
                chamber.occupants.remove(occupant_index - occupant_index_index);
                config_changed = true;
            }
        }
        if config_changed {
            let self_clone = self.clone();
            tokio::task::spawn(async move {
                if let Err(err) = self_clone.save_config().await {
                    error!("Failed to save stasis-config: {err:?}");
                }
            });
        }
    }

    pub fn pull_chamber<F: Fn(/*error*/ bool, &str) + Send + Sync + 'static>(
        &self,
        occupant: &str,
        definition: StasisChamberDefinition,
        bot: &mut Client,
        bot_state: &BotState,
        feedback: Arc<F>,
    ) -> anyhow::Result<()> {
        info!("Pulling chamber {definition:?} for {occupant}...");

        let mut has_null_pearl = false;
        let mut occupying_pearl_uuids = Vec::new();
        for chamber in &self.config.lock().chambers {
            if chamber.definition == definition {
                for occupant in &chamber.occupants {
                    if let Some(pearl_uuid) = occupant.pearl_uuid {
                        occupying_pearl_uuids.push(pearl_uuid);
                    } else {
                        has_null_pearl = true;
                    }
                }
            }
        }

        match definition {
            StasisChamberDefinition::FlippableTrapdoor { trapdoor_pos: trigger_pos }
            | StasisChamberDefinition::SecuredFlippableTrapdoor {
                trigger_trapdoor_pos: trigger_pos,
                ..
            }
            | StasisChamberDefinition::MultiPullPickles {
                pull_trigger_pos: trigger_pos, ..
            } => {
                let trigger_goal: Arc<BoxedPathfindGoal> = Arc::new(BoxedPathfindGoal::new(InteractableGoal::new(trigger_pos)));
                let return_goal = goals::BlockPosGoal(self.return_pos(&mut bot.clone()).expect("No return pos"));
                let greeting = format!("Welcome back, {occupant}!");

                let mut group = TaskGroup::new(format!("Pull {occupant}'s chamber"));
                group = group.with(PathfindTask::new_concrete(!OPTS.no_mining, trigger_goal, format!("near {occupant}'s Pearl")));

                // Has closed security trapdoor: open it
                if let StasisChamberDefinition::SecuredFlippableTrapdoor { security_trapdoor_pos, .. } = definition
                    && let Some(state) = bot.world().read().get_block_state(&security_trapdoor_pos)
                    && !state.property::<Open>().unwrap_or(false)
                {
                    group.add(AffectBlockTask::new(security_trapdoor_pos)); // Open security trapdoor
                }

                // Multipull has pearls at throw pos, should trigger align trigger to move to pickle poss
                if let StasisChamberDefinition::MultiPullPickles {
                    align_trigger_pos, throw_pos, ..
                } = definition
                {
                    let mut any_pearls_at_throw_pos = false;
                    for chamber in &self.config.lock().chambers {
                        if chamber.definition == definition {
                            for occupant in &chamber.occupants {
                                let pearl_entity = bot.entity_by::<With<EnderPearl>, (&EntityUuid,)>(|(entity_uuid,): &(&EntityUuid,)| {
                                    Some(***entity_uuid) == occupant.pearl_uuid
                                });
                                if let Some(pearl_entity) = pearl_entity
                                    && let Some(pearl_pos) = bot.get_entity_component::<Position>(pearl_entity)
                                {
                                    let pearl_block_pos = pearl_pos.to_block_pos_floor();
                                    if pearl_block_pos.x == throw_pos.x && pearl_block_pos.z == throw_pos.z {
                                        any_pearls_at_throw_pos = true;
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    if any_pearls_at_throw_pos {
                        group = group.with(AffectBlockTask::new(align_trigger_pos)).with(DelayTicksTask::new(40)); // Wait some time (2s)
                    }
                }

                let mut trigger_group = TaskGroup::new_with_hide_fail("Check and trigger", true);

                let occupying_pearl_uuids_clone = occupying_pearl_uuids.clone();
                let feedback_clone = feedback.clone();
                let (self_clone, self_clone_2, self_clone_3) = (self.clone(), self.clone(), self.clone());
                let definition_clone = definition.clone();
                trigger_group.add(OnceFuncTask::new("Check if any pearl exists", move |mut bot, bot_state| {
                    let any_pearl = has_null_pearl
                        || bot
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
                            true,
                            "Sorry, but it seems this stasis chamber has no pearls in it! I removed it. Try again to pull the next if you got one.",
                        );
                        bail!("Chamber had no pearls!");
                    }
                    Ok(())
                }));
                let mut bot_clone = bot.clone();
                let is_trigger_a_flippable_trapdoor = matches!(
                    definition,
                    StasisChamberDefinition::FlippableTrapdoor { .. } | StasisChamberDefinition::SecuredFlippableTrapdoor { .. }
                );
                trigger_group = trigger_group
                    .with(OnceFuncTask::new("Add expected despawning pearls", move |_bot, _bot_state| {
                        let mut expect_despawn_of = self_clone.expect_despawn_of.lock();
                        occupying_pearl_uuids.iter().for_each(|pearl_uuid| {
                            expect_despawn_of.insert(*pearl_uuid);
                        });
                        Ok(())
                    }))
                    .with(AffectBlockTask::new(trigger_pos))
                    .with(OnceFuncTask::new(format!("Greet {occupant}"), move |_bot, _bot_state| {
                        feedback(false, &greeting);
                        Ok(())
                    }))
                    .with(OnceFuncTask::new("Clean uncertain pearls", move |_bot, _bot_state| {
                        self_clone_2.remove_uncertain_occupants(&mut bot_clone, &definition, true, true, true);
                        Ok(())
                    }));

                if self.do_reopen_trapdoor && is_trigger_a_flippable_trapdoor {
                    trigger_group = trigger_group
                        .with(DelayTicksTask::new(Self::recommended_closed_trapdoor_ticks(bot_state)))
                        .with(AffectBlockTask::new(trigger_pos))
                }

                group.add(trigger_group);
                group.add(PathfindTask::new(!OPTS.no_mining, return_goal, "old spot"));

                bot_state.add_task(group);
            }
            StasisChamberDefinition::RedcoderTrapdoor { ref endpoint, .. } | StasisChamberDefinition::RedcoderShay { ref endpoint, .. } => {
                let endpoint = endpoint.clone();
                let mut terminal = None;
                for lectern_redcoder_terminal in &self.config.lock().lectern_redcoder_terminals {
                    if lectern_redcoder_terminal.id == endpoint.lectern_redcoder_terminal_id {
                        terminal = Some(lectern_redcoder_terminal.clone());
                    }
                }
                if terminal.is_none() {
                    feedback(
                        true,
                        &format!(
                            "Skill issue! I can't find details on LecternRedcoderTerminal \"{}\"!",
                            endpoint.lectern_redcoder_terminal_id
                        ),
                    );
                    return Ok(());
                }
                let terminal = terminal.unwrap();

                let lectern_goal: Arc<BoxedPathfindGoal> = Arc::new(BoxedPathfindGoal::new(InteractableGoal::new(terminal.lectern)));
                let button_goal: Arc<BoxedPathfindGoal> = Arc::new(BoxedPathfindGoal::new(InteractableGoal::new(terminal.button)));
                let return_goal = goals::BlockPosGoal(self.return_pos(&mut bot.clone()).expect("No return pos"));

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
                        // Do not use anything that sends a ContainerCloseEvent after this!
                        // It somehow breaks the page selection (probably weird event order)!
                        Ok(())
                    }))
                    .with(CloseInventoryAndSyncTask::default());

                let mut trigger_group = TaskGroup::new_with_hide_fail("Check and trigger", true);

                let occupying_pearl_uuids_clone = occupying_pearl_uuids.clone();
                let feedback_clone = feedback.clone();
                let (self_clone, self_clone_2, self_clone_3) = (self.clone(), self.clone(), self.clone());
                let definition_clone = definition.clone();
                trigger_group.add(OnceFuncTask::new("Check if any pearl exists", move |mut bot, bot_state| {
                    let any_pearl = has_null_pearl
                        || bot
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
                            true,
                            "Sorry, but it seems this redcoder stasis chamber has no pearls in it! I removed it. Try again to pull the next if you got one.",
                        );
                        bail!("Chamber had no pearls!");
                    }
                    Ok(())
                }));
                let mut bot_clone = bot.clone();
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
                    .with(OnceFuncTask::new("Clean uncertain pearls", move |_bot, _bot_state| {
                        self_clone_2.remove_uncertain_occupants(&mut bot_clone, &definition, true, true, true);
                        Ok(())
                    }));

                group.add(trigger_group);
                group.add(PathfindTask::new(!OPTS.no_mining, return_goal, "old spot"));

                bot_state.add_task(group);
            }
            StasisChamberDefinition::RedstoneSingleTrigger { trigger_pos, .. } | StasisChamberDefinition::RedstoneDoubleTrigger { trigger_pos, .. } => {
                let trapdoor_goal: Arc<BoxedPathfindGoal> = Arc::new(BoxedPathfindGoal::new(InteractableGoal::new(trigger_pos.to_owned())));
                let return_goal = goals::BlockPosGoal(self.return_pos(&mut bot.clone()).expect("No return pos"));
                let greeting = format!("Welcome back, {occupant}!");

                let mut group = TaskGroup::new(format!("Pull {occupant}'s chamber"));
                group = group.with(PathfindTask::new_concrete(
                    !OPTS.no_mining,
                    trapdoor_goal,
                    format!("near trigger Block for {occupant}'s Chamber"),
                ));

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
                            true,
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
                    .with(AffectBlockTask::new(trigger_pos));

                if let StasisChamberDefinition::RedstoneDoubleTrigger { delay_ticks, .. } = definition {
                    trigger_group.add(DelayTicksTask::new(delay_ticks));
                    trigger_group.add(AffectBlockTask::new(trigger_pos));
                }

                let mut bot_clone = bot.clone();
                trigger_group = trigger_group
                    .with(OnceFuncTask::new(format!("Greet {occupant}"), move |_bot, _bot_state| {
                        feedback(false, &greeting);
                        Ok(())
                    }))
                    .with(OnceFuncTask::new("Clean uncertain pearls", move |_bot, _bot_state| {
                        self_clone_2.remove_uncertain_occupants(&mut bot_clone, &definition, true, true, true);
                        Ok(())
                    }));

                group.add(trigger_group);
                group.add(PathfindTask::new(!OPTS.no_mining, return_goal, "old spot"));

                bot_state.add_task(group);
            }
        }
        Ok(())
    }

    pub fn check_occupants_near_despawned_pearls(&self, bot: &mut Client) {
        let mut remove_indices: Vec<usize> = Vec::with_capacity(0);
        let mut remove_pearl_uuids: HashSet<Uuid> = HashSet::with_capacity(0);
        let mut remove_occupant_if_player_gets_near = self.remove_occupant_if_player_gets_near.lock();

        for (index, (pearl_uuid, chamber_pos, player_uuid, until)) in remove_occupant_if_player_gets_near.iter().enumerate() {
            let player_entity = bot.entity_by::<With<Player>, (&GameProfileComponent,)>(|(profile,): &(&GameProfileComponent,)| &profile.uuid == player_uuid);
            if until <= &Instant::now() {
                remove_indices.push(index);
                continue;
            }
            if let Some(player_entity) = player_entity
                && let Some(player_pos) = bot.get_entity_component::<Position>(player_entity)
            {
                let own_pos = bot.component::<Position>();
                if player_pos.horizontal_distance_squared_to(&chamber_pos.center()) <= 2.5f64.powi(2)
                    && own_pos.horizontal_distance_squared_to(&chamber_pos.center()) <= 56.0f64.powi(2)
                {
                    info!(
                        "Pearl {} likely was removed when teleporting the owner ({}) to it (owner came back later). Removing it.",
                        *pearl_uuid, player_uuid
                    );
                    remove_indices.push(index);
                    remove_pearl_uuids.insert(*pearl_uuid);
                }
            }
        }

        for (occupant_index_index, occupant_index) in remove_indices.into_iter().enumerate() {
            remove_occupant_if_player_gets_near.remove(occupant_index - occupant_index_index);
        }

        if !remove_pearl_uuids.is_empty() {
            let mut config_changed = false;
            for chamber in &mut self.config.lock().chambers {
                let mut remove_occupant_indices = Vec::with_capacity(0);
                for (occupant_index, occupant) in chamber.occupants.iter().enumerate() {
                    if let Some(ref pearl_uuid) = occupant.pearl_uuid
                        && remove_pearl_uuids.contains(pearl_uuid)
                    {
                        remove_occupant_indices.push(occupant_index);
                    }
                }
                for (occupant_index_index, occupant_index) in remove_occupant_indices.into_iter().enumerate() {
                    chamber.occupants.remove(occupant_index - occupant_index_index);
                    config_changed = true;
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
    }

    fn on_block_entity(&self, pos: BlockPos, kind: BlockEntityKind, data: &Nbt) {
        if !OPTS.chambers_use_sign_fallback {
            return;
        }
        if kind != BlockEntityKind::Sign && kind != BlockEntityKind::HangingSign {
            return;
        }
        let chunk_pos = ChunkPos::new(pos.x >> 4, pos.z >> 4);
        let mut sign_cache = self.sign_cache.lock();
        match data {
            Nbt::None => {
                let remove_chunk = if let Some(chunk) = sign_cache.get_mut(&chunk_pos) {
                    if chunk.remove(&pos).is_some() {
                        trace!("Removed sign data at {pos}.");
                    } else {
                        trace!("There was no sign data at {pos}.");
                    }
                    chunk.is_empty()
                } else {
                    trace!("Chunk for sign at {pos} is not in cache.");
                    false
                };
                if remove_chunk {
                    sign_cache.remove(&chunk_pos);
                    trace!("Removed chunk {}, {} from sign cache because it is/became empty.", chunk_pos.x, chunk_pos.z);
                }
            }
            Nbt::Some(data) => match SignBlockEntity::from_nbt(&*data) {
                Ok(sign) => {
                    trace!("Sign-Data at {pos}: {sign:?}");
                    sign_cache.entry(chunk_pos).or_default().insert(pos, sign);
                }
                Err(err) => {
                    warn!("Failed to parse sign block entity at {pos}: {err:?}");
                }
            },
        }
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
                info!("Loading stasis-config...");
                if let Err(err) = self.load_config().await {
                    error!("Failed to load stasis-config: {err:?}");
                    std::fs::rename(
                        Self::config_path(),
                        format!("{}.broken", Self::config_path().as_os_str().to_str().ok_or(anyhow!("Path err"))?),
                    )?;
                    self.load_config().await?;
                }
                self.clear_idle_pos("Login-Event");
                self.spawned_pearl_uuids.lock().clear();
                self.expect_despawn_of.lock().clear();
                self.sign_cache.lock().clear();
            }
            Event::Disconnect(_) => {
                self.clear_idle_pos("Disconnect-Event");
                self.spawned_pearl_uuids.lock().clear();
                self.expect_despawn_of.lock().clear();
                self.sign_cache.lock().clear();
            }
            Event::Packet(packet) => match packet.as_ref() {
                /*ClientboundGamePacket::PlayerPosition(_) => {
                    self.clear_idle_pos("Got teleport-Packet");
                }*/
                ClientboundGamePacket::Respawn(_) => {
                    self.spawned_pearl_uuids.lock().clear();
                    self.expect_despawn_of.lock().clear();
                    self.sign_cache.lock().clear();
                }
                ClientboundGamePacket::AddEntity(packet) => {
                    if packet.entity_type == EntityKind::EnderPearl {
                        // Ignore
                        {
                            let pos = packet.position.to_block_pos_floor();
                            if OPTS.pearls_min_pos.map(|min| pos.x < min.x || pos.y < min.y || pos.z < min.z).unwrap_or(false)
                                || OPTS.pearls_max_pos.map(|max| pos.x > max.x || pos.y > max.y || pos.z > max.z).unwrap_or(false)
                            {
                                info!(
                                    "Ignoring Pearl ({} at {}) as it either below --pearls-min-pos or above --pearls-above-pos",
                                    packet.position, packet.uuid
                                );
                                return Ok(());
                            }
                        }

                        self.spawned_pearl_uuids.lock().insert(packet.id, EntityUuid::new(packet.uuid));

                        let owning_player_entity_id = packet.data as i32;
                        let player_entity =
                            bot.entity_by::<With<Player>, (&MinecraftEntityId,)>(|(entity_id,): &(&MinecraftEntityId,)| entity_id.0 == owning_player_entity_id);

                        if let Some(player_entity) = player_entity {
                            let game_profile = bot.entity_component::<GameProfileComponent>(player_entity);
                            let player_pos = bot.entity_component::<Position>(player_entity);
                            self.on_pearl_throw(
                                &mut bot,
                                bot_state,
                                player_entity,
                                &game_profile.name,
                                game_profile.uuid,
                                Vec3::from(&player_pos),
                                packet.uuid,
                                packet.position,
                                packet.id,
                            );
                        } else {
                            let pearl_uuid = packet.uuid;
                            let pearl_pos = packet.position;
                            let mut config = self.config.lock();
                            for chamber in &config.chambers {
                                if chamber.occupants.iter().any(|occupant| occupant.pearl_uuid == Some(pearl_uuid)) {
                                    return Ok(()); // Known pearl
                                }
                            }

                            if let Some(chamber) = Self::chamber_for_pearl_pos(&mut bot, &mut config, pearl_pos) {
                                chamber.occupants.push(ChamberOccupant {
                                    pearl_uuid: Some(pearl_uuid),
                                    player_uuid: None,
                                    thrown_at: chrono::Local::now(),
                                });
                                warn!(
                                    "Detected new Pearl ({pearl_uuid} at {pearl_pos}) with no known owning player (owner id: {owning_player_entity_id}). Found a chamber with {} existent occupants and assigned it to that.",
                                    chamber.occupants.len() - 1
                                );
                                let self_clone = self.clone();
                                tokio::spawn(async move {
                                    if let Err(err) = self_clone.save_config().await {
                                        error!("Failed to save stasis-config: {err:?}");
                                    }
                                });
                            } else {
                                warn!(
                                    "Detected new Pearl ({pearl_uuid} at {pearl_pos}) with no known owning player (owner id: {owning_player_entity_id}). No fitting chamber found to assign it to."
                                );
                            }
                        }
                    } else if packet.entity_type == EntityKind::Player {
                        self.check_occupants_near_despawned_pearls(&mut bot);
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
                                    StasisChamberDefinition::RedstoneSingleTrigger { base_pos: pos, .. } => pos,
                                    StasisChamberDefinition::RedstoneDoubleTrigger { base_pos: pos, .. } => pos,
                                    StasisChamberDefinition::MultiPullPickles { pickles_pos: pos, .. } => pos,
                                };

                                let own_pos = Vec3::from(&bot.component::<Position>());
                                let mut remove_occupant_indices = Vec::new();
                                for (occupant_index, occupant) in chamber.occupants.iter().enumerate() {
                                    if let Some(oc_pearl_uuid) = occupant.pearl_uuid
                                        && let Some(oc_player_uuid) = occupant.player_uuid
                                        && oc_pearl_uuid == *pearl_uuid
                                    {
                                        let player_entity = bot.entity_by::<With<Player>, (&EntityUuid,)>(|(entity_uuid,): &(&EntityUuid,)| {
                                            Some(***entity_uuid) == occupant.player_uuid
                                        });
                                        let mut added = false;
                                        if let Some(player_entity) = player_entity {
                                            let player_pos = bot.entity_component::<Position>(player_entity);
                                            if player_pos.horizontal_distance_squared_to(&chamber_pos.center()) <= 2.0f64.powi(2)
                                                && own_pos.horizontal_distance_squared_to(&chamber_pos.center()) <= 56.0f64.powi(2)
                                            {
                                                info!(
                                                    "Pearl {} likely was removed when teleporting the owner ({}) to it. Removing it.",
                                                    *pearl_uuid, oc_player_uuid
                                                );
                                                remove_occupant_indices.push(occupant_index);
                                                added = true;
                                            } else {
                                                self.remove_occupant_if_player_gets_near.lock().push((
                                                    *pearl_uuid,
                                                    chamber_pos,
                                                    oc_player_uuid,
                                                    Instant::now() + Duration::from_secs(3),
                                                ));
                                            }
                                        } else {
                                            self.remove_occupant_if_player_gets_near.lock().push((
                                                *pearl_uuid,
                                                chamber_pos,
                                                oc_player_uuid,
                                                Instant::now() + Duration::from_secs(3),
                                            ));
                                        }

                                        if !added && expected && own_pos.horizontal_distance_squared_to(&chamber_pos.center()) <= 56.0f64.powi(2) {
                                            info!("Despawning of Pearl {} (owned by {}) was expected. Removing it.", *pearl_uuid, oc_player_uuid);
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
                ClientboundGamePacket::SetEntityData(packet) => {
                    let mut is_interacting = false;
                    for item in &packet.packed_items.0 {
                        if item.index == 8
                            && let EntityDataValue::Byte(value) = item.value
                            && value & 0x01 > 0
                        {
                            is_interacting = true;
                            break;
                        }
                    }
                    if !is_interacting {
                        return Ok(());
                    }

                    let mut mass_adding = self.mass_adding.lock();
                    if !mass_adding.is_empty()
                        && let Some(player_entity) =
                            bot.entity_by::<With<Player>, (&MinecraftEntityId,)>(|(mc_id,): &(&MinecraftEntityId,)| **mc_id == packet.id)
                    {
                        let player_uuid = bot.entity_component::<EntityUuid>(player_entity);
                        if let Some((terminal_name, is_shay, index)) = mass_adding.get_mut(&*player_uuid) {
                            let player_pos = bot.entity_component::<Position>(player_entity);

                            let mut config = self.config.lock();
                            for chamber in &config.chambers {
                                match &chamber.definition {
                                    StasisChamberDefinition::RedcoderShay { endpoint, .. } | StasisChamberDefinition::RedcoderTrapdoor { endpoint, .. } => {
                                        if &endpoint.lectern_redcoder_terminal_id == terminal_name && endpoint.chamber_index == *index {
                                            warn!("Skipping existing index {index} on terminal {terminal_name}");
                                            *index += 1;
                                            return Ok(());
                                        }
                                    }
                                    _ => {}
                                }
                            }

                            if *is_shay {
                                config.chambers.push(StasisChamberEntry {
                                    definition: StasisChamberDefinition::RedcoderShay {
                                        endpoint: LecternRedcoderEndpoint {
                                            lectern_redcoder_terminal_id: terminal_name.to_owned(),
                                            chamber_index: *index,
                                        },
                                        base_pos: player_pos.to_block_pos_floor(),
                                    },
                                    occupants: Vec::new(),
                                });
                            } else {
                                config.chambers.push(StasisChamberEntry {
                                    definition: StasisChamberDefinition::RedcoderTrapdoor {
                                        endpoint: LecternRedcoderEndpoint {
                                            lectern_redcoder_terminal_id: terminal_name.to_owned(),
                                            chamber_index: *index,
                                        },
                                        trapdoor_pos: player_pos.to_block_pos_floor(),
                                    },
                                    occupants: Vec::new(),
                                });
                            }
                            info!(
                                "Added endpoint index {index} to terminal {terminal_name} by Player (uuid: {}) interacting (mass add)",
                                *player_uuid
                            );
                            let self_clone = self.clone();
                            tokio::spawn(async move {
                                if let Err(err) = self_clone.save_config().await {
                                    error!("Failed to save stasis-config: {err:?}");
                                }
                            });
                            *index += 1;
                        }
                    }
                }
                ClientboundGamePacket::Animate(packet) => {
                    if packet.action as usize != AnimationAction::SwingMainHand as usize {
                        return Ok(());
                    }

                    let mut mass_adding = self.mass_adding.lock();
                    if !mass_adding.is_empty()
                        && let Some(player_entity) =
                            bot.entity_by::<With<Player>, (&MinecraftEntityId,)>(|(mc_id,): &(&MinecraftEntityId,)| **mc_id == packet.id)
                    {
                        let profile = bot.entity_component::<GameProfileComponent>(player_entity);
                        if let Some((terminal_name, _is_shay, index)) = mass_adding.remove(&profile.uuid) {
                            info!(
                                "{} (uuid: {}) finished mass adding to terminal {terminal_name}. Next id was {index}",
                                profile.name, profile.uuid
                            );
                            if let Some(chat) = &bot_state.chat {
                                chat.msg(&profile.name, format!("Mass adding finished! Last added index was {}", index - 1))
                            }
                        }
                    }
                }
                ClientboundGamePacket::BlockEntityData(packet) => {
                    self.on_block_entity(packet.pos, packet.block_entity_type, &packet.tag);
                }
                ClientboundGamePacket::LevelChunkWithLight(packet) => {
                    for block_entity in &packet.chunk_data.block_entities {
                        self.on_block_entity(
                            BlockPos::new(
                                (packet.x << 4) + (block_entity.packed_xz >> 4) as i32,
                                (block_entity.y as i16) as i32,
                                (packet.z << 4) + (block_entity.packed_xz & 0x0F) as i32,
                            ),
                            block_entity.kind,
                            &block_entity.data,
                        );
                    }
                }
                ClientboundGamePacket::ForgetLevelChunk(packet) => {
                    if self.sign_cache.lock().remove(&packet.pos).is_some() {
                        trace!("Removed chunk {}, {} from sign cache because chunk got unloaded.", packet.pos.x, packet.pos.z);
                    }
                }
                _ => {}
            },
            Event::Tick => {
                if bot_state.tasks() > 0 {
                    *self.last_task_seen_at.lock() = Instant::now();
                }
                if self.last_task_seen_at.lock().elapsed() >= Duration::from_secs(3) {
                    self.update_idle_pos(&mut bot);
                }

                self.check_occupants_near_despawned_pearls(&mut bot);
            }
            _ => {}
        }
        Ok(())
    }
}
