//! macOS notification click handling.
//!
//! `mac-notification-sys` can only report clicks by blocking the sending thread
//! in a polling run loop (`wait_for_click`), which burns CPU forever whenever a
//! notification is never interacted with. Instead we deliver notifications
//! fire-and-forget and install our own long-lived
//! `NSUserNotificationCenterDelegate` on the app's main run loop. The delegate
//! receives `didActivateNotification:` for whichever notification the user
//! clicks, with no extra threads and no polling.
//!
//! `mac-notification-sys` replaces the notification center delegate on every
//! send, so [`reassert_delegate`] must run after each delivery.
//!
//! `NSUserNotification` is deprecated in favour of `UserNotifications.framework`,
//! which requires a signed bundle; `mac-notification-sys` already delivers through
//! the legacy API, so the click delegate has to speak the same one.
#![allow(deprecated)]

use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
};

use objc2::{
    define_class, msg_send,
    rc::{Allocated, Retained},
    runtime::ProtocolObject,
    AnyThread,
};
use objc2_foundation::{
    NSObject, NSObjectProtocol, NSUserNotification, NSUserNotificationActivationType,
    NSUserNotificationCenter, NSUserNotificationCenterDelegate,
};
use tauri::{AppHandle, Emitter};

use crate::{debug_log, settings::read_settings, ui_shell, CachedMessage};

/// How many delivered notifications we remember so a click can be mapped back
/// to the message that produced it.
const DELIVERED_HISTORY_LIMIT: usize = 64;

struct DeliveredNotification {
    title: String,
    subtitle: String,
    body: String,
    message: CachedMessage,
}

static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();
static DELIVERED: OnceLock<Mutex<VecDeque<DeliveredNotification>>> = OnceLock::new();

fn delivered() -> &'static Mutex<VecDeque<DeliveredNotification>> {
    DELIVERED.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Remembers the notification text we just handed to macOS so a later click can
/// be resolved back to its message.
pub(crate) fn remember_delivered(title: &str, subtitle: &str, body: &str, message: &CachedMessage) {
    let Ok(mut history) = delivered().lock() else {
        return;
    };
    history.push_back(DeliveredNotification {
        title: title.to_string(),
        subtitle: subtitle.to_string(),
        body: body.to_string(),
        message: message.clone(),
    });
    while history.len() > DELIVERED_HISTORY_LIMIT {
        history.pop_front();
    }
}

fn find_delivered(title: &str, subtitle: &str, body: &str) -> Option<CachedMessage> {
    let history = delivered().lock().ok()?;
    history
        .iter()
        .rev()
        .find(|entry| entry.title == title && entry.subtitle == subtitle && entry.body == body)
        .map(|entry| entry.message.clone())
}

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - `ClickDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[name = "GotifyNotificationClickDelegate"]
    #[ivars = ()]
    struct ClickDelegate;

    unsafe impl NSObjectProtocol for ClickDelegate {}

    unsafe impl NSUserNotificationCenterDelegate for ClickDelegate {
        #[unsafe(method(userNotificationCenter:didActivateNotification:))]
        #[allow(non_snake_case)]
        fn userNotificationCenter_didActivateNotification(
            &self,
            _center: &NSUserNotificationCenter,
            notification: &NSUserNotification,
        ) {
            handle_activation(notification);
        }
    }
);

impl ClickDelegate {
    fn new() -> Retained<Self> {
        let this: Allocated<Self> = Self::alloc();
        let this = this.set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// The delegate is stored unretained by the notification center, so the
/// instance is created once and intentionally leaked for the app's lifetime.
/// Only ever called from the main thread.
fn delegate() -> &'static ProtocolObject<dyn NSUserNotificationCenterDelegate> {
    static DELEGATE_PTR: OnceLock<usize> = OnceLock::new();
    let ptr = *DELEGATE_PTR.get_or_init(|| {
        let object: Retained<ProtocolObject<dyn NSUserNotificationCenterDelegate>> =
            ProtocolObject::from_retained(ClickDelegate::new());
        Retained::into_raw(object) as usize
    });
    // SAFETY: the pointer comes from a leaked `Retained`, so it stays valid.
    unsafe { &*(ptr as *const ProtocolObject<dyn NSUserNotificationCenterDelegate>) }
}

/// Installs (or re-installs) our delegate on the shared notification center.
///
/// Must be called after every `mac-notification-sys` delivery, because it
/// overwrites the delegate each time it sends.
pub(crate) fn reassert_delegate(app: &AppHandle) {
    let _ = APP_HANDLE.set(app.clone());
    let result = app.run_on_main_thread(|| {
        let center = NSUserNotificationCenter::defaultUserNotificationCenter();
        // SAFETY: the delegate is leaked, so it outlives the notification center's
        // unretained reference.
        unsafe {
            center.setDelegate(Some(delegate()));
        }
    });
    if let Err(error) = result {
        debug_log(&format!(
            "failed to install notification click delegate: {error}"
        ));
    }
}

fn handle_activation(notification: &NSUserNotification) {
    let activation_type = notification.activationType();
    let is_click = matches!(
        activation_type,
        NSUserNotificationActivationType::ContentsClicked
            | NSUserNotificationActivationType::ActionButtonClicked
    );
    if !is_click {
        return;
    }

    let Some(app) = APP_HANDLE.get() else {
        return;
    };

    let open_main_window = read_settings(app)
        .map(|settings| settings.open_main_window_on_notification_click)
        .unwrap_or(true);
    if !open_main_window {
        debug_log("mac notify click ignored: open-on-click disabled");
        return;
    }

    let title = notification
        .title()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let subtitle = notification
        .subtitle()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let body = notification
        .informativeText()
        .map(|value| value.to_string())
        .unwrap_or_default();

    let message = find_delivered(&title, &subtitle, &body);
    debug_log(&format!(
        "mac notify click id={}",
        message
            .as_ref()
            .map(|message| message.id.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    ));

    ui_shell::show_main_window(app);
    if let Some(message) = message {
        let _ = app.emit_to("main", "notification-clicked", message.clone());
        let _ = app.emit_to("quick", "notification-clicked", message);
    }
}
