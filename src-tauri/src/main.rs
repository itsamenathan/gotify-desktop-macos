#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::Engine as _;
use chrono::Timelike;
use futures_util::{SinkExt, StreamExt};
#[cfg(target_os = "macos")]
use mac_notification_sys::{MainButton, Notification, NotificationResponse};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::Write as _,
    net::{IpAddr, Ipv6Addr, ToSocketAddrs},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
        OnceLock,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WindowEvent};
use tokio::sync::watch;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
};

#[cfg(target_os = "macos")]
const LAUNCH_AGENT_LABEL: &str = "net.gotify.desktop";
const DEFAULT_CACHE_LIMIT: usize = 100;
const MAX_API_PAGE_LIMIT: usize = 200;
const MAX_CACHE_LIMIT: usize = 2000;
/// Resolved at startup; must be set before any `load_settings` / `save_settings` call.
static SETTINGS_FILE: OnceLock<PathBuf> = OnceLock::new();
/// Monotonic counter for generating unique temp/backup file suffixes.
static FILE_SUFFIX_COUNTER: AtomicU64 = AtomicU64::new(0);

const STREAM_CONNECT_TIMEOUT_SECS: u64 = 10;
const STREAM_SYNC_INTERVAL_SECS: u64 = 5;
const STREAM_LIVENESS_CHECK_INTERVAL_SECS: u64 = 15;
const STREAM_LIVENESS_IDLE_SECS: u64 = 90;
const STREAM_LIVENESS_PING_GRACE_SECS: u64 = 30;
const PREVIEW_REQUEST_TIMEOUT_SECS: u64 = 6;
const PREVIEW_MAX_REDIRECTS: usize = 5;
const PREVIEW_MAX_HTML_BYTES: usize = 120_000;
const APP_ICON_MAX_BYTES: usize = 256_000;
const PAUSE_FOREVER_SENTINEL: u64 = 0;
const PAUSE_MODE_15M: &str = "15m";
const PAUSE_MODE_1H: &str = "1h";
const PAUSE_MODE_CUSTOM: &str = "custom";
const PAUSE_MODE_FOREVER: &str = "forever";

#[derive(Clone)]
struct TrayPauseMenuState {
    status_item: MenuItem<tauri::Wry>,
    pause_15m_item: MenuItem<tauri::Wry>,
    pause_1h_item: MenuItem<tauri::Wry>,
    pause_forever_item: MenuItem<tauri::Wry>,
    resume_item: MenuItem<tauri::Wry>,
}

struct AppState {
    runtime: Mutex<RuntimeState>,
    messages: Mutex<Vec<CachedMessage>>,
    app_meta: Mutex<HashMap<i64, ApplicationMeta>>,
    tray_pause_menu: Mutex<Option<TrayPauseMenuState>>,
}

impl AppState {
    fn new(messages: Vec<CachedMessage>) -> Self {
        Self {
            runtime: Mutex::new(RuntimeState::default()),
            messages: Mutex::new(messages),
            app_meta: Mutex::new(HashMap::new()),
            tray_pause_menu: Mutex::new(None),
        }
    }
}

struct RuntimeState {
    stop_tx: Option<watch::Sender<bool>>,
    /// Incremented every time a new stream task is spawned. The task captures
    /// its own epoch at spawn time and only writes cleanup state if the epoch
    /// still matches, preventing a late-exiting old task from clobbering a
    /// freshly started replacement task's state.
    stream_epoch: u64,
    connection_state: String,
    should_run: bool,
    last_connected_at: Option<u64>,
    last_stream_event_at: Option<u64>,
    last_message_at: Option<u64>,
    last_message_id: Option<i64>,
    last_error: Option<String>,
    backoff_seconds: u64,
    reconnect_attempts: u64,
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

#[derive(Debug, Serialize, Clone)]
struct RuntimeDiagnostics {
    connection_state: String,
    should_run: bool,
    last_connected_at: Option<u64>,
    last_stream_event_at: Option<u64>,
    last_message_at: Option<u64>,
    last_message_id: Option<i64>,
    stale_for_seconds: Option<u64>,
    last_error: Option<String>,
    backoff_seconds: u64,
    reconnect_attempts: u64,
}

#[derive(Debug, Serialize, Clone)]
struct DeleteMessageDebugEvent {
    at: u64,
    message_id: i64,
    phase: String,
    detail: String,
    status: Option<u16>,
}

#[derive(Debug, Serialize)]
struct UrlPreview {
    url: String,
    title: Option<String>,
    description: Option<String>,
    site_name: Option<String>,
    image: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(default)]
struct StoredSettings {
    base_url: String,
    token: Option<String>,
    min_priority: i64,
    cache_limit: usize,
    launch_at_login: bool,
    start_minimized_to_tray: bool,
    pause_until: Option<u64>,
    pause_mode: Option<String>,
    quiet_hours_start: Option<u8>,
    quiet_hours_end: Option<u8>,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            token: None,
            min_priority: 0,
            cache_limit: DEFAULT_CACHE_LIMIT,
            launch_at_login: true,
            start_minimized_to_tray: true,
            pause_until: None,
            pause_mode: None,
            quiet_hours_start: None,
            quiet_hours_end: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct SettingsResponse {
    base_url: String,
    has_token: bool,
    min_priority: i64,
    cache_limit: usize,
    launch_at_login: bool,
    start_minimized_to_tray: bool,
    pause_until: Option<u64>,
    pause_mode: Option<String>,
    quiet_hours_start: Option<u8>,
    quiet_hours_end: Option<u8>,
}

#[derive(Debug, Serialize)]
struct PauseStateResponse {
    pause_until: Option<u64>,
    pause_mode: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct CachedMessage {
    id: i64,
    #[serde(default)]
    app_id: i64,
    title: String,
    message: String,
    priority: i64,
    #[serde(default)]
    app: String,
    #[serde(default)]
    app_icon: Option<String>,
    date: String,
}

#[derive(Debug, Deserialize)]
struct GotifyMessageWire {
    id: i64,
    appid: i64,
    message: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    priority: i64,
    #[serde(default)]
    date: String,
}

#[derive(Debug, Deserialize)]
struct GotifyMessageListWire {
    #[serde(default)]
    messages: Vec<GotifyMessageWire>,
}

#[derive(Debug, Deserialize)]
struct GotifyApplicationWire {
    id: i64,
    name: String,
    #[serde(default)]
    image: String,
}

#[derive(Debug, Clone)]
struct ApplicationMeta {
    name: String,
    icon_url: String,
}

#[tauri::command]
fn load_settings(app: AppHandle) -> Result<SettingsResponse, String> {
    let stored = read_settings(&app)?;
    let has_token = stored.token.as_deref().map_or(false, |t| !t.trim().is_empty());

    Ok(SettingsResponse {
        base_url: stored.base_url,
        has_token,
        min_priority: stored.min_priority,
        cache_limit: normalize_cache_limit(stored.cache_limit),
        launch_at_login: stored.launch_at_login,
        start_minimized_to_tray: stored.start_minimized_to_tray,
        pause_until: stored.pause_until,
        pause_mode: stored.pause_mode,
        quiet_hours_start: stored.quiet_hours_start,
        quiet_hours_end: stored.quiet_hours_end,
    })
}

#[tauri::command]
fn save_settings(
    app: AppHandle,
    base_url: String,
    token: String,
    min_priority: Option<i64>,
    cache_limit: Option<usize>,
    launch_at_login: Option<bool>,
    start_minimized_to_tray: Option<bool>,
    quiet_hours_start: Option<u8>,
    quiet_hours_end: Option<u8>,
) -> Result<(), String> {
    debug_log(&format!(
        "save_settings called: base_url={base_url:?} token_len={} min_priority={min_priority:?} cache_limit={cache_limit:?}",
        token.trim().len()
    ));
    let normalized_url = normalize_base_url(&base_url)?;
    let current = read_settings(&app).unwrap_or_default();
    let quiet_start = quiet_hours_start.or(current.quiet_hours_start);
    let quiet_end = quiet_hours_end.or(current.quiet_hours_end);

    // Resolve the token to persist: use the new value if provided, otherwise keep existing.
    let new_token = if token.trim().is_empty() {
        debug_log("save_settings: no new token provided, keeping existing");
        match current.token.as_deref() {
            Some(t) if !t.trim().is_empty() => {
                debug_log("save_settings: existing token retained");
                current.token.clone()
            }
            _ => {
                debug_log("save_settings: no existing token and none provided — error");
                return Err("Token is required".to_string());
            }
        }
    } else {
        debug_log(&format!("save_settings: saving new token (len={})", token.trim().len()));
        Some(token.trim().to_string())
    };

    save_non_secret_settings(
        &app,
        &StoredSettings {
            base_url: normalized_url.clone(),
            token: new_token,
            min_priority: min_priority.unwrap_or(current.min_priority).clamp(0, 10),
            cache_limit: normalize_cache_limit(cache_limit.unwrap_or(current.cache_limit)),
            launch_at_login: launch_at_login.unwrap_or(current.launch_at_login),
            start_minimized_to_tray: start_minimized_to_tray
                .unwrap_or(current.start_minimized_to_tray),
            pause_until: current.pause_until,
            pause_mode: current.pause_mode,
            quiet_hours_start: quiet_start.map(|h| h % 24),
            quiet_hours_end: quiet_end.map(|h| h % 24),
        },
    )?;
    debug_log("save_settings: settings (including token) written to disk");

    #[cfg(target_os = "macos")]
    if let Err(error) = apply_launch_at_login(launch_at_login.unwrap_or(current.launch_at_login)) {
        debug_log(&format!("failed to apply launch-at-login change: {error}"));
    }

    debug_log("save_settings: complete");
    Ok(())
}

#[tauri::command]
async fn test_connection(base_url: String, token: Option<String>) -> Result<String, String> {
    debug_log(&format!(
        "test_connection: base_url={base_url:?} token_provided={}",
        token.as_deref().map_or(false, |t| !t.trim().is_empty())
    ));
    let normalized_url = normalize_base_url(&base_url)?;

    let token_to_use = match token {
        Some(value) if !value.trim().is_empty() => {
            debug_log("test_connection: using caller-supplied token");
            value.trim().to_string()
        }
        _ => {
            debug_log("test_connection: no token supplied, loading from keychain");
            let t = load_token()?
                .ok_or_else(|| "No token found. Save one in settings first.".to_string())?;
            debug_log(&format!("test_connection: loaded token from keychain len={}", t.len()));
            t
        }
    };

    let endpoint = format!("{normalized_url}/application");
    debug_log(&format!("test_connection: GET {endpoint}"));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Failed to build HTTP client: {error}"))?;
    let response = client
        .get(&endpoint)
        .header("X-Gotify-Key", &token_to_use)
        .send()
        .await
        .map_err(|error| format!("Connection request failed: {error}"))?;

    let status = response.status().as_u16();
    debug_log(&format!("test_connection: HTTP {status}"));
    if response.status().is_success() {
        return Ok("Connection successful".to_string());
    }

    let body = response
        .text()
        .await
        .unwrap_or_else(|_| "<unable to read response body>".to_string());

    Err(format!(
        "Gotify request failed (HTTP {status}): {}",
        truncate_message(&body, 200)
    ))
}

#[tauri::command]
fn load_messages(app: AppHandle) -> Result<Vec<CachedMessage>, String> {
    let state = app.state::<AppState>();
    let messages = state
        .messages
        .lock()
        .map_err(|_| "Message cache lock poisoned".to_string())?
        .clone();
    Ok(messages)
}

#[tauri::command]
fn get_pause_state(app: AppHandle) -> Result<PauseStateResponse, String> {
    let settings = read_settings(&app)?;
    Ok(PauseStateResponse {
        pause_until: settings.pause_until,
        pause_mode: settings.pause_mode,
    })
}

#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    let candidate = url.trim();
    if candidate.is_empty() {
        return Err("Missing URL".to_string());
    }
    let parsed = reqwest::Url::parse(candidate).map_err(|error| format!("Invalid URL: {error}"))?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    if scheme != "http" && scheme != "https" && scheme != "mailto" {
        return Err(format!("Unsupported URL scheme: {scheme}"));
    }

    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(candidate).status();
    #[cfg(target_os = "linux")]
    let status = Command::new("xdg-open").arg(candidate).status();
    #[cfg(target_os = "windows")]
    let status = Command::new("cmd")
        .arg("/C")
        .arg("start")
        .arg("")
        .arg(candidate)
        .status();

