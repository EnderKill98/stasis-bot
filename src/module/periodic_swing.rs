use crate::module::Module;
use crate::{BotState, OPTS};
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::s_interact::InteractionHand;
use azalea::protocol::packets::game::{ServerboundGamePacket, ServerboundSwing};
use azalea::{Client, Event};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone)]
pub struct PeriodicSwingModule {
    tick_period: u32,
    ticks_since_last_swing: Arc<AtomicU32>,
}

impl PeriodicSwingModule {
    fn new(tick_period: u32) -> Self {
        Self {
            tick_period,
            ticks_since_last_swing: Default::default(),
        }
    }
}

impl Default for PeriodicSwingModule {
    fn default() -> Self {
        Self::new(20 * 30)
    }
}

#[async_trait::async_trait]
impl Module for PeriodicSwingModule {
    fn name(&self) -> &'static str {
        "PeriodicSwing"
    }

    async fn handle(&self, bot: Client, event: &Event, _bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Tick => {
                // 2b2t Anti AFK
                if OPTS.periodic_swing {
                    let mut ticks = self.ticks_since_last_swing.load(Ordering::Relaxed);
                    ticks += 1;
                    if ticks >= self.tick_period {
                        ticks = 0;
                        bot.ecs.lock().send_event(SendPacketEvent {
                            sent_by: bot.entity,
                            packet: ServerboundGamePacket::Swing(ServerboundSwing {
                                hand: InteractionHand::MainHand,
                            }),
                        });
                    }
                    self.ticks_since_last_swing.store(ticks, Ordering::Relaxed);
                }
            }
            _ => {}
        }
        Ok(())
    }
}
