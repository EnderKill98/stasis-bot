use crate::module::Module;
use crate::{BotState, EXITCODE_LOW_HEALTH_OR_TOTEM_POP};
use azalea::core::game_type::GameMode;
use azalea::entity::Position;
use azalea::protocol::packets::game::ClientboundGamePacket;
use azalea::world::MinecraftEntityId;
use azalea::{Client, Event, Vec3};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone)]
pub struct EmergencyQuitModule {
    hp_threshold: f32,
    strict: bool,

    last_hp: Arc<Mutex<f32>>,
    ignore_until: Arc<Mutex<Option<Instant>>>,
}

impl EmergencyQuitModule {
    pub fn new(hp_threshold: f32, strict: bool) -> Self {
        Self {
            hp_threshold,
            strict,

            last_hp: Arc::new(Mutex::new(0.0)),
            ignore_until: Default::default(),
        }
    }

    pub fn ignore_harm_for(&self, duration: Duration) {
        *self.ignore_until.lock() = Some(Instant::now() + duration);
        trace!("Ignoring harm for {duration:?}...");
    }

    fn allowing_harm(&self, bot_state: &BotState) -> bool {
        if let Some(current_task) = bot_state.root_task_group.lock().subtasks.get(0)
            && current_task.to_string().contains("Suicide")
        {
            self.ignore_harm_for(Duration::from_secs(3));
            true
        } else {
            self.ignore_until.lock().map(|until| until >= Instant::now()).unwrap_or(false)
        }
    }
}

#[async_trait::async_trait]
impl Module for EmergencyQuitModule {
    fn name(&self) -> &'static str {
        "EmergencyQuit"
    }

    async fn handle(&self, mut bot: Client, event: &Event, bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Init | Event::Spawn => {
                self.ignore_harm_for(Duration::from_secs(2));
                *self.last_hp.lock() = 0.0;
            }
            Event::Tick => {
                /*
                let health = bot.component::<Health>().0;
                if health <= self.hp_threshold {
                    if bot
                        .tab_list()
                        .get(&bot.uuid())
                        .map(|entry| entry.gamemode == GameMode::Spectator)
                        .unwrap_or(false)
                    {
                        // Ignore in spectator
                        return Ok(());
                    }
                    if self.allowing_harm(bot_state) {
                        info!(
                            "Detected my Health to be below {:.02} (is {health:.02})! But currently ignoring harm.",
                            self.hp_threshold
                        );
                        return Ok(());
                    }
                    warn!(
                        "Detected my Health to be below {:.02} (is {health:.02})! Disconnecting and quitting...",
                        self.hp_threshold
                    );
                    let last_damage_event_suffix = if let Some(soundness) = &bot_state.soundness
                        && let Some((when, message)) = &*soundness.last_damage_event.lock()
                    {
                        format!("\n{:?} ago: {message}", when.elapsed())
                    } else {
                        String::new()
                    };
                    bot_state.webhook_alert(format!(
                        "`❤️‍🩹` Detected my Health to be below {:.02} (is {health:.02})! Disconnecting and quitting...{last_damage_event_suffix}",
                        self.hp_threshold
                    ));
                    bot.disconnect();
                    bot_state.wait_on_webhooks_and_exit(EXITCODE_LOW_HEALTH_OR_TOTEM_POP);
                }
                */
            }
            Event::Packet(packet) => match packet.as_ref() {
                ClientboundGamePacket::Respawn(_) => {
                    *self.ignore_until.lock() = Some(Instant::now() + Duration::from_secs(3));
                }
                ClientboundGamePacket::EntityEvent(packet) => {
                    let my_entity_id = bot.entity_component::<MinecraftEntityId>(bot.entity).0;
                    if packet.entity_id.0 == my_entity_id && packet.event_id == 35 {
                        if bot
                            .tab_list()
                            .get(&bot.uuid())
                            .map(|entry| entry.gamemode == GameMode::Spectator)
                            .unwrap_or(false)
                        {
                            // Ignore in spectator
                            return Ok(());
                        }
                        if self.allowing_harm(bot_state) {
                            info!("A totem was popped! But currently ignoring harm.");
                            return Ok(());
                        }
                        let last_damage_event_suffix = if let Some(soundness) = &bot_state.soundness
                            && let Some((when, message)) = &*soundness.last_damage_event.lock()
                        {
                            format!("\n{:?} ago: {message}", when.elapsed())
                        } else {
                            String::new()
                        };
                        // Totem popped!
                        warn!("A totem was popped! Disconnecting and quitting...");
                        bot_state.webhook_alert(format!("`💔` Popped a totem! Disconnecting and quitting...{last_damage_event_suffix}"));
                        bot.disconnect();
                        bot_state.wait_on_webhooks_and_exit(EXITCODE_LOW_HEALTH_OR_TOTEM_POP);
                    }
                }
                ClientboundGamePacket::SetHealth(packet) => {
                    if packet.health <= self.hp_threshold && (self.strict || packet.health < *self.last_hp.lock()) {
                        if bot
                            .tab_list()
                            .get(&bot.uuid())
                            .map(|entry| entry.gamemode == GameMode::Spectator)
                            .unwrap_or(false)
                        {
                            // Ignore in spectator
                            return Ok(());
                        }
                        if self.allowing_harm(bot_state) {
                            info!(
                                "My health got below {:.02} (is {:.02})! But currently ignoring harm.",
                                self.hp_threshold, packet.health
                            );
                            return Ok(());
                        }
                        warn!(
                            "My Health got below {:.02} (is {:.02})! Disconnecting and quitting...",
                            self.hp_threshold, packet.health
                        );
                        if let Some(position) = bot.get_component::<Position>() {
                            info!("I'm at {}", Vec3::from(&position));
                        }
                        let last_damage_event_suffix = if let Some(soundness) = &bot_state.soundness
                            && let Some((when, message)) = &*soundness.last_damage_event.lock()
                        {
                            format!("\n{:?} ago: {message}", when.elapsed())
                        } else {
                            String::new()
                        };
                        bot_state.webhook_alert(format!(
                            "`❤️‍🩹` My Health got below {:.02} (is {:.02})! Disconnecting and quitting...{last_damage_event_suffix}",
                            self.hp_threshold, packet.health
                        ));
                        bot.disconnect();
                        bot_state.wait_on_webhooks_and_exit(EXITCODE_LOW_HEALTH_OR_TOTEM_POP);
                    }
                }
                _ => {}
            },
            Event::Death(packet) => {
                if let Some(packet) = packet {
                    info!("You died: {}", packet.message);
                    bot_state.webhook_alert(format!("`💀` You died: {}", packet.message));
                } else {
                    info!("You died!");
                    bot_state.webhook_alert("`💀` You died");
                }
            }
            _ => {}
        }
        Ok(())
    }
}
