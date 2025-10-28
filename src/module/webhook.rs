use crate::BotState;
use crate::module::Module;
use azalea::auth::game_profile::GameProfile;
use azalea::{Client, Event};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::MissedTickBehavior;

#[derive(Clone)]
pub struct WebhookModule {
    pub url: Arc<String>,
    pub alert_role_id: Option<u64>,
    pub profile: Arc<Mutex<Option<GameProfile>>>,
    pub reqwest_client: Arc<reqwest::Client>,
    pub queue: Arc<Mutex<VecDeque<serde_json::Value>>>,
    pub queue_open: Arc<AtomicBool>,
    pub sending_message: Arc<AtomicBool>,
}

impl WebhookModule {
    pub fn new(url: impl AsRef<str>, alert_role_id: Option<u64>) -> Self {
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let queue_open = Arc::new(AtomicBool::new(true));
        let reqwest_client = Arc::new(reqwest::Client::new());
        let sending_message = Arc::new(AtomicBool::new(false));
        let instance = Self {
            url: Arc::new(url.as_ref().to_string()),
            alert_role_id,
            profile: Arc::new(Mutex::new(None)),
            reqwest_client: reqwest_client.clone(),
            queue: queue.clone(),
            queue_open: queue_open.clone(),
            sending_message: sending_message.clone(),
        };
        let url = url.as_ref().to_string();
        tokio::spawn(async {
            info!("Started webhook sender task.");
            Self::webhook_sender(url, reqwest_client, queue, queue_open, sending_message).await;
            warn!("Webhook sender task finished.");
        });
        instance
    }

    async fn webhook_sender(
        url: impl AsRef<str>,
        reqwest_client: Arc<reqwest::Client>,
        queue: Arc<Mutex<VecDeque<serde_json::Value>>>,
        queue_open: Arc<AtomicBool>,
        sending_message: Arc<AtomicBool>,
    ) {
        info!("Started webhook_sender!");

        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;

            let maybe_json_value = queue.lock().pop_front();
            if let Some(json_value) = maybe_json_value {
                sending_message.store(true, Ordering::Release);
                if let Err(err) = reqwest_client.post(url.as_ref()).json(&json_value).send().await {
                    error!("Failed to send webhook (alert): {err:?}");
                }
                sending_message.store(false, Ordering::Release);
            } else if !queue_open.load(Ordering::Relaxed) {
                info!("Queue is no longer open and queue empty!");
                break;
            }
        }

        //error!("webhook_sender finished unexpectedly!");
    }

    pub fn webhook_alert(&self, message: impl AsRef<str>) {
        self.queue_webhook(message, false, true);
    }

    pub fn webhook_silent(&self, message: impl AsRef<str>) {
        self.queue_webhook(message, true, false);
    }

    pub fn webhook(&self, message: impl AsRef<str>) {
        self.queue_webhook(message, false, false);
    }

    pub fn queue_webhook(&self, message: impl AsRef<str>, silent: bool, ping_group: bool) {
        let flags = if silent {
            1 << 12 /* SUPPRESS_NOTIFICATIONS */
        } else {
            0
        };

        let profile = self.profile.lock();
        let ping_prefix = if ping_group && let Some(role_id) = self.alert_role_id {
            format!("<@&{role_id}>\n")
        } else {
            String::new()
        };
        let json_value = serde_json::json!({
            "username": if let Some(profile) = profile.as_ref() { profile.name.to_owned() } else { "NoUsername".to_owned() },
            "avatar_url": if let Some(profile) = profile.as_ref() { format!("https://mc-heads.net/head/{}", profile.uuid) } else { "https://mc-heads.net/head/Steve".to_owned() },
            "content": format!("{ping_prefix}{}", message.as_ref()),
            "allowed_mentions": { "parse": if !ping_prefix.is_empty() { vec![ "roles" ] } else { vec![] } },
            "flags": flags,
        });
        if self.queue_open.load(Ordering::Relaxed) {
            info!("Queued: {}", message.as_ref());
            self.queue.lock().push_back(json_value);
        } else {
            warn!("Discarded webhook message {:?} because queue not open!", message.as_ref());
        }
    }
}

#[async_trait::async_trait]
impl Module for WebhookModule {
    fn name(&self) -> &'static str {
        "Webhook"
    }

    async fn handle(&self, bot: Client, event: &Event, _bot_state: &BotState) -> anyhow::Result<()> {
        match event {
            Event::Init => *self.profile.lock() = Some(bot.profile),
            _ => {}
        }
        Ok(())
    }
}
