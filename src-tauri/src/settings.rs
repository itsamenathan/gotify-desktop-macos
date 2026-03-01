use serde::{Deserialize, Serialize};
use std::{fs, time::Duration};
use tauri::{AppHandle, Runtime};

use crate::{
    apply_launch_at_login, debug_log, get_settings_path, normalize_cache_limit,
    restrict_file_permissions, settings_file, truncate_message, DEFAULT_CACHE_LIMIT,
};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(default)]
pub(crate) struct StoredSettings {
    pub(crate) base_url: String,
    pub(crate) token: Option<String>,
    pub(crate) min_priority: i64,
    pub(crate) cache_limit: usize,
    pub(crate) launch_at_login: bool,
    pub(crate) start_minimized_to_tray: bool,
    pub(crate) pause_until: Option<u64>,
    pub(crate) pause_mode: Option<String>,
    pub(crate) quiet_hours_start: Option<u8>,
    pub(crate) quiet_hours_end: Option<u8>,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            token: None,
            min_priority: 0,
            cache_limit: DEFAULT_CACHE_LIMIT,
            launch_at_login: false,
            start_minimized_to_tray: false,
            pause_until: None,
            pause_mode: None,
            quiet_hours_start: None,
            quiet_hours_end: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct SettingsResponse {
    pub(crate) base_url: String,
    pub(crate) has_token: bool,
    pub(crate) min_priority: i64,
    pub(crate) cache_limit: usize,
    pub(crate) launch_at_login: bool,
    pub(crate) start_minimized_to_tray: bool,
    pub(crate) pause_until: Option<u64>,
    pub(crate) pause_mode: Option<String>,
    pub(crate) quiet_hours_start: Option<u8>,
    pub(crate) quiet_hours_end: Option<u8>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PauseStateResponse {
    pub(crate) pause_until: Option<u64>,
    pub(crate) pause_mode: Option<String>,
}

pub(crate) fn load_settings<R: Runtime>(app: &AppHandle<R>) -> Result<SettingsResponse, String> {
    let stored = read_settings(app)?;
    let has_token = stored
        .token
        .as_deref()
        .map_or(false, |t| !t.trim().is_empty());

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

pub(crate) fn save_settings<R: Runtime>(
    app: &AppHandle<R>,
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
    let current = read_settings(app).unwrap_or_default();
    let quiet_start = quiet_hours_start.or(current.quiet_hours_start);
    let quiet_end = quiet_hours_end.or(current.quiet_hours_end);

    let new_token = if token.trim().is_empty() {
        debug_log("save_settings: no new token provided, keeping existing");
        match current.token.as_deref() {
            Some(t) if !t.trim().is_empty() => {
                debug_log("save_settings: existing token retained");
                current.token.clone()
            }
            _ => {
                debug_log("save_settings: no existing token and none provided - error");
                return Err("Token is required".to_string());
            }
        }
    } else {
        debug_log(&format!(
            "save_settings: saving new token (len={})",
            token.trim().len()
        ));
        Some(token.trim().to_string())
    };

    save_non_secret_settings(
        app,
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

pub(crate) async fn test_connection(
    base_url: String,
    token: Option<String>,
) -> Result<String, String> {
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
            debug_log(&format!(
                "test_connection: loaded token from keychain len={}",
                t.len()
            ));
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

pub(crate) fn get_pause_state<R: Runtime>(app: &AppHandle<R>) -> Result<PauseStateResponse, String> {
    let settings = read_settings(app)?;
    Ok(PauseStateResponse {
        pause_until: settings.pause_until,
        pause_mode: settings.pause_mode,
    })
}

pub(crate) fn read_settings<R: Runtime>(app: &AppHandle<R>) -> Result<StoredSettings, String> {
    let path = settings_file(app)?;
    if !path.exists() {
        return Ok(StoredSettings::default());
    }

    let content =
        fs::read_to_string(path).map_err(|error| format!("Failed to read settings: {error}"))?;
    serde_json::from_str::<StoredSettings>(&content)
        .map_err(|error| format!("Failed to parse settings: {error}"))
}

pub(crate) fn save_non_secret_settings<R: Runtime>(
    app: &AppHandle<R>,
    settings: &StoredSettings,
) -> Result<(), String> {
    let path = settings_file(app)?;
    let content = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("Failed to serialize settings: {error}"))?;
    fs::write(&path, content).map_err(|error| format!("Failed to write settings: {error}"))?;
    restrict_file_permissions(&path);
    Ok(())
}

pub(crate) fn load_token() -> Result<Option<String>, String> {
    let path = get_settings_path()?;
    debug_log(&format!("load_token: reading settings from {path:?}"));
    if !path.exists() {
        debug_log("load_token: settings file not found - no token");
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

pub(crate) fn normalize_base_url(input: &str) -> Result<String, String> {
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

pub(crate) fn build_stream_ws_url(base_url: &str) -> Result<String, String> {
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
