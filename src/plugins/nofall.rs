use std::borrow::BorrowMut;
use std::io::Cursor;

use azalea::app::{PostStartup, Update};
use azalea::buf::McBufReadable;
use azalea::packet_handling::game::SendPacketEvent;
use azalea::prelude::*;
use azalea::protocol::packets::game::serverbound_move_player_pos_packet::ServerboundMovePlayerPosPacket;
use azalea::protocol::packets::game::ServerboundGamePacket;
use azalea::protocol::packets::login::ClientboundLoginPacket;
use azalea::raw_connection::RawConnection;
use azalea::{
    app::{App, Plugin, PreUpdate},
    ecs::{
        event::EventReader,
        system::{Commands, Query},
    },
    packet_handling::login::{IgnoreQueryIds, LoginPacketEvent},
    prelude::Resource,
    Account,
};

use bevy_ecs::change_detection::DetectChangesMut;
use bevy_ecs::event::{event_update_system, EventWriter, Events};
use bevy_ecs::schedule::{IntoSystemConfigs, IntoSystemSet};
use bevy_ecs::system::ResMut;
use tokio::sync::oneshot;

#[derive(Clone, Debug, Resource, Default)]
pub struct NoFallPlugin {}

impl Plugin for NoFallPlugin {
    fn build(&self, app: &mut App) {
        /*app.add_systems(
            PreUpdate,
            handle_nofall.before(azalea::packet_handling::game::handle_send_packet_event),
            bevy_ecs::event::EventWriter::
        );*/
        /*app.add_systems(
            PreUpdate,
            handle_nofall_2.before(event_update_system::<SendPacketEvent>),
        );*/
        //app.add_systems(PreUpdate, filter_packets.into_system_set());
        /*app.add_systems(
            Update,
            filter_packets.before(azalea::packet_handling::game::handle_send_packet_event),
        );*/
        app.add_systems(
            Update,
            handle_nofall_2.before(azalea::packet_handling::game::handle_send_packet_event),
        );
        app.insert_resource(self.clone());
    }
}

fn handle_nofall(
    mut send_packet_events: EventReader<SendPacketEvent>,
    _query: Query<&mut RawConnection>,
) {
    for send_packet_event in send_packet_events.read() {
        match &send_packet_event.packet {
            ServerboundGamePacket::MovePlayerPos(packet) => {
                info!("OnGround: {}", packet.on_ground);
            }
            ServerboundGamePacket::MovePlayerRot(packet) => {
                info!("OnGround: {}", packet.on_ground);
            }
            ServerboundGamePacket::MovePlayerStatusOnly(packet) => {
                info!("OnGround: {}", packet.on_ground);
            }
            ServerboundGamePacket::MovePlayerPosRot(packet) => {
                info!("OnGround: {}", packet.on_ground);
            }
            _ => {}
        }
    }
}

fn handle_nofall_2(mut events: ResMut<Events<SendPacketEvent>>) {
    let mut new_events = Vec::with_capacity(events.len());
    let events = events.bypass_change_detection();
    for mut send_packet_event in events.update_drain() {
        match &mut send_packet_event.packet {
            ServerboundGamePacket::MovePlayerPos(packet) => {
                info!("OnGround: {}", packet.on_ground);
                packet.on_ground = true;
            }
            ServerboundGamePacket::MovePlayerRot(packet) => {
                info!("OnGround: {}", packet.on_ground);
                packet.on_ground = true;
            }
            ServerboundGamePacket::MovePlayerStatusOnly(packet) => {
                info!("OnGround: {}", packet.on_ground);
                packet.on_ground = true;
            }
            ServerboundGamePacket::MovePlayerPosRot(packet) => {
                info!("OnGround: {}", packet.on_ground);
                packet.on_ground = true;
            }
            _ => {}
        }
        new_events.push(send_packet_event);
    }
    events.send_batch(new_events);
    //events.update();
}

fn filter_packets(
    events: EventReader<SendPacketEvent>,
    filtered_events: EventWriter<SendPacketEvent>,
) {
}
