use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
};

use chrono::Timelike;
#[cfg(target_os = "macos")]
use mac_notification_sys::{MainButton, Notification, NotificationResponse};
use tauri::{AppHandle, Emitter, Manager};

use crate::{
    debug_log, decode_data_url_bytes, settings::read_settings, truncate_message, ui_shell,
    unix_now_secs, AppState, ApplicationMeta, CachedMessage, APP_ICON_MAX_BYTES,
    PAUSE_FOREVER_SENTINEL,
};

pub(crate) fn maybe_notify_message(app: &AppHandle, message: &CachedMessage) {
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

pub(crate) fn is_quiet_hours(start: Option<u8>, end: Option<u8>) -> bool {
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
pub(crate) fn send_macos_notification(app: AppHandle, message: CachedMessage) {
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
            notification.app_icon(sender_icon_path);
        }

        let content_image_path = resolve_notification_content_image_path(&app, &message);
        if let Some(content_image_path) = content_image_path.as_deref() {
            notification.content_image(content_image_path);
        }

        match notification.send() {
            Ok(NotificationResponse::Click) | Ok(NotificationResponse::ActionButton(_)) => {
                ui_shell::show_main_window(&app);
                let _ = app.emit("notification-clicked", message.clone());
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.emit("notification-clicked", message);
                }
            }
            Ok(_) => {}
            Err(error) => {
                debug_log(&format!("failed to show macOS notification: {error}"));
            }
        }
    });
}

#[cfg(target_os = "macos")]
pub(crate) fn ensure_macos_notification_application() {
    static INIT_NOTIFICATION_APP: std::sync::Once = std::sync::Once::new();
    INIT_NOTIFICATION_APP.call_once(|| {
        for bundle_id in [
            "net.gotify.desktop",
            "com.apple.Terminal",
            "com.apple.Finder",
        ] {
            match mac_notification_sys::set_application(bundle_id) {
                Ok(_) => {
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
pub(crate) fn resolve_notification_content_image_path(
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
pub(crate) fn resolve_default_notification_app_icon_path(app: &AppHandle) -> Option<String> {
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
pub(crate) fn cache_remote_notification_icon_png(
    app: &AppHandle,
    app_id: i64,
    icon_url: &str,
) -> Option<String> {
    let icons_dir = notification_icon_cache_dir(app)?;
    let file_path = icons_dir.join(format!("app-{app_id}.png"));
    if file_path.exists() {
        return Some(file_path.to_string_lossy().to_string());
    }

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

    None
}

#[cfg(target_os = "macos")]
pub(crate) fn notification_icon_cache_dir(app: &AppHandle) -> Option<PathBuf> {
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
pub(crate) fn ensure_icns_from_png(source_png: &Path, target_icns: &Path) -> bool {
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

#[cfg(target_os = "macos")]
pub(crate) fn warm_notification_icon_cache(
    app: &AppHandle,
    app_meta: &HashMap<i64, ApplicationMeta>,
) {
    for (app_id, meta) in app_meta {
        if meta.icon_url.trim().is_empty() {
            continue;
        }
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
