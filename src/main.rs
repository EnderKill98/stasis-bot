//! A bot that logs chat messages sent in the server to the console.

#[macro_use]
extern crate log;

use anyhow::{Context, Result};
use azalea::{prelude::*, protocol::packets::game::ClientboundGamePacket, registry::EntityKind};
use clap::Parser;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

/// A simple stasis bot, using azalea!
#[derive(Parser)]
#[clap(author, version)]
struct Opts {
    // What server ((and port) to connect to
    server_address: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
struct BlockPos {
    x: i32,
    y: i32,
    z: i32,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();

    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }

    let account = Account::offline("unnamed_bot");
    //let account = Account::microsoft("example@example.com").await.unwrap();
    /*let auth_result = azalea::auth::auth(
        "default",
        azalea::auth::AuthOpts {
            cache_file: Some(PathBuf::from("login-secrets.json")),
            ..Default::default()
        },
    )
    .await?;
    let account = azalea::Account {
        username: auth_result.profile.name,
        access_token: Some(Arc::new(Mutex::new(auth_result.access_token))),
        uuid: Some(auth_result.profile.id),
        account_opts: azalea::AccountOpts::Microsoft {
            email: "default".to_owned(),
        },
        // we don't do chat signing by default unless the user asks for it
        certs: None,
    };*/

    ClientBuilder::new()
        .set_handler(handle)
        .start(account, opts.server_address.as_str())
        .await
        .context("Running bot")?;
}

#[derive(Default, Clone, Component)]
pub struct State {}

async fn handle(bot: Client, event: Event, state: State) -> anyhow::Result<()> {
    match event {
        Event::Chat(m) => {
            info!("[CHAT] {}", m.message().to_ansi());
        }
        Event::Packet(packet) => match packet.as_ref() {
            ClientboundGamePacket::AddEntity(packet) => {
                if packet.entity_type == EntityKind::EnderPearl {
                    info!("Enderpearl spawned at {}", packet.position);
                }
            }
            _ => {}
        },
        _ => {}
    }

    Ok(())
}
