use crate::module::Module;
use crate::{BotState, EXITCODE_LOW_HEALTH_OR_TOTEM_POP};
use azalea::protocol::packets::game::ClientboundGamePacket;
use azalea::world::MinecraftEntityId;
use azalea::{Client, Event};

#[derive(Clone)]
pub struct EmergencyQuitModule {
    hp_threshold: f32,
}

impl EmergencyQuitModule {
    pub fn new(hp_threshold: f32) -> Self {
        Self { hp_threshold }
    }
}

#[async_trait::async_trait]
impl Module for EmergencyQuitModule {
    fn name(&self) -> &'static str {
        "EmergencyQuit"
    }

    async fn handle(&self, mut bot: Client, event: &Event, _bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Packet(packet) => match packet.as_ref() {
                ClientboundGamePacket::EntityEvent(packet) => {
                    let my_entity_id = bot.entity_component::<MinecraftEntityId>(bot.entity).0;
                    if packet.entity_id.0 == my_entity_id && packet.event_id == 35 {
                        // Totem popped!
                        warn!("Disconnecting and quitting because --autolog-hp is enabled...");
                        bot.disconnect();
                        std::process::exit(EXITCODE_LOW_HEALTH_OR_TOTEM_POP);
                    }
                }
                ClientboundGamePacket::SetHealth(packet) => {
                    if packet.health <= self.hp_threshold {
                        warn!("My Health got below {:.02}! Disconnecting and quitting...", self.hp_threshold);
                        bot.disconnect();
                        std::process::exit(EXITCODE_LOW_HEALTH_OR_TOTEM_POP);
                    }
                }
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }
}
