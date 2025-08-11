use crate::module::Module;
use crate::{BotState, OPTS, commands};
use azalea::protocol::packets::game::ClientboundGamePacket;
use azalea::{Client, Event};
use parking_lot::Mutex;
use rand::Rng;
use std::collections::VecDeque;
use std::fmt::Display;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

#[derive(Copy, Clone, PartialOrd, PartialEq, Debug)]
pub enum InGameMessageKind {
    Direct,
    PublicChat,
}

#[derive(Clone, PartialOrd, PartialEq, Debug)]
pub struct InGameMessage {
    pub kind: InGameMessageKind,
    pub user: String,
    pub message: String,
}

#[derive(Clone, PartialOrd, PartialEq, Debug)]
pub enum ChatAction {
    Message(InGameMessage),
    Command(String),
    Chat(String),
}

impl Display for ChatAction {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ChatAction::Message(message) => match message.kind {
                InGameMessageKind::Direct => write!(f, "Direct message to {:?}: {:?}", message.user, message.message),
                InGameMessageKind::PublicChat => write!(f, "Message to {:?} via public chat: {:?}", message.user, message.message),
            },
            ChatAction::Command(command) => write!(f, "Command: {command:?}"),
            ChatAction::Chat(chat) => write!(f, "Chat message: {chat:?}"),
        }
    }
}

#[derive(Clone, Default)]
pub struct ChatModule {
    queue: Arc<Mutex<VecDeque<ChatAction>>>,
    available_actions: Arc<AtomicU32>,
    last_dm_from: Arc<Mutex<Option<String>>>,
    last_anti_spam_word_count: Arc<AtomicU32>,
}

impl ChatModule {
    fn reset(&self) {
        self.available_actions.store(9, Ordering::Relaxed);
        self.queue.lock().clear();
        *self.last_dm_from.lock() = None;
    }

    pub fn queue(&self, action: ChatAction) {
        debug!("Queuing {action}");
        self.queue.lock().push_back(action);
    }

    pub fn msg(&self, to_user: impl AsRef<str>, message: impl AsRef<str>) {
        self.queue(ChatAction::Message(InGameMessage {
            kind: InGameMessageKind::Direct,
            user: to_user.as_ref().to_string(),
            message: message.as_ref().to_string(),
        }));
    }

    pub fn cmd(&self, command: impl AsRef<str>) {
        self.queue(ChatAction::Command(command.as_ref().to_string()));
    }

    pub fn chat(&self, chat: impl AsRef<str>) {
        let chat = chat.as_ref().to_string();
        if chat.starts_with("/") {
            self.queue(ChatAction::Command(chat));
        } else {
            self.queue(ChatAction::Chat(chat));
        }
    }

    fn send_msg_now(&self, bot: &mut Client, user: String, message: String) {
        let message = if OPTS.anti_anti_spam {
            let mut rng = rand::rng();
            let last_word_count = self.last_anti_spam_word_count.load(Ordering::Relaxed);
            let mut words: Vec<u32> = vec![1, 2, 3, 4, 5, 6];
            words.retain_mut(|v| (*v as i32 - last_word_count as i32).abs() > 1);
            let word_count = words[rng.random_range(0..words.len())];
            self.last_anti_spam_word_count.store(word_count, Ordering::Relaxed);
            let word_len = if word_count > 3 { rng.random_range(2..=4) } else { rng.random_range(4..=6) };

            let mut anti_spam = String::with_capacity(48);
            let chars = "abcdefghijklmnopqrstuvwxyz".chars().collect::<Vec<_>>();
            for i in 0..word_count {
                if i != 0 {
                    anti_spam.push(' ');
                }
                for _ in 0..word_len {
                    let char = &chars[rng.random_range(0..chars.len())];
                    anti_spam.push(*char);
                }
            }
            format!("{message} | {anti_spam}")
        } else {
            message
        };

        if let Some(reply_command) = &OPTS.reply_command
            && let Some(last_dm_from) = self.last_dm_from.lock().as_ref()
            && last_dm_from == &user
        {
            Self::send_command_now(bot, format!("{reply_command} {message}"));
        } else {
            let message_command = &OPTS.message_command;
            Self::send_command_now(bot, format!("{message_command} {user} {message}"));
        }
    }