    let status = status.map_err(|error| format!("Failed to open URL: {error}"))?;
    if !status.success() {
        return Err(format!(
            "Failed to open URL (exit code {})",
            status.code().unwrap_or(-1)
        ));
    }

    Ok(())
}

#[tauri::command]
#[allow(non_snake_case)]
async fn delete_message(
    app: AppHandle,
    messageId: Option<i64>,
    message_id: Option<i64>,
) -> Result<(), String> {
    let message_id = message_id
        .or(messageId)
        .ok_or_else(|| "Missing message id".to_string())?;
    if message_id <= 0 {
        return Err("Invalid message id".to_string());
    }
    debug_log(&format!("delete_message requested id={message_id}"));
    emit_delete_debug(&app, message_id, "start", "delete requested", None);

    let settings = read_settings(&app)?;
    let base_url = normalize_base_url(&settings.base_url)?;
    let token =
        load_token()?.ok_or_else(|| "No token found. Save token in settings first.".to_string())?;

    let endpoint = format!("{base_url}/message/{message_id}");
    let url =
        reqwest::Url::parse(&endpoint).map_err(|error| format!("Invalid delete URL: {error}"))?;
    emit_delete_debug(
        &app,
        message_id,
        "request",
        &format!("DELETE {} auth=X-Gotify-Key", url),
        None,
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Failed to build HTTP client: {error}"))?;
    let response = client
        .delete(url)
        .header("X-Gotify-Key", &token)
        .send()
        .await
        .map_err(|error| {
            emit_delete_debug(
                &app,
                message_id,
                "network-error",
                &format!("request failed: {error}"),
                None,
            );
            format!("Failed to delete message {message_id}: {error}")
        })?;

    let status = response.status().as_u16();
    debug_log(&format!(
        "delete_message status id={message_id} http={status}"
    ));
    if !(200..300).contains(&status) && status != 404 {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read response body>".to_string());
        emit_delete_debug(
            &app,
            message_id,
            "http-error",
            &format!("HTTP {status}: {}", truncate_message(&body, 500)),
            Some(status),
        );
        return Err(format!(
            "Delete failed (HTTP {status}): {}",
            truncate_message(&body, 200)
        ));
    }
    emit_delete_debug(
        &app,
        message_id,
        "http-ok",
        &format!("HTTP {status}"),
        Some(status),
    );

    remove_message_from_cache(&app, message_id)?;
    emit_delete_debug(
        &app,
        message_id,
        "cache",
        "removed from local cache",
        Some(status),
    );

    let app_for_sync = app.clone();
    let base_for_sync = base_url.clone();
    tauri::async_runtime::spawn(async move {
        if let Ok(token) = load_token() {
            if let Some(token_value) = token {
                if let Err(error) =
                    fetch_recent_messages(&app_for_sync, &base_for_sync, &token_value).await
                {
                    emit_delete_debug(
                        &app_for_sync,
                        message_id,
                        "post-sync-error",
                        &format!("refresh failed: {error}"),
                        None,
                    );
                } else {
                    emit_delete_debug(
                        &app_for_sync,
                        message_id,
                        "post-sync-ok",
                        "refresh completed",
                        None,
                    );
                }
            }
        }
    });
    Ok(())
}

#[tauri::command]
async fn start_stream(app: AppHandle, token: Option<String>) -> Result<(), String> {
    start_stream_internal(app, token)
}

#[tauri::command]
fn stop_stream(app: AppHandle) -> Result<(), String> {
    stop_stream_internal(&app)
}

#[tauri::command]
fn get_connection_state(app: AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();
    let runtime = state
        .runtime
        .lock()
        .map_err(|_| "Runtime lock poisoned".to_string())?;
    if runtime.connection_state.is_empty() {
        return Ok("Disconnected".to_string());
    }
    Ok(runtime.connection_state.clone())
}

#[tauri::command]
fn get_runtime_diagnostics(app: AppHandle) -> Result<RuntimeDiagnostics, String> {
    snapshot_runtime(&app)
}

#[tauri::command]
fn recover_stream(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (should_run, connection_state) = state
        .runtime
        .lock()
        .map(|runtime| (runtime.should_run, runtime.connection_state.clone()))
        .unwrap_or((false, "Disconnected".to_string()));

    if !should_run {
        return Ok(());
    }
    if connection_state == "Connected" || connection_state == "Connecting" {
        return Ok(());
    }

    let _ = stop_stream_internal(&app);
    start_stream_internal(app, None)
}

#[tauri::command]
fn restart_stream(app: AppHandle) -> Result<(), String> {
    let _ = stop_stream_internal(&app);
    start_stream_internal(app, None)
}

#[tauri::command]
fn pause_notifications(app: AppHandle, minutes: u64) -> Result<(), String> {
    if minutes == 0 {
        return Err("Pause duration must be greater than 0 minutes".to_string());
    }
    let until = unix_now_secs().saturating_add(minutes.saturating_mul(60));
    let mode = match minutes {
        15 => PAUSE_MODE_15M,
        60 => PAUSE_MODE_1H,
        _ => PAUSE_MODE_CUSTOM,
    };
    set_notification_pause_until(&app, Some(until), Some(mode))
}

#[tauri::command]
fn pause_notifications_forever(app: AppHandle) -> Result<(), String> {
    // Sentinel value: 0 means pause indefinitely.
    set_notification_pause_until(&app, Some(PAUSE_FOREVER_SENTINEL), Some(PAUSE_MODE_FOREVER))
}

#[tauri::command]
fn resume_notifications(app: AppHandle) -> Result<(), String> {
    set_notification_pause_until(&app, None, None)
}

fn is_pause_active(pause_until: Option<u64>) -> bool {
    match pause_until {
        Some(PAUSE_FOREVER_SENTINEL) => true,
        Some(until) => unix_now_secs() < until,
        None => false,
    }
}

fn format_pause_remaining(total_seconds: u64) -> String {
    let seconds = total_seconds.max(1);
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    let rem_minutes = minutes % 60;
    if rem_minutes == 0 {
        format!("{hours}h")
    } else {
        format!("{hours}h {rem_minutes}m")
    }
}

fn apply_pause_state_to_tray(app: &AppHandle, pause_until: Option<u64>, pause_mode: Option<&str>) {
    let state = app.state::<AppState>();
    let handles = state
        .tray_pause_menu
        .lock()
        .ok()
        .and_then(|guard| guard.clone());

    let Some(handles) = handles else {
        return;
    };

    let now = unix_now_secs();
    let status_label = match pause_until {
        Some(PAUSE_FOREVER_SENTINEL) => "Notifications: Paused Forever".to_string(),
        Some(until) if until > now => {
            let remaining = until.saturating_sub(now);
            format!(
                "Notifications: Paused {} left",
                format_pause_remaining(remaining)
            )
        }
        _ => "Notifications: On".to_string(),
    };
    let pause_active = is_pause_active(pause_until);
    let pause_15m_active = pause_active && pause_mode == Some(PAUSE_MODE_15M);
    let pause_1h_active = pause_active && pause_mode == Some(PAUSE_MODE_1H);
    let pause_forever_active = pause_active && pause_mode == Some(PAUSE_MODE_FOREVER);

    let _ = handles.status_item.set_text(&status_label);
    let _ = handles.resume_item.set_enabled(pause_active);
    let _ = handles.pause_15m_item.set_text(if pause_15m_active {
        "Pause 15m ✓"
    } else {
        "Pause 15m"
    });
    let _ = handles.pause_1h_item.set_text(if pause_1h_active {
        "Pause 1h ✓"
    } else {
        "Pause 1h"
    });
    let _ = handles
        .pause_forever_item
        .set_text(if pause_forever_active {
            "Pause Forever ✓"
        } else {
            "Pause Forever"
        });
    let _ = handles.pause_15m_item.set_enabled(true);
    let _ = handles.pause_1h_item.set_enabled(true);
}

fn set_notification_pause_until(
    app: &AppHandle,
    pause_until: Option<u64>,
    pause_mode: Option<&str>,
) -> Result<(), String> {
    let mut settings = read_settings(&app)?;
    settings.pause_until = pause_until;
    settings.pause_mode = pause_mode.map(|mode| mode.to_string());
    save_non_secret_settings(&app, &settings)?;
    apply_pause_state_to_tray(app, pause_until, settings.pause_mode.as_deref());
    emit_pause_state_events(app, pause_until, settings.pause_mode.as_deref());

    Ok(())
}

fn refresh_pause_state_from_settings(app: &AppHandle) {
    let settings = match read_settings(app) {
        Ok(settings) => settings,
        Err(_) => return,
    };

    if let Some(until) = settings.pause_until {
        if until != PAUSE_FOREVER_SENTINEL && unix_now_secs() >= until {
            let _ = set_notification_pause_until(app, None, None);
            return;
        }
    }

    apply_pause_state_to_tray(app, settings.pause_until, settings.pause_mode.as_deref());
}

fn emit_pause_state_events(app: &AppHandle, pause_until: Option<u64>, pause_mode: Option<&str>) {
    let pause_mode_payload = pause_mode.map(|mode| mode.to_string());

    if let Err(error) = app.emit("notifications-pause-state", pause_until) {
        debug_log(&format!(
            "failed to emit notifications-pause-state: {error}"
        ));
    }
    if let Err(error) = app.emit("notifications-pause-mode", pause_mode_payload.clone()) {
        debug_log(&format!("failed to emit notifications-pause-mode: {error}"));
    }
    match pause_until {
        Some(until) => {
            if let Err(error) = app.emit("notifications-paused-until", until) {
                debug_log(&format!(
                    "failed to emit notifications-paused-until: {error}"
                ));
            }
        }
        None => {
            if let Err(error) = app.emit("notifications-resumed", true) {
                debug_log(&format!("failed to emit notifications-resumed: {error}"));
            }
        }
    }

    for label in ["main", "quick"] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.emit("notifications-pause-state", pause_until);
            let _ = window.emit("notifications-pause-mode", pause_mode_payload.clone());
            match pause_until {
                Some(until) => {
                    let _ = window.emit("notifications-paused-until", until);
                }
                None => {
                    let _ = window.emit("notifications-resumed", true);
                }
            }
        }
    }
}

#[tauri::command]
async fn fetch_url_preview(url: String) -> Result<UrlPreview, String> {
    let mut current_url =
        reqwest::Url::parse(url.trim()).map_err(|error| format!("Invalid URL: {error}"))?;
    ensure_preview_http_scheme(&current_url)?;
    enforce_preview_target_policy(&current_url).await?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(PREVIEW_REQUEST_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("Failed to build preview HTTP client: {error}"))?;

    for redirect_hops in 0..=PREVIEW_MAX_REDIRECTS {
        let response = client
            .get(current_url.clone())
            .send()
            .await
            .map_err(|error| format!("Preview request failed: {error}"))?;

        if response.status().is_redirection() {
            if redirect_hops == PREVIEW_MAX_REDIRECTS {
                return Err(format!(
                    "Preview request redirected too many times (>{PREVIEW_MAX_REDIRECTS})"
                ));
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or_else(|| "Preview redirect missing location header".to_string())?;
            let location_value = location
                .to_str()
                .map_err(|error| format!("Preview redirect location is invalid: {error}"))?;
            current_url = resolve_preview_redirect_url(&current_url, location_value)?;
            enforce_preview_target_policy(&current_url).await?;
            continue;
        }

        if !response.status().is_success() {
            return Err(format!(
                "Preview request failed with HTTP {}",
                response.status().as_u16()
            ));
        }

        if let Some(content_length) = response.content_length() {
            if content_length > PREVIEW_MAX_HTML_BYTES as u64 {
                return Err(format!(
                    "Preview response too large ({content_length} bytes > {PREVIEW_MAX_HTML_BYTES} bytes)"
                ));
            }
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !content_type.contains("text/html") {
            return Ok(UrlPreview {
                url: current_url.to_string(),
                title: None,
                description: None,
                site_name: current_url.host_str().map(ToString::to_string),
                image: None,
            });
        }

        let body = read_limited_preview_body(response, PREVIEW_MAX_HTML_BYTES).await?;
        let title = find_meta(&body, &["og:title"]).or_else(|| find_title(&body));
        let description = find_meta(&body, &["og:description", "description"]);
        let site_name = find_meta(&body, &["og:site_name"])
            .or_else(|| current_url.host_str().map(ToString::to_string));
        let image = find_meta(&body, &["og:image"])
            .and_then(|value| resolve_meta_url(&current_url, &value));

        return Ok(UrlPreview {
            url: current_url.to_string(),
            title,
            description,
            site_name,
            image,
        });
    }

    Err("Preview request failed after redirects".to_string())
}

fn ensure_preview_http_scheme(url: &reqwest::Url) -> Result<(), String> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        other => Err(format!(
            "Only http/https URLs are supported for previews (got '{other}')"
        )),
    }
}

async fn enforce_preview_target_policy(url: &reqwest::Url) -> Result<(), String> {
    ensure_preview_http_scheme(url)?;

    let host = url
        .host_str()
        .ok_or_else(|| "Preview URL is missing a host".to_string())?;
    if is_blocked_preview_hostname(host) {
        return Err(format!("Preview blocked for restricted hostname '{host}'"));
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        if let Some(reason) = preview_block_reason_for_ip(ip) {
            return Err(format!("Preview blocked for {reason} target '{ip}'"));
        }
    } else {
        let port = url
            .port_or_known_default()
            .ok_or_else(|| "Preview URL missing a known port for scheme".to_string())?;
        let ips = resolve_preview_domain_ips(host, port).await?;
        for ip in ips {
            if let Some(reason) = preview_block_reason_for_ip(ip) {
                return Err(format!(
                    "Preview blocked for {reason} target (domain '{host}' resolved to {ip})"
                ));
            }
        }
    }

    Ok(())
}

fn is_blocked_preview_hostname(host: &str) -> bool {
    let normalized = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if normalized.is_empty() {
        return true;
    }
    if normalized == "localhost" || normalized.ends_with(".localhost") {
        return true;
    }
    matches!(
        normalized.as_str(),
        "metadata"
            | "metadata.google.internal"
            | "metadata.azure.internal"
            | "instance-data.ec2.internal"
    )
}

fn preview_block_reason_for_ip(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_unspecified() {
                return Some("unspecified");
            }
            if v4.is_loopback() {
                return Some("loopback");
            }
            if v4.is_link_local() {
                return Some("link-local");
            }
            let octets = v4.octets();
            if matches!(
                octets,
                [169, 254, 169, 254] | [169, 254, 170, 2] | [100, 100, 100, 200]
            ) {
                return Some("metadata endpoint");
            }
            None
        }
        IpAddr::V6(v6) => {
            if v6.is_unspecified() {
                return Some("unspecified");
            }
            if v6.is_loopback() {
                return Some("loopback");
            }
            if v6.is_unicast_link_local() {
                return Some("link-local");
            }
            if v6 == Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254) {
                return Some("metadata endpoint");
            }
            None
        }
    }
}

