use crate::BotState;
use crate::blockpos_string;
use crate::module::Module;
use crate::task::delay_ticks::DelayTicksTask;
use crate::task::group::TaskGroup;
use crate::task::oncefunc::OnceFuncTask;
use crate::task::pathfind::PathfindTask;
use crate::task::sip_beer::SipBeerTask;
use crate::task::swing_beer::SwingBeerTask;
use anyhow::{Context, anyhow};
use azalea::ecs::entity::Entity;
use azalea::ecs::prelude::With;
use azalea::entity::metadata::Player;
use azalea::entity::{EntityDataValue, EyeHeight, LookDirection, Pose, Position};
use azalea::inventory::operations::ClickType;
use azalea::inventory::{Inventory, ItemStack};
use azalea::packet::game::SendPacketEvent;
use azalea::pathfinder::goals::BlockPosGoal;
use azalea::protocol::packets::game::c_animate::AnimationAction;
use azalea::protocol::packets::game::c_set_equipment::EquipmentSlot;
use azalea::protocol::packets::game::{ClientboundGamePacket, ServerboundContainerClick, ServerboundGamePacket, ServerboundMovePlayerRot};
use azalea::registry::Item;
use azalea::world::MinecraftEntityId;
use azalea::{BlockPos, BotClientExt, Client, Event, GameProfileComponent, Vec3};
use parking_lot::Mutex;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ops::Add;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct BeerConfig {
    pub max_beer: u32,
    pub throw_pitch: f32,
    pub jump_before_throw: bool,
    pub beer_handed_out: HashMap<String, u32>,
    pub sip_random_min_secs: u64,
    pub sip_random_max_secs: u64,

    pub sip_when_others_sip: bool,
    pub prost_when_others_prost: bool,

    pub messages_come_closer: Vec<String>,
    pub messages_denied_beer: Vec<String>,
    pub messages_gave_beer: Vec<String>,
    pub messages_gave_last_beer: Vec<String>,
    pub messages_out_of_beer: Vec<String>,

    #[serde(with = "blockpos_string::vec")]
    pub random_positions: Vec<BlockPos>,
    pub random_positions_min_secs: u64,
    pub random_positions_max_secs: u64,
}

impl Default for BeerConfig {
    fn default() -> Self {
        Self {
            max_beer: 10,
            throw_pitch: -30.0,
            jump_before_throw: false,
            beer_handed_out: Default::default(),
            sip_random_min_secs: 5,
            sip_random_max_secs: 15,

            sip_when_others_sip: true,
            prost_when_others_prost: true,

            messages_come_closer: vec!["Come closer".to_owned()],
            messages_denied_beer: vec!["You had enough buddy".to_owned()],
            messages_gave_beer: vec![],
            messages_gave_last_beer: vec!["This was your last one".to_owned()],
            messages_out_of_beer: vec!["Uhm... This is akward... I'm out of beer!".to_owned()],

            random_positions: vec![],
            random_positions_min_secs: 30,
            random_positions_max_secs: 120,
        }
    }
}

#[derive(Clone, Default)]
pub struct BeertenderModule {
    pub config: Arc<Mutex<BeerConfig>>,

    random_sipping: Arc<Mutex<Option<(u64 /*Ticks*/, u64 /*Target ticks*/)>>>,
    random_positions: Arc<Mutex<Option<(u64 /*Ticks*/, u64 /*Target ticks*/)>>>,
    holds_beer: Arc<Mutex<HashSet<i32>>>,
}

impl BeertenderModule {
    pub fn config_path() -> PathBuf {
        PathBuf::from("beertender-config.json")
    }

    pub async fn load_config(&self) -> anyhow::Result<()> {
        let config_path = Self::config_path();
        if config_path.exists() && !config_path.is_dir() {
            *self.config.lock() = serde_json::from_str(&tokio::fs::read_to_string(config_path).await.context("Read beertender-config file")?)
                .context("Parsing beertender-config content")?;
            info!("Loaded beertender-config from file.");
            let config = self.config.lock();
            if config.sip_random_min_secs > config.sip_random_max_secs {
                warn!("Random sipping will not work as sip_random_min_secs must be lower or equal to sip_random_max_secs!");
            }
            if config.random_positions_min_secs > config.random_positions_max_secs {
                warn!("Random positions will not work as random_positions_min_secs must be lower or equal to random_positions_max_secs!");
            }
        } else {
            *self.config.lock() = Default::default();
            warn!("File for beertender-config doesn't exist, yet.");
            self.save_config().await?;
        };

        Ok(())
    }

