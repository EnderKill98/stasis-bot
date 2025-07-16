use anyhow::{Context, bail};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Destination {
    pub id: String,
    pub bot_name: String,
    pub reachable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Message {
    #[serde(rename_all = "camelCase")]
    DestinationsRequest,
    #[serde(rename_all = "camelCase")]
    DestinationsResponse { destinations: Vec<Destination> },
    #[serde(rename = "check_request")]
    CheckRequest { destination: String, for_mc_id: Uuid },
    #[serde(rename_all = "camelCase")]
    CheckResponse {
        // destination: String, // Not needed from destination (auto-added for client)
        for_mc_id: Uuid,
        pearls: Vec<serde_json::Value>, // JSON Objects
    },
    #[serde(rename_all = "camelCase")]
    PullRequest { destination: String, pearl_index: i32, for_mc_id: Uuid },
    #[serde(rename_all = "camelCase")]
    BotFeedback {
        error: bool,
        sender: String,
        //destination: String, // Not needed from destination (auto-added for client)
        message: String,
        for_player: Uuid,
    },
}

pub async fn run_forever(url: String, access_token: String, mut message_tx: Receiver<Message>, mut message_rx: Sender<Message>) -> ! {
    loop {
        if let Err(err) = run(&url, &access_token, &mut message_tx, &mut message_rx).await {
            error!("DevNet connection failed: {err:?}");
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn run(url: &str, access_token: &str, message_tx: &mut Receiver<Message>, message_rx: &mut Sender<Message>) -> anyhow::Result<()> {
    info!("Connecting to DevNet...");
    let uri = tokio_tungstenite::tungstenite::http::Uri::from_str(url)?;
    let request = tokio_tungstenite::tungstenite::ClientRequestBuilder::new(uri).with_header("Authorization", format!("Bot {access_token}"));
    let (mut socket, response) = tokio_tungstenite::connect_async(request).await?;

    if response.status().as_u16() >= 400 {
        bail!("Got {} when connecting to DevNet!", response.status());
    }
    info!("Connected to DevNet.");

    let mut keepalive_interval = tokio::time::interval(Duration::from_secs(25));

    loop {
        tokio::select! {
            maybe_message = socket.next() => {
                if let Some(message) = maybe_message {
                    let message = message.context("Receive message")?;
                    match message {
                        WsMessage::Text(content) => {
                            trace!("Received text message: {content}");
                            let message = serde_json::from_str(content.as_str()).context("Deserialize received message")?;
                            debug!("Received {message:?}");
                            message_rx.send(message).await.context("Forward received message to message_tx")?;
                        }
                        WsMessage::Binary(content) => {
                            let content = String::from_utf8(content.to_vec()).context("Decode received binary message as UTF-8")?;
                            trace!("Received binary message: {content}");
                            let message = serde_json::from_str(&content).context("Deserialize received message")?;
                            debug!("Received (binary) {message:?}");
                            message_rx.send(message).await.context("Forward received message to message_tx")?;
                        }
                        /*WsMessage::Ping(content) => {
                            info!("Received ping. Responding with pong");
                            socket.send(WsMessage::Pong(content)).await.context("Send pong")?;
                        }*/
                        WsMessage::Close(maybe_frame) => {
                            warn!("Received close frame from websocket ({maybe_frame:?})! Closing...");
                            socket.close(None).await?;
                            return Ok(());
                        }
                        _ => {} // Ignore
                    }
                }else {
                    warn!("No message received. Assuming connection got closed.");
                    socket.close(None).await?;
                    return Ok(());
                }
            }
            maybe_message = message_tx.recv() => {
                if let Some(message) = maybe_message {
                    debug!("Sending {message:?}");
                    let message = serde_json::to_string(&message).context("Serialize message")?;
                    socket.send(WsMessage::Text(message.into())).await.context("Send message to websocket")?;
                }else {
                    //warn!("Received None for message to transmit over websocket (channel might be closed)!")
                }
            }
            _ = keepalive_interval.tick() => {
                socket.send(WsMessage::Ping(vec![0x01, 0x02, 0x03].into())).await.context("Send ping to websocket")?;
            }
        }
    }
}