async fn resolve_preview_domain_ips(domain: &str, port: u16) -> Result<Vec<IpAddr>, String> {
    let domain_for_lookup = domain.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let mut ips = Vec::new();
        let addrs = (domain_for_lookup.as_str(), port)
            .to_socket_addrs()
            .map_err(|error| {
                format!("Failed to resolve preview host '{domain_for_lookup}': {error}")
            })?;
        for addr in addrs {
            let ip = addr.ip();
            if !ips.contains(&ip) {
                ips.push(ip);
            }
        }
        if ips.is_empty() {
            return Err(format!(
                "Failed to resolve preview host '{domain_for_lookup}' to an IP address"
            ));
        }
        Ok(ips)
    })
    .await
    .map_err(|error| format!("Failed to join DNS lookup task: {error}"))?
}

fn resolve_preview_redirect_url(
    current_url: &reqwest::Url,
    location: &str,
) -> Result<reqwest::Url, String> {
    let trimmed = location.trim();
    if trimmed.is_empty() {
        return Err("Preview redirect location is empty".to_string());
    }
    let next = current_url
        .join(trimmed)
        .map_err(|error| format!("Invalid preview redirect location: {error}"))?;
    ensure_preview_http_scheme(&next)?;
    Ok(next)
}

async fn read_limited_preview_body(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<String, String> {
    let mut out = Vec::new();
    loop {
        let next_chunk = response
            .chunk()
            .await
            .map_err(|error| format!("Failed to read preview response body: {error}"))?;
        let Some(chunk) = next_chunk else {
            break;
        };
        if out.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("Preview response exceeded {max_bytes} byte limit"));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&out).to_string())
}

fn start_stream_internal(app: AppHandle, token_override: Option<String>) -> Result<(), String> {
    let settings = read_settings(&app)?;
    let base_url = normalize_base_url(&settings.base_url)?;
    let token = match token_override {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => load_token()?
            .ok_or_else(|| "No token found. Save token in settings first.".to_string())?,
    };
    debug_log(&format!("start_stream requested for {}", base_url));

    {
        let state = app.state::<AppState>();
        let mut runtime = state
            .runtime
            .lock()
            .map_err(|_| "Runtime lock poisoned".to_string())?;

        if runtime.stop_tx.is_some() {
            return Ok(());
        }

        let (tx, rx) = watch::channel(false);
        runtime.stop_tx = Some(tx);
        runtime.stream_epoch = runtime.stream_epoch.wrapping_add(1);
        let task_epoch = runtime.stream_epoch;
        runtime.should_run = true;
        runtime.last_error = None;
        runtime.backoff_seconds = 0;
        runtime.reconnect_attempts = 0;
        drop(runtime);

        emit_connection_state(&app, "Connecting");
        emit_runtime_diagnostics(&app);
        let app_for_task = app.clone();
        debug_log("spawning stream task");
        tauri::async_runtime::spawn(async move {
            // Do not block websocket startup on prefetch calls.
            let app_for_prefetch = app_for_task.clone();
            let base_url_for_prefetch = base_url.clone();
            let token_for_prefetch = token.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = fetch_applications(
                    &app_for_prefetch,
                    &base_url_for_prefetch,
                    &token_for_prefetch,
                )
                .await
                {
                    debug_log(&format!("failed to fetch applications: {error}"));
                }
                if let Err(error) = fetch_recent_messages(
                    &app_for_prefetch,
                    &base_url_for_prefetch,
                    &token_for_prefetch,
                )
                .await
                {
                    debug_log(&format!("failed to fetch recent messages: {error}"));
                }
            });
            run_stream_loop(app_for_task, base_url, token, rx, task_epoch).await;
        });
    }

    Ok(())
}

