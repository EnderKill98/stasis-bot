use crate::{BotState, OPTS};
use azalea::{
    ecs::query::With,
    entity::{metadata::Player, Position},
    pathfinder::goals::{BlockPosGoal, ReachBlockPosGoal},
    prelude::*,
    GameProfileComponent, Vec3,
};

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
                commands.append(&mut vec!["!comehere", "!say", "!stop"]);
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
            send_command(bot, &format!("msg {sender} Hi, I'm running EnderKill98's azalea-based stasis-bot: github.com/EnderKill98/stasis-bot"));
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
            std::process::exit(20);
        }

        _ => Ok(false), // Do nothing if unrecognized command
    }
}

pub fn send_command(bot: &mut Client, command: &str) {
    if OPTS.quiet {
        info!("Quiet mode: Supressed sending command: {command}");
    } else {
        info!("Sending command: {command}");
        bot.send_command_packet(command);
    }
}
