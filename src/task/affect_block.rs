use crate::task::{Task, TaskOutcome};
use crate::{BotState, util};
use anyhow::{anyhow, bail};
use azalea::blocks::BlockState;
use azalea::core::direction::Direction;
use azalea::entity::LookDirection;
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::s_interact::InteractionHand;
use azalea::protocol::packets::game::s_use_item_on::BlockHit;
use azalea::protocol::packets::game::{ServerboundGamePacket, ServerboundUseItemOn};
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
}

impl AffectBlockTask {
    pub fn new(block_pos: BlockPos) -> Self {
        Self {
            block_pos,

            did_interact: false,

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

        let look_at_block = azalea::direction_looking_at(&util::own_eye_pos(&bot), &self.block_pos.center());
        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = look_at_block;
        Ok(())
    }

    fn handle(&mut self, bot: Client, _bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        if let Event::Tick = event {
            if self.started_at.elapsed() >= Duration::from_millis(100) && !self.did_interact {
                bot.ecs.lock().send_event(SendPacketEvent {
                    sent_by: bot.entity,
                    packet: ServerboundGamePacket::UseItemOn(ServerboundUseItemOn {
                        block_hit: BlockHit {
                            block_pos: self.block_pos,
                            direction: Direction::Down,
                            location: self.block_pos.center(),
                            inside: true,
                            world_border: false,
                        },
                        hand: InteractionHand::MainHand,
                        sequence: 0,
                    }),
                });
                self.did_interact = true;
            } else if self.started_at.elapsed() >= Duration::from_secs(5) && self.did_interact {
                return Ok(TaskOutcome::Failed {
                    reason: "Block did not get affected 5s after starting AffectBlockTask!".to_owned(),
                });
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