fn stop_stream_internal(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut runtime = state
        .runtime
        .lock()
        .map_err(|_| "Runtime lock poisoned".to_string())?;

    if let Some(stop_tx) = runtime.stop_tx.take() {
        let _ = stop_tx.send(true);
    }
    runtime.should_run = false;
    runtime.backoff_seconds = 0;
    drop(runtime);

    emit_connection_state(app, "Disconnected");
    Ok(())
}

async fn run_stream_loop(
    app: AppHandle,
    base_url: String,
    token: String,
    mut stop_rx: watch::Receiver<bool>,
    task_epoch: u64,
) {
    let mut backoff_secs: u64 = 1;
    debug_log("stream task started");

    loop {
        if *stop_rx.borrow() {
            break;
        }

        emit_connection_state(&app, "Connecting");
        debug_log("attempting stream connection");
        match stream_once(&app, &base_url, &token, &mut stop_rx).await {
            Ok(()) => {
                if *stop_rx.borrow() {
                    break;
                }
                debug_log("stream session ended without error");
                emit_connection_state(&app, "Disconnected");
                emit_runtime_diagnostics(&app);
            }
            Err(err) => {
                if *stop_rx.borrow() {
                    break;
                }

                debug_log(&format!("stream loop error: {err}"));
                emit_connection_state(&app, "Backoff");
                let _ = app.emit("connection-error", truncate_message(&err, 200));
                if let Some(state) = app.try_state::<AppState>() {
                    if let Ok(mut runtime) = state.runtime.lock() {
                        runtime.last_error = Some(truncate_message(&err, 300));
                        runtime.backoff_seconds = backoff_secs;
                        runtime.reconnect_attempts = runtime.reconnect_attempts.saturating_add(1);
                    }
                }
                emit_runtime_diagnostics(&app);

                let jitter_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| (d.subsec_millis() % 500) as u64)
                    .unwrap_or(0);

                tokio::time::sleep(
                    Duration::from_secs(backoff_secs) + Duration::from_millis(jitter_ms),
                )
                .await;
                backoff_secs = std::cmp::min(backoff_secs.saturating_mul(2), 30);
            }
        }
    }

    let state = app.state::<AppState>();
    if let Ok(mut runtime) = state.runtime.lock() {
        // Only clear state if this is still the active stream task. If
        // restart_stream already spawned a new task, leave its state alone.
        if runtime.stream_epoch == task_epoch {
            runtime.stop_tx = None;
            runtime.should_run = false;
            runtime.backoff_seconds = 0;
        }
    }
    emit_connection_state(&app, "Disconnected");
    emit_runtime_diagnostics(&app);
}

async fn stream_once(
    app: &AppHandle,
    base_url: &str,
    token: &str,
    stop_rx: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let ws_url = build_stream_ws_url(base_url)?;
    debug_log(&format!("ws connect {}", redact_ws_url(&ws_url)));
    let mut ws_request = ws_url
        .as_str()
        .into_client_request()
        .map_err(|error| format!("Failed to build websocket request: {error}"))?;
    let token_header = HeaderValue::from_str(token.trim())
        .map_err(|error| format!("Invalid token for websocket header: {error}"))?;
    ws_request
        .headers_mut()
        .insert("X-Gotify-Key", token_header);
    let (mut ws_stream, _) = tokio::time::timeout(
        Duration::from_secs(STREAM_CONNECT_TIMEOUT_SECS),
        connect_async(ws_request),
    )
    .await
    .map_err(|_| {
        format!(
            "Stream connection timed out after {} seconds",
            STREAM_CONNECT_TIMEOUT_SECS
        )
    })?
    .map_err(|error| format!("Stream connection failed: {error}"))?;

    debug_log("ws connected");
    let now = unix_now_secs();
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut runtime) = state.runtime.lock() {
            runtime.last_connected_at = Some(now);
            runtime.last_stream_event_at = Some(now);
            runtime.last_error = None;
            runtime.backoff_seconds = 0;
        }
    }
    emit_connection_state(app, "Connected");
    emit_runtime_diagnostics(app);
    let mut sync_interval = tokio::time::interval(Duration::from_secs(STREAM_SYNC_INTERVAL_SECS));
    sync_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    sync_interval.tick().await;
    let mut liveness_interval =
        tokio::time::interval(Duration::from_secs(STREAM_LIVENESS_CHECK_INTERVAL_SECS));
    liveness_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    liveness_interval.tick().await;
    let mut last_activity_at = now;
    let mut pending_ping_since: Option<u64> = None;

    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    let _ = ws_stream.close(None).await;
                    return Ok(());
                }
            }
            incoming = ws_stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let event_now = unix_now_secs();
                        last_activity_at = event_now;
                        pending_ping_since = None;
                        mark_stream_activity(app, event_now);
                        debug_log(&format!("ws text frame bytes={}", text.len()));
                        if let Some(msg) = parse_stream_message(app, text.as_ref()) {
                            let _ = cache_and_emit_message(app, msg, true);
                        } else {
                            debug_log(&format!(
                                "ws text parse miss: {}",
                                truncate_message(text.as_ref(), 140)
                            ));
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let event_now = unix_now_secs();
                        last_activity_at = event_now;
                        pending_ping_since = None;
                        mark_stream_activity(app, event_now);
                        ws_stream.send(Message::Pong(payload)).await
                            .map_err(|error| format!("Failed to send pong: {error}"))?;
                    }
                    Some(Ok(Message::Pong(_))) => {
                        let event_now = unix_now_secs();
                        last_activity_at = event_now;
                        pending_ping_since = None;
                        mark_stream_activity(app, event_now);
                    }
                    Some(Ok(Message::Close(_))) => {
                        return Err("Stream closed by server".to_string());
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(format!("Stream read error: {error}")),
                    None => return Err("Stream ended unexpectedly".to_string()),
                }
            }
            _ = sync_interval.tick() => {
                let app_for_sync = app.clone();
                let base_for_sync = base_url.to_string();
                let token_for_sync = token.to_string();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = fetch_recent_messages(&app_for_sync, &base_for_sync, &token_for_sync).await {
                        debug_log(&format!("periodic sync failed: {error}"));
                    }
                });
            }
            _ = liveness_interval.tick() => {
                let event_now = unix_now_secs();
                if event_now.saturating_sub(last_activity_at) < STREAM_LIVENESS_IDLE_SECS {
                    emit_runtime_diagnostics(app);
                    continue;
                }
                match pending_ping_since {
                    None => {
                        debug_log("ws liveness ping sent");
                        ws_stream
                            .send(Message::Ping(Vec::<u8>::new().into()))
                            .await
                            .map_err(|error| format!("Failed to send liveness ping: {error}"))?;
                        pending_ping_since = Some(event_now);
                    }
                    Some(started) => {
                        if event_now.saturating_sub(started) >= STREAM_LIVENESS_PING_GRACE_SECS {
                            return Err(format!(
                                "Stream liveness timeout after {}s idle",
                                event_now.saturating_sub(last_activity_at)
                            ));
                        }
                    }
                }
                emit_runtime_diagnostics(app);
            }
        }
    }
}

async fn fetch_recent_messages(app: &AppHandle, base_url: &str, token: &str) -> Result<(), String> {
    let cache_limit = desired_cache_limit(app);
    let mut fresh = Vec::new();
    let mut since: Option<i64> = None;

    while fresh.len() < cache_limit {
        let remaining = cache_limit.saturating_sub(fresh.len());
        let limit = remaining.min(MAX_API_PAGE_LIMIT);
        if limit == 0 {
            break;
        }

        let mut endpoint = format!("{base_url}/message?limit={limit}");
        if let Some(cursor) = since {
            endpoint.push_str(&format!("&since={cursor}"));
        }

        let response = reqwest::Client::new()
            .get(endpoint)
            .header("X-Gotify-Key", token)
            .send()
            .await
            .map_err(|error| format!("Failed to fetch recent messages: {error}"))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unable to read response body>".to_string());
            return Err(format!(
                "Recent message request failed with HTTP {status}: {}",
                truncate_message(&body, 200)
            ));
        }

        let json = response
            .json::<GotifyMessageListWire>()
            .await
            .map_err(|error| format!("Failed to decode recent messages: {error}"))?;

        if json.messages.is_empty() {
            break;
        }

        let mut min_id_in_page: Option<i64> = None;
        let mut page_count = 0usize;
        for item in json.messages {
            min_id_in_page = Some(match min_id_in_page {
                Some(min_id) => min_id.min(item.id),
                None => item.id,
            });
            fresh.push(convert_wire_message(app, item));
            page_count = page_count.saturating_add(1);
            if fresh.len() >= cache_limit {
                break;
            }
        }

        if let Some(min_id) = min_id_in_page {
            if since == Some(min_id) {
                break;
            }
            since = Some(min_id);
        } else {
            break;
        }

        if page_count < limit {
            break;
        }
    }

    fresh.sort_by(cached_message_cmp);
    fresh.dedup_by_key(|message| message.id);
    fresh.sort_by(cached_message_cmp);
    if fresh.len() > cache_limit {
        fresh.truncate(cache_limit);
    }
    replace_message_cache(app, fresh)?;

    Ok(())
}

async fn fetch_applications(app: &AppHandle, base_url: &str, token: &str) -> Result<(), String> {
    let endpoint = format!("{base_url}/application");
    let response = reqwest::Client::new()
        .get(endpoint)
        .header("X-Gotify-Key", token)
        .send()
        .await
        .map_err(|error| format!("Failed to fetch applications: {error}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "Application request failed with HTTP {}",
            response.status().as_u16()
        ));
    }

    let apps = response
        .json::<Vec<GotifyApplicationWire>>()
        .await
        .map_err(|error| format!("Failed to decode applications: {error}"))?;

    let icon_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(PREVIEW_REQUEST_TIMEOUT_SECS))
        .build()
        .map_err(|error| format!("Failed to build icon HTTP client: {error}"))?;

    let mut next_map = HashMap::with_capacity(apps.len());
    for app_item in apps {
        let icon_url = match resolve_application_image_data_url(
            &icon_client,
            base_url,
            &app_item.image,
            token,
        )
        .await
        {
            Ok(icon_url) => icon_url,
            Err(error) => {
                debug_log(&format!(
                    "failed to fetch application icon app_id={} name={}: {error}",
                    app_item.id,
                    truncate_message(&app_item.name, 48)
                ));
                String::new()
            }
        };
        next_map.insert(
            app_item.id,
            ApplicationMeta {
                name: app_item.name,
                icon_url,
            },
        );
    }

    // Pre-warm the on-disk notification icon cache from the freshly-fetched data
    // URLs. When a notification fires, the PNG file will already be on disk so no
    // blocking HTTP download is needed in the notification thread.
    #[cfg(target_os = "macos")]
    warm_notification_icon_cache(app, &next_map);

    let state = app.state::<AppState>();
    let mut map = state
        .app_meta
        .lock()
        .map_err(|_| "Application map lock poisoned".to_string())?;
    *map = next_map;
    Ok(())
}

