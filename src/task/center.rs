use crate::BotState;
use crate::task::{Task, TaskOutcome};
use anyhow::anyhow;
use azalea::entity::{LookDirection, Position};
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket, ServerboundPingRequest};
use azalea::{BlockPos, Client, Event, Vec3, WalkDirection};
use rand::Rng;
use std::fmt::{Display, Formatter};
use std::time::Duration;
use tokio::time::Instant;

/*
#[derive(Default)]
pub struct LookTowardsCenterPlugin {}

#[derive(Component)]
pub struct LookTowardsCenterComponent {
    pub block_pos: BlockPos,
}

impl Plugin for LookTowardsCenterPlugin {
    fn build(&self, app: &mut App) {
        //app.add_systems(Update, Self::look_towards_block_center.before(handle_walk));
        app.add_systems(Update, Self::look_towards_block_center.after(clamp_look_direction).before(MoveEventsSet));
    }
}

impl LookTowardsCenterPlugin {
    fn look_towards_block_center(mut data: Query<(/*Entity,*/ &Position, &mut LookDirection, &LookTowardsCenterComponent), With<LocalEntity>>) {
        for (own_pos, mut look_direction, center) in &mut data {
            let target_pos = center.block_pos.to_vec3_floored() + Vec3::new(0.5, 0.0, 0.5);
            let yaw_to_center = azalea::direction_looking_at(&Vec3::new(own_pos.x, 0.0, own_pos.z), &Vec3::new(target_pos.x, 0.0, target_pos.z)).y_rot;
            *look_direction = util::fixed_look_direction(yaw_to_center, 45.0);
            trace!("Updated look towards center!");
        }
    }
}*/

#[derive(Debug, Clone)]
pub enum CenterState {
    WaitForStandstill {
        last_pos: Option<Vec3>,
    },
    CheckAndLook,
    StartWalk,
    StartMiniTeleport,
    CheckWalk,
    CheckMiniTeleport {
        started_at: Instant,
        sent_ping_id: Option<(i64, Instant)>,
        received_teleport: bool,
        received_pong: Option<Duration>,
    },
}
impl Default for CenterState {
    fn default() -> Self {
        CenterState::WaitForStandstill { last_pos: None }
    }
}

pub struct CenterTask {
    block_pos: BlockPos,

    attempt: u32,
    state: CenterState,
}

impl CenterTask {
    pub fn new(block_pos: BlockPos) -> Self {
        Self {
            block_pos,

            // Doesn't matter:
            attempt: 0,
            state: Default::default(),
        }
    }
}

impl Display for CenterTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Center")
    }
}

impl Task for CenterTask {
    fn start(&mut self, _bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        self.attempt = 0;
        self.state = Default::default();
        Ok(())
    }

