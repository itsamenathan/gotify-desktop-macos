use serde::Serialize;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{ipc::Channel, AppHandle, Manager};

use crate::{
    debug_log, AppState, CachedMessage, RevisionKey, RuntimeDiagnostics, SettingsResponse,
};

#[derive(Debug, Serialize, Clone)]
pub(crate) struct DomainSnapshot<T> {
    pub(crate) revision: u64,
    pub(crate) updated_at_ms: u64,
    pub(crate) data: T,
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct PauseStateData {
    pub(crate) pause_until: Option<u64>,
    pub(crate) pause_mode: Option<String>,
    pub(crate) is_active: bool,
    pub(crate) remaining_sec: u64,
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct ConnectionStateData {
    pub(crate) state: String,
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct MessageRemovedData {
    pub(crate) message_id: i64,
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct StreamErrorData {
    pub(crate) message: String,
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct BootstrapState {
    pub(crate) settings: DomainSnapshot<SettingsResponse>,
    pub(crate) pause: DomainSnapshot<PauseStateData>,
    pub(crate) messages: DomainSnapshot<Vec<CachedMessage>>,
    pub(crate) connection: DomainSnapshot<ConnectionStateData>,
    pub(crate) runtime: DomainSnapshot<RuntimeDiagnostics>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", content = "payload")]
pub(crate) enum AppUpdate {
    #[serde(rename = "settings.updated")]
    SettingsUpdated(DomainSnapshot<SettingsResponse>),
    #[serde(rename = "pause.updated")]
    PauseUpdated(DomainSnapshot<PauseStateData>),
    #[serde(rename = "messages.replace")]
    MessagesReplace(DomainSnapshot<Vec<CachedMessage>>),
    #[serde(rename = "messages.upsert")]
    MessagesUpsert(DomainSnapshot<CachedMessage>),
    #[serde(rename = "messages.remove")]
    MessagesRemove(DomainSnapshot<MessageRemovedData>),
    #[serde(rename = "connection.updated")]
    ConnectionUpdated(DomainSnapshot<ConnectionStateData>),
    #[serde(rename = "runtime.updated")]
    RuntimeUpdated(DomainSnapshot<RuntimeDiagnostics>),
    #[serde(rename = "stream.error")]
    StreamError(DomainSnapshot<StreamErrorData>),
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn current_revision(app: &AppHandle, key: RevisionKey) -> u64 {
    let state = app.state::<AppState>();
    state
        .revisions
        .lock()
        .map(|revisions| revisions.current(key))
        .unwrap_or(1)
}

fn bump_revision(app: &AppHandle, key: RevisionKey) -> u64 {
    let state = app.state::<AppState>();
    state
        .revisions
        .lock()
        .map(|mut revisions| revisions.bump(key))
        .unwrap_or_else(|_| {
            debug_log("revision lock poisoned; falling back to current time revision");
            now_ms()
        })
}

pub(crate) fn snapshot_at_revision<T>(revision: u64, data: T) -> DomainSnapshot<T> {
    DomainSnapshot {
        revision,
        updated_at_ms: now_ms(),
        data,
    }
}

pub(crate) fn snapshot_with_bump<T>(
    app: &AppHandle,
    key: RevisionKey,
    data: T,
) -> DomainSnapshot<T> {
    snapshot_at_revision(bump_revision(app, key), data)
}

pub(crate) fn register_app_update_channel(
    app: &AppHandle,
    label: &str,
    channel: Channel<Value>,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut channels = state
        .update_channels
        .lock()
        .map_err(|_| "Update channel lock poisoned".to_string())?;
    channels.insert(label.to_string(), channel);
    Ok(())
}

pub(crate) fn unregister_app_update_channel(app: &AppHandle, label: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut channels = state
        .update_channels
        .lock()
        .map_err(|_| "Update channel lock poisoned".to_string())?;
    channels.remove(label);
    Ok(())
}

pub(crate) fn publish_update(app: &AppHandle, update: AppUpdate) {
    let payload = match serde_json::to_value(&update) {
        Ok(payload) => payload,
        Err(error) => {
            debug_log(&format!("failed to serialize app update: {error}"));
            return;
        }
    };

    let state = app.state::<AppState>();
    let channels_snapshot = match state.update_channels.lock() {
        Ok(channels) => channels
            .iter()
            .map(|(label, channel)| (label.clone(), channel.clone()))
            .collect::<Vec<_>>(),
        Err(_) => {
            debug_log("update channel lock poisoned");
            return;
        }
    };

    if channels_snapshot.is_empty() {
        return;
    }

    let mut failed_labels = Vec::new();
    for (label, channel) in channels_snapshot {
        if let Err(error) = channel.send(payload.clone()) {
            debug_log(&format!("failed to send app update to {label}: {error}"));
            failed_labels.push(label);
        }
    }

    if failed_labels.is_empty() {
        return;
    }

    {
        if let Ok(mut channels) = state.update_channels.lock() {
            for label in failed_labels {
                channels.remove(&label);
            }
        };
    }
}

pub(crate) fn publish_settings_update(
    app: &AppHandle,
    settings: SettingsResponse,
) -> DomainSnapshot<SettingsResponse> {
    let snapshot = snapshot_with_bump(app, RevisionKey::Settings, settings);
    publish_update(app, AppUpdate::SettingsUpdated(snapshot.clone()));
    snapshot
}

pub(crate) fn publish_pause_update(
    app: &AppHandle,
    pause: PauseStateData,
) -> DomainSnapshot<PauseStateData> {
    let snapshot = snapshot_with_bump(app, RevisionKey::Pause, pause);
    publish_update(app, AppUpdate::PauseUpdated(snapshot.clone()));
    snapshot
}

pub(crate) fn publish_messages_replace(
    app: &AppHandle,
    messages: Vec<CachedMessage>,
) -> DomainSnapshot<Vec<CachedMessage>> {
    let snapshot = snapshot_with_bump(app, RevisionKey::Messages, messages);
    publish_update(app, AppUpdate::MessagesReplace(snapshot.clone()));
    snapshot
}

pub(crate) fn publish_message_upsert(
    app: &AppHandle,
    message: CachedMessage,
) -> DomainSnapshot<CachedMessage> {
    let snapshot = snapshot_with_bump(app, RevisionKey::Messages, message);
    publish_update(app, AppUpdate::MessagesUpsert(snapshot.clone()));
    snapshot
}

pub(crate) fn publish_message_remove(
    app: &AppHandle,
    message_id: i64,
) -> DomainSnapshot<MessageRemovedData> {
    let snapshot = snapshot_with_bump(
        app,
        RevisionKey::Messages,
        MessageRemovedData { message_id },
    );
    publish_update(app, AppUpdate::MessagesRemove(snapshot.clone()));
    snapshot
}

pub(crate) fn publish_connection_update(
    app: &AppHandle,
    state: String,
) -> DomainSnapshot<ConnectionStateData> {
    let snapshot = snapshot_with_bump(app, RevisionKey::Connection, ConnectionStateData { state });
    publish_update(app, AppUpdate::ConnectionUpdated(snapshot.clone()));
    snapshot
}

pub(crate) fn publish_runtime_update(
    app: &AppHandle,
    runtime: RuntimeDiagnostics,
) -> DomainSnapshot<RuntimeDiagnostics> {
    let snapshot = snapshot_with_bump(app, RevisionKey::Runtime, runtime);
    publish_update(app, AppUpdate::RuntimeUpdated(snapshot.clone()));
    snapshot
}

pub(crate) fn publish_stream_error(
    app: &AppHandle,
    message: String,
) -> DomainSnapshot<StreamErrorData> {
    let snapshot = snapshot_with_bump(app, RevisionKey::StreamError, StreamErrorData { message });
    publish_update(app, AppUpdate::StreamError(snapshot.clone()));
    snapshot
}