/// Writes each application icon (already fetched as a base64 data URL during
/// `fetch_applications`) to the notification icon PNG cache directory so that
/// the notification thread can use it without any further network I/O.
#[cfg(target_os = "macos")]
fn warm_notification_icon_cache(app: &AppHandle, app_meta: &HashMap<i64, ApplicationMeta>) {
    for (app_id, meta) in app_meta {
        if meta.icon_url.trim().is_empty() {
            continue;
        }
        // Only data: URLs are available at this point — the async HTTP fetch already
        // happened in fetch_applications. Skip bare URLs to avoid blocking I/O.
        if !meta.icon_url.trim_start().starts_with("data:") {
            continue;
        }
        let Some(icons_dir) = notification_icon_cache_dir(app) else {
            continue;
        };
        let file_path = icons_dir.join(format!("app-{app_id}.png"));
        if file_path.exists() {
            continue;
        }
        match decode_data_url_bytes(&meta.icon_url, APP_ICON_MAX_BYTES) {
            Ok(bytes) if !bytes.is_empty() => {
                if let Err(error) = fs::write(&file_path, &bytes) {
                    debug_log(&format!(
                        "warm_notification_icon_cache: failed writing icon app_id={app_id}: {error}"
                    ));
                } else {
                    debug_log(&format!(
                        "warm_notification_icon_cache: cached icon app_id={app_id}"
                    ));
                }
            }
            Ok(_) => {}
            Err(error) => {
                debug_log(&format!(
                    "warm_notification_icon_cache: failed decoding icon app_id={app_id}: {error}"
                ));
            }
        }
    }
}

fn parse_stream_message(app: &AppHandle, text: &str) -> Option<CachedMessage> {
    match serde_json::from_str::<GotifyMessageWire>(text) {
        Ok(message) => {
            debug_log(&format!("message parsed id={}", message.id));
            Some(convert_wire_message(app, message))
        }
        Err(error) => {
            debug_log(&format!(
                "stream decode failed: {} payload={}",
                error,
                truncate_message(text, 140)
            ));
            None
        }
    }
}

fn convert_wire_message(app: &AppHandle, message: GotifyMessageWire) -> CachedMessage {
    let (app_label, app_icon) = resolve_app_meta(app, message.appid);
    CachedMessage {
        id: message.id,
        app_id: message.appid,
        title: message.title,
        message: message.message,
        priority: message.priority,
        app: app_label,
        app_icon,
        date: message.date,
    }
}

fn resolve_app_meta(app: &AppHandle, app_id: i64) -> (String, Option<String>) {
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(map) = state.app_meta.lock() {
            if let Some(meta) = map.get(&app_id) {
                let icon = if meta.icon_url.trim().is_empty() {
                    None
                } else {
                    Some(meta.icon_url.clone())
                };
                return (meta.name.clone(), icon);
            }
        }
    }
    (format!("app:{app_id}"), None)
}

fn cache_and_emit_message(
    app: &AppHandle,
    message: CachedMessage,
    allow_notification: bool,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut lock = state
        .messages
        .lock()
        .map_err(|_| "Message cache lock poisoned".to_string())?;

    let mut existed = false;
    if let Some(pos) = lock.iter().position(|m| m.id == message.id) {
        existed = true;
        lock.remove(pos);
    }

    lock.insert(0, message.clone());
    let cache_limit = desired_cache_limit(app);
    if lock.len() > cache_limit {
        lock.truncate(cache_limit);
    }

    let cache_snapshot = lock.clone();
    let cache_snapshot_for_persist = cache_snapshot.clone();
    let cache_path = messages_file(app)?;
    drop(lock);

    thread::spawn(move || {
        if let Err(error) = persist_messages_to_path(&cache_path, &cache_snapshot_for_persist) {
            debug_log(&format!("failed to persist message cache: {error}"));
        }
    });

    debug_log(&format!(
        "message received id={} title={}",
        message.id,
        truncate_message(&message.title, 60)
    ));
    debug_log(&format!("message-received emit id={}", message.id));
    let event_now = unix_now_secs();
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut runtime) = state.runtime.lock() {
            runtime.last_message_at = Some(event_now);
            runtime.last_message_id = Some(message.id);
            runtime.last_stream_event_at = Some(event_now);
        }
    }
    // Emit on both scopes so frontend listeners are resilient across target modes.
    let _ = app.emit("message-received", message.clone());
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.emit("message-received", message.clone());
    }
    if allow_notification && !existed {
        maybe_notify_message(app, &message);
    }
    Ok(())
}

fn mark_stream_activity(app: &AppHandle, at: u64) {
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut runtime) = state.runtime.lock() {
            runtime.last_stream_event_at = Some(at);
        }
    }
}

fn replace_message_cache(app: &AppHandle, fresh: Vec<CachedMessage>) -> Result<(), String> {
    let state = app.state::<AppState>();
    let cache_limit = desired_cache_limit(app);
    let mut normalized = fresh;
    normalized.sort_by(cached_message_cmp);
    normalized.dedup_by_key(|message| message.id);
    normalized.sort_by(cached_message_cmp);
    if normalized.len() > cache_limit {
        normalized.truncate(cache_limit);
    }

    {
        let mut lock = state
            .messages
            .lock()
            .map_err(|_| "Message cache lock poisoned".to_string())?;
        let changed = lock.len() != normalized.len()
            || lock.iter().zip(normalized.iter()).any(|(a, b)| {
                a.id != b.id
                    || a.date != b.date
                    || a.priority != b.priority
                    || a.title != b.title
                    || a.message != b.message
            });
        if !changed {
            return Ok(());
        }
        *lock = normalized.clone();
    }

    persist_current_cache_async(app)?;
    let _ = app.emit("messages-synced", true);
    let _ = app.emit("messages-updated", normalized.clone());
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.emit("messages-updated", normalized.clone());
    }
    Ok(())
}

fn remove_message_from_cache(app: &AppHandle, message_id: i64) -> Result<(), String> {
    let state = app.state::<AppState>();
    let updated_snapshot;
    {
        let mut lock = state
            .messages
            .lock()
            .map_err(|_| "Message cache lock poisoned".to_string())?;
        lock.retain(|m| m.id != message_id);
        updated_snapshot = lock.clone();
    }

    persist_current_cache_async(app)?;
    let _ = app.emit("messages-synced", true);
    let _ = app.emit("messages-updated", updated_snapshot.clone());
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.emit("messages-updated", updated_snapshot.clone());
    }
    Ok(())
}

fn persist_current_cache_async(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let cache_snapshot = state
        .messages
        .lock()
        .map_err(|_| "Message cache lock poisoned".to_string())?
        .clone();
    let cache_path = messages_file(app)?;
    thread::spawn(move || {
        if let Err(error) = persist_messages_to_path(&cache_path, &cache_snapshot) {
            debug_log(&format!("failed to persist message cache: {error}"));
        }
    });
    Ok(())
}

fn cached_message_cmp(a: &CachedMessage, b: &CachedMessage) -> std::cmp::Ordering {
    let ta = chrono::DateTime::parse_from_rfc3339(&a.date)
        .map(|v| v.timestamp())
        .ok();
    let tb = chrono::DateTime::parse_from_rfc3339(&b.date)
        .map(|v| v.timestamp())
        .ok();
    match (tb, ta) {
        (Some(tb), Some(ta)) if tb != ta => tb.cmp(&ta),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        _ => b.id.cmp(&a.id),
    }
}

fn desired_cache_limit(app: &AppHandle) -> usize {
    read_settings(app)
        .map(|settings| normalize_cache_limit(settings.cache_limit))
        .unwrap_or(DEFAULT_CACHE_LIMIT)
}

fn normalize_cache_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_CACHE_LIMIT)
}

fn maybe_notify_message(app: &AppHandle, message: &CachedMessage) {
    let settings = match read_settings(app) {
        Ok(settings) => settings,
        Err(error) => {
            debug_log(&format!("failed to read settings for notify: {error}"));
            return;
        }
    };

    if let Some(until) = settings.pause_until {
        if until == PAUSE_FOREVER_SENTINEL || unix_now_secs() < until {
            return;
        }
    }

    if message.priority < settings.min_priority {
        return;
    }
    if is_quiet_hours(settings.quiet_hours_start, settings.quiet_hours_end) {
        return;
    }

    let _ = app.emit("notification-message", message);
    #[cfg(target_os = "macos")]
    send_macos_notification(app.clone(), message.clone());
}

fn is_quiet_hours(start: Option<u8>, end: Option<u8>) -> bool {
    let (start, end) = match (start, end) {
        (Some(start), Some(end)) => (start, end),
        _ => return false,
    };

    let now: u8 = chrono::Local::now().hour() as u8;

    if start == end {
        return true;
    }
    if start < end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}

#[cfg(target_os = "macos")]
fn send_macos_notification(app: AppHandle, message: CachedMessage) {
    thread::spawn(move || {
        ensure_macos_notification_application();
        let title = if message.app.trim().is_empty() {
            format!("Priority {}", message.priority)
        } else {
            format!("{} · Priority {}", message.app, message.priority)
        };
        let subtitle = if message.title.trim().is_empty() {
            "Gotify message".to_string()
        } else {
            message.title.clone()
        };
        let body = truncate_message(&message.message, 220);

        let mut notification = Notification::new();
        notification
            .title(&title)
            .subtitle(&subtitle)
            .message(&body)
            .main_button(MainButton::SingleAction("Open"))
            .close_button("Dismiss")
            .default_sound()
            .wait_for_click(true)
            .asynchronous(false);

        let sender_icon_path = resolve_default_notification_app_icon_path(&app);
        if let Some(sender_icon_path) = sender_icon_path.as_deref() {
            debug_log(&format!(
                "notification sender icon path for id={}: {}",
                message.id, sender_icon_path
            ));
            notification.app_icon(sender_icon_path);
        }

        let content_image_path = resolve_notification_content_image_path(&app, &message);
        if let Some(content_image_path) = content_image_path.as_deref() {
            debug_log(&format!(
                "notification content image path for id={}: {}",
                message.id, content_image_path
            ));
            notification.content_image(content_image_path);
        } else {
            debug_log(&format!(
                "notification content image path for id={}: <none>",
                message.id
            ));
        }

        debug_log(&format!(
            "sending macOS notification id={} title={}",
            message.id,
            truncate_message(&title, 80)
        ));
        match notification.send() {
            Ok(NotificationResponse::Click) | Ok(NotificationResponse::ActionButton(_)) => {
                debug_log(&format!("macOS notification clicked id={}", message.id));
                show_main_window(&app);
                let _ = app.emit("notification-clicked", message.clone());
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.emit("notification-clicked", message);
                }
            }
            Ok(_) => {
                debug_log(&format!("macOS notification delivered id={}", message.id));
            }
            Err(error) => {
                debug_log(&format!("failed to show macOS notification: {error}"));
                // mac-notification-sys uses the same UNUserNotificationCenter API
                // that any fallback would need. If it fails, no alternative mechanism
                // will succeed, so we log and drop rather than attempt osascript.
            }
        }
    });
}

