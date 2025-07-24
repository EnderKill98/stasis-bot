use crate::BotState;
use crate::task::{Task, TaskOutcome};
use anyhow::anyhow;
use azalea::core::direction::Direction;
use azalea::entity::{LookDirection, Position};
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::s_interact::InteractionHand;
use azalea::protocol::packets::game::s_use_item_on::BlockHit;
use azalea::protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket, ServerboundUseItemOn};
use azalea::{BlockPos, Client, Event, Vec3};
use std::fmt::{Display, Formatter};
use std::time::Duration;
use tokio::time::Instant;

pub struct OpenContainerBlockTask {
    pub block_pos: BlockPos,

    started_at: Instant,
    orig_look_dir: LookDirection,
    did_interact: bool,
}

impl OpenContainerBlockTask {
    pub fn new(block_pos: BlockPos) -> Self {
        Self {
            block_pos,

            did_interact: false,

            // Doesn't matter:
            started_at: Instant::now(),
            orig_look_dir: LookDirection::new(0.0, 0.0),
        }
    }
}

impl Display for OpenContainerBlockTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "AffectBlock")
    }
}

impl Task for OpenContainerBlockTask {
    fn start(&mut self, bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        self.started_at = Instant::now();
        self.orig_look_dir = bot.component();

        let look_at_block = azalea::direction_looking_at(&Vec3::from(&bot.component::<Position>()), &self.block_pos.center());
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
                    reason: "Block not result in a screen open 5s after starting OpenContainerBlockTask!".to_owned(),
                });
            }
        } else if let Event::Packet(packet) = event {
            if let ClientboundGamePacket::OpenScreen(_) = packet.as_ref()
                && self.did_interact
            {
                return Ok(TaskOutcome::Succeeded);
            }
        }

        Ok(TaskOutcome::Ongoing)
    }
}
