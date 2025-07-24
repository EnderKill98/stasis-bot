use crate::BotState;
use crate::module::Module;
use azalea::protocol::packets::game::ClientboundGamePacket;
use azalea::{Client, Event};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct FinishedMeasurement {
    when: Instant,
    tps: f32,
}

struct PendingMeasurement {
    last_worldtime_update_at: Instant,
    last_time_ticks: u64,
}

#[derive(Clone, Default)]
pub struct ServerTpsModule {
    pending: Arc<Mutex<Option<PendingMeasurement>>>,
    finished: Arc<Mutex<Option<FinishedMeasurement>>>,
}

impl ServerTpsModule {
    fn reset(&self) {
        *self.finished.lock() = None;
        *self.pending.lock() = None;
        debug!("Reset tps measurement data");
    }

    pub fn current_tps(&self) -> Option<f32> {
        self.finished.lock().as_ref().map(|mp| mp.tps)
    }

    pub fn is_server_likely_hanging(&self) -> bool {
        if let Some(mp) = self.finished.lock().as_ref() {
            mp.when.elapsed() > Duration::from_secs(2)
        } else {
            // Not enough data, yet
            false
        }
    }
}

#[async_trait::async_trait]
impl Module for ServerTpsModule {
    fn name(&self) -> &'static str {
        "TpsMeasurement"
    }

    async fn handle(&self, _bot: Client, event: &Event, _bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Login | Event::Disconnect(_) => self.reset(),
            Event::Packet(packet) => match packet.as_ref() {
                ClientboundGamePacket::SetTime(packet) => {
                    let mut pending = self.pending.lock();
                    if let Some(some_pending) = pending.as_mut()
                        && packet.game_time < some_pending.last_time_ticks
                    {
                        // Cause re-initialize
                        warn!("We went backwards in time ({} < {})?!?", packet.game_time, some_pending.last_time_ticks);
                        *pending = None;
                    }

                    if pending.is_none() {
                        // Initialize
                        *pending = Some(PendingMeasurement {
                            last_worldtime_update_at: Instant::now(),
                            last_time_ticks: packet.game_time,
                        });
                        return Ok(());
                    }

                    // Calculate
                    let pending = pending.as_mut().unwrap();

                    let now = Instant::now();
                    let worldtime_gap_duration = now - pending.last_worldtime_update_at;
                    let ticks_elapsed = packet.game_time - pending.last_time_ticks;

                    pending.last_worldtime_update_at = now;
                    pending.last_time_ticks = packet.game_time;

                    let single_tick_duration = worldtime_gap_duration.as_millis() as f32 / ticks_elapsed as f32;
                    let current_tps = (1000f32 / single_tick_duration).min(20.0f32); // Ignore server catching up
                    if current_tps.is_finite() {
                        *self.finished.lock() = Some(FinishedMeasurement { when: now, tps: current_tps });
                    } else {
                        error!(
                            "WTF! Measured, non-finite ticks (WTGap={:?}, elapsed={}, 1TDur={})!?!?",
                            worldtime_gap_duration, ticks_elapsed, single_tick_duration
                        );
                    }
                }
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }
}
