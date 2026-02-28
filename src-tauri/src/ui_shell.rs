use tauri::image::Image;
use tauri::{AppHandle, Manager, Runtime, WindowEvent};

pub(crate) fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(quick_window) = app.get_webview_window("quick") {
        let _ = quick_window.hide();
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub(crate) fn position_quick_window_under_tray<R: Runtime>(
    app: &AppHandle<R>,
    click_position: tauri::PhysicalPosition<f64>,
) {
    let Some(window) = app.get_webview_window("quick") else {
        return;
    };

    let window_size = window.outer_size().or_else(|_| window.inner_size());
    let Ok(window_size) = window_size else {
        return;
    };

    let width = window_size.width as f64;
    let height = window_size.height as f64;

    let mut x = click_position.x - (width / 2.0);
    let mut y = click_position.y + 10.0;

    let monitor_for_click = window
        .available_monitors()
        .ok()
        .and_then(|monitors| {
            monitors.into_iter().find(|monitor| {
                let work_area = monitor.work_area();
                let left = work_area.position.x as f64;
                let top = work_area.position.y as f64;
                let right = left + work_area.size.width as f64;
                let bottom = top + work_area.size.height as f64;
                click_position.x >= left
                    && click_position.x <= right
                    && click_position.y >= top
                    && click_position.y <= bottom
            })
        })
        .or_else(|| window.current_monitor().ok().flatten());

    if let Some(monitor) = monitor_for_click {
        let work_area = monitor.work_area();
        let left = work_area.position.x as f64;
        let top = work_area.position.y as f64;
        let right = left + work_area.size.width as f64;
        let bottom = top + work_area.size.height as f64;

        if x < left {
            x = left;
        }
        if x + width > right {
            x = (right - width).max(left);
        }
        if y + height > bottom {
            y = (click_position.y - height - 10.0).max(top);
        }
        if y < top {
            y = top;
        }
    }

    let _ = window.set_position(tauri::PhysicalPosition::new(
        x.round() as i32,
        y.round() as i32,
    ));
}

pub(crate) fn toggle_quick_window<R: Runtime>(
    app: &AppHandle<R>,
    tray_click_position: Option<tauri::PhysicalPosition<f64>>,
) {
    if let Some(window) = app.get_webview_window("quick") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            if let Some(position) = tray_click_position {
                position_quick_window_under_tray(app, position);
            }
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        return;
    }

    toggle_main_window(app);
}

pub(crate) fn toggle_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }
}

pub(crate) fn handle_window_event<R: Runtime>(window: &tauri::Window<R>, event: &WindowEvent) {
    if window.label() == "main" {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = window.hide();
        }
        return;
    }

    if window.label() == "quick" {
        match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            WindowEvent::Focused(false) => {
                let _ = window.hide();
            }
            _ => {}
        }
    }
}

pub(crate) fn tray_icon_for_status(status: &str) -> Option<Image<'static>> {
    let bytes = match status {
        "Connected" => include_bytes!("../icons/tray-connected.png").as_slice(),
        "Connecting" => include_bytes!("../icons/tray-connecting.png").as_slice(),
        "Backoff" => include_bytes!("../icons/tray-backoff.png").as_slice(),
        _ => include_bytes!("../icons/tray-disconnected.png").as_slice(),
    };
    Image::from_bytes(bytes).ok().map(|icon| icon.to_owned())
}
