use crate::module::stasis::{ChamberOccupant, LecternRecoderTerminal, LecternRedcoderEndpoint, StasisChamberDefinition, StasisChamberEntry};
use crate::task::Task;
use crate::task::eat::EatTask;
use crate::task::group::TaskGroup;
use crate::task::pathfind::PathfindTask;
use crate::{BotState, DEVNET_TX, OPTS, devnet};
use anyhow::Context;
use azalea::blocks::Block;
use azalea::core::direction::Direction;
use azalea::inventory::operations::ClickType;
use azalea::inventory::{Inventory, ItemStack, SetSelectedHotbarSlotEvent};
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::s_player_action::Action;
use azalea::protocol::packets::game::{ServerboundContainerClick, ServerboundGamePacket, ServerboundPlayerAction, ServerboundSetCarriedItem};
use azalea::registry::Item;
use azalea::{
    BlockPos, GameProfileComponent, Vec3,
    ecs::query::With,
    entity::{Position, metadata::Player},
    pathfinder::goals::BlockPosGoal,
    prelude::*,
    world::InstanceName,
};
use std::collections::{HashMap, HashSet};
use std::ops::Add;
use uuid::Uuid;

pub async fn execute(bot: &mut Client, bot_state: &BotState, sender: String, mut command: String, args: Vec<String>) -> anyhow::Result<bool> {
    if command.starts_with('!') {
        command.remove(0);
    }
    command = command.to_lowercase();
    let sender_is_admin = OPTS.admin.iter().any(|a| sender.eq_ignore_ascii_case(a) || a == "*");

    match command.as_str() {
        "help" => {
            let mut commands = vec!["!help", "!about", "!modules"];
            if bot_state.stasis.is_some() {
                commands.push("!tp");
            }
            if bot_state.server_tps.is_some() {
                commands.push("!tps");
            }
            if sender_is_admin {
                commands.append(&mut vec![
                    "!comehere",
                    "!say",
                    "!stop",
                    "!selecthand",
                    "!drop",
                    "!dropall",
                    "!swap",
                    "!printinv",
                    "!equip",
                    "!unequip",
                    "!canceltasks",
                    "!task",
                    "!eat",
                ]);
                if OPTS.enable_pos_command {
                    commands.push("!pos");
                }
                if bot_state.stasis.is_some() {
                    commands.push("!stasis-rt");
                    commands.push("!stasis-rte");
                }
            }
            if !OPTS.admin.is_empty() {
                commands.push("!admins");
            }
            commands.sort();

            send_command(bot, format!("msg {sender} Commands: {}", commands.join(", ")));
            Ok(true)
        }
        "about" => {
            send_command(
                bot,
                format!(
                    "msg {sender} Hi, I'm running EnderKill98's azalea-based stasis-bot {}: github.com/EnderKill98/stasis-bot",
                    env!("CARGO_PKG_VERSION")
                ),
            );
            Ok(true)
        }
        "modules" => {
            let mut module_names = vec![];
            for module in bot_state.modules() {
                module_names.push(module.name().to_string());
            }

            send_command(bot, format!("msg {sender} Active modules: {}", module_names.join(", "),));
            Ok(true)
        }
        "tp" => {
            let stasis = bot_state.stasis.as_ref();
            if stasis.is_none() {
                send_command(bot, format!("msg {sender} I'm not allowed to do pearl duties :(..."));
                return Ok(true);
            }
            let stasis = stasis.unwrap();
            let bot = bot.clone();
            stasis
                .pull_pearl(&sender.clone(), &bot.clone(), bot_state, 0, move |_error, message| {
                    send_command(&mut bot.clone(), format!("msg {sender} {message}"));
                })
                .context("Pull pearl")?;
            Ok(true)
        }
        "stasis-rt" => {
            let stasis = bot_state.stasis.as_ref();
            if stasis.is_none() {
                send_command(bot, format!("msg {sender} I'm not allowed to do pearl duties :(..."));
                return Ok(true);
            }
            let stasis = stasis.unwrap();

            let is_add = if args.len() > 1 && args[0].eq_ignore_ascii_case("add") {
                true
            } else if args.len() > 0 && args[0].eq_ignore_ascii_case("rm") {
                false
            } else {
                send_command(bot, format!("msg {sender} Please specify either action: \"add <Name>\" or \"rm\"!"));
                return Ok(true);
            };

            let sender_entity = bot.entity_by::<With<Player>, (&GameProfileComponent,)>(|(profile,): &(&GameProfileComponent,)| profile.name == sender);
            if sender_entity.is_none() {
                send_command(bot, format!("msg {sender} I can't find you!"));
                return Ok(true);
            }
            let sender_entity = sender_entity.unwrap();
            let sender_pos = Vec3::from(&bot.entity_component::<Position>(sender_entity));

            let mut button: Option<BlockPos> = None;
            let mut lectern: Option<BlockPos> = None;
            {
                let world = bot.world();
                let world = world.read();
                for x in -4..=4 {
                    for y in -4..=4 {
                        for z in -4..=4 {
                            let pos = sender_pos.to_block_pos_floor().add(BlockPos::new(x, y, z));
                            if world
                                .get_block_state(&pos)
                                .map(|state| Box::<dyn Block>::from(state).id().ends_with("_button"))
                                .unwrap_or(false)
                            {
                                if button
                                    .map(|b| b.center().distance_squared_to(&sender_pos) > pos.center().distance_squared_to(&sender_pos))
                                    .unwrap_or(true)
                                {
                                    button = Some(pos);
                                }
                            } else if world
                                .get_block_state(&pos)
                                .map(|state| Box::<dyn Block>::from(state).id().ends_with("lectern"))
                                .unwrap_or(false)
                            {
                                if lectern
                                    .map(|l| l.center().distance_squared_to(&sender_pos) > pos.center().distance_squared_to(&sender_pos))
                                    .unwrap_or(true)
                                {
                                    lectern = Some(pos);
                                }
                            }
                        }
                    }
                }
            }

            if lectern.is_none() && button.is_none() {
                send_command(bot, format!("msg {sender} Failed to find button and lectern near you!"));
                return Ok(true);
            } else if button.is_none() {
                send_command(bot, format!("msg {sender} Failed to find button near you!"));
                return Ok(true);
            } else if lectern.is_none() {
                send_command(bot, format!("msg {sender} Failed to find lectern near you!"));
                return Ok(true);
            }
            let button = button.unwrap();
            let lectern = lectern.unwrap();

            if is_add {
                // Add
                let name = &args[1];
                let mut config = stasis.config.lock();
                if config
                    .lectern_redcoder_terminals
                    .iter()
                    .find(|term| term.id.eq_ignore_ascii_case(name))
                    .is_some()
                {
                    send_command(bot, format!("msg {sender} A terminal with this name does already exist!"));
                    return Ok(true);
                }

                if config.lectern_redcoder_terminals.iter().find(|term| term.button == button).is_some() {
                    send_command(bot, format!("msg {sender} A terminal with this button pos already exists!!"));
                    return Ok(true);
                }

                config.lectern_redcoder_terminals.push(LecternRecoderTerminal {
                    id: name.to_owned(),
                    button,
                    lectern,
                });
                send_command(bot, format!("msg {sender} Created new terminal with name \"{name}\"!"));
            } else {
                // Remove
                let mut config = stasis.config.lock();
                if let Some((terminal_index, terminal_name)) = config
                    .lectern_redcoder_terminals
                    .iter()
                    .enumerate()
                    .filter(|(_, term)| term.button == button && term.lectern == lectern)
                    .map(|(index, term)| (index, term.id.to_owned()))
                    .find(|_| true)
                {
                    config.lectern_redcoder_terminals.remove(terminal_index);
                    let mut remove_chamber_indices = Vec::new();
                    for (chamber_index, chamber) in config.chambers.iter().enumerate() {
                        match &chamber.definition {
                            StasisChamberDefinition::RedcoderShay { endpoint, .. } | StasisChamberDefinition::RedcoderTrapdoor { endpoint, .. } => {
                                if endpoint.lectern_recoder_terminal_id == terminal_name {
                                    remove_chamber_indices.push(chamber_index);
                                }
                            }
                            _ => {}
                        }
                    }
                    for (chamber_index_index, chamber_index) in remove_chamber_indices.iter().enumerate() {
                        config.chambers.remove(chamber_index - chamber_index_index);
                    }
                    send_command(
                        bot,
                        format!(
                            "msg {sender} Removed terminal {terminal_name} and {} associated chambers!",
                            remove_chamber_indices.len()
                        ),
                    );
                } else {
                    send_command(bot, format!("msg {sender} No fitting terminal found!"));
                }
            }
            stasis.save_config().await?;
            Ok(true)
        }
        "stasis-rte" => {
            let stasis = bot_state.stasis.as_ref();
            if stasis.is_none() {
                send_command(bot, format!("msg {sender} I'm not allowed to do pearl duties :(..."));
                return Ok(true);
            }
            let stasis = stasis.unwrap();

            let usage = "Usage: <TerminalName> <addShay/addTrapdoor/rm> <Index>";
            if args.len() < 3 {
                send_command(bot, format!("msg {sender} {usage}"));
                return Ok(true);
            }

            let terminal_name = &args[0];
            let command = &args[1];
            let index = args[2].parse::<usize>()?;

            if command.eq_ignore_ascii_case("addShay") || command.eq_ignore_ascii_case("addTrapdoor") {
                let is_shay = args[1].eq_ignore_ascii_case("addShay");
                let index = args[2].parse::<usize>()?;

                let mut config = stasis.config.lock();
                for chamber in &config.chambers {
                    match &chamber.definition {
                        StasisChamberDefinition::RedcoderShay { endpoint, .. } | StasisChamberDefinition::RedcoderTrapdoor { endpoint, .. } => {
                            if &endpoint.lectern_recoder_terminal_id == terminal_name && endpoint.chamber_index == index {
                                send_command(bot, format!("msg {sender} This chamber already exists!"));
                                return Ok(true);
                            }
                        }
                        _ => {}
                    }
                }

                let sender_entity = bot.entity_by::<With<Player>, (&GameProfileComponent,)>(|(profile,): &(&GameProfileComponent,)| profile.name == sender);
                if sender_entity.is_none() {
                    send_command(bot, format!("msg {sender} I can't find you!"));
                    return Ok(true);
                }
                let sender_entity = sender_entity.unwrap();
                let sender_pos = Vec3::from(&bot.entity_component::<Position>(sender_entity));

                if is_shay {
                    config.chambers.push(StasisChamberEntry {
                        definition: StasisChamberDefinition::RedcoderShay {
                            endpoint: LecternRedcoderEndpoint {
                                lectern_recoder_terminal_id: terminal_name.to_owned(),
                                chamber_index: index,
                            },
                            base_pos: sender_pos.to_block_pos_floor(),
                        },
                        occupants: Vec::new(),
                    });
                } else {
                    config.chambers.push(StasisChamberEntry {
                        definition: StasisChamberDefinition::RedcoderTrapdoor {
                            endpoint: LecternRedcoderEndpoint {
                                lectern_recoder_terminal_id: terminal_name.to_owned(),
                                chamber_index: index,
                            },
                            trapdoor_pos: sender_pos.to_block_pos_floor(),
                        },
                        occupants: Vec::new(),
                    });
                }
                send_command(bot, format!("msg {sender} Endpoint added!"));
            } else if command.eq_ignore_ascii_case("rm") {
                let mut config = stasis.config.lock();
                let mut remove_chamber_indices = Vec::new();
                for (chamber_index, chamber) in config.chambers.iter().enumerate() {
                    match &chamber.definition {
                        StasisChamberDefinition::RedcoderShay { endpoint, .. } | StasisChamberDefinition::RedcoderTrapdoor { endpoint, .. } => {
                            if &endpoint.lectern_recoder_terminal_id == terminal_name && endpoint.chamber_index == index {
                                remove_chamber_indices.push(chamber_index);
                            }
                        }
                        _ => {}
                    }
                }
                for (chamber_index_index, chamber_index) in remove_chamber_indices.iter().enumerate() {
                    config.chambers.remove(chamber_index - chamber_index_index);
                }
                if remove_chamber_indices.is_empty() {
                    send_command(bot, format!("msg {sender} Not found!"));
                    return Ok(true);
                } else {
                    send_command(bot, format!("msg {sender} Removed chamber with index {index} on terminal \"{terminal_name}\"!"));
                }
            } else {
                send_command(bot, format!("msg {sender} {usage}"));
                return Ok(true);
            }
            stasis.save_config().await?;
            Ok(true)
        }
        "comehere" => {
            if !sender_is_admin {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"),
                );
                return Ok(true);
            }

            let sender_entity = bot.entity_by::<With<Player>, (&GameProfileComponent,)>(|(profile,): &(&GameProfileComponent,)| profile.name == sender);
            if let Some(sender_entity) = sender_entity {
                let goal = BlockPosGoal(bot.entity_component::<Position>(sender_entity).to_block_pos_floor());
                let tasks = bot_state.tasks();
                bot_state.add_task(PathfindTask::new(!OPTS.no_mining, goal, format!("{sender}'s BlockPos")));
                if tasks > 0 {
                    send_command(bot, format!("msg {sender} Hang on. Walking to your block position in due time..."));
                } else {
                    send_command(bot, format!("msg {sender} Walking to your block position..."));
                }
            } else {
                send_command(bot, format!("msg {sender} I could not find you in my render distance!"));
            }
            Ok(true)
        }
        "admins" => {
            send_command(bot, format!("msg {sender} Admins: {}", OPTS.admin.join(", ")));
            Ok(true)
        }
        "say" => {
            if !sender_is_admin {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"),
                );
                return Ok(true);
            }

            let command_or_chat = args.join(" ");
            if command_or_chat.starts_with("/") {
                info!("Sending command: {command_or_chat}");
                bot.send_command_packet(&format!("{}", &command_or_chat[1..]));
            } else {
                info!("Sending chat message: {command_or_chat}");
                bot.send_chat_packet(&command_or_chat);
            }
            Ok(true)
        }
        "stop" => {
            if !sender_is_admin {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"),
                );
                return Ok(true);
            }

            info!("Stopping... Bye!");
            std::process::exit(crate::EXITCODE_USER_REQUESTED_STOP);
        }
        "pos" => {
            if !sender_is_admin {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"),
                );
                return Ok(true);
            }
            if !OPTS.enable_pos_command {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but this command was not enabled. The owner needs to add the flag --enable-pos-command in order to do so!"),
                );
                return Ok(true);
            }

            let pos = bot.component::<Position>();
            let world_name = bot.component::<InstanceName>();
            send_command(
                bot,
                format!("msg {sender} I'm at {:.03} {:.03} {:.03} in {}", pos.x, pos.y, pos.z, world_name.path,),
            );
            Ok(true)
        }

        "tps" => {
            if let Some(server_tps) = &bot_state.server_tps {
                if server_tps.is_server_likely_hanging() {
                    send_command(bot, format!("msg {sender} Seems that the server is currently hanging..."));
                } else if let Some(tps) = server_tps.current_tps() {
                    send_command(bot, format!("msg {sender} Current TPS: {tps:.02}"));
                } else {
                    send_command(bot, format!("msg {sender} I don't know the TPS, yet!"));
                }
            } else {
                send_command(bot, format!("msg {sender} Sorry, but ServerTps is not active!"));
            }
            Ok(true)
        }

        "task" => {
            if !sender_is_admin {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"),
                );
                return Ok(true);
            }

            send_command(bot, format!("msg {sender} Task: {}", bot_state.root_task_group.lock()));
            Ok(true)
        }

        "canceltasks" => {
            if !sender_is_admin {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"),
                );
                return Ok(true);
            }

            let tasks = bot_state.tasks();
            if let Err(err) = bot_state.root_task_group.lock().stop(bot.clone(), &bot_state) {
                error!("Failed to stop root task group: {err:?}");
                send_command(bot, format!("msg {sender} Failed: {err:?}"));
            };
            *bot_state.root_task_group.lock() = TaskGroup::new_root();
            send_command(bot, format!("msg {sender} Stopped and removed all tasks ({tasks})!"));
            Ok(true)
        }

        "eat" => {
            if !sender_is_admin {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"),
                );
                return Ok(true);
            }

            if EatTask::find_food_in_hotbar(bot).is_none() {
                send_command(bot, format!("msg {sender} No food found in hotbar!"));
                return Ok(true);
            }
            bot_state.add_task_now(bot.clone(), bot_state, EatTask::default()).context("Add EatTask now")?;
            send_command(bot, format!("msg {sender} Attempting to eat now..."));
            Ok(true)
        }

        "selecthand" => {
            if !sender_is_admin {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"),
                );
                return Ok(true);
            }

            if args.len() != 1 {
                send_command(bot, format!("msg {sender} Please specify hotbar index (0-8)."));
                return Ok(true);
            }

            let index = args[0].parse::<u8>()?;

            {
                let mut ecs = bot.ecs.lock();
                if index > 0 && index < 9 {
                    ecs.send_event(SetSelectedHotbarSlotEvent {
                        entity: bot.entity,
                        slot: index,
                    });
                }
                ecs.send_event(SendPacketEvent {
                    sent_by: bot.entity,
                    packet: ServerboundGamePacket::SetCarriedItem(ServerboundSetCarriedItem { slot: index as u16 }),
                });
            }

            send_command(bot, format!("msg {sender} Selected index {index}!"));
            Ok(true)
        }

        "drop" => {
            if !sender_is_admin {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"),
                );
                return Ok(true);
            }

            if args.is_empty() {
                // Drop hand
                bot.ecs.lock().send_event(SendPacketEvent {
                    sent_by: bot.entity,
                    packet: ServerboundGamePacket::PlayerAction(ServerboundPlayerAction {
                        action: Action::DropAllItems,
                        pos: BlockPos::default(),
                        sequence: 0,
                        direction: Direction::Down,
                    }),
                });

                send_command(bot, format!("msg {sender} I dropped the item in my hand!"));
            } else {
                {
                    let mut ecs = bot.ecs.lock();
                    for arg in args {
                        let slot = arg.parse::<i16>()?;
                        ecs.send_event(SendPacketEvent {
                            sent_by: bot.entity,
                            packet: ServerboundGamePacket::ContainerClick(ServerboundContainerClick {
                                container_id: 0,
                                state_id: 0,

                                slot_num: slot,
                                button_num: 1,
                                click_type: ClickType::Throw,

                                carried_item: ItemStack::Empty,
                                changed_slots: HashMap::default(),
                            }),
                        });
                    }
                }
                send_command(bot, format!("msg {sender} Dropped specified indices!"));
            }
            Ok(true)
        }

        "dropall" => {
            if !sender_is_admin {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"),
                );
                return Ok(true);
            }

            let inv = bot.entity_component::<Inventory>(bot.entity);
            let inv_menu = inv.inventory_menu;
            let slots = inv_menu
                .player_slots_range()
                .chain(5..=8 /*Armor*/)
                .chain(45..=45 /*Offhand*/)
                .filter(|slot| inv_menu.slot(*slot).map(|stack| stack.is_present()).unwrap_or(false))
                .collect::<Vec<_>>();
            {
                let mut ecs = bot.ecs.lock();
                for slot in &slots {
                    ecs.send_event(SendPacketEvent {
                        sent_by: bot.entity,
                        packet: ServerboundGamePacket::ContainerClick(ServerboundContainerClick {
                            container_id: 0,
                            state_id: 0,

                            slot_num: *slot as i16,
                            button_num: 1,
                            click_type: ClickType::Throw,

                            carried_item: ItemStack::Empty,
                            changed_slots: HashMap::default(),
                        }),
                    });
                }
            }

            send_command(bot, format!("msg {sender} I dropped all my items ({} stacks)!", slots.len()));
            Ok(true)
        }

        "swap" => {
            if !sender_is_admin {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"),
                );
                return Ok(true);
            }

            bot.ecs.lock().send_event(SendPacketEvent {
                sent_by: bot.entity,
                packet: ServerboundGamePacket::PlayerAction(ServerboundPlayerAction {
                    action: Action::SwapItemWithOffhand,
                    pos: BlockPos::default(),
                    sequence: 0,
                    direction: Direction::Down,
                }),
            });

            send_command(bot, format!("msg {sender} Swapped main- and off-hand items!"));
            Ok(true)
        }

        "printinv" => {
            if !sender_is_admin {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"),
                );
                return Ok(true);
            }

            let inv = bot.entity_component::<Inventory>(bot.entity);
            let inv_menu = inv.inventory_menu;
            let mut slot_strings = Vec::new();
            for slot in inv_menu.player_slots_range().chain(5..=8 /*Armor*/).chain(45..=45 /*Offhand*/) {
                if let Some(item_stack_data) = inv_menu.slot(slot).and_then(|stack| stack.as_present()) {
                    slot_strings.push(format!(
                        "{slot}: {}x {}",
                        item_stack_data.count,
                        item_stack_data.kind.to_string().replace("minecraft:", "")
                    ))
                }
            }

            send_command(bot, format!("msg {sender} Inv ({}): {}", slot_strings.len(), slot_strings.join(", "),));
            Ok(true)
        }

        "equip" => {
            if !sender_is_admin {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"),
                );
                return Ok(true);
            }

            let inv = bot.entity_component::<Inventory>(bot.entity);

            let mut equipped_armor_slots = HashSet::<usize>::default();
            let inv_menu = inv.inventory_menu;
            {
                let mut ecs = bot.ecs.lock();
                for slot in inv_menu.player_slots_range() {
                    if let Some(item_stack_data) = inv_menu.slot(slot).and_then(|stack| stack.as_present()) {
                        let item_name = item_stack_data.kind.to_string();
                        let target_slot: Option<usize> = if item_name.ends_with("_helmet") {
                            Some(5)
                        } else if item_name.ends_with("_chestplate") || item_stack_data.kind == Item::Elytra {
                            Some(6)
                        } else if item_name.ends_with("_leggings") {
                            Some(7)
                        } else if item_name.ends_with("_boots") {
                            Some(8)
                        } else {
                            None
                        };

                        if let Some(target_slot) = target_slot {
                            if !equipped_armor_slots.contains(&target_slot) && !inv_menu.slot(target_slot).map(|stack| stack.is_present()).unwrap_or(false) {
                                ecs.send_event(SendPacketEvent {
                                    sent_by: bot.entity,
                                    packet: ServerboundGamePacket::ContainerClick(ServerboundContainerClick {
                                        container_id: 0,
                                        state_id: 0,

                                        slot_num: slot as i16,
                                        button_num: 0,
                                        click_type: ClickType::QuickMove,

                                        carried_item: ItemStack::Empty,
                                        changed_slots: HashMap::default(),
                                    }),
                                });
                                equipped_armor_slots.insert(target_slot);
                            }
                        }
                    }
                }
            }

            send_command(bot, format!("msg {sender} Equipped {} armor pieces!", equipped_armor_slots.len(),));
            Ok(true)
        }

        "unequip" => {
            if !sender_is_admin {
                send_command(
                    bot,
                    format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"),
                );
                return Ok(true);
            }

            let inv = bot.entity_component::<Inventory>(bot.entity);

            let inv_menu = inv.inventory_menu;
            let mut total = 0;
            {
                let mut ecs = bot.ecs.lock();
                for slot in 5..=8 {
                    if inv_menu.slot(slot).map(|stack| stack.is_present()).unwrap_or(false) {
                        ecs.send_event(SendPacketEvent {
                            sent_by: bot.entity,
                            packet: ServerboundGamePacket::ContainerClick(ServerboundContainerClick {
                                container_id: 0,
                                state_id: 0,

                                slot_num: slot as i16,
                                button_num: 0,
                                click_type: ClickType::QuickMove,

                                carried_item: ItemStack::Empty,
                                changed_slots: HashMap::default(),
                            }),
                        });
                        total += 1;
                    }
                }
            }

            send_command(bot, format!("msg {sender} Unequipped {total} armor pieces!",));
            Ok(true)
        }

        _ => Ok(false), // Do nothing if unrecognized command
    }
}

