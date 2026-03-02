use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Mutex};
use tauri::ipc::Channel;
use tauri::menu::MenuItem;
use tokio::sync::watch;

#[derive(Clone)]
pub(crate) struct TrayPauseMenuState {
    pub(crate) status_item: MenuItem<tauri::Wry>,
    pub(crate) pause_15m_item: MenuItem<tauri::Wry>,
    pub(crate) pause_1h_item: MenuItem<tauri::Wry>,
    pub(crate) pause_forever_item: MenuItem<tauri::Wry>,
    pub(crate) resume_item: MenuItem<tauri::Wry>,
}

pub(crate) struct AppState {
    pub(crate) runtime: Mutex<RuntimeState>,
    pub(crate) messages: Mutex<Vec<CachedMessage>>,
    pub(crate) app_meta: Mutex<HashMap<i64, ApplicationMeta>>,
    pub(crate) tray_pause_menu: Mutex<Option<TrayPauseMenuState>>,
    pub(crate) revisions: Mutex<RevisionState>,
    pub(crate) update_channels: Mutex<HashMap<String, Channel<Value>>>,
    pub(crate) settings_lock: Mutex<()>,
    pub(crate) message_persist_lock: Mutex<()>,
}

impl AppState {
    pub(crate) fn new(messages: Vec<CachedMessage>) -> Self {
        Self {
            runtime: Mutex::new(RuntimeState::default()),
            messages: Mutex::new(messages),
            app_meta: Mutex::new(HashMap::new()),
            tray_pause_menu: Mutex::new(None),
            revisions: Mutex::new(RevisionState::default()),
            update_channels: Mutex::new(HashMap::new()),
            settings_lock: Mutex::new(()),
            message_persist_lock: Mutex::new(()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum RevisionKey {
    Settings,
    Pause,
    Messages,
    Connection,
    Runtime,
    StreamError,
}

#[derive(Debug, Clone)]
pub(crate) struct RevisionState {
    pub(crate) settings: u64,
    pub(crate) pause: u64,
    pub(crate) messages: u64,
    pub(crate) connection: u64,
    pub(crate) runtime: u64,
    pub(crate) stream_error: u64,
}

impl RevisionState {
    pub(crate) fn current(&self, key: RevisionKey) -> u64 {
        match key {
            RevisionKey::Settings => self.settings,
            RevisionKey::Pause => self.pause,
            RevisionKey::Messages => self.messages,
            RevisionKey::Connection => self.connection,
            RevisionKey::Runtime => self.runtime,
            RevisionKey::StreamError => self.stream_error,
        }
    }

    pub(crate) fn bump(&mut self, key: RevisionKey) -> u64 {
        let slot = match key {
            RevisionKey::Settings => &mut self.settings,
            RevisionKey::Pause => &mut self.pause,
            RevisionKey::Messages => &mut self.messages,
            RevisionKey::Connection => &mut self.connection,
            RevisionKey::Runtime => &mut self.runtime,
            RevisionKey::StreamError => &mut self.stream_error,
        };
        *slot = slot.saturating_add(1);
        *slot
    }
}

impl Default for RevisionState {
    fn default() -> Self {
        Self {
            settings: 1,
            pause: 1,
            messages: 1,
            connection: 1,
            runtime: 1,
            stream_error: 1,
        }
    }
}

pub(crate) struct RuntimeState {
    pub(crate) stop_tx: Option<watch::Sender<bool>>,
    pub(crate) stream_epoch: u64,
    pub(crate) connection_state: String,
    pub(crate) should_run: bool,
    pub(crate) last_connected_at: Option<u64>,
    pub(crate) last_stream_event_at: Option<u64>,
    pub(crate) last_message_at: Option<u64>,
    pub(crate) last_message_id: Option<i64>,
    pub(crate) last_error: Option<String>,
    pub(crate) backoff_seconds: u64,
    pub(crate) reconnect_attempts: u64,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            stop_tx: None,
            stream_epoch: 0,
            connection_state: "Disconnected".to_string(),
            should_run: false,
            last_connected_at: None,
            last_stream_event_at: None,
            last_message_at: None,
            last_message_id: None,
            last_error: None,
            backoff_seconds: 0,
            reconnect_attempts: 0,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct UrlPreview {
    pub(crate) url: String,
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) site_name: Option<String>,
    pub(crate) image: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct CachedMessage {
    pub(crate) id: i64,
    #[serde(default)]
    pub(crate) app_id: i64,
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) priority: i64,
    #[serde(default)]
    pub(crate) app: String,
    #[serde(default)]
    pub(crate) app_icon: Option<String>,
    pub(crate) date: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GotifyMessageWire {
    pub(crate) id: i64,
    pub(crate) appid: i64,
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) priority: i64,
    #[serde(default)]
    pub(crate) date: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GotifyMessageListWire {
    #[serde(default)]
    pub(crate) messages: Vec<GotifyMessageWire>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GotifyApplicationWire {
    pub(crate) id: i64,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) image: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ApplicationMeta {
    pub(crate) name: String,
    pub(crate) icon_url: String,
}