    pub async fn save_config(&self) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(&*self.config.as_ref().lock()).context("Convert beertender-config to json")?;
        tokio::fs::write(Self::config_path(), json).await.context("Save beertender-config as file")?;
        Ok(())
    }

    fn had_enough_beer(&self, username: &str) -> bool {
        let mut config = self.config.lock();
        *config.beer_handed_out.entry(username.to_lowercase()).or_default() >= config.max_beer
    }

    fn gave_beer(&self, username: &str) -> anyhow::Result<()> {
        *self.config.lock().beer_handed_out.entry(username.to_lowercase()).or_default() += 1;
        let self_clone = self.clone();
        tokio::spawn(async move {
            if let Err(err) = self_clone.save_config().await {
                error!("Failed to save beerender-config: {err}");
            }
        });
        Ok(())
    }

    fn random_message(messages: &[impl AsRef<str>]) -> Option<&str> {
        if messages.is_empty() {
            return None;
        }
        Some(messages[rand::rng().random_range(0..messages.len())].as_ref())
    }

    pub fn find_beer_slot(bot: &mut Client) -> Option<u16> {
        // Find first beer item in hotbar
        let inv = bot.entity_component::<Inventory>(bot.entity);
        let inv_menu = inv.inventory_menu;
        for (slot, _itemstack) in inv_menu.slots().iter().enumerate() {
            if let Some(item_stack_data) = inv_menu.slot(slot).and_then(|stack| stack.as_present()) {
                if item_stack_data.kind == Item::HoneyBottle {
                    return Some(slot as u16);
                }
            }
        }

        None
    }

    pub fn beer_count(bot: &mut Client) -> u32 {
        let mut beer = 0;
        let inv = bot.entity_component::<Inventory>(bot.entity);
        let inv_menu = inv.inventory_menu;
        for (slot, _itemstack) in inv_menu.slots().iter().enumerate() {
            if let Some(item_stack_data) = inv_menu.slot(slot).and_then(|stack| stack.as_present()) {
                if item_stack_data.kind == Item::HoneyBottle {
                    beer += item_stack_data.count.max(0) as u32;
                }
            }
        }
        beer
    }

    pub fn request_beer<F: Fn(/*error*/ bool, &str) + Send + Sync + 'static>(
        &self,
        username: &str,
        bot: &mut Client,
        bot_state: &BotState,
        feedback: F,
    ) -> anyhow::Result<()> {
        let username = username.to_owned();

        let own_pos = bot.entity_component::<Position>(bot.entity);
        let own_eye_pos = {
            let eye_height = bot.entity_component::<EyeHeight>(bot.entity);
            let pose = bot.entity_component::<Pose>(bot.entity);

            let y_offset = match pose {
                Pose::FallFlying | Pose::Swimming | Pose::SpinAttack => 0.5f64,
                Pose::Sleeping => 0.25f64,
                Pose::Sneaking => (*eye_height as f64) * 0.85,
                _ => *eye_height as f64,
            };
            own_pos.add(Vec3::new(0.0, y_offset, 0.0))
        };

        let (sender_pos, sender_eye_pos) = if let Some(sender_entity) =
            bot.entity_by::<With<Player>, (&GameProfileComponent,)>(|(profile,): &(&GameProfileComponent,)| profile.name == username)
        {
            let pos = bot.entity_component::<Position>(sender_entity);
            let eye_height = bot.entity_component::<EyeHeight>(sender_entity);
            let pose = bot.entity_component::<Pose>(sender_entity);

            let y_offset = match pose {
                Pose::FallFlying | Pose::Swimming | Pose::SpinAttack => 0.5f64,
                Pose::Sleeping => 0.25f64,
                Pose::Sneaking => (*eye_height as f64) * 0.85,
                _ => *eye_height as f64,
            };
            (pos, pos.add(Vec3::new(0.0, y_offset, 0.0)))
        } else {
            feedback(true, "Is it the beer or why can't I see you?");
            return Ok(());
        };

        *self.random_positions.lock() = None; // Reset time for position change

        let message_come_closer = Self::random_message(&self.config.lock().messages_come_closer).map(|s| s.to_owned());
        let message_denied_beer = Self::random_message(&self.config.lock().messages_denied_beer).map(|s| s.to_owned());
        let message_gave_beer = Self::random_message(&self.config.lock().messages_gave_beer).map(|s| s.to_owned());
        let message_gave_last_beer = Self::random_message(&self.config.lock().messages_gave_last_beer).map(|s| s.to_owned());
        let message_out_of_beer = Self::random_message(&self.config.lock().messages_out_of_beer).map(|s| s.to_owned());

        let look_direction = azalea::direction_looking_at(&own_eye_pos, &sender_eye_pos);

        if sender_pos.distance_to(&Vec3::from(&own_pos)) >= 5.0 {
            let almost_down = LookDirection::new(look_direction.y_rot, 60.0);
            let almost_down_2 = LookDirection::new(look_direction.y_rot, 60.0);
            bot_state.add_task(
                TaskGroup::new(format!("Urge {username} to come closer for beer"))
                    .with(DelayTicksTask::new(4))
                    .with(OnceFuncTask::new("Look at user 1", move |bot, _bot_state| {
                        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = look_direction;
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(8))
                    .with(OnceFuncTask::new("Nudge closer 1", move |bot, _bot_state| {
                        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = almost_down;
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(4))
                    .with(OnceFuncTask::new("Look back up a bit", move |bot, _bot_state| {
                        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = almost_down_2;
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(4))
                    .with(OnceFuncTask::new("Nudge closer 2", move |bot, _bot_state| {
                        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = almost_down;
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(4))
                    .with(OnceFuncTask::new("Look back up a bit 2", move |bot, _bot_state| {
                        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = almost_down_2;
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(4))
                    .with(OnceFuncTask::new("Nudge closer 3", move |bot, _bot_state| {
                        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = almost_down;
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(4))
                    .with(OnceFuncTask::new("Look at user 2", move |bot, _bot_state| {
                        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = look_direction;
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(4))
                    .with(OnceFuncTask::new("Nope message", move |_bot, _bot_state| {
                        if let Some(ref msg) = message_come_closer {
                            feedback(true, msg);
                        }
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(20)),
            );

            return Ok(());
        }

        if self.had_enough_beer(&username) {
            // Task to visually deny the player

            let no_left = LookDirection::new((look_direction.y_rot - 30.0) % 360.0, look_direction.x_rot);
            let no_right = LookDirection::new((look_direction.y_rot + 30.0) % 360.0, look_direction.x_rot);
            bot_state.add_task(
                TaskGroup::new(format!("Deny {username}'s Beer"))
                    .with(DelayTicksTask::new(4))
                    .with(OnceFuncTask::new("Look at user 1", move |bot, _bot_state| {
                        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = look_direction;
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(8))
                    .with(OnceFuncTask::new("Swing head left (nope)", move |bot, _bot_state| {
                        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = no_left;
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(4))
                    .with(OnceFuncTask::new("Swing head right (nope)", move |bot, _bot_state| {
                        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = no_right;
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(4))
                    .with(OnceFuncTask::new("Swing head left again (nope)", move |bot, _bot_state| {
                        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = no_left;
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(4))
                    .with(OnceFuncTask::new("Look at user 2", move |bot, _bot_state| {
                        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = look_direction;
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(4))
                    .with(OnceFuncTask::new("Nope message", move |_bot, _bot_state| {
                        if let Some(ref msg) = message_denied_beer {
                            feedback(true, msg);
                        }
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(20)),
            );
        } else {
            // Task to give beer to player
            //let nod_up = LookDirection::new(look_direction.y_rot, (look_direction.x_rot - 15.0).max(-90.0));
            let nod_down = LookDirection::new(look_direction.y_rot, (look_direction.x_rot + 15.0).min(90.0));

            let throw_pitch = self.config.lock().throw_pitch;
            let throw_look = LookDirection::new(look_direction.y_rot, throw_pitch);
            let self_clone = self.clone();
            let mut task = TaskGroup::new(format!("Give {username}'s Beer"))
                .with(DelayTicksTask::new(4))
                .with(OnceFuncTask::new("Look at user 1", move |bot, _bot_state| {
                    *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = look_direction;
                    Ok(())
                }))
                .with(DelayTicksTask::new(8))
                .with(OnceFuncTask::new("Move head up (Nod)", move |bot, _bot_state| {
                    *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = nod_down;
                    Ok(())
                }))
                /*.with(DelayTicksTask::new(4))
                .with(OnceFuncTask::new("Nod 2", move |bot, _bot_state| {
                    *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = nod_up;
                    Ok(())
                }))*/
                .with(DelayTicksTask::new(4))
                .with(OnceFuncTask::new("Look at user 2", move |bot, _bot_state| {
                    *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = look_direction;
                    Ok(())
                }))
                .with(DelayTicksTask::new(8));

            if self.config.lock().jump_before_throw {
                task = task
                    .with(OnceFuncTask::new("Jump before throw", move |mut bot, _bot_state| {
                        bot.jump();
                        Ok(())
                    }))
                    .with(DelayTicksTask::new(4)); // Should be most height of jump
            }

            task = task
                .with(OnceFuncTask::new("Recheck and throw beer", move |mut bot, _bot_state| {
                    if self_clone.had_enough_beer(&username) {
                        if let Some(ref msg) = message_denied_beer {
                            feedback(true, msg);
                        }
                        return Ok(());
                    }
                    let beer_slot = Self::find_beer_slot(&mut bot);
                    if beer_slot.is_none() {
                        if let Some(ref msg) = message_out_of_beer {
                            feedback(true, msg);
                        }
                        return Ok(());
                    }
                    let beer_slot = beer_slot.unwrap();
                    info!(
                        "Giving one beer to {username} (from slot {beer_slot}). I currently have {} beer left.",
                        Self::beer_count(&mut bot.clone())
                    );
                    let mut ecs = bot.ecs.lock();
                    let sent_by = bot.entity;
                    ecs.send_event(SendPacketEvent {
                        sent_by,
                        packet: ServerboundGamePacket::MovePlayerRot(ServerboundMovePlayerRot {
                            look_direction: throw_look,
                            on_ground: true,
                        }),
                    });
                    ecs.send_event(SendPacketEvent {
                        sent_by,
                        packet: ServerboundGamePacket::ContainerClick(ServerboundContainerClick {
                            container_id: 0,
                            state_id: 0,

                            slot_num: beer_slot as i16,
                            button_num: 0,
                            click_type: ClickType::Throw,

                            carried_item: ItemStack::Empty,
                            changed_slots: HashMap::default(),
                        }),
                    });
                    ecs.send_event(SendPacketEvent {
                        sent_by,
                        packet: ServerboundGamePacket::MovePlayerRot(ServerboundMovePlayerRot {
                            look_direction,
                            on_ground: true,
                        }),
                    });

                    *ecs.get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = look_direction;
                    self_clone.gave_beer(&username)?;
                    drop(ecs);
                    if self_clone.had_enough_beer(&username) {
                        if let Some(ref msg) = message_gave_last_beer {
                            feedback(true, msg);
                        }
                    } else {
                        if let Some(ref msg) = message_gave_beer {
                            feedback(true, msg);
                        }
                    }
                    Ok(())
                }))
                .with(DelayTicksTask::new(8));
            bot_state.add_task(task);
        }
        Ok(())
    }

    fn close_and_holding_beer(&self, bot: &mut Client, id: MinecraftEntityId) -> Option<(Entity, String /*PlayerName*/)> {
        let entity = bot.entity_by::<With<Player>, (&MinecraftEntityId,)>(|(entity_id,): &(&MinecraftEntityId,)| entity_id.0 == id.0);
        if entity.is_none() {
            return None; // Not a player
        }
        let entity = entity.unwrap();

        let own_pos = bot.entity_component::<Position>(bot.entity);
        let entity_pos = bot.entity_component::<Position>(entity);

        if own_pos.distance_to(&Vec3::from(&entity_pos)) >= 12.0 {
            return None; // Too far away
        }

        if !self.holds_beer.lock().contains(&id.0) {
            return None; // Not holding beer
        }

        Some((entity, bot.entity_component::<GameProfileComponent>(entity).name.to_owned()))
    }
}

#[async_trait::async_trait]
impl Module for BeertenderModule {
    fn name(&self) -> &'static str {
        "Stasis"
    }

    async fn handle(&self, mut bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Login => {
                info!("Loading beertender-config...");
                self.load_config().await?;
                self.holds_beer.lock().clear();
            }
            Event::Disconnect(_) => {}
            Event::Packet(packet) => match packet.as_ref() {
                ClientboundGamePacket::SetEquipment(packet) => {
                    for (slot, itemstack) in packet.slots.slots.iter() {
                        if *slot as usize == EquipmentSlot::MainHand as usize {
                            if itemstack.kind() == Item::HoneyBottle {
                                // Now holding beer
                                self.holds_beer.lock().insert(packet.entity_id.0);
                            } else {
                                // No longer holding beer
                                self.holds_beer.lock().remove(&packet.entity_id.0);
                            }
                        }
                    }
                }
                ClientboundGamePacket::RemoveEntities(packet) => {
                    let mut holds_beer = self.holds_beer.lock();
                    for id in &packet.entity_ids {
                        holds_beer.remove(&id.0);
                    }
                }
                ClientboundGamePacket::SetEntityData(packet) => {
                    let mut is_interacting = false;
                    for item in packet.packed_items.0.iter() {
                        // https://minecraft.wiki/w/Java_Edition_protocol/Entity_metadata#Entity_Metadata_Format
                        if item.index == 8
                            && let EntityDataValue::Byte(byte) = item.value
                            && byte & 0x02 == 0 /* Is Main hand */
                            && byte & 0x01 > 0
                        /* Is interacting */
                        {
                            is_interacting = true;
                            break;
                        }
                    }

                    if !is_interacting || !self.config.lock().sip_when_others_sip || bot_state.tasks() > 0 {
                        return Ok(());
                    }

                    let maybe_entity_and_name = self.close_and_holding_beer(&mut bot, packet.id);
                    if maybe_entity_and_name.is_none() {
                        return Ok(());
                    }
                    let (_entity, entity_name) = maybe_entity_and_name.unwrap();
                    info!("Noticed that {entity_name} is interacting with their beer (Prost!). Doing the same!");
                    bot_state.add_task(SipBeerTask::default());
                }
                ClientboundGamePacket::Animate(packet) => {
                    if packet.action as usize != AnimationAction::SwingMainHand as usize || !self.config.lock().prost_when_others_prost || bot_state.tasks() > 0
                    {
                        return Ok(());
                    }

                    let maybe_entity_and_name = self.close_and_holding_beer(&mut bot, packet.id);
                    if maybe_entity_and_name.is_none() {
                        return Ok(());
                    }
                    let (_entity, entity_name) = maybe_entity_and_name.unwrap();

                    info!("Noticed that {entity_name} is swinging their beer (Prost!). Doing the same!");
                    bot_state.add_task(SwingBeerTask::default());
                }
                _ => {}
            },
            Event::Tick => {
                if bot_state.tasks() > 0 {
                    // Reset when bot does something
                    *self.random_sipping.lock() = None;
                } else {
                    // Sipping
                    let mut random_sipping = self.random_sipping.lock();
                    if random_sipping.is_none() {
                        let config = self.config.lock();
                        if config.sip_random_min_secs <= config.sip_random_max_secs {
                            *random_sipping = Some((0, rand::rng().random_range(config.sip_random_min_secs * 20..=config.sip_random_max_secs * 20)));
                        }
                    }

                    let mut reset_random_sipping = false;
                    if let Some(ref mut random_sipping) = *random_sipping {
                        random_sipping.0 += 1;
                        if random_sipping.0 >= random_sipping.1 {
                            bot_state.add_task(SipBeerTask::default());
                            reset_random_sipping = true;
                        }
                    }

                    if reset_random_sipping {
                        *random_sipping = None;
                    }

                    // Random positions
                    let mut random_positions = self.random_positions.lock();
                    if random_positions.is_none() {
                        let config = self.config.lock();
                        if config.random_positions_min_secs <= config.random_positions_max_secs && !config.random_positions.is_empty() {
                            *random_positions = Some((
                                0,
                                rand::rng().random_range(config.random_positions_min_secs * 20..=config.random_positions_max_secs * 20),
                            ));
                        }
                    }

                    let mut reset_random_positions = false;
                    if let Some(ref mut random_positions) = *random_positions {
                        random_positions.0 += 1;
                        if random_positions.0 >= random_positions.1 {
                            let config = self.config.lock();
                            if config.random_positions.is_empty() {
                                warn!("No random position to walk to!");
                            } else {
                                let random_pos = config.random_positions[rand::rng().random_range(0..config.random_positions.len())].clone();
                                bot_state.add_task(PathfindTask::new(false, BlockPosGoal(random_pos), "New random pos"));
                            }
                            reset_random_positions = true;
                        }
                    }

                    if reset_random_positions {
                        *random_positions = None;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