    fn send_command_now(bot: &mut Client, command: String) {
        let command = if command.starts_with("/") { &command[1..] } else { &command }; // Remove leading /
        let command = if command.len() > 255 { &command[..255] } else { &command }; // Truncate
        bot.send_command_packet(command);
    }

    fn tick_queue(&self, bot: &mut Client) {
        let mut queue = self.queue.lock();
        loop {
            if queue.is_empty() {
                return;
            }
            if let Err(_) = self
                .available_actions
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| if v > 1 { Some(v - 1) } else { None })
            {
                return; // Nothing changed (None returned). We're at 0
            }

            let next_message = queue.pop_front().unwrap();
            info!("Sending {next_message}");
            match next_message {
                ChatAction::Message(ingame_message) => match ingame_message.kind {
                    InGameMessageKind::Direct => self.send_msg_now(bot, ingame_message.user, ingame_message.message),
                    InGameMessageKind::PublicChat => bot.send_chat_packet(&ingame_message.message),
                },
                ChatAction::Command(command) => Self::send_command_now(bot, command),
                ChatAction::Chat(chat) => bot.send_command_packet(&chat),
            }
        }
    }
}

#[async_trait::async_trait]
impl Module for ChatModule {
    fn name(&self) -> &'static str {
        "Chat"
    }

    async fn handle(&self, mut bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Login | Event::Disconnect(_) => self.reset(),
            Event::Packet(packet) => match packet.as_ref() {
                ClientboundGamePacket::SetTime(_) => {
                    self.available_actions
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| if v < 9 { Some(v + 1) } else { None })
                        .ok();
                }
                _ => {}
            },
            Event::Tick => {
                self.tick_queue(&mut bot);
            }
            Event::Chat(packet) => {
                info!(
                    "CHAT: {}",
                    if OPTS.no_color {
                        packet.message().to_string()
                    } else {
                        packet.message().to_ansi()
                    }
                );

                let message = packet.message().to_string();

                // Very security and sane way to find out, if message was a dm to self.
                // Surely no way to cheese it..
                let mut dm = None;
                if message.starts_with('[') && message.contains("-> me] ") {
                    // Common format used by Essentials and other custom plugins: [Someone -> me] Message
                    dm = Some((
                        message.split(" ").next().unwrap()[1..].to_owned(),
                        message.split("-> me] ").collect::<Vec<_>>()[1].to_owned(),
                    ));
                } else if message.contains(" whispers to you: ") {
                    // Vanilla minecraft: Someone whispers to you: Message
                    dm = Some((
                        message.split(" ").next().unwrap().to_owned(),
                        message.split("whispers to you: ").collect::<Vec<_>>()[1].to_owned(),
                    ));
                } else if message.contains(" whispers: ") {
                    // Used on 2b2t: Someone whispers: Message
                    let sender = message.split(" ").next().unwrap().to_owned();
                    let mut message = message.split(" whispers: ").collect::<Vec<_>>()[1].to_owned();
                    if message.ends_with(&sender) {
                        message = message[..(message.len() - sender.len())].to_owned();
                    }
                    dm = Some((sender, message));
                }

                if let Some((sender, content)) = dm {
                    *self.last_dm_from.lock() = Some(sender.to_owned());

                    let (command, args) = if content.contains(' ') {
                        let mut all_args: Vec<_> = content.to_owned().split(' ').map(|s| s.to_owned()).collect();
                        let command = all_args.remove(0);
                        (command, all_args)
                    } else {
                        (content.to_owned(), vec![])
                    };

                    info!("Executing command {command:?} sent by {sender:?} with args {args:?}");
                    let self_clone = self.clone();
                    let sender_clone = sender.clone();
                    match commands::execute(&mut bot, bot_state, sender.clone(), command, args, move |feedback| {
                        if OPTS.quiet {
                            info!("Suppressing feedback (--quiet): {feedback}")
                        } else {
                            self_clone.msg(&sender_clone, feedback);
                        }
                    })
                    .await
                    {
                        Ok(executed) => {
                            if executed {
                                *bot_state.last_dm_handled_at.lock() = Some(Instant::now());
                            } else {
                                warn!("Command was not executed. Most likely an unknown command.");
                            }
                        }
                        Err(err) => {
                            error!("Something went wrong when executing {sender:?}'s command {content:?}: {err:?}");
                            if !OPTS.quiet {
                                self.msg(sender, &format!("Oops: {err}"));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