#[cfg(target_os = "macos")]
fn ensure_macos_notification_application() {
    static INIT_NOTIFICATION_APP: std::sync::Once = std::sync::Once::new();
    INIT_NOTIFICATION_APP.call_once(|| {
        // In dev mode the app may not be registered with the target bundle id yet.
        // Try production id first, then a known local fallback to keep notifications working.
        for bundle_id in [
            "net.gotify.desktop",
            "com.apple.Terminal",
            "com.apple.Finder",
        ] {
            match mac_notification_sys::set_application(bundle_id) {
                Ok(_) => {
                    debug_log(&format!(
                        "macOS notification bundle id configured: {bundle_id}"
                    ));
                    return;
                }
                Err(error) => {
                    debug_log(&format!(
                        "failed to set macOS notification bundle id {bundle_id}: {error}"
                    ));
                }
            }
        }
    });
}

#[cfg(target_os = "macos")]
fn resolve_notification_content_image_path(
    app: &AppHandle,
    message: &CachedMessage,
) -> Option<String> {
    let mut icon_url_candidates: Vec<String> = Vec::new();
    if let Some(icon_url) = message.app_icon.as_deref() {
        if !icon_url.trim().is_empty() {
            icon_url_candidates.push(icon_url.to_string());
        }
    }
    if icon_url_candidates.is_empty() {
        if let Some(state) = app.try_state::<AppState>() {
            if let Ok(map) = state.app_meta.lock() {
                if let Some(meta) = map.get(&message.app_id) {
                    if !meta.icon_url.trim().is_empty() {
                        icon_url_candidates.push(meta.icon_url.clone());
                    }
                }
            }
        }
    }

    let app_id = if message.app_id <= 0 {
        message.id
    } else {
        message.app_id
    };
    for icon_url in icon_url_candidates {
        if let Some(png_path) = cache_remote_notification_icon_png(app, app_id, &icon_url) {
            return Some(png_path);
        }
    }

    None
}

#[cfg(target_os = "macos")]
fn resolve_default_notification_app_icon_path(app: &AppHandle) -> Option<String> {
    if let Ok(resource_dir) = app.path().resource_dir() {
        let bundled_icns = resource_dir.join("icons/icon.icns");
        if bundled_icns.exists() {
            return Some(bundled_icns.to_string_lossy().to_string());
        }
        let bundled_png = resource_dir.join("icons/icon.png");
        if bundled_png.exists() {
            let cache_dir = notification_icon_cache_dir(app)?;
            let icns_path = cache_dir.join("default-app-icon.icns");
            if ensure_icns_from_png(&bundled_png, &icns_path) {
                return Some(icns_path.to_string_lossy().to_string());
            }
            return Some(bundled_png.to_string_lossy().to_string());
        }
    }

    let dev_icns = std::env::current_dir()
        .ok()?
        .join("src-tauri/icons/icon.icns");
    if dev_icns.exists() {
        return Some(dev_icns.to_string_lossy().to_string());
    }
    let dev_png = std::env::current_dir()
        .ok()?
        .join("src-tauri/icons/icon.png");
    if dev_png.exists() {
        let cache_dir = notification_icon_cache_dir(app)?;
        let icns_path = cache_dir.join("default-app-icon.icns");
        if ensure_icns_from_png(&dev_png, &icns_path) {
            return Some(icns_path.to_string_lossy().to_string());
        }
        return Some(dev_png.to_string_lossy().to_string());
    }

    None
}

#[cfg(target_os = "macos")]
fn cache_remote_notification_icon_png(
    app: &AppHandle,
    app_id: i64,
    icon_url: &str,
) -> Option<String> {
    let icons_dir = notification_icon_cache_dir(app)?;
    let file_path = icons_dir.join(format!("app-{app_id}.png"));

    // Fast path: already on disk (written by warm_notification_icon_cache at startup).
    if file_path.exists() {
        return Some(file_path.to_string_lossy().to_string());
    }

    // Data-URL path: icon is embedded in memory, write it to disk now.
    if icon_url.trim_start().starts_with("data:") {
        let bytes = match decode_data_url_bytes(icon_url, APP_ICON_MAX_BYTES) {
            Ok(bytes) => bytes,
            Err(error) => {
                debug_log(&format!(
                    "failed decoding data-url app icon for app_id={app_id}: {error}"
                ));
                return None;
            }
        };
        if bytes.is_empty() {
            return None;
        }
        if let Err(error) = fs::write(&file_path, &bytes) {
            debug_log(&format!("failed writing app icon cache file: {error}"));
            return None;
        }
        return Some(file_path.to_string_lossy().to_string());
    }

    // Icon is neither cached on disk nor a data URL. Icons are pre-warmed by
    // warm_notification_icon_cache when fetch_applications runs, so reaching here
    // means the icon was unavailable at startup. Fire the notification without an
    // image rather than blocking the notification thread on a network request.
    debug_log(&format!(
        "notification icon not in cache for app_id={app_id}, firing without image"
    ));
    None
}

#[cfg(target_os = "macos")]
fn notification_icon_cache_dir(app: &AppHandle) -> Option<PathBuf> {
    let base_cache_dir = app
        .path()
        .app_cache_dir()
        .or_else(|_| app.path().app_config_dir())
        .ok()?;
    let icons_dir = base_cache_dir.join("notification-icons");
    if fs::create_dir_all(&icons_dir).is_err() {
        return None;
    }
    Some(icons_dir)
}

#[cfg(target_os = "macos")]
fn ensure_icns_from_png(source_png: &Path, target_icns: &Path) -> bool {
    if target_icns.exists() {
        return true;
    }

    let iconset_dir = target_icns.with_extension("iconset");
    if fs::create_dir_all(&iconset_dir).is_err() {
        return false;
    }

    let sizes: [(&str, u32, u32); 10] = [
        ("icon_16x16.png", 16, 16),
        ("icon_16x16@2x.png", 32, 32),
        ("icon_32x32.png", 32, 32),
        ("icon_32x32@2x.png", 64, 64),
        ("icon_128x128.png", 128, 128),
        ("icon_128x128@2x.png", 256, 256),
        ("icon_256x256.png", 256, 256),
        ("icon_256x256@2x.png", 512, 512),
        ("icon_512x512.png", 512, 512),
        ("icon_512x512@2x.png", 1024, 1024),
    ];

    for (file_name, pixels_h, pixels_w) in sizes {
        let out = iconset_dir.join(file_name);
        let status = Command::new("sips")
            .arg("-z")
            .arg(pixels_h.to_string())
            .arg(pixels_w.to_string())
            .arg(source_png)
            .arg("--out")
            .arg(&out)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !matches!(status, Ok(s) if s.success()) {
            let _ = fs::remove_dir_all(&iconset_dir);
            return false;
        }
    }

    let status = Command::new("iconutil")
        .arg("-c")
        .arg("icns")
        .arg(&iconset_dir)
        .arg("-o")
        .arg(target_icns)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = fs::remove_dir_all(&iconset_dir);
    matches!(status, Ok(s) if s.success() && target_icns.exists())
}

fn snapshot_runtime(app: &AppHandle) -> Result<RuntimeDiagnostics, String> {
    let state = app.state::<AppState>();
    let runtime = state
        .runtime
        .lock()
        .map_err(|_| "Runtime lock poisoned".to_string())?;

    let now = unix_now_secs();
    let stale_for_seconds = runtime
        .last_stream_event_at
        .map(|last| now.saturating_sub(last));

    Ok(RuntimeDiagnostics {
        connection_state: runtime.connection_state.clone(),
        should_run: runtime.should_run,
        last_connected_at: runtime.last_connected_at,
        last_stream_event_at: runtime.last_stream_event_at,
        last_message_at: runtime.last_message_at,
        last_message_id: runtime.last_message_id,
        stale_for_seconds,
        last_error: runtime.last_error.clone(),
        backoff_seconds: runtime.backoff_seconds,
        reconnect_attempts: runtime.reconnect_attempts,
    })
}

fn emit_runtime_diagnostics(app: &AppHandle) {
    match snapshot_runtime(app) {
        Ok(diag) => {
            let _ = app.emit("runtime-diagnostics", diag);
        }
        Err(err) => {
            debug_log(&format!("failed to snapshot runtime: {err}"));
        }
    }
}

fn emit_connection_state(app: &AppHandle, status: &str) {
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut runtime) = state.runtime.lock() {
            runtime.connection_state = status.to_string();
        }
    }

    if let Err(error) = app.emit("connection-state", status) {
        debug_log(&format!("failed to emit connection-state: {error}"));
    }
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_icon(tray_icon_for_status(status));
    }
    emit_runtime_diagnostics(app);
}

fn tray_icon_for_status(status: &str) -> Option<Image<'static>> {
    let bytes = match status {
        "Connected" => include_bytes!("../icons/tray-connected.png").as_slice(),
        "Connecting" => include_bytes!("../icons/tray-connecting.png").as_slice(),
        "Backoff" => include_bytes!("../icons/tray-backoff.png").as_slice(),
        _ => include_bytes!("../icons/tray-disconnected.png").as_slice(),
    };
    Image::from_bytes(bytes).ok().map(|icon| icon.to_owned())
}

fn settings_file(app: &AppHandle) -> Result<PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("Failed to resolve app config dir: {error}"))?;

    fs::create_dir_all(&config_dir)
        .map_err(|error| format!("Failed to create config directory: {error}"))?;

    Ok(config_dir.join("settings.json"))
}

