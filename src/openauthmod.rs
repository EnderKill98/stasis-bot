use std::io::Cursor;

use azalea::app::Update;
use azalea::auth::sessionserver::ClientSessionServerError;
use azalea::buf::McBufReadable;
use azalea::packet_handling::login::{self, SendLoginPacketEvent};
use azalea::prelude::*;
use azalea::protocol::packets::login::serverbound_custom_query_answer_packet::ServerboundCustomQueryAnswerPacket;
use azalea::protocol::packets::login::ClientboundLoginPacket;
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

use azalea::ecs::system::Res;
use bevy_ecs::entity::Entity;
use bevy_ecs::event::EventWriter;
use bevy_ecs::schedule::IntoSystemConfigs;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Debug, Resource)]
pub struct OpenAuthModPlugin {
    auth_request_tx: mpsc::UnboundedSender<AuthRequest>,
}

impl Plugin for OpenAuthModPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            handle_openauthmod.before(login::process_packet_events),
        );
        app.add_systems(Update, handle_join_task);

        app.insert_resource(self.clone());
    }
}

impl Default for OpenAuthModPlugin {
    fn default() -> Self {
        let (auth_request_tx, auth_request_rx) = mpsc::unbounded_channel();

        tokio::spawn(handle_auth_requests_loop(auth_request_rx));

        Self { auth_request_tx }
    }
}

pub struct AuthRequest {
    server_id_hash: String,
    account: Account,
    tx: oneshot::Sender<Result<(), ClientSessionServerError>>,
}

async fn handle_auth_requests_loop(mut rx: mpsc::UnboundedReceiver<AuthRequest>) {
    while let Some(AuthRequest {
        server_id_hash,
        account,
        tx,
    }) = rx.recv().await
    {
        let client = reqwest::Client::new();

        let uuid = account.uuid_or_offline();
        let Some(access_token) = account.access_token.clone() else {
            continue;
        };

        let mut attempts = 0;
        let result = loop {
            if let Err(e) = {
                let access_token = access_token.lock().clone();
                azalea::auth::sessionserver::join_with_server_id_hash(
                    &client,
                    &access_token,
                    &uuid,
                    &server_id_hash,
                )
                .await
            } {
                if attempts >= 2 {
                    // if this is the second attempt and we failed both times, give up
                    break Err(e.into());
                }
                if matches!(
                    e,
                    ClientSessionServerError::InvalidSession
                        | ClientSessionServerError::ForbiddenOperation
                ) {
                    // uh oh, we got an invalid session and have to reauthenticate now
                    if let Err(e) = account.refresh().await {
                        error!("Failed to refresh account: {e:?}");
                        continue;
                    }
                } else {
                    break Err(e.into());
                }
                attempts += 1;
            } else {
                break Ok(());
            }
        };

        let _ = tx.send(result);
    }
}

fn handle_openauthmod(
    mut commands: Commands,
    mut events: EventReader<LoginPacketEvent>,
    mut query: Query<(&Account, &mut IgnoreQueryIds)>,

    plugin: Res<OpenAuthModPlugin>,
) {
    for event in events.read() {
        let ClientboundLoginPacket::CustomQuery(p) = &*event.packet else {
            continue;
        };
        let mut data = Cursor::new(&*p.data);

        match p.identifier.to_string().as_str() {
            "oam:join" => {
                let Ok(server_id_hash) = String::read_from(&mut data) else {
                    error!("Failed to read server id hash from oam:join packet");
                    continue;
                };

                let (account, mut ignore_query_ids) = query.get_mut(event.entity).unwrap();

                ignore_query_ids.insert(p.transaction_id);

                if account.access_token.is_none() {
                    error!("Server is online-mode, but our account is offline-mode");
                    continue;
                };

                let (tx, rx) = oneshot::channel();

                let request = AuthRequest {
                    server_id_hash,
                    account: account.clone(),
                    tx,
                };

                plugin.auth_request_tx.send(request).unwrap();

                commands.spawn(JoinServerTask {
                    entity: event.entity,
                    rx,
                    transaction_id: p.transaction_id,
                });
            }
            "oam:sign_nonce" => {}
            "oam:data" => {}
            _ => {}
        }
    }
}

#[derive(Component)]
struct JoinServerTask {
    entity: Entity,
    rx: oneshot::Receiver<Result<(), ClientSessionServerError>>,
    transaction_id: u32,
}

fn handle_join_task(
    mut commands: Commands,
    mut join_server_tasks: Query<(Entity, &mut JoinServerTask)>,
    mut send_packets: EventWriter<SendLoginPacketEvent>,
) {
    for (entity, mut task) in &mut join_server_tasks {
        if let Ok(result) = task.rx.try_recv() {
            // Task is complete, so remove task component from entity
            commands.entity(entity).remove::<JoinServerTask>();

            if let Err(e) = &result {
                error!("Sessionserver error: {e:?}");
            }

            send_packets.send(SendLoginPacketEvent {
                entity: task.entity,
                packet: ServerboundCustomQueryAnswerPacket {
                    transaction_id: task.transaction_id,
                    data: Some(vec![if result.is_ok() { 1 } else { 0 }].into()),
                }
                .get(),
            });
        }
    }
}
