use tauri::menu::MenuItem;
use tauri::{AppHandle, Emitter, Manager};

use crate::{
    debug_log, settings::read_settings, settings::save_non_secret_settings, unix_now_secs,
    AppState, TrayPauseMenuState, PAUSE_FOREVER_SENTINEL, PAUSE_MODE_15M, PAUSE_MODE_1H,
    PAUSE_MODE_CUSTOM, PAUSE_MODE_FOREVER,
};

pub(crate) struct PauseMenuItems {
    pub(crate) status_item: MenuItem<tauri::Wry>,
    pub(crate) pause_15m_item: MenuItem<tauri::Wry>,
    pub(crate) pause_1h_item: MenuItem<tauri::Wry>,
    pub(crate) pause_forever_item: MenuItem<tauri::Wry>,
    pub(crate) resume_item: MenuItem<tauri::Wry>,
}

pub(crate) fn create_pause_menu_items(app: &AppHandle) -> Result<PauseMenuItems, tauri::Error> {
    let status_item = MenuItem::with_id(
        app,
        "pause_status",
        "Notifications: On",
        false,
        None::<&str>,
    )?;
    let pause_15m_item = MenuItem::with_id(app, "pause_15m", "Pause 15m", true, None::<&str>)?;
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

    Ok(PauseMenuItems {
        status_item,
        pause_15m_item,
        pause_1h_item,
        pause_forever_item,
        resume_item,
    })
}

pub(crate) fn install_pause_menu_state(
    app: &AppHandle,
    items: &PauseMenuItems,
    pause_until: Option<u64>,
    pause_mode: Option<&str>,
) {
    let state = app.state::<AppState>();
    if let Ok(mut tray_pause_menu_lock) = state.tray_pause_menu.lock() {
        *tray_pause_menu_lock = Some(TrayPauseMenuState {
            status_item: items.status_item.clone(),
            pause_15m_item: items.pause_15m_item.clone(),
            pause_1h_item: items.pause_1h_item.clone(),
            pause_forever_item: items.pause_forever_item.clone(),
            resume_item: items.resume_item.clone(),
        });
    }
    apply_pause_state_to_tray(app, pause_until, pause_mode);
}

pub(crate) fn pause_notifications(app: AppHandle, minutes: u64) -> Result<(), String> {
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

pub(crate) fn pause_notifications_forever(app: AppHandle) -> Result<(), String> {
    set_notification_pause_until(&app, Some(PAUSE_FOREVER_SENTINEL), Some(PAUSE_MODE_FOREVER))
}

pub(crate) fn resume_notifications(app: AppHandle) -> Result<(), String> {
    set_notification_pause_until(&app, None, None)
}

pub(crate) fn set_notification_pause_until(
    app: &AppHandle,
    pause_until: Option<u64>,
    pause_mode: Option<&str>,
) -> Result<(), String> {
    let mut settings = read_settings(app)?;
    settings.pause_until = pause_until;
    settings.pause_mode = pause_mode.map(|mode| mode.to_string());
    save_non_secret_settings(app, &settings)?;
    apply_pause_state_to_tray(app, pause_until, settings.pause_mode.as_deref());
    emit_pause_state_events(app, pause_until, settings.pause_mode.as_deref());

    Ok(())
}

pub(crate) fn refresh_pause_state_from_settings(app: &AppHandle) {
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
