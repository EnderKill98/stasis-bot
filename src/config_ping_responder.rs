//! Fixes an issue joining newer version minecraft servers through any kind of ViaVersion.
//! ViaVersion started sending out a ping in configuration phase and expects a response before continuing on.
//!
//! The current version of azalea did create an event for it, but never responded to it/the ping at all!

use azalea::packet::config::{ReceiveConfigPacketEvent, SendConfigPacketEvent};
use azalea::protocol::packets::config::{ClientboundConfigPacket, ServerboundConfigPacket, ServerboundPong};
use azalea::{
    app::{App, Plugin, PreUpdate},
    ecs::prelude::*,
    packet::config,
    prelude::*,
};

#[derive(Clone, Debug, Resource)]
pub struct ConfigurationPingResponder {}

impl Plugin for ConfigurationPingResponder {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, handle_configuration_ping.before(config::process_packet_events));
    }
}

fn handle_configuration_ping(
    mut commands: Commands,
    mut events: EventReader<ReceiveConfigPacketEvent>,
    //plugin: Res<ConfigurationPingResponder>,
    //entity: Single<Entity>,
) {
    for event in events.read() {
        if let ClientboundConfigPacket::Ping(ping) = &event.packet {
            let id = ping.id;
            commands.send_event(SendConfigPacketEvent {
                packet: ServerboundConfigPacket::Pong(ServerboundPong { id }),
                sent_by: event.entity,
            });
            debug!("Responded to ping in Configuration phase ({id})!");
        }
    }
}
