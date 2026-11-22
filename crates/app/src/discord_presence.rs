//! Discord Rich Presence — shows **KapturKard** as the current activity.
//!
//! Needs a Discord Application ID (Developer Portal). The activity `.name("KapturKard")`
//! overrides the portal app name. Missing ID / Discord closed → silent no-op.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use discord_rich_presence::activity::{Activity, ActivityType, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

const ACTIVITY_NAME: &str = "KapturKard";
const REFRESH: Duration = Duration::from_secs(15);

enum Cmd {
    Update { details: String, state: String },
    Shutdown,
}

/// Background Discord IPC worker. Drop / [`shutdown`][Self::shutdown] to clear presence.
pub struct DiscordPresence {
    tx: Option<Sender<Cmd>>,
}

impl DiscordPresence {
    /// Start the worker when `enabled` and `app_id` is non-empty.
    pub fn start(enabled: bool, app_id: &str) -> Self {
        let app_id = app_id.trim().to_string();
        if !enabled || app_id.is_empty() {
            if enabled && app_id.is_empty() {
                tracing::info!(
                    "discord presence idle: set Application ID in Infos (or env KAPTURKARD_DISCORD_APP_ID)"
                );
            }
            return Self { tx: None };
        }

        let (tx, rx) = mpsc::channel();
        let id = app_id.clone();
        match thread::Builder::new()
            .name("discord-rpc".into())
            .spawn(move || worker(id, rx))
        {
            Ok(_) => {
                tracing::info!(app_id = %app_id, "discord presence worker started");
                Self { tx: Some(tx) }
            }
            Err(e) => {
                tracing::warn!(error = %e, "discord presence thread failed to start");
                Self { tx: None }
            }
        }
    }

    pub fn is_active(&self) -> bool {
        self.tx.is_some()
    }

    pub fn update(&self, details: impl Into<String>, state: impl Into<String>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Cmd::Update {
                details: details.into(),
                state: state.into(),
            });
        }
    }

    pub fn shutdown(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Cmd::Shutdown);
        }
    }
}

impl Drop for DiscordPresence {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Resolve Application ID: non-empty prefs → else `KAPTURKARD_DISCORD_APP_ID` env.
pub fn resolve_app_id(prefs_id: &str) -> String {
    let trimmed = prefs_id.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    std::env::var("KAPTURKARD_DISCORD_APP_ID")
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn worker(app_id: String, rx: Receiver<Cmd>) {
    let mut client = DiscordIpcClient::new(&app_id);
    let mut connected = false;
    let mut details = String::from("Capture HDMI");
    let mut state = String::from("En ligne");
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    loop {
        if !connected {
            match client.connect() {
                Ok(()) => {
                    connected = true;
                    tracing::info!("discord IPC connected");
                    let _ = set_activity(&mut client, &details, &state, started);
                }
                Err(e) => {
                    tracing::debug!(error = %e, "discord IPC connect failed (is Discord running?)");
                }
            }
        }

        match rx.recv_timeout(REFRESH) {
            Ok(Cmd::Shutdown) => {
                if connected {
                    let _ = client.clear_activity();
                    let _ = client.close();
                }
                break;
            }
            Ok(Cmd::Update {
                details: d,
                state: s,
            }) => {
                details = d;
                state = s;
                if connected {
                    if set_activity(&mut client, &details, &state, started).is_err() {
                        connected = false;
                        let _ = client.close();
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if connected {
                    if set_activity(&mut client, &details, &state, started).is_err() {
                        connected = false;
                        let _ = client.close();
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn set_activity(
    client: &mut DiscordIpcClient,
    details: &str,
    state: &str,
    started: i64,
) -> Result<(), discord_rich_presence::error::Error> {
    let activity = Activity::new()
        .name(ACTIVITY_NAME)
        .activity_type(ActivityType::Playing)
        .details(details)
        .state(state)
        .timestamps(Timestamps::new().start(started));
    client.set_activity(activity)
}
