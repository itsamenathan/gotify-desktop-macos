#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::{atomic::AtomicU64, OnceLock},
    time::Duration,
};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl};

mod consts;
mod diagnostics;
use diagnostics::RuntimeDiagnostics;
mod core;
mod messages;
mod model;
mod notifications;
mod pause;
mod preview;
mod settings;
mod stream;
mod ui_shell;
pub(crate) use consts::*;
pub(crate) use core::{
    debug_log, decode_data_url_bytes, emit_delete_debug, get_settings_path, messages_file,
    redact_ws_url, restrict_file_permissions, settings_file, truncate_message, unique_time_suffix,
    unix_now_secs,
};
pub(crate) use model::{
    AppState, ApplicationMeta, CachedMessage, GotifyApplicationWire, GotifyMessageListWire,
    GotifyMessageWire, TrayPauseMenuState, UrlPreview,
};
use settings::{
    get_pause_state as get_pause_state_impl, load_settings as load_settings_impl, load_token,
    normalize_base_url, read_settings, save_settings as save_settings_impl,
    test_connection as test_connection_impl, PauseStateResponse, PriorityColorMode,
    PriorityGradient, PriorityThreshold, SettingsResponse,
};

/// Resolved at startup; must be set before any `load_settings` / `save_settings` call.
static SETTINGS_FILE: OnceLock<PathBuf> = OnceLock::new();
/// Monotonic counter for generating unique temp/backup file suffixes.
static FILE_SUFFIX_COUNTER: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
fn load_settings(app: AppHandle) -> Result<SettingsResponse, String> {
    load_settings_impl(&app)
}

#[tauri::command]
fn save_settings(
    app: AppHandle,
    base_url: String,
    token: String,
    min_priority: Option<i64>,
    priority_color_mode: Option<PriorityColorMode>,
    priority_thresholds: Option<Vec<PriorityThreshold>>,
    priority_gradient: Option<PriorityGradient>,
    cache_limit: Option<usize>,
    launch_at_login: Option<bool>,
    start_minimized_to_tray: Option<bool>,
    quiet_hours_start: Option<u8>,
    quiet_hours_end: Option<u8>,
) -> Result<(), String> {
    save_settings_impl(
        &app,
        base_url,
        token,
        min_priority,
        priority_color_mode,
        priority_thresholds,
        priority_gradient,
        cache_limit,
        launch_at_login,
        start_minimized_to_tray,
        quiet_hours_start,
        quiet_hours_end,
    )
}

#[tauri::command]
async fn test_connection(base_url: String, token: Option<String>) -> Result<String, String> {
    test_connection_impl(base_url, token).await
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
    get_pause_state_impl(&app)
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

    messages::remove_message_from_cache(&app, message_id)?;
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
                    messages::fetch_recent_messages(&app_for_sync, &base_for_sync, &token_value)
                        .await
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
    stream::start_stream(app, token)
}

#[tauri::command]
fn stop_stream(app: AppHandle) -> Result<(), String> {
    stream::stop_stream(app)
}

#[tauri::command]
fn get_connection_state(app: AppHandle) -> Result<String, String> {
    stream::get_connection_state(app)
}

#[tauri::command]
fn get_runtime_diagnostics(app: AppHandle) -> Result<RuntimeDiagnostics, String> {
    stream::get_runtime_diagnostics(app)
}

#[tauri::command]
fn recover_stream(app: AppHandle) -> Result<(), String> {
    stream::recover_stream(app)
}

#[tauri::command]
fn restart_stream(app: AppHandle) -> Result<(), String> {
    stream::restart_stream(app)
}

#[tauri::command]
fn pause_notifications(app: AppHandle, minutes: u64) -> Result<(), String> {
    pause::pause_notifications(app, minutes)
}

#[tauri::command]
fn pause_notifications_forever(app: AppHandle) -> Result<(), String> {
    pause::pause_notifications_forever(app)
}

#[tauri::command]
fn resume_notifications(app: AppHandle) -> Result<(), String> {
    pause::resume_notifications(app)
}

#[tauri::command]
async fn fetch_url_preview(url: String) -> Result<UrlPreview, String> {
    preview::fetch_url_preview(url).await
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

fn main() {
    debug_log("═══════════════════════════════════════");
    debug_log(&format!(
        "gotify-desktop starting (pid={})",
        std::process::id()
    ));
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

            // Resolve and register the settings path before any settings/token access.
            let config_dir = app
                .path()
                .app_config_dir()
                .map_err(|error| format!("Failed to resolve app config dir: {error}"))?;
            fs::create_dir_all(&config_dir)
                .map_err(|error| format!("Failed to create config directory: {error}"))?;
            // Register settings.json in OnceLock so helper paths can resolve it globally.
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
                load_token().map_or_else(
                    |e| format!("err:{e}"),
                    |t| t.map_or("none".into(), |_| "yes".into())
                )
            ));
            #[cfg(target_os = "macos")]
            if let Err(error) = apply_launch_at_login(startup_settings.launch_at_login) {
                debug_log(&format!("failed to configure launch at login: {error}"));
            }

            let existing_messages = messages::load_messages_from_disk(app.handle())?;
            let app_state = app.state::<AppState>();
            if let Ok(mut messages_guard) = app_state.messages.lock() {
                *messages_guard = existing_messages;
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
                ui_shell::show_main_window(app.handle());
            }

            let pause_items = pause::create_pause_menu_items(app.handle())?;
            let open_item = MenuItem::with_id(app, "open_inbox", "Open Inbox", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &pause_items.status_item,
                    &open_item,
                    &pause_items.pause_15m_item,
                    &pause_items.pause_1h_item,
                    &pause_items.pause_forever_item,
                    &pause_items.resume_item,
                    &quit_item,
                ],
            )?;
            pause::install_pause_menu_state(
                app.handle(),
                &pause_items,
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
                        ui_shell::toggle_quick_window(tray.app_handle(), Some(position));
                    }
                })
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "open_inbox" => {
                        ui_shell::show_main_window(app);
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
                        let _ = stream::stop_stream(app.clone());
                        app.exit(0);
                    }
                    _ => {}
                });
            if let Some(icon) = ui_shell::tray_icon_for_status("Disconnected")
                .or_else(|| app.default_window_icon().cloned())
            {
                tray_builder = tray_builder.icon(icon);
            }
            tray_builder.build(app)?;

            let app_for_pause_refresh = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    pause::refresh_pause_state_from_settings(&app_for_pause_refresh);
                }
            });

            match stream::start_stream(app.handle().clone(), None) {
                Ok(_) => {}
                Err(error) => {
                    let _ = app.emit("connection-error", format!("Auto-connect failed: {error}"));
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| ui_shell::handle_window_event(window, event))
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
