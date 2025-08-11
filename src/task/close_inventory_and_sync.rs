use crate::BotState;
use crate::task::{Task, TaskOutcome};
use azalea::inventory::operations::ClickType;
use azalea::inventory::{ClientSideCloseContainerEvent, Inventory, ItemStack};
use azalea::packet::game::SendPacketEvent;
use azalea::protocol::packets::game::{ClientboundGamePacket, ServerboundContainerClick, ServerboundContainerClose, ServerboundGamePacket};
use azalea::{Client, Event};
use std::fmt::{Display, Formatter};
use std::time::Duration;
use tokio::time::Instant;

pub struct CloseInventoryAndSyncTask {
    started_at: Instant,
}

impl Default for CloseInventoryAndSyncTask {
    fn default() -> Self {
        Self {
            // Doesn't matter:
            started_at: Instant::now(),
        }
    }
}

impl Display for CloseInventoryAndSyncTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "CloseInventoryAndSync")
    }
}

impl Task for CloseInventoryAndSyncTask {
    fn start(&mut self, bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        self.started_at = Instant::now();
        let inv = bot.component::<Inventory>();
        let mut ecs = bot.ecs.lock();
        // Bricks lectern page choosing somehow!!!
        /*ecs.send_event(CloseContainerEvent {
            entity: bot.entity,
            id: inv.id,
        });*/
        ecs.send_event(SendPacketEvent {
            sent_by: bot.entity,
            packet: ServerboundGamePacket::ContainerClose(ServerboundContainerClose { container_id: inv.id }),
        });
        ecs.send_event(ClientSideCloseContainerEvent { entity: bot.entity });
        ecs.send_event(SendPacketEvent {
            sent_by: bot.entity,
            packet: ServerboundGamePacket::ContainerClick(ServerboundContainerClick {
                container_id: 0,
                click_type: ClickType::Pickup,
                slot_num: 0,
                button_num: 3,
                state_id: i32::MAX as u32,
                carried_item: ItemStack::Empty,
                changed_slots: Default::default(),
            }),
        });
        // Maybe fix grim button press issues
        ecs.send_event(SendPacketEvent {
            sent_by: bot.entity,
            packet: ServerboundGamePacket::ContainerClose(ServerboundContainerClose { container_id: 0 }),
        });
        Ok(())
    }

    fn handle(&mut self, _bot: Client, _bot_state: &BotState, event: &Event) -> anyhow::Result<TaskOutcome> {
        match event {
            Event::Tick => {
                if self.started_at.elapsed() > Duration::from_secs(5) {
                    return Ok(TaskOutcome::Failed {
                        reason: "Waited over 5s waiting for inventory to sync!".to_owned(),
                    });
                }
            }
            Event::Packet(packet) => match packet.as_ref() {
                ClientboundGamePacket::ContainerSetContent(_) => {
                    return Ok(TaskOutcome::Succeeded);
                }
                ClientboundGamePacket::ContainerClose(packet) => {
                    debug!("Got a container close packet for container id {}", packet.container_id);
                }
                _ => {}
            },
            _ => {}
        }
        Ok(TaskOutcome::Ongoing)
    }
}