pub fn send_command(bot: &mut Client, command: impl AsRef<str>) {
    let command = command.as_ref();
    if OPTS.quiet {
        info!("Quiet mode: Suppressed sending command: {command}");
    } else {
        let truncated_command = if command.len() > 255 { &command[..255] } else { command };
        info!(
            "Sending command{}: {command}",
            if command.len() != truncated_command.len() {
                " (truncated to 255 chars!)"
            } else {
                ""
            },
        );
        bot.send_command_packet(truncated_command);
    }
}

pub async fn handle_devnet_message(bot: &mut Client, bot_state: &BotState, message: devnet::Message) -> anyhow::Result<()> {
    let stasis = bot_state.stasis.as_ref();
    if stasis.is_none() {
        match message {
            devnet::Message::CheckRequest { for_mc_id, .. } | devnet::Message::PullRequest { for_mc_id, .. } => {
                send_devnet_feedback(bot.username(), for_mc_id, true, "Stasis is not enabled!");
            }
            _ => {}
        }
        return Ok(());
    }
    let stasis = stasis.unwrap();

    match message {
        devnet::Message::CheckRequest { for_mc_id, destination } => {
            let sender = bot
                .tab_list()
                .iter()
                .find(|(uuid, _info)| uuid == &&for_mc_id)
                .map(|(_uuid, info)| info.profile.name.to_owned());
            if sender.is_none() {
                send_devnet_feedback(
                    bot.username(),
                    for_mc_id,
                    true,
                    "You need to be online for me to know your username and check your pearl!",
                );
                return Ok(());
            }
            let sender = sender.unwrap();
            info!("Got devnet request to check pearl for {sender} ({for_mc_id}) at destination {destination}");

            #[derive(serde::Serialize)]
            struct DevnetChamberInfo {
                r#type: &'static str,
                occupants: Vec<ChamberOccupant>,
            }

            let mut pearls = vec![];
            for chamber in &stasis.config.lock().chambers {
                for occupant in &chamber.occupants {
                    if occupant.player_uuid == for_mc_id {
                        pearls.push(DevnetChamberInfo {
                            r#type: chamber.definition.type_name(),
                            occupants: chamber.occupants.clone(),
                        });
                        break;
                    }
                }
            }

            let devnet_tx = DEVNET_TX.lock().clone();
            if let Some(devnet_tx) = devnet_tx {
                devnet_tx
                    .send(devnet::Message::CheckResponse {
                        for_mc_id,
                        pearls: pearls
                            .iter()
                            .map(|info| serde_json::to_value(info))
                            .collect::<Result<Vec<_>, serde_json::Error>>()?,
                    })
                    .await?;
            }
        }

        devnet::Message::PullRequest {
            pearl_index,
            for_mc_id,
            destination,
        } => {
            let sender = bot
                .tab_list()
                .iter()
                .find(|(uuid, _info)| uuid == &&for_mc_id)
                .map(|(_uuid, info)| info.profile.name.to_owned());
            if sender.is_none() {
                send_devnet_feedback(
                    bot.username(),
                    for_mc_id,
                    true,
                    "You need to be online for me to know your username and pull your pearl!",
                );
                return Ok(());
            }
            let sender = sender.unwrap();
            info!("Got devnet request to pull pearl {pearl_index} for {sender} ({for_mc_id}) at destination {destination}");

            let own_username = bot.username();
            let bot = bot.clone();
            stasis
                .pull_pearl(&sender.clone(), &bot.clone(), bot_state, pearl_index.max(0) as usize, move |error, message| {
                    send_devnet_feedback(&own_username, for_mc_id, error, message)
                })
                .context("Pull pearl")?;
        }
        _ => {}
    }
    Ok(())
}

pub fn send_devnet_feedback(own_username: impl AsRef<str>, for_mc_id: Uuid, error: bool, message: impl AsRef<str>) {
    info!("DevNet BotFeedback to {for_mc_id}: {}", message.as_ref());
    let devnet_tx = DEVNET_TX.lock().clone();
    if let Some(devnet_tx) = devnet_tx {
        let message = message.as_ref().to_string();
        let own_username = own_username.as_ref().to_string();
        tokio::spawn(async move {
            if let Err(err) = devnet_tx
                .send(devnet::Message::BotFeedback {
                    error,
                    sender: own_username,
                    message,
                    for_player: for_mc_id,
                })
                .await
            {
                error!("Failed to send devnet bot_feedback: {err:?}");
            }
        });
    }
}
