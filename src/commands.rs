use crate::module::beertender::BeertenderModule;
use crate::task::Task;
use crate::task::eat::EatTask;
use crate::task::group::TaskGroup;
use crate::task::pathfind::PathfindTask;
use crate::{BotState, DEVNET_TX, OPTS, devnet};
use anyhow::Context;
use azalea::core::direction::Direction;
use azalea::inventory::operations::ClickType;
use azalea::inventory::{Inventory, ItemStack, SetSelectedHotbarSlotEvent};
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::s_player_action::Action;
use azalea::protocol::packets::game::{ServerboundContainerClick, ServerboundGamePacket, ServerboundPlayerAction, ServerboundSetCarriedItem};
use azalea::registry::Item;
use azalea::{
    BlockPos, GameProfileComponent,
    ecs::query::With,
    entity::{Position, metadata::Player},
    pathfinder::goals::BlockPosGoal,
    prelude::*,
    world::InstanceName,
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub async fn execute<F: Fn(&str) + Send + Sync + 'static>(
    bot: &mut Client,
    bot_state: &BotState,
    sender: String,
    mut command: String,
    args: Vec<String>,
    feedback: F,
) -> anyhow::Result<bool> {
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
            if bot_state.beertender.is_some() {
                commands.push("!beer");
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
                if bot_state.beertender.is_some() {
                    commands.push("!totalBeer");
                    commands.push("!maxBeer");
                    commands.push("!resetBeerFor");
                    commands.push("!resetBeerForAll");
                    commands.push("!reloadBeerConfig");
                }
            }
            if !OPTS.admin.is_empty() {
                commands.push("!admins");
            }
            commands.sort();

            feedback(&format!("Commands: {}", commands.join(", ")));
            Ok(true)
        }
        "about" => {
            feedback(&format!(
                "Hi, I'm running EnderKill98's azalea-based stasis-bot {}: github.com/EnderKill98/stasis-bot",
                env!("CARGO_PKG_VERSION")
            ));
            Ok(true)
        }
        "modules" => {
            let mut module_names = vec![];
            for module in bot_state.modules() {
                module_names.push(module.name().to_string());
            }

            feedback(&format!("Active modules: {}", module_names.join(", "),));
            Ok(true)
        }
        "tp" => {
            let stasis = bot_state.stasis.as_ref();
            if stasis.is_none() {
                feedback("I'm not allowed to do pearl duties :(...");
                return Ok(true);
            }
            let stasis = stasis.unwrap();
            let bot = bot.clone();
            stasis
                .pull_pearl(&sender.clone(), &bot.clone(), bot_state, move |_error, message| {
                    feedback(message);
                })
                .context("Pull pearl")?;
            Ok(true)
        }
        "comehere" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
                return Ok(true);
            }

            let sender_entity = bot.entity_by::<With<Player>, (&GameProfileComponent,)>(|(profile,): &(&GameProfileComponent,)| profile.name == sender);
            if let Some(sender_entity) = sender_entity {
                let goal = BlockPosGoal(bot.entity_component::<Position>(sender_entity).to_block_pos_floor());
                let tasks = bot_state.tasks();
                bot_state.add_task(PathfindTask::new(!OPTS.no_mining, goal, format!("{sender}'s BlockPos")));
                if tasks > 0 {
                    feedback("Hang on. Walking to your block position in due time...");
                } else {
                    feedback("Walking to your block position...");
                }
            } else {
                feedback("I could not find you in my render distance!");
            }
            Ok(true)
        }
        "admins" => {
            feedback(&format!("Admins: {}", OPTS.admin.join(", ")));
            Ok(true)
        }
        "say" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
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
                feedback("Sorry, but you need to be specified as an admin to use this command!");
                return Ok(true);
            }

            info!("Stopping... Bye!");
            std::process::exit(crate::EXITCODE_USER_REQUESTED_STOP);
        }
        "pos" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
                return Ok(true);
            }
            if !OPTS.enable_pos_command {
                feedback("Sorry, but this command was not enabled. The owner needs to add the flag --enable-pos-command in order to do so!");
                return Ok(true);
            }

            let pos = bot.component::<Position>();
            let world_name = bot.component::<InstanceName>();
            feedback(&format!("I'm at {:.03} {:.03} {:.03} in {}", pos.x, pos.y, pos.z, world_name.path,));
            Ok(true)
        }

        "tps" => {
            if let Some(server_tps) = &bot_state.server_tps {
                if server_tps.is_server_likely_hanging() {
                    feedback("Seems that the server is currently hanging...");
                } else if let Some(tps) = server_tps.current_tps() {
                    feedback(&format!("Current TPS: {tps:.02}"));
                } else {
                    feedback("I don't know the TPS, yet!");
                }
            } else {
                feedback("Sorry, but ServerTps is not active!");
            }
            Ok(true)
        }

        "task" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
                return Ok(true);
            }

            feedback(&format!("Task: {}", bot_state.root_task_group.lock()));
            Ok(true)
        }

        "canceltasks" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
                return Ok(true);
            }

            let tasks = bot_state.tasks();
            if let Err(err) = bot_state.root_task_group.lock().stop(bot.clone(), &bot_state) {
                error!("Failed to stop root task group: {err:?}");
                feedback(&format!("Failed: {err:?}"));
            };
            *bot_state.root_task_group.lock() = TaskGroup::new_root();
            feedback(&format!("Stopped and removed all tasks ({tasks})!"));
            Ok(true)
        }

        "eat" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
                return Ok(true);
            }

            if EatTask::find_food_in_hotbar(bot).is_none() {
                feedback("No food found in hotbar!");
                return Ok(true);
            }
            bot_state.add_task_now(bot.clone(), bot_state, EatTask::default()).context("Add EatTask now")?;
            feedback("Attempting to eat now...");
            Ok(true)
        }

        "selecthand" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
                return Ok(true);
            }

            if args.len() != 1 {
                feedback("Please specify hotbar index (0-8).");
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

            feedback(&format!("Selected index {index}!"));
            Ok(true)
        }

        "drop" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
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

                feedback("I dropped the item in my hand!");
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
                feedback("Dropped specified indices!");
            }
            Ok(true)
        }

        "dropall" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
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

            feedback(&format!("I dropped all my items ({} stacks)!", slots.len()));
            Ok(true)
        }

        "swap" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
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

            feedback(&format!("Swapped main- and off-hand items!"));
            Ok(true)
        }

        "printinv" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
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

            feedback(&format!("Inv ({}): {}", slot_strings.len(), slot_strings.join(", "),));
            Ok(true)
        }

        "equip" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
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

            feedback(&format!("Equipped {} armor pieces!", equipped_armor_slots.len(),));
            Ok(true)
        }

        "unequip" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
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

            feedback(&format!("Unequipped {total} armor pieces!",));
            Ok(true)
        }

        "beer" => {
            let beertender = bot_state.beertender.as_ref();
            if beertender.is_none() {
                feedback("No beertender module active! :(...");
                return Ok(true);
            }
            let stasis = beertender.unwrap();
            let bot = bot.clone();
            stasis
                .request_beer(&sender.clone(), &mut bot.clone(), bot_state, move |_error, message| {
                    feedback(message);
                })
                .context("Request beer")?;
            Ok(true)
        }

        "totalbeer" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
                return Ok(true);
            }

            feedback(&format!("I have {} beer left.", BeertenderModule::beer_count(bot)));
            Ok(true)
        }

        "maxbeer" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
                return Ok(true);
            }
            let beertender = bot_state.beertender.as_ref();
            if beertender.is_none() {
                feedback("No beertender module active! :(...");
                return Ok(true);
            }
            let beertender = beertender.unwrap();

            if args.len() >= 1 {
                beertender.config.lock().max_beer = args[0].parse::<u32>()?;
                beertender.save_config().await?;
            }

            feedback(&format!("Max beer per person: {} (add number to change)", beertender.config.lock().max_beer));
            Ok(true)
        }

        "resetbeerfor" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
                return Ok(true);
            }
            let beertender = bot_state.beertender.as_ref();
            if beertender.is_none() {
                feedback("No beertender module active! :(...");
                return Ok(true);
            }
            let beertender = beertender.unwrap();

            if args.len() == 0 {
                feedback("Specify who's handed out beer count to reset.");
                return Ok(true);
            }

            beertender.config.lock().beer_handed_out.remove(&args[0].to_lowercase());
            beertender.save_config().await?;
            feedback(&format!("Reset {}'s handed out beer count.", args[0]));
            Ok(true)
        }

        "resetbeerforall" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
                return Ok(true);
            }
            let beertender = bot_state.beertender.as_ref();
            if beertender.is_none() {
                feedback("No beertender module active! :(...");
                return Ok(true);
            }
            let beertender = beertender.unwrap();

            beertender.config.lock().beer_handed_out.clear();
            beertender.save_config().await?;

            feedback("Reset everyones handed out beer count!");
            Ok(true)
        }

        "reloadbeerconfig" => {
            if !sender_is_admin {
                feedback("Sorry, but you need to be specified as an admin to use this command!");
                return Ok(true);
            }
            let beertender = bot_state.beertender.as_ref();
            if beertender.is_none() {
                feedback("No beertender module active! :(...");
                return Ok(true);
            }
            let beertender = beertender.unwrap();

            beertender.load_config().await?;
            feedback("Reloaded beertender-config!");
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

            let has_trapdoor = stasis.remembered_trapdoor_positions.lock().contains_key(&sender);
            let devnet_tx = DEVNET_TX.lock().clone();
            if let Some(devnet_tx) = devnet_tx {
                devnet_tx
                    .send(devnet::Message::CheckResponse {
                        for_mc_id,
                        pearls: if has_trapdoor { vec![serde_json::json!({})] } else { vec![] },
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
                .pull_pearl(&sender.clone(), &bot.clone(), bot_state, move |error, message| {
                    send_devnet_feedback(&own_username, for_mc_id, error, message)
                })
                .context("Pull pearl")?;
        }
        _ => {}
    }
    Ok(())
}

pub fn send_devnet_feedback(own_username: impl AsRef<str>, for_mc_id: Uuid, error: bool, message: impl AsRef<str>) {
    info!("FEEDBACK: {}", message.as_ref());
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
    info!("FEEDBACK2: {}", message.as_ref());
}
