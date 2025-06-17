use crate::{BotState, OPTS};
use azalea::pathfinder::goals::{XZGoal, YGoal};
use azalea::{
    ecs::query::With,
    entity::{metadata::Player, Position},
    pathfinder::goals::BlockPosGoal,
    prelude::*,
    world::InstanceName,
    GameProfileComponent,
};
use rand::Rng;

pub fn execute(
    bot: &mut Client,
    _bot_state: &BotState,
    sender: String,
    mut command: String,
    args: Vec<String>,
) -> anyhow::Result<bool> {
    if command.starts_with('!') {
        command.remove(0);
    }
    command = command.to_lowercase();
    let sender_is_admin = OPTS
        .admin
        .iter()
        .any(|a| sender.eq_ignore_ascii_case(a) || a == "*")
        || OPTS.admin.is_empty();

    match command.as_str() {
        "help" => {
            let mut commands = vec!["!help", "!about", "!admins", "!ping"];
            if sender_is_admin {
                commands.append(&mut vec![
                    "!comehere",
                    "!say",
                    "!stop",
                    "!pos",
                    "!goto",
                    "!cancel",
                ]);
            }
            commands.sort();

            send_chat(bot, &format!("Commands: {}", commands.join(", ")));
            Ok(true)
        }
        "about" => {
            send_chat(bot, &format!("Hi {sender}, I'm running EnderKill98's azalea-based stasis-bot {}: github.com/EnderKill98/stasis-bot", env!("CARGO_PKG_VERSION")));
            Ok(true)
        }
        "comehere" => {
            if !sender_is_admin {
                send_chat(
                    bot,
                    &format!(
                        "Sorry, but you need to be specified as an admin to use this command!"
                    ),
                );
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
                send_chat(bot, &format!("Walking to your block position, {sender}..."));
            } else {
                send_chat(
                    bot,
                    &format!("I could not find you in my render distance, {sender}!"),
                );
            }
            Ok(true)
        }
        "cancel" => {
            if !sender_is_admin {
                send_chat(
                    bot,
                    &format!(
                        "Sorry, but you need to be specified as an admin to use this command!"
                    ),
                );
                return Ok(true);
            }

            bot.stop_pathfinding();
            send_chat(bot, &format!("Cancelled pathfinding!"));
            Ok(true)
        }
        "goto" => {
            if !sender_is_admin {
                send_chat(
                    bot,
                    &format!(
                        "Sorry, but you need to be specified as an admin to use this command!"
                    ),
                );
                return Ok(true);
            }

            let own_pos = bot.component::<Position>();
            let own_block_pos = azalea::BlockPos {
                x: own_pos.x.floor() as i32,
                y: own_pos.y.floor() as i32,
                z: own_pos.z.floor() as i32,
            };
            let components = match resolve_pos(
                own_block_pos,
                &args.iter().map(|arg| arg.as_str()).collect::<Vec<_>>(),
            ) {
                Ok(components) => components,
                Err(err) => {
                    send_chat(bot, &format!("Failed to parse coords: {err}"));
                    error!("Failed to parse coords: {err:?}");
                    return Ok(true);
                }
            };
            if components.len() == 3 {
                let goal = BlockPosGoal(azalea::BlockPos {
                    x: components[0],
                    y: components[1],
                    z: components[2],
                });
                if OPTS.no_mining {
                    bot.goto_without_mining(goal);
                } else {
                    bot.goto(goal)
                }
                send_chat(
                    bot,
                    &format!(
                        "Going to {} {} {}!",
                        components[0], components[1], components[2]
                    ),
                );
                Ok(true)
            } else if components.len() == 2 {
                let goal = XZGoal {
                    x: components[0],
                    z: components[1],
                };
                if OPTS.no_mining {
                    bot.goto_without_mining(goal);
                } else {
                    bot.goto(goal)
                }
                send_chat(
                    bot,
                    &format!("Going to XZ {} {}!", components[0], components[1]),
                );
                Ok(true)
            } else if components.len() == 1 {
                let goal = YGoal { y: components[0] };
                if OPTS.no_mining {
                    bot.goto_without_mining(goal);
                } else {
                    bot.goto(goal)
                }
                send_chat(bot, &format!("Going to Y {}!", components[0]));
                Ok(true)
            } else {
                send_chat(bot, "Expecting coordinates as either X Y Z, X Z or Y. Component examples: ~5, -100, 10..150, ~1..=100");
                Ok(true)
            }
        }
        "admins" => {
            send_chat(bot, &format!("Admins: {}", OPTS.admin.join(", ")));
            Ok(true)
        }
        "say" => {
            if !sender_is_admin {
                send_chat(
                    bot,
                    &format!(
                        "Sorry, but you need to be specified as an admin to use this command!"
                    ),
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
                send_chat(
                    bot,
                    &format!(
                        "Sorry, but you need to be specified as an admin to use this command!"
                    ),
                );
                return Ok(true);
            }

            info!("Stopping... Bye!");
            std::process::exit(crate::EXITCODE_USER_REQUESTED_STOP);
        }
        "pos" => {
            if !sender_is_admin {
                send_chat(
                    bot,
                    &format!(
                        "Sorry, but you need to be specified as an admin to use this command!"
                    ),
                );
                return Ok(true);
            }

            let pos = bot.component::<Position>();
            let world_name = bot.component::<InstanceName>();
            send_chat(
                bot,
                &format!(
                    "msg {sender} I'm at {:.03} {:.03} {:.03} in {}",
                    pos.x, pos.y, pos.z, world_name.path,
                ),
            );
            Ok(true)
        }
        "ping" => {
            send_chat(bot, "Pong!");
            Ok(true)
        }
        _ => Ok(false), // Do nothing if unrecognized command
    }
}

pub fn resolve_pos(own_pos: azalea::BlockPos, args: &[&str]) -> anyhow::Result<Vec<i32>> {
    let mut resolved = vec![];
    for component in args.into_iter() {
        let mut component = (*component).to_owned();
        let is_relative = if component.starts_with("~") {
            component.remove(0);
            true
        } else {
            false
        };
        if component.ends_with(",") {
            component = component[..component.len() - 1].to_string();
        }
        if component.is_empty() {
            component = String::from("0");
        }

        // start and end are both inclusive
        let (mut start, mut end) = if component.contains("..=") {
            let end: Vec<_> = component.split("..=").collect();
            (end[0].parse::<i32>()?, end[1].parse::<i32>()?)
        } else if component.contains("..") {
            let end: Vec<_> = component.split("..").collect();
            (end[0].parse::<i32>()?, end[1].parse::<i32>()? - 1)
        } else {
            let val = component.parse::<i32>()?;
            (val, val)
        };

        if start > end {
            (end, start) = (end, start);
        }

        if start == end {
            resolved.push((is_relative, start));
        } else {
            // Get random value in range
            resolved.push((is_relative, rand::rng().random_range(start..=end)));
        }
    }

    if resolved.len() == 1 {
        // <Y>
        if resolved[0].0 {
            resolved[0].1 = own_pos.y + resolved[0].1;
        }
    } else if resolved.len() == 2 {
        // <X> <Z>
        if resolved[0].0 {
            resolved[0].1 = own_pos.x + resolved[0].1;
        }
        if resolved[1].0 {
            resolved[1].1 = own_pos.z + resolved[1].1;
        }
    } else if resolved.len() == 3 {
        // <X> <Y> <Z>
        if resolved[0].0 {
            resolved[0].1 = own_pos.x + resolved[0].1;
        }
        if resolved[1].0 {
            resolved[1].1 = own_pos.y + resolved[1].1;
        }
        if resolved[2].0 {
            resolved[2].1 = own_pos.z + resolved[2].1;
        }
    }

    Ok(resolved
        .into_iter()
        .map(|(_is_relative, value)| value)
        .collect())
}

pub fn send_command(bot: &mut Client, command: &str) {
    if OPTS.quiet {
        info!("Quiet mode: Supressed sending command: {command}");
    } else {
        info!("Sending command: {command}");
        bot.send_command_packet(command);
    }
}

pub fn send_chat(bot: &mut Client, chat: &str) {
    if OPTS.quiet {
        info!("Quiet mode: Supressed sending chat: {chat}");
    } else {
        info!("Sending chat: {chat}");
        bot.send_chat_packet(chat);
    }
}
