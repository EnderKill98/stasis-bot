use crate::{BotState, EatingProgress, OPTS};
use azalea::core::direction::Direction;
use azalea::inventory::operations::ClickType;
use azalea::inventory::{Inventory, ItemStack, SetSelectedHotbarSlotEvent};
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::s_player_action::Action;
use azalea::protocol::packets::game::{
    ServerboundContainerClick, ServerboundGamePacket, ServerboundPlayerAction,
    ServerboundSetCarriedItem,
};
use azalea::registry::Item;
use azalea::{
    ecs::query::With,
    entity::{metadata::Player, Position},
    pathfinder::goals::{BlockPosGoal, ReachBlockPosGoal},
    prelude::*,
    world::InstanceName,
    BlockPos, GameProfileComponent, Vec3,
};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub fn execute(
    bot: &mut Client,
    bot_state: &BotState,
    sender: String,
    mut command: String,
    args: Vec<String>,
) -> anyhow::Result<bool> {
    if command.starts_with('!') {
        command.remove(0);
    }
    command = command.to_lowercase();
    let sender_is_admin = OPTS.admin.iter().any(|a| sender.eq_ignore_ascii_case(a));

    match command.as_str() {
        "help" => {
            let mut commands = vec!["!help", "!about"];
            if !OPTS.no_stasis {
                commands.push("!tp");
            }
            if sender_is_admin {
                commands.append(&mut vec![
                    "!comehere",
                    "!say",
                    "!stop",
                    "!selecthand",
                    "!drop",
                    "!dropall",
                    "!eat",
                    "!swap",
                    "!printinv",
                    "!equip",
                    "!unequip",
                ]);
                if OPTS.enable_pos_command {
                    commands.push("!pos");
                }
            }
            if !OPTS.admin.is_empty() {
                commands.push("!admins");
            }
            commands.sort();

            send_command(
                bot,
                &format!("msg {sender} Commands: {}", commands.join(", ")),
            );
            Ok(true)
        }
        "about" => {
            send_command(bot, &format!("msg {sender} Hi, I'm running EnderKill98's azalea-based stasis-bot {}: github.com/EnderKill98/stasis-bot", env!("CARGO_PKG_VERSION")));
            Ok(true)
        }
        "tp" => {
            let remembered_trapdoor_positions = bot_state.remembered_trapdoor_positions.lock();
            if OPTS.no_stasis {
                send_command(
                    bot,
                    &format!("msg {sender} I'm not allowed to do pearl duties :(..."),
                );
                return Ok(true);
            }

            if let Some(trapdoor_pos) = remembered_trapdoor_positions.get(&sender) {
                if bot_state.pathfinding_requested_by.lock().is_some() {
                    send_command(bot, &format!("msg {sender} Please ask again in a bit. I'm currently already going somewhere..."));
                    return Ok(true);
                }
                send_command(
                    bot,
                    &format!("msg {sender} Walking to your stasis chamber..."),
                );

                *bot_state.return_to_after_pulled.lock() =
                    Some(Vec3::from(&bot.entity_component::<Position>(bot.entity)));

                info!("Walking to {trapdoor_pos:?}...");
                let goal = ReachBlockPosGoal {
                    pos: azalea::BlockPos::from(*trapdoor_pos),
                    chunk_storage: bot.world().read().chunks.clone(),
                };
                if OPTS.no_mining {
                    bot.goto_without_mining(goal);
                } else {
                    bot.goto(goal);
                }
                *bot_state.pathfinding_requested_by.lock() = Some(sender.clone());
            } else {
                send_command(
                    bot,
                    &format!("msg {sender} I'm not aware whether you have a pearl here. Sorry!"),
                );
            }

            Ok(true)
        }
        "comehere" => {
            if !sender_is_admin {
                send_command(bot, &format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"));
                return Ok(true);
            }

            let sender_entity = bot.entity_by::<With<Player>, (&GameProfileComponent,)>(
                |(profile,): &(&GameProfileComponent,)| profile.name == sender,
            );
            if let Some(sender_entity) = sender_entity {
                let position = bot.entity_component::<Position>(sender_entity);
                let goal = BlockPosGoal(azalea::BlockPos {
                    x: position.x.floor() as i32,
                    y: position.y.floor() as i32,
                    z: position.z.floor() as i32,
                });
                if OPTS.no_mining {
                    bot.goto_without_mining(goal);
                } else {
                    bot.goto(goal)
                }
                send_command(
                    bot,
                    &format!("msg {sender} Walking to your block position..."),
                );
            } else {
                send_command(
                    bot,
                    &format!("msg {sender} I could not find you in my render distance!"),
                );
            }
            Ok(true)
        }
        "admins" => {
            send_command(
                bot,
                &format!("msg {sender} Admins: {}", OPTS.admin.join(", ")),
            );
            Ok(true)
        }
        "say" => {
            if !sender_is_admin {
                send_command(bot, &format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"));
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
                send_command(bot, &format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"));
                return Ok(true);
            }

            info!("Stopping... Bye!");
            std::process::exit(crate::EXITCODE_USER_REQUESTED_STOP);
        }
        "pos" => {
            if !sender_is_admin {
                send_command(bot, &format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"));
                return Ok(true);
            }
            if !OPTS.enable_pos_command {
                send_command(bot, &format!("msg {sender} Sorry, but this command was not enabled. The owner needs to add the flag --enable-pos-command in order to do so!"));
                return Ok(true);
            }

            let pos = bot.component::<Position>();
            let world_name = bot.component::<InstanceName>();
            send_command(
                bot,
                &format!(
                    "msg {sender} I'm at {:.03} {:.03} {:.03} in {}",
                    pos.x, pos.y, pos.z, world_name.path,
                ),
            );
            Ok(true)
        }

        "selecthand" => {
            if !sender_is_admin {
                send_command(bot, &format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"));
                return Ok(true);
            }

            if args.len() != 1 {
                send_command(
                    bot,
                    &format!("msg {sender} Please specify hotbar index (0-8)."),
                );
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
                    packet: ServerboundGamePacket::SetCarriedItem(ServerboundSetCarriedItem {
                        slot: index as u16,
                    }),
                });
            }

            send_command(bot, &format!("msg {sender} Selected index {index}!"));
            Ok(true)
        }

        "drop" => {
            if !sender_is_admin {
                send_command(bot, &format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"));
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

                send_command(bot, &format!("msg {sender} I dropped the item in my hand!"));
            } else {
                {
                    let mut ecs = bot.ecs.lock();
                    for arg in args {
                        let slot = arg.parse::<i16>()?;
                        ecs.send_event(SendPacketEvent {
                            sent_by: bot.entity,
                            packet: ServerboundGamePacket::ContainerClick(
                                ServerboundContainerClick {
                                    container_id: 0,
                                    state_id: 0,

                                    slot_num: slot,
                                    button_num: 1,
                                    click_type: ClickType::Throw,

                                    carried_item: ItemStack::Empty,
                                    changed_slots: HashMap::default(),
                                },
                            ),
                        });
                    }
                }
                send_command(bot, &format!("msg {sender} Dropped specified indices!"));
            }
            Ok(true)
        }

        "dropall" => {
            if !sender_is_admin {
                send_command(bot, &format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"));
                return Ok(true);
            }

            let inv = bot.entity_component::<Inventory>(bot.entity);
            let inv_menu = inv.inventory_menu;
            let slots = inv_menu
                .player_slots_range()
                .chain(5..=8 /*Armor*/)
                .chain(45..=45 /*Offhand*/)
                .filter(|slot| {
                    inv_menu
                        .slot(*slot)
                        .map(|stack| stack.is_present())
                        .unwrap_or(false)
                })
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

            send_command(
                bot,
                &format!(
                    "msg {sender} I dropped all my items ({} stacks)!",
                    slots.len()
                ),
            );
            Ok(true)
        }

        "eat" => {
            if !sender_is_admin {
                send_command(bot, &format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"));
                return Ok(true);
            }
            if bot_state.attempt_eat(bot) {
                *bot_state.eating_progress.lock() = (Instant::now(), EatingProgress::StartedEating);
                send_command(bot, &format!("msg {sender} Attempting to eat."));
            } else {
                send_command(bot, &format!("msg {sender} No edibles in my hotbar!"));
            }

            Ok(true)
        }

        "swap" => {
            if !sender_is_admin {
                send_command(bot, &format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"));
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

            send_command(
                bot,
                &format!("msg {sender} Swapped main- and off-hand items!"),
            );
            Ok(true)
        }

        "printinv" => {
            if !sender_is_admin {
                send_command(bot, &format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"));
                return Ok(true);
            }

            let inv = bot.entity_component::<Inventory>(bot.entity);
            let inv_menu = inv.inventory_menu;
            let mut slot_strings = Vec::new();
            for slot in inv_menu
                .player_slots_range()
                .chain(5..=8 /*Armor*/)
                .chain(45..=45 /*Offhand*/)
            {
                if let Some(item_stack_data) =
                    inv_menu.slot(slot).and_then(|stack| stack.as_present())
                {
                    slot_strings.push(format!(
                        "{slot}: {}x {}",
                        item_stack_data.count,
                        item_stack_data.kind.to_string().replace("minecraft:", "")
                    ))
                }
            }

            send_command(
                bot,
                &format!(
                    "msg {sender} Inv ({}): {}",
                    slot_strings.len(),
                    slot_strings.join(", "),
                ),
            );
            Ok(true)
        }

        "equip" => {
            if !sender_is_admin {
                send_command(bot, &format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"));
                return Ok(true);
            }

            let inv = bot.entity_component::<Inventory>(bot.entity);

            let mut equipped_armor_slots = HashSet::<usize>::default();
            let inv_menu = inv.inventory_menu;
            {
                let mut ecs = bot.ecs.lock();
                for slot in inv_menu.player_slots_range() {
                    if let Some(item_stack_data) =
                        inv_menu.slot(slot).and_then(|stack| stack.as_present())
                    {
                        let item_name = item_stack_data.kind.to_string();
                        let target_slot: Option<usize> = if item_name.ends_with("_helmet") {
                            Some(5)
                        } else if item_name.ends_with("_chestplate")
                            || item_stack_data.kind == Item::Elytra
                        {
                            Some(6)
                        } else if item_name.ends_with("_leggings") {
                            Some(7)
                        } else if item_name.ends_with("_boots") {
                            Some(8)
                        } else {
                            None
                        };

                        if let Some(target_slot) = target_slot {
                            if !equipped_armor_slots.contains(&target_slot)
                                && !inv_menu
                                    .slot(target_slot)
                                    .map(|stack| stack.is_present())
                                    .unwrap_or(false)
                            {
                                ecs.send_event(SendPacketEvent {
                                    sent_by: bot.entity,
                                    packet: ServerboundGamePacket::ContainerClick(
                                        ServerboundContainerClick {
                                            container_id: 0,
                                            state_id: 0,

                                            slot_num: slot as i16,
                                            button_num: 0,
                                            click_type: ClickType::QuickMove,

                                            carried_item: ItemStack::Empty,
                                            changed_slots: HashMap::default(),
                                        },
                                    ),
                                });
                                equipped_armor_slots.insert(target_slot);
                            }
                        }
                    }
                }
            }

            send_command(
                bot,
                &format!(
                    "msg {sender} Equipped {} armor pieces!",
                    equipped_armor_slots.len(),
                ),
            );
            Ok(true)
        }

        "unequip" => {
            if !sender_is_admin {
                send_command(bot, &format!("msg {sender} Sorry, but you need to be specified as an admin to use this command!"));
                return Ok(true);
            }

            let inv = bot.entity_component::<Inventory>(bot.entity);

            let inv_menu = inv.inventory_menu;
            let mut total = 0;
            {
                let mut ecs = bot.ecs.lock();
                for slot in 5..=8 {
                    if inv_menu
                        .slot(slot)
                        .map(|stack| stack.is_present())
                        .unwrap_or(false)
                    {
                        ecs.send_event(SendPacketEvent {
                            sent_by: bot.entity,
                            packet: ServerboundGamePacket::ContainerClick(
                                ServerboundContainerClick {
                                    container_id: 0,
                                    state_id: 0,

                                    slot_num: slot as i16,
                                    button_num: 0,
                                    click_type: ClickType::QuickMove,

                                    carried_item: ItemStack::Empty,
                                    changed_slots: HashMap::default(),
                                },
                            ),
                        });
                        total += 1;
                    }
                }
            }

            send_command(
                bot,
                &format!("msg {sender} Unequipped {total} armor pieces!",),
            );
            Ok(true)
        }

        _ => Ok(false), // Do nothing if unrecognized command
    }
}

pub fn send_command(bot: &mut Client, command: &str) {
    if OPTS.quiet {
        info!("Quiet mode: Suppressed sending command: {command}");
    } else {
        let truncated_command = if command.len() > 255 {
            &command[..255]
        } else {
            command
        };
        info!(
            "Sending command{}: {command}",
            if command.len() != truncated_command.len() {
                " (truncated to 255 chars!)"
            } else {
                ""
            }
        );
        bot.send_command_packet(truncated_command);
    }
}
