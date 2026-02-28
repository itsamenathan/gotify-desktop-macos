use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Mutex};
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
}

impl AppState {
    pub(crate) fn new(messages: Vec<CachedMessage>) -> Self {
        Self {
            runtime: Mutex::new(RuntimeState::default()),
            messages: Mutex::new(messages),
            app_meta: Mutex::new(HashMap::new()),
            tray_pause_menu: Mutex::new(None),
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