fn messages_file(app: &AppHandle) -> Result<PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("Failed to resolve app config dir: {error}"))?;

    fs::create_dir_all(&config_dir)
        .map_err(|error| format!("Failed to create config directory: {error}"))?;

    Ok(config_dir.join("messages.json"))
}

/// Set file permissions to 0o600 (owner read/write only).
///
/// Called after every write to `settings.json` and `messages.json`, and once
/// on startup as a self-healing check in case permissions were reset by a
/// backup restore or copy. Silently no-ops if the file does not yet exist.
/// This is not a substitute for Keychain — a process running as the same user
/// can still read the file — but it prevents other users and casual inspection
/// via `cat` / Finder from exposing the token.
fn restrict_file_permissions(path: &Path) {
    if path.exists() {
        if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            debug_log(&format!("restrict_file_permissions: failed for {path:?}: {error}"));
        }
    }
}

fn read_settings(app: &AppHandle) -> Result<StoredSettings, String> {
    let path = settings_file(app)?;
    if !path.exists() {
        return Ok(StoredSettings::default());
    }

    let content =
        fs::read_to_string(path).map_err(|error| format!("Failed to read settings: {error}"))?;
    serde_json::from_str::<StoredSettings>(&content)
        .map_err(|error| format!("Failed to parse settings: {error}"))
}

fn save_non_secret_settings(app: &AppHandle, settings: &StoredSettings) -> Result<(), String> {
    let path = settings_file(app)?;
    let content = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("Failed to serialize settings: {error}"))?;
    fs::write(&path, content).map_err(|error| format!("Failed to write settings: {error}"))?;
    restrict_file_permissions(&path);
    Ok(())
}

fn load_messages_from_disk(app: &AppHandle) -> Result<Vec<CachedMessage>, String> {
    let path = messages_file(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&path)
        .map_err(|error| format!("Failed to read message cache: {error}"))?;
    match serde_json::from_str::<Vec<CachedMessage>>(&content) {
        Ok(messages) => Ok(messages),
        Err(error) => {
            let backup_path = path.with_extension(format!("corrupt-{}.json", unique_time_suffix()));
            if let Err(rename_error) = fs::rename(&path, &backup_path) {
                debug_log(&format!(
                    "failed to back up corrupt cache file: {rename_error}"
                ));
            } else {
                debug_log(&format!(
                    "moved corrupt cache file to {}",
                    backup_path.to_string_lossy()
                ));
            }
            debug_log(&format!("cache parse failed, starting fresh: {error}"));
            Ok(Vec::new())
        }
    }
}

fn persist_messages_to_path(path: &PathBuf, messages: &[CachedMessage]) -> Result<(), String> {
    let content = serde_json::to_string(messages)
        .map_err(|error| format!("Failed to serialize message cache: {error}"))?;
    let tmp_path = path.with_extension(format!("tmp-{}", unique_time_suffix()));
    // Set permissions on the temp file before the atomic rename so the final
    // file is never briefly visible with overly permissive permissions.
    fs::write(&tmp_path, content)
        .map_err(|error| format!("Failed to write message cache temp file: {error}"))?;
    restrict_file_permissions(&tmp_path);
    fs::rename(&tmp_path, path)
        .map_err(|error| format!("Failed to atomically replace message cache: {error}"))
}

fn get_settings_path() -> Result<&'static PathBuf, String> {
    SETTINGS_FILE.get().ok_or_else(|| "Settings path not initialised (setup not complete)".to_string())
}

fn load_token() -> Result<Option<String>, String> {
    let path = get_settings_path()?;
    debug_log(&format!("load_token: reading settings from {path:?}"));
    if !path.exists() {
        debug_log("load_token: settings file not found — no token");
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read settings for token: {error}"))?;
    let settings: StoredSettings = serde_json::from_str(&raw).unwrap_or_default();
    match settings.token.filter(|t| !t.trim().is_empty()) {
        Some(t) => {
            debug_log(&format!("load_token: found token len={}", t.len()));
            Ok(Some(t))
        }
        None => {
            debug_log("load_token: no token in settings");
            Ok(None)
        }
    }
}

fn normalize_base_url(input: &str) -> Result<String, String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("Server URL is required".to_string());
    }

    let url =
        reqwest::Url::parse(trimmed).map_err(|error| format!("Invalid server URL: {error}"))?;

    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err("Server URL must start with http:// or https://".to_string());
    }

    Ok(trimmed.to_string())
}

fn build_stream_ws_url(base_url: &str) -> Result<String, String> {
    let mut ws_url =
        reqwest::Url::parse(base_url).map_err(|error| format!("Invalid server URL: {error}"))?;

    match ws_url.scheme() {
        "http" => {
            ws_url
                .set_scheme("ws")
                .map_err(|_| "Unable to convert URL scheme to ws".to_string())?;
        }
        "https" => {
            ws_url
                .set_scheme("wss")
                .map_err(|_| "Unable to convert URL scheme to wss".to_string())?;
        }
        _ => return Err("Server URL must start with http:// or https://".to_string()),
    }

    let mut path = ws_url.path().trim_end_matches('/').to_string();
    path.push_str("/stream");
    ws_url.set_path(&path);
    Ok(ws_url.to_string())
}

fn resolve_application_image_url(base_url: &str, image_path: &str) -> Result<String, String> {
    if image_path.trim().is_empty() {
        return Ok(String::new());
    }

    let mut base =
        reqwest::Url::parse(base_url).map_err(|error| format!("Invalid server URL: {error}"))?;
    let joined = base
        .join(image_path)
        .map_err(|error| format!("Failed to resolve application image path: {error}"))?;

    base = joined;
    Ok(base.to_string())
}

async fn resolve_application_image_data_url(
    client: &reqwest::Client,
    base_url: &str,
    image_path: &str,
    token: &str,
) -> Result<String, String> {
    let image_url = resolve_application_image_url(base_url, image_path)?;
    if image_url.is_empty() {
        return Ok(String::new());
    }

    let response = client
        .get(&image_url)
        .header("X-Gotify-Key", token)
        .send()
        .await
        .map_err(|error| format!("Application icon request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Application icon request failed with HTTP {}",
            response.status().as_u16()
        ));
    }

    if let Some(content_length) = response.content_length() {
        if content_length > APP_ICON_MAX_BYTES as u64 {
            return Err(format!(
                "Application icon too large ({content_length} bytes > {APP_ICON_MAX_BYTES})"
            ));
        }
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("image/png")
        .to_ascii_lowercase();
    if !content_type.starts_with("image/") {
        return Err(format!(
            "Application icon response is not an image ({content_type})"
        ));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("Failed to read application icon body: {error}"))?;
    if bytes.is_empty() {
        return Ok(String::new());
    }
    if bytes.len() > APP_ICON_MAX_BYTES {
        return Err(format!(
            "Application icon too large ({} bytes > {APP_ICON_MAX_BYTES})",
            bytes.len()
        ));
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{content_type};base64,{encoded}"))
}

fn decode_data_url_bytes(data_url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    let trimmed = data_url.trim();
    if !trimmed.starts_with("data:") {
        return Err("Not a data URL".to_string());
    }
    let (meta, payload) = trimmed
        .split_once(',')
        .ok_or_else(|| "Malformed data URL".to_string())?;
    let meta_lower = meta.to_ascii_lowercase();
    if !meta_lower.starts_with("data:image/") {
        return Err("Data URL is not an image".to_string());
    }
    if !meta_lower.contains(";base64") {
        return Err("Data URL is not base64 encoded".to_string());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|error| format!("Invalid base64 payload: {error}"))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "Data URL image too large ({} bytes > {max_bytes})",
            bytes.len()
        ));
    }
    Ok(bytes)
}