    fn handle(&mut self, mut bot: Client, _bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        match event {
            Event::Tick => {
                if self.attempt > 25 {
                    return Ok(TaskOutcome::Failed {
                        reason: "Failed to center after 25 tries!".to_owned(),
                    });
                }

                trace!("Center State: {:?}", self.state);
                let own_pos = Vec3::from(&bot.component::<Position>());
                let target_pos = Vec3::new(self.block_pos.x as f64 + 0.5, own_pos.y, self.block_pos.z as f64 + 0.5);

                match self.state {
                    CenterState::WaitForStandstill { last_pos } => {
                        if Some(own_pos) == last_pos {
                            self.attempt += 1;
                            self.state = CenterState::CheckAndLook;
                        } else {
                            self.state = CenterState::WaitForStandstill { last_pos: Some(own_pos) };
                        }
                    }
                    CenterState::CheckAndLook => {
                        /*{
                            let mut ecs = bot.ecs.lock();
                            let mut entity = ecs.entity_mut(bot.entity);
                            entity.remove::<LookTowardsCenterComponent>();
                        }*/
                        let desired_dist = if self.attempt < 8 { 0.1 } else { 0.12 }; // 0.12 seems fine by grim

                        let yaw_to_center =
                            azalea::direction_looking_at(&Vec3::new(own_pos.x, 0.0, own_pos.z), &Vec3::new(target_pos.x, 0.0, target_pos.z)).y_rot;
                        *bot.ecs.lock().get_mut::<LookDirection>(bot.entity).ok_or(anyhow!("No lookdir"))? = LookDirection::new(yaw_to_center, 45.0);

                        let dist = own_pos.distance_to(&target_pos);
                        if dist <= desired_dist {
                            // Good enough, do mini teleport now
                            debug!("Got in close enough at attempt {}. {dist} blocks remain. Doing mini teleport!", self.attempt);
                            self.state = CenterState::StartMiniTeleport;
                        } else {
                            /*let mut ecs = bot.ecs.lock();
                            let mut entity = ecs.entity_mut(bot.entity);
                            entity.insert(LookTowardsCenterComponent { block_pos: self.chamber_pos });*/
                            self.state = CenterState::StartWalk;
                        }
                    }
                    CenterState::StartWalk => {
                        bot.walk(WalkDirection::Forward);
                        self.state = CenterState::CheckWalk;
                    }
                    CenterState::CheckWalk => {
                        let own_pos = Vec3::from(&bot.component::<Position>());
                        let walk_away = self.attempt == 4 || self.attempt == 18;
                        let target_pos = self.block_pos.to_vec3_floored() + Vec3::new(0.5, 0.0, 0.5);
                        if (!walk_away && own_pos.distance_to(&target_pos) <= 0.3) || walk_away && own_pos.distance_to(&target_pos) >= 0.1999 {
                            bot.walk(WalkDirection::None);
                            self.state = CenterState::WaitForStandstill { last_pos: Some(own_pos) };
                        }
                    }
                    CenterState::StartMiniTeleport => {
                        self.state = CenterState::CheckMiniTeleport {
                            started_at: Instant::now(),
                            sent_ping_id: None,
                            received_teleport: false,
                            received_pong: None,
                        };
                        *bot.ecs.lock().get_mut::<Position>(bot.entity).ok_or(anyhow!("No pos"))? = Position::new(target_pos);
                    }
                    CenterState::CheckMiniTeleport {
                        started_at,
                        ref mut sent_ping_id,
                        received_pong,
                        received_teleport,
                    } => {
                        if sent_ping_id.is_none() {
                            let ping_id = i64::MIN + rand::rng().random_range(0..i64::MAX / 4);
                            bot.ecs.lock().send_event(SendPacketEvent {
                                sent_by: bot.entity,
                                packet: ServerboundGamePacket::PingRequest(ServerboundPingRequest { time: ping_id as u64 }),
                            });
                            trace!("Sending ping with id {ping_id}");
                            *sent_ping_id = Some((ping_id, Instant::now()));
                        } else if let Some(ping_duration) = received_pong {
                            if received_teleport {
                                warn!("MiniTeleport failed (received teleport)! Attempting again...");
                                self.state = CenterState::WaitForStandstill { last_pos: Some(own_pos) };
                            } else if started_at.elapsed() >= Duration::from_millis(100).max(ping_duration * 2) {
                                info!("MiniTeleport seems to have been successful! Finished centering!");
                                return Ok(TaskOutcome::Succeeded);
                            }
                        }
                    }
                }
            }
            Event::Packet(packet) => match packet.as_ref() {
                ClientboundGamePacket::PlayerPosition(packet) => {
                    if let CenterState::CheckMiniTeleport { ref mut received_teleport, .. } = self.state {
                        debug!("Got a teleport to {} (id: {}) after starting mini teleport!", packet.change.pos, packet.id);
                        *received_teleport = true;
                    }
                }
                ClientboundGamePacket::PongResponse(packet) => {
                    if let CenterState::CheckMiniTeleport {
                        ref mut sent_ping_id,
                        ref mut received_pong,
                        ..
                    } = self.state
                    {
                        if let Some((id, when)) = sent_ping_id
                            && packet.time as i64 == *id
                        {
                            let ping_duration = when.elapsed();
                            if received_pong.is_some() {
                                warn!("Received another ping response to ping {id}?!?!?!?");
                            } else {
                                debug!("Got ping response ({ping_duration:?}).");
                                *received_pong = Some(ping_duration);
                            }
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }

        Ok(TaskOutcome::Ongoing)
    }

    fn stop(&mut self, mut bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        /*{
            let mut ecs = bot.ecs.lock();
            let mut entity = ecs.entity_mut(bot.entity);
            entity.remove::<LookTowardsCenterComponent>();
        }*/
        if let CenterState::CheckWalk = self.state {
            info!("Center task got stopped. So stopped walking");
            bot.walk(WalkDirection::None);
        }
        Ok(())
    }
}
