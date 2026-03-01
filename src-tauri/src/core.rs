use base64::Engine as _;
use serde::Serialize;
#[cfg(debug_assertions)]
use std::io::Write as _;
use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::{FILE_SUFFIX_COUNTER, SETTINGS_FILE};

#[derive(Debug, Serialize, Clone)]
pub(crate) struct DeleteMessageDebugEvent {
    pub(crate) at: u64,
    pub(crate) message_id: i64,
    pub(crate) phase: String,
    pub(crate) detail: String,
    pub(crate) status: Option<u16>,
}

pub(crate) fn settings_file<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("Failed to resolve app config dir: {error}"))?;

    fs::create_dir_all(&config_dir)
        .map_err(|error| format!("Failed to create config directory: {error}"))?;

    Ok(config_dir.join("settings.json"))
}

pub(crate) fn messages_file<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("Failed to resolve app config dir: {error}"))?;

    fs::create_dir_all(&config_dir)
        .map_err(|error| format!("Failed to create config directory: {error}"))?;

    Ok(config_dir.join("messages.json"))
}

pub(crate) fn restrict_file_permissions(path: &Path) {
    if path.exists() {
        if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            debug_log(&format!(
                "restrict_file_permissions: failed for {path:?}: {error}"
            ));
        }
    }
}

pub(crate) fn decode_data_url_bytes(data_url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
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

pub(crate) fn redact_ws_url(url: &str) -> String {
    let mut parsed = match reqwest::Url::parse(url) {
        Ok(url) => url,
        Err(_) => return "<invalid-url>".to_string(),
    };
    if parsed.query().is_some() {
        parsed.set_query(Some("token=***"));
    }
    parsed.to_string()
}

pub(crate) fn truncate_message(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }

    let truncated: String = input.chars().take(max_chars).collect();
    format!("{truncated}...")
}

pub(crate) fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(crate) fn unique_time_suffix() -> u64 {
    FILE_SUFFIX_COUNTER.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn debug_log(message: &str) {
    #[cfg(not(debug_assertions))]
    let _ = message;
    #[cfg(debug_assertions)]
    {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let line = format!("[gotify-desktop][{ts}] {message}\n");
        eprint!("{line}");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/gotify-desktop.log")
        {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

pub(crate) fn emit_delete_debug(
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

pub(crate) fn get_settings_path() -> Result<&'static PathBuf, String> {
    SETTINGS_FILE
        .get()
        .ok_or_else(|| "Settings path not initialised (setup not complete)".to_string())
}
