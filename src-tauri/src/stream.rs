use futures_util::{SinkExt, StreamExt};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::watch;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
};

use crate::{
    debug_log,
    diagnostics::{
        emit_runtime_diagnostics, mark_stream_activity, snapshot_runtime, RuntimeDiagnostics,
    },
    messages, redact_ws_url,
    settings::{build_stream_ws_url, load_token, normalize_base_url, read_settings},
    truncate_message, unix_now_secs, AppState, STREAM_CONNECT_TIMEOUT_SECS,
    STREAM_LIVENESS_CHECK_INTERVAL_SECS, STREAM_LIVENESS_IDLE_SECS,
    STREAM_LIVENESS_PING_GRACE_SECS, STREAM_SYNC_INTERVAL_SECS,
};

pub(crate) fn start_stream(app: AppHandle, token: Option<String>) -> Result<(), String> {
    start_stream_internal(app, token)
}

pub(crate) fn stop_stream(app: AppHandle) -> Result<(), String> {
    stop_stream_internal(&app)
}

pub(crate) fn get_connection_state(app: AppHandle) -> Result<String, String> {
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

pub(crate) fn get_runtime_diagnostics(app: AppHandle) -> Result<RuntimeDiagnostics, String> {
    snapshot_runtime(&app)
}

pub(crate) fn recover_stream(app: AppHandle) -> Result<(), String> {
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

pub(crate) fn restart_stream(app: AppHandle) -> Result<(), String> {
    let _ = stop_stream_internal(&app);
    start_stream_internal(app, None)
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
            let app_for_prefetch = app_for_task.clone();
            let base_url_for_prefetch = base_url.clone();
            let token_for_prefetch = token.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = messages::fetch_applications(
                    &app_for_prefetch,
                    &base_url_for_prefetch,
                    &token_for_prefetch,
                )
                .await
                {
                    debug_log(&format!("failed to fetch applications: {error}"));
                }
                if let Err(error) = messages::fetch_recent_messages(
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

                let jitter_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| (d.subsec_millis() % 500) as u64)
                    .unwrap_or(0);

                tokio::time::sleep(
                    std::time::Duration::from_secs(backoff_secs)
                        + std::time::Duration::from_millis(jitter_ms),
                )
                .await;
                backoff_secs = std::cmp::min(backoff_secs.saturating_mul(2), 30);
            }
        }
    }

    let state = app.state::<AppState>();
    if let Ok(mut runtime) = state.runtime.lock() {
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
        std::time::Duration::from_secs(STREAM_CONNECT_TIMEOUT_SECS),
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
    let mut sync_interval =
        tokio::time::interval(std::time::Duration::from_secs(STREAM_SYNC_INTERVAL_SECS));
    sync_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    sync_interval.tick().await;
    let mut liveness_interval = tokio::time::interval(std::time::Duration::from_secs(
        STREAM_LIVENESS_CHECK_INTERVAL_SECS,
    ));
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
                        mark_stream_activity(app, event_now, "ws-text");
                        debug_log(&format!("ws text frame bytes={}", text.len()));
                        if let Some(msg) = messages::parse_stream_message(app, text.as_ref()) {
                            let _ = messages::cache_and_emit_message(app, msg, true);
                        } else {
                            debug_log(&format!("ws text parse miss: {}", truncate_message(text.as_ref(), 140)));
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let event_now = unix_now_secs();
                        last_activity_at = event_now;
                        pending_ping_since = None;
                        mark_stream_activity(app, event_now, "ws-ping");
                        ws_stream.send(Message::Pong(payload)).await
                            .map_err(|error| format!("Failed to send pong: {error}"))?;
                    }
                    Some(Ok(Message::Pong(_))) => {
                        let event_now = unix_now_secs();
                        last_activity_at = event_now;
                        pending_ping_since = None;
                        mark_stream_activity(app, event_now, "ws-pong");
                    }
                    Some(Ok(Message::Close(_))) => {
                        return Err("Stream closed by server".to_string());
                    }
                    Some(Ok(_)) => {
                        let event_now = unix_now_secs();
                        last_activity_at = event_now;
                        pending_ping_since = None;
                        mark_stream_activity(app, event_now, "ws-other");
                    }
                    Some(Err(error)) => return Err(format!("Stream read error: {error}")),
                    None => return Err("Stream ended unexpectedly".to_string()),
                }
            }
            _ = sync_interval.tick() => {
                let app_for_sync = app.clone();
                let base_for_sync = base_url.to_string();
                let token_for_sync = token.to_string();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = messages::fetch_recent_messages(&app_for_sync, &base_for_sync, &token_for_sync).await {
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
        let _ = tray.set_icon(crate::ui_shell::tray_icon_for_status(status));
    }
    emit_runtime_diagnostics(app);
}
