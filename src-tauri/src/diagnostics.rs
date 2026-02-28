use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::{debug_log, unix_now_secs, AppState};

#[derive(Debug, Serialize, Clone)]
pub(crate) struct RuntimeDiagnostics {
    pub(crate) connection_state: String,
    pub(crate) should_run: bool,
    pub(crate) last_connected_at: Option<u64>,
    pub(crate) last_stream_event_at: Option<u64>,
    pub(crate) last_message_at: Option<u64>,
    pub(crate) last_message_id: Option<i64>,
    pub(crate) stale_for_seconds: Option<u64>,
    pub(crate) last_error: Option<String>,
    pub(crate) backoff_seconds: u64,
    pub(crate) reconnect_attempts: u64,
}

pub(crate) fn snapshot_runtime(app: &AppHandle) -> Result<RuntimeDiagnostics, String> {
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

pub(crate) fn emit_runtime_diagnostics(app: &AppHandle) {
    match snapshot_runtime(app) {
        Ok(diag) => {
            let _ = app.emit("runtime-diagnostics", diag.clone());
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.emit("runtime-diagnostics", diag);
            }
        }
        Err(err) => {
            debug_log(&format!("failed to snapshot runtime: {err}"));
        }
    }
}

pub(crate) fn mark_stream_activity(app: &AppHandle, at: u64, _source: &str) {
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut runtime) = state.runtime.lock() {
            runtime.last_stream_event_at = Some(at);
        }
    }
    emit_runtime_diagnostics(app);
}
