use crate::task::{Task, TaskOutcome};
use crate::{BotState, util};
use anyhow::anyhow;
use azalea::container::ContainerClientExt;
use azalea::entity::LookDirection;
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::s_interact::InteractionHand;
use azalea::protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket, ServerboundSwing, ServerboundUseItemOn};
use azalea::{BlockPos, Client, Event};
use std::fmt::{Display, Formatter};
use std::time::Duration;
use tokio::time::Instant;

pub struct OpenContainerBlockTask {
    pub block_pos: BlockPos,

    started_at: Instant,
    attempt: usize,
    orig_look_dir: LookDirection,
    did_interact: bool,
    got_open_packet_at: Option<Instant>,
    got_inventory_data: bool,
    got_container_menu: bool,
}

impl OpenContainerBlockTask {
    pub fn new(block_pos: BlockPos) -> Self {
        Self {
            block_pos,

            did_interact: false,

            // Doesn't matter:
            started_at: Instant::now(),
            orig_look_dir: LookDirection::new(0.0, 0.0),

            attempt: 0,
            got_open_packet_at: None,
            got_container_menu: false,
            got_inventory_data: false,
        }
    }
}

impl Display for OpenContainerBlockTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "OpenContainerBlock")
    }
}

impl Task for OpenContainerBlockTask {
    fn start(&mut self, bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        self.started_at = Instant::now();
        self.orig_look_dir = bot.component();

        let look_at_block = util::nice_blockhit_look(&util::own_eye_pos(&bot).ok_or(anyhow!("No own_eye_pos"))?, &self.block_pos);
        *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = look_at_block;
        Ok(())
    }

    fn handle(&mut self, bot: Client, _bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        if let Event::Tick = event {
            /*let eye_pos = util::own_eye_pos(&bot);
            let pos_at_block = util::closest_aabb_pos_towards(eye_pos, util::aabb_from_blockpos(self.block_pos), 0.2);
            let look_at_block = azalea::direction_looking_at(&eye_pos, &pos_at_block);
            *bot.ecs.lock().get_mut(bot.entity).ok_or(anyhow!("No lookdir"))? = look_at_block;*/
            let eye_pos = match util::own_eye_pos(&bot) {
                Some(pos) => pos,
                None => return Ok(TaskOutcome::Ongoing),
            };

            if self.got_open_packet_at.is_some() && bot.get_open_container().is_some() {
                self.got_container_menu = true;
            }

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
                self.did_interact = true;
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
            } else if let Some(got_open_packet_at) = self.got_open_packet_at {
                match (self.got_inventory_data, self.got_container_menu) {
                    (false, false) => {
                        if got_open_packet_at.elapsed() >= Duration::from_secs(5) {
                            if self.attempt <= 2 {
                                warn!(
                                    "Did neither receive inventory data, nor a container_menu handle 5s after receiving OpenScreen packet. This is attempt {}. Re-attempting...!",
                                    self.attempt
                                );
                                // Re-attempt
                                self.attempt += 1;
                                self.did_interact = false;
                                self.got_open_packet_at = None;
                                self.started_at = Instant::now();
                            } else {
                                return Ok(TaskOutcome::Failed {
                                    reason: "Did neither receive inventory data, nor a container_menu handle 5s after receiving OpenScreen packet. And ran out of attempts!".to_owned(),
                                });
                            }
                        }
                    }
                    (true, false) => {
                        if got_open_packet_at.elapsed() >= Duration::from_secs(10) {
                            if self.attempt <= 2 {
                                warn!(
                                    "Did receive inventory data, but no container_menu handle 10s after receiving OpenScreen packet. This is attempt {}. Re-attempting...!",
                                    self.attempt
                                );
                                // Re-attempt
                                self.attempt += 1;
                                self.did_interact = false;
                                self.got_open_packet_at = None;
                                self.started_at = Instant::now();
                            } else {
                                return Ok(TaskOutcome::Failed {
                                        reason: "Did receive inventory data, but no container_menu handle 10s after receiving OpenScreen packet. And ran out of attempts!".to_owned(),
                                    });
                            }
                        }
                    }
                    (false, true) => {
                        if got_open_packet_at.elapsed() >= Duration::from_millis(5) {
                            warn!(
                                "Did receive no inventory data, but container_menu handle 5s after receiving OpenScreen packet. Assuming the sevrer just didn't send it, which should be fine. Success."
                            );
                            return Ok(TaskOutcome::Succeeded);

                            /*
                            if self.attempt <= 2 {
                                warn!(
                                    "Did receive no inventory data, but container_menu handle 10s after receiving OpenScreen packet. This is attempt {}. Re-attempting...!",
                                    self.attempt
                                );
                                // Re-attempt
                                self.attempt += 1;
                                self.did_interact = false;
                                self.got_open_packet_at = None;
                                self.started_at = Instant::now();
                            } else {
                                return Ok(TaskOutcome::Failed {
                                    reason: "Did receive no inventory data, but container_menu handle 10s after receiving OpenScreen packet. And ran out of attempts!".to_owned(),
                                });
                            }*/
                        }
                    }
                    (true, true) => {
                        info!(
                            "Got both Inventory data and ContainerMenu {:?} after receiving OpenScreen packet. Success!",
                            got_open_packet_at.elapsed()
                        );
                        return Ok(TaskOutcome::Succeeded);
                    }
                }
            } else if self.started_at.elapsed() >= Duration::from_secs(5) && self.did_interact {
                if self.attempt <= 2 {
                    warn!(
                        "Block not result in a screen open 5s after starting OpenContainerBlockTask. This is attempt {}. Re-attempting...!",
                        self.attempt + 1
                    );
                    // Re-attempt
                    self.attempt += 1;
                    self.did_interact = false;
                    self.got_open_packet_at = None;
                    self.started_at = Instant::now();
                } else {
                    return Ok(TaskOutcome::Failed {
                        reason: "Block not result in a screen open 5s after starting OpenContainerBlockTask!".to_owned(),
                    });
                }
            }
        } else if let Event::Packet(packet) = event {
            match packet.as_ref() {
                ClientboundGamePacket::OpenScreen(_) => {
                    if self.did_interact {
                        self.got_open_packet_at = Some(Instant::now());
                    }
                }
                ClientboundGamePacket::ContainerSetContent(_) => {
                    if self.got_open_packet_at.is_some() {
                        self.got_inventory_data = true;
                    }
                }
                _ => {}
            }
        }

        Ok(TaskOutcome::Ongoing)
    }
}