fn find_title(html: &str) -> Option<String> {
    let doc = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("title").ok()?;
    let el = doc.select(&selector).next()?;
    let text: String = el.text().collect();
    let trimmed = text.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

fn find_meta(html: &str, keys: &[&str]) -> Option<String> {
    let doc = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("meta").ok()?;
    for el in doc.select(&selector) {
        let prop = el.value().attr("property")
            .or_else(|| el.value().attr("name"))
            .unwrap_or("");
        let prop_lower = prop.to_ascii_lowercase();
        if keys.iter().any(|k| prop_lower == k.to_ascii_lowercase()) {
            if let Some(content) = el.value().attr("content") {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn resolve_meta_url(base_url: &reqwest::Url, raw: &str) -> Option<String> {
    if raw.trim().is_empty() {
        return None;
    }
    let resolved = if let Ok(url) = reqwest::Url::parse(raw) {
        url
    } else {
        base_url.join(raw).ok()?
    };
    if !matches!(resolved.scheme(), "http" | "https") {
        return None;
    }
    if let Some(host) = resolved.host_str() {
        if is_blocked_preview_hostname(host) {
            return None;
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            if preview_block_reason_for_ip(ip).is_some() {
                return None;
            }
        }
    }
    Some(resolved.to_string())
}

fn redact_ws_url(url: &str) -> String {
    let mut parsed = match reqwest::Url::parse(url) {
        Ok(url) => url,
        Err(_) => return "<invalid-url>".to_string(),
    };
    if parsed.query().is_some() {
        parsed.set_query(Some("token=***"));
    }
    parsed.to_string()
}

fn truncate_message(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }

    let truncated: String = input.chars().take(max_chars).collect();
    format!("{truncated}...")
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn unique_time_suffix() -> u64 {
    // Use a monotonic counter rather than wall-clock nanoseconds. On systems
    // with coarse-grained clocks two rapid calls can return the same timestamp,
    // causing temp files to collide and silently overwrite each other.
    FILE_SUFFIX_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn debug_log(message: &str) {
    // Only active in debug builds. In release builds this compiles to nothing,
    // preventing sensitive connection metadata from leaking to stderr or disk.
    #[cfg(debug_assertions)]
    {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let line = format!("[gotify-desktop][{ts}] {message}\n");
        eprint!("{line}");
        // Also write to a log file so output is visible even when the terminal
        // disconnects stderr (common with macOS GUI app launch paths).
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/gotify-desktop.log")
        {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

fn emit_delete_debug(
    app: &AppHandle,
    message_id: i64,
    phase: &str,
    detail: &str,
    status: Option<u16>,
) {
    let event = DeleteMessageDebugEvent {
        at: unix_now_secs(),
        message_id,
        phase: phase.to_string(),
        detail: truncate_message(detail, 800),
        status,
    };
    let _ = app.emit("delete-message-debug", event.clone());
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.emit("delete-message-debug", event);
    }
}

#[cfg(target_os = "macos")]
fn launch_agent_plist_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|error| format!("HOME is not set: {error}"))?;
    let launch_agents_dir = PathBuf::from(home).join("Library/LaunchAgents");
    fs::create_dir_all(&launch_agents_dir)
        .map_err(|error| format!("Failed to create LaunchAgents dir: {error}"))?;
    Ok(launch_agents_dir.join(format!("{LAUNCH_AGENT_LABEL}.plist")))
}

#[cfg(target_os = "macos")]
fn apply_launch_at_login(enabled: bool) -> Result<(), String> {
    let plist_path = launch_agent_plist_path()?;
    if !enabled {
        let _ = Command::new("launchctl")
            .arg("unload")
            .arg("-w")
            .arg(&plist_path)
            .output();
        if plist_path.exists() {
            let _ = fs::remove_file(&plist_path);
        }
        return Ok(());
    }

    let exe = std::env::current_exe()
        .map_err(|error| format!("Failed to resolve app executable: {error}"))?;
    let exe_str = exe.to_string_lossy();

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#,
        LAUNCH_AGENT_LABEL,
        xml_escape(&exe_str)
    );

    fs::write(&plist_path, plist)
        .map_err(|error| format!("Failed to write launch agent: {error}"))?;

    let _ = Command::new("launchctl")
        .arg("load")
        .arg("-w")
        .arg(&plist_path)
        .output();

    Ok(())
}

#[cfg(target_os = "macos")]
fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&apos;")
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(quick_window) = app.get_webview_window("quick") {
        let _ = quick_window.hide();
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn position_quick_window_under_tray<R: Runtime>(
    app: &AppHandle<R>,
    click_position: tauri::PhysicalPosition<f64>,
) {
    let Some(window) = app.get_webview_window("quick") else {
        return;
    };

    let window_size = window.outer_size().or_else(|_| window.inner_size());
    let Ok(window_size) = window_size else {
        return;
    };

    let width = window_size.width as f64;
    let height = window_size.height as f64;

    let mut x = click_position.x - (width / 2.0);
    let mut y = click_position.y + 10.0;

    let monitor_for_click = window
        .available_monitors()
        .ok()
        .and_then(|monitors| {
            monitors.into_iter().find(|monitor| {
                let work_area = monitor.work_area();
                let left = work_area.position.x as f64;
                let top = work_area.position.y as f64;
                let right = left + work_area.size.width as f64;
                let bottom = top + work_area.size.height as f64;
                click_position.x >= left
                    && click_position.x <= right
                    && click_position.y >= top
                    && click_position.y <= bottom
            })
        })
        .or_else(|| window.current_monitor().ok().flatten());

    if let Some(monitor) = monitor_for_click {
        let work_area = monitor.work_area();
        let left = work_area.position.x as f64;
        let top = work_area.position.y as f64;
        let right = left + work_area.size.width as f64;
        let bottom = top + work_area.size.height as f64;

        if x < left {
            x = left;
        }
        if x + width > right {
            x = (right - width).max(left);
        }
        if y + height > bottom {
            y = (click_position.y - height - 10.0).max(top);
        }
        if y < top {
            y = top;
        }
    }

    // Tauri's PhysicalPosition expects integer pixels. `round()` before the
    // cast is intentional: it minimises placement error on HiDPI displays while
    // satisfying the API type, which does not accept f64 on all platforms.
    let _ = window.set_position(tauri::PhysicalPosition::new(
        x.round() as i32,
        y.round() as i32,
    ));
}

fn toggle_quick_window<R: Runtime>(
    app: &AppHandle<R>,
    tray_click_position: Option<tauri::PhysicalPosition<f64>>,
) {
    if let Some(window) = app.get_webview_window("quick") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            if let Some(position) = tray_click_position {
                position_quick_window_under_tray(app, position);
            }
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        return;
    }

    toggle_main_window(app);
}

fn toggle_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }
}

fn main() {
    debug_log("═══════════════════════════════════════");
    debug_log(&format!("gotify-desktop starting (pid={})", std::process::id()));
    debug_log("Logs also written to: /tmp/gotify-desktop.log");
    debug_log("═══════════════════════════════════════");
    tauri::Builder::default()
        .manage(AppState::new(Vec::new()))
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            test_connection,
            load_messages,
            get_pause_state,
            open_external_url,
            delete_message,
            start_stream,
            stop_stream,
            get_connection_state,
            get_runtime_diagnostics,
            recover_stream,
            restart_stream,
            pause_notifications,
            pause_notifications_forever,
            resume_notifications,
            fetch_url_preview
        ])
        .setup(|app| {
            debug_log("setup: starting");

            // Resolve and register the settings path before any load_token/save_settings calls.
            let config_dir = app
                .path()
                .app_config_dir()
                .map_err(|error| format!("Failed to resolve app config dir: {error}"))?;
            fs::create_dir_all(&config_dir)
                .map_err(|error| format!("Failed to create config directory: {error}"))?;
            // Register the settings.json path in the OnceLock so load_token()
            // (which lacks an AppHandle) can find it without going through read_settings.
            let settings_path = config_dir.join("settings.json");
            debug_log(&format!("setup: settings file path = {settings_path:?}"));
            let _ = SETTINGS_FILE.set(settings_path.clone());
            // Enforce 0o600 on startup — self-heals after backup restores or copies.
            restrict_file_permissions(&settings_path);
            if let Ok(messages_path) = messages_file(app.handle()) {
                restrict_file_permissions(&messages_path);
            }

            let startup_settings = read_settings(app.handle()).unwrap_or_default();
            debug_log(&format!(
                "setup: loaded settings base_url={:?} has_token={}",
                startup_settings.base_url,
                load_token()
                    .map_or_else(|e| format!("err:{e}"), |t| t.map_or("none".into(), |_| "yes".into()))
            ));
            #[cfg(target_os = "macos")]
            if let Err(error) = apply_launch_at_login(startup_settings.launch_at_login) {
                debug_log(&format!("failed to configure launch at login: {error}"));
            }

            let existing_messages = load_messages_from_disk(app.handle())?;
            let state = app.state::<AppState>();
            if let Ok(mut lock) = state.messages.lock() {
                *lock = existing_messages;
            } else {
                return Err("Message cache lock poisoned".into());
            }

            if app.get_webview_window("quick").is_none() {
                tauri::WebviewWindowBuilder::new(
                    app,
                    "quick",
                    WebviewUrl::App("index.html".into()),
                )
                .title("Gotify Inbox")
                .inner_size(470.0, 640.0)
                .min_inner_size(360.0, 420.0)
                .visible(false)
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .build()?;
            }

            if startup_settings.start_minimized_to_tray {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            } else {
                show_main_window(app.handle());
            }

            let pause_status_item = MenuItem::with_id(
                app,
                "pause_status",
                "Notifications: On",
                false,
                None::<&str>,
            )?;
            let open_item = MenuItem::with_id(app, "open_inbox", "Open Inbox", true, None::<&str>)?;
            let pause_15m_item =
                MenuItem::with_id(app, "pause_15m", "Pause 15m", true, None::<&str>)?;
            let pause_1h_item = MenuItem::with_id(app, "pause_1h", "Pause 1h", true, None::<&str>)?;
            let pause_forever_item =
                MenuItem::with_id(app, "pause_forever", "Pause Forever", true, None::<&str>)?;
            let resume_item = MenuItem::with_id(
                app,
                "resume_notifications",
                "Resume Notifications",
                true,
                None::<&str>,
            )?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &pause_status_item,
                    &open_item,
                    &pause_15m_item,
                    &pause_1h_item,
                    &pause_forever_item,
                    &resume_item,
                    &quit_item,
                ],
            )?;

            if let Ok(mut tray_pause_menu_lock) = state.tray_pause_menu.lock() {
                *tray_pause_menu_lock = Some(TrayPauseMenuState {
                    status_item: pause_status_item.clone(),
                    pause_15m_item: pause_15m_item.clone(),
                    pause_1h_item: pause_1h_item.clone(),
                    pause_forever_item: pause_forever_item.clone(),
                    resume_item: resume_item.clone(),
                });
            }
            apply_pause_state_to_tray(
                app.handle(),
                startup_settings.pause_until,
                startup_settings.pause_mode.as_deref(),
            );

            let mut tray_builder = TrayIconBuilder::with_id("main-tray")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        position,
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_quick_window(tray.app_handle(), Some(position));
                    }
                })
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "open_inbox" => {
                        show_main_window(app);
                    }
                    "pause_15m" => {
                        if let Err(error) = pause_notifications(app.clone(), 15) {
                            let _ = app.emit(
                                "connection-error",
                                format!("Failed to pause notifications: {error}"),
                            );
                        }
                    }
                    "pause_1h" => {
                        if let Err(error) = pause_notifications(app.clone(), 60) {
                            let _ = app.emit(
                                "connection-error",
                                format!("Failed to pause notifications: {error}"),
                            );
                        }
                    }
                    "pause_forever" => {
                        if let Err(error) = pause_notifications_forever(app.clone()) {
                            let _ = app.emit(
                                "connection-error",
                                format!("Failed to pause notifications: {error}"),
                            );
                        }
                    }
                    "resume_notifications" => {
                        if let Err(error) = resume_notifications(app.clone()) {
                            let _ = app.emit(
                                "connection-error",
                                format!("Failed to resume notifications: {error}"),
                            );
                        }
                    }
                    "quit" => {
                        let _ = stop_stream_internal(app);
                        app.exit(0);
                    }
                    _ => {}
                });
            if let Some(icon) =
                tray_icon_for_status("Disconnected").or_else(|| app.default_window_icon().cloned())
            {
                tray_builder = tray_builder.icon(icon);
            }
            tray_builder.build(app)?;

            let app_for_pause_refresh = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    refresh_pause_state_from_settings(&app_for_pause_refresh);
                }
            });

            match start_stream_internal(app.handle().clone(), None) {
                Ok(_) => {}
                Err(error) => {
                    let _ = app.emit("connection-error", format!("Auto-connect failed: {error}"));
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
                return;
            }

            if window.label() == "quick" {
                match event {
                    WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                    WindowEvent::Focused(false) => {
                        let _ = window.hide();
                    }
                    _ => {}
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
