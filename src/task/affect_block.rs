use crate::task::{Task, TaskOutcome};
use crate::{BotState, util};
use anyhow::{anyhow, bail};
use azalea::blocks::BlockState;
use azalea::entity::LookDirection;
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::s_interact::InteractionHand;
use azalea::protocol::packets::game::{ServerboundGamePacket, ServerboundSwing, ServerboundUseItemOn};
use azalea::{BlockPos, Client, Event};
use std::fmt::{Display, Formatter};
use std::time::Duration;
use tokio::time::Instant;

pub struct AffectBlockTask {
    pub block_pos: BlockPos,

    started_at: Instant,
    orig_look_dir: LookDirection,
    orig_block_state: BlockState,
    did_interact: bool,
    attempt: usize,
}

impl AffectBlockTask {
    pub fn new(block_pos: BlockPos) -> Self {
        Self {
            block_pos,

            did_interact: false,
            attempt: 0,

            // Doesn't matter:
            started_at: Instant::now(),
            orig_look_dir: LookDirection::new(0.0, 0.0),
            orig_block_state: BlockState::default(),
        }
    }
}

impl Display for AffectBlockTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "AffectBlock")
    }
}

impl Task for AffectBlockTask {
    fn start(&mut self, bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        self.started_at = Instant::now();
        self.orig_look_dir = bot.component();
        if !self.did_interact {
            if let Some(state) = bot.world().read().get_block_state(&self.block_pos) {
                self.orig_block_state = state;
            } else {
                bail!("Failed to find block state of block to affect!");
            }
        }

        let look_at_block = util::nice_blockhit_look(&util::own_eye_pos(&bot).ok_or(anyhow!("No own_eye_pos"))?, &self.block_pos);
        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = look_at_block;
        Ok(())
    }

    fn handle(&mut self, bot: Client, _bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        if let Event::Tick = event {
            let eye_pos = match util::own_eye_pos(&bot) {
                Some(pos) => pos,
                None => return Ok(TaskOutcome::Ongoing),
            };

            if self.started_at.elapsed() >= Duration::from_millis(100) && !self.did_interact {
                if let Some(state) = bot.world().read().get_block_state(&self.block_pos) {
                    if state.is_air() {
                        return Ok(TaskOutcome::Failed {
                            reason: "Target block is air!".to_owned(),
                        });
                    }
                }

                let block_hit = match util::nice_blockhit(&eye_pos, &self.block_pos) {
                    Ok((_look_dir, block_hit)) => block_hit, // Look dir was already set earlier
                    Err(err) => {
                        return Ok(TaskOutcome::Failed {
                            reason: format!("Failed to calculate block hit: {err}"),
                        });
                    }
                };
                debug!("BlockHit: {block_hit:?}");
                bot.ecs.lock().send_event(SendPacketEvent {
                    sent_by: bot.entity,
                    packet: ServerboundGamePacket::UseItemOn(ServerboundUseItemOn {
                        block_hit,
                        hand: InteractionHand::MainHand,
                        sequence: 0,
                    }),
                });
                // Aren't we nice?
                bot.ecs.lock().send_event(SendPacketEvent {
                    sent_by: bot.entity,
                    packet: ServerboundGamePacket::Swing(ServerboundSwing {
                        hand: InteractionHand::MainHand,
                    }),
                });
                self.did_interact = true;
            } else if self.started_at.elapsed() >= Duration::from_secs(5) && self.did_interact {
                if self.attempt <= 2 {
                    self.attempt += 1;
                    warn!(
                        "Block did not get affected 5s after starting AffectBlockTask! Reattempting... (re-attempt {})",
                        self.attempt
                    );
                    self.did_interact = false;
                    let look_at_block = util::nice_blockhit_look(&util::own_eye_pos(&bot).ok_or(anyhow!("No own_eye_pos"))?, &self.block_pos);
                    *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = look_at_block;
                    self.started_at = Instant::now();
                } else {
                    return Ok(TaskOutcome::Failed {
                        reason: "Block did not get affected 5s after starting AffectBlockTask (and ran out of attempts)!".to_owned(),
                    });
                }
            }

            if self.did_interact
                && let Some(state) = bot.world().read().get_block_state(&self.block_pos)
                && state != self.orig_block_state
            {
                return Ok(TaskOutcome::Succeeded);
            }
        }

        Ok(TaskOutcome::Ongoing)
    }
}
