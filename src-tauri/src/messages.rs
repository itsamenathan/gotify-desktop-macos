use std::{collections::HashMap, fs, path::PathBuf, thread};

use base64::Engine as _;
use tauri::{AppHandle, Emitter, Manager};

use crate::{
    debug_log, messages_file, truncate_message, unix_now_secs, AppState, ApplicationMeta,
    CachedMessage, GotifyApplicationWire, GotifyMessageListWire, GotifyMessageWire,
    APP_ICON_MAX_BYTES,
};

pub(crate) async fn fetch_recent_messages(
    app: &AppHandle,
    base_url: &str,
    token: &str,
) -> Result<(), String> {
    let cache_limit = crate::desired_cache_limit(app);
    let mut fresh = Vec::new();
    let mut since: Option<i64> = None;

    while fresh.len() < cache_limit {
        let remaining = cache_limit.saturating_sub(fresh.len());
        let limit = remaining.min(crate::MAX_API_PAGE_LIMIT);
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

    fresh.sort_by(crate::cached_message_cmp);
    fresh.dedup_by_key(|message| message.id);
    fresh.sort_by(crate::cached_message_cmp);
    if fresh.len() > cache_limit {
        fresh.truncate(cache_limit);
    }
    replace_message_cache(app, fresh)?;

    Ok(())
}

pub(crate) async fn fetch_applications(
    app: &AppHandle,
    base_url: &str,
    token: &str,
) -> Result<(), String> {
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
        .timeout(std::time::Duration::from_secs(
            crate::PREVIEW_REQUEST_TIMEOUT_SECS,
        ))
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

    #[cfg(target_os = "macos")]
    crate::notifications::warm_notification_icon_cache(app, &next_map);

    let state = app.state::<AppState>();
    let mut map = state
        .app_meta
        .lock()
        .map_err(|_| "Application map lock poisoned".to_string())?;
    *map = next_map;
    Ok(())
}

pub(crate) fn parse_stream_message(app: &AppHandle, text: &str) -> Option<CachedMessage> {
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

pub(crate) fn convert_wire_message(app: &AppHandle, message: GotifyMessageWire) -> CachedMessage {
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

pub(crate) fn resolve_app_meta(app: &AppHandle, app_id: i64) -> (String, Option<String>) {
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

pub(crate) fn cache_and_emit_message(
    app: &AppHandle,
    message: CachedMessage,
    allow_notification: bool,
) -> Result<(), String> {
    let app_state = app.state::<AppState>();
    let mut messages_guard = app_state
        .messages
        .lock()
        .map_err(|_| "Message cache lock poisoned".to_string())?;

    let mut existed = false;
    if let Some(pos) = messages_guard.iter().position(|m| m.id == message.id) {
        existed = true;
        messages_guard.remove(pos);
    }

    messages_guard.insert(0, message.clone());
    let cache_limit = crate::desired_cache_limit(app);
    if messages_guard.len() > cache_limit {
        messages_guard.truncate(cache_limit);
    }

    let cache_snapshot = messages_guard.clone();
    let cache_snapshot_for_persist = cache_snapshot.clone();
    let cache_path = messages_file(app)?;
    drop(messages_guard);

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
    let _ = app.emit("message-received", message.clone());
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.emit("message-received", message.clone());
    }
    if allow_notification && !existed {
        crate::notifications::maybe_notify_message(app, &message);
    }
    Ok(())
}

pub(crate) fn replace_message_cache(
    app: &AppHandle,
    fresh: Vec<CachedMessage>,
) -> Result<(), String> {
    let app_state = app.state::<AppState>();
    let cache_limit = crate::desired_cache_limit(app);
    let mut normalized = fresh;
    normalized.sort_by(crate::cached_message_cmp);
    normalized.dedup_by_key(|message| message.id);
    normalized.sort_by(crate::cached_message_cmp);
    if normalized.len() > cache_limit {
        normalized.truncate(cache_limit);
    }

    {
        let mut messages_guard = app_state
            .messages
            .lock()
            .map_err(|_| "Message cache lock poisoned".to_string())?;
        let changed = messages_guard.len() != normalized.len()
            || messages_guard.iter().zip(normalized.iter()).any(|(a, b)| {
                a.id != b.id
                    || a.date != b.date
                    || a.priority != b.priority
                    || a.title != b.title
                    || a.message != b.message
            });
        if !changed {
            return Ok(());
        }
        *messages_guard = normalized.clone();
    }

    persist_current_cache_async(app)?;
    let _ = app.emit("messages-synced", true);
    let _ = app.emit("messages-updated", normalized.clone());
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.emit("messages-updated", normalized.clone());
    }
    Ok(())
}

pub(crate) fn remove_message_from_cache(app: &AppHandle, message_id: i64) -> Result<(), String> {
    let app_state = app.state::<AppState>();
    let updated_snapshot;
    {
        let mut messages_guard = app_state
            .messages
            .lock()
            .map_err(|_| "Message cache lock poisoned".to_string())?;
        messages_guard.retain(|m| m.id != message_id);
        updated_snapshot = messages_guard.clone();
    }

    persist_current_cache_async(app)?;
    let _ = app.emit("messages-synced", true);
    let _ = app.emit("messages-updated", updated_snapshot.clone());
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.emit("messages-updated", updated_snapshot.clone());
    }
    Ok(())
}

pub(crate) fn persist_current_cache_async(app: &AppHandle) -> Result<(), String> {
    let app_state = app.state::<AppState>();
    let cache_snapshot = app_state
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

pub(crate) fn load_messages_from_disk(app: &AppHandle) -> Result<Vec<CachedMessage>, String> {
    let path = messages_file(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&path)
        .map_err(|error| format!("Failed to read message cache: {error}"))?;
    match serde_json::from_str::<Vec<CachedMessage>>(&content) {
        Ok(messages) => Ok(messages),
        Err(error) => {
            let backup_path =
                path.with_extension(format!("corrupt-{}.json", crate::unique_time_suffix()));
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

pub(crate) fn persist_messages_to_path(
    path: &PathBuf,
    messages: &[CachedMessage],
) -> Result<(), String> {
    let content = serde_json::to_string(messages)
        .map_err(|error| format!("Failed to serialize message cache: {error}"))?;
    let tmp_path = path.with_extension(format!("tmp-{}", crate::unique_time_suffix()));
    fs::write(&tmp_path, content)
        .map_err(|error| format!("Failed to write message cache temp file: {error}"))?;
    crate::restrict_file_permissions(&tmp_path);
    fs::rename(&tmp_path, path)
        .map_err(|error| format!("Failed to atomically replace message cache: {error}"))
}

pub(crate) fn resolve_application_image_url(
    base_url: &str,
    image_path: &str,
) -> Result<String, String> {
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

pub(crate) async fn resolve_application_image_data_url(
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
