//! The floating overlay ("main" window): a small always-on-top pill parked
//! above the taskbar. It shows on hotkey press, displays a live waveform
//! while recording, and auto-hides after `HIDE_DELAY` of inactivity. The
//! Settings window (hotkey rebinding + transcript manager) is created on
//! demand, e.g. from a right-click on the overlay.

use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, LogicalPosition, Manager, WebviewUrl, WebviewWindowBuilder};

const HIDE_DELAY: Duration = Duration::from_secs(10);
const OVERLAY_WIDTH: f64 = 392.0;
const OVERLAY_HEIGHT: f64 = 64.0;
/// Approximate clearance above the taskbar. Tauri has no direct "taskbar
/// bounds" API, so this is a fixed margin rather than an exact measurement.
const BOTTOM_MARGIN: f64 = 72.0;

pub struct OverlayState {
    last_activity: Mutex<Instant>,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self {
            last_activity: Mutex::new(Instant::now()),
        }
    }
}

fn touch(app: &AppHandle) {
    if let Some(state) = app.try_state::<OverlayState>() {
        *state.last_activity.lock().unwrap() = Instant::now();
    }
}

/// Repositions the overlay to bottom-center of its current monitor (falling
/// back to the primary monitor). Works in logical units so it lands
/// correctly regardless of display scaling.
pub fn position_overlay(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };

    let scale = monitor.scale_factor();
    let monitor_pos = monitor.position();
    let monitor_size = monitor.size();

    let logical_width = monitor_size.width as f64 / scale;
    let logical_height = monitor_size.height as f64 / scale;
    let logical_x = monitor_pos.x as f64 / scale;
    let logical_y = monitor_pos.y as f64 / scale;

    // The React HUD can grow beyond the compact startup pill when it shows a
    // task, result, or approval card. Read the live native size on every
    // show so a re-shown overlay remains fully above the taskbar even when
    // its content did not change since it was last hidden.
    let (overlay_width, overlay_height) = window
        .inner_size()
        .map(|size| (size.width as f64 / scale, size.height as f64 / scale))
        .unwrap_or((OVERLAY_WIDTH, OVERLAY_HEIGHT));

    let x = logical_x + (logical_width - overlay_width) / 2.0;
    let y = logical_y + logical_height - overlay_height - BOTTOM_MARGIN;

    let _ = window.set_position(LogicalPosition::new(x, y));
}

/// Makes the overlay visible (repositioning it first) without stealing
/// focus from whatever window the user is dictating into.
pub fn show(app: &AppHandle) {
    touch(app);
    position_overlay(app);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
    }
}

/// Starts (or restarts) the idle-hide countdown. Only ever called when the
/// overlay should eventually fade — never while actively recording, so
/// holding the hotkey for longer than `HIDE_DELAY` never hides it mid-flight.
pub fn schedule_hide(app: &AppHandle) {
    touch(app);
    let app = app.clone();
    thread::spawn(move || {
        thread::sleep(HIDE_DELAY);
        let Some(state) = app.try_state::<OverlayState>() else {
            return;
        };
        let elapsed = state.last_activity.lock().unwrap().elapsed();
        // If a newer `show`/`schedule_hide` bumped activity since this
        // thread started sleeping, its own delayed check will hide the
        // window later — this one is stale and should do nothing.
        if elapsed >= HIDE_DELAY {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.hide();
            }
        }
    });
}

/// Shows the Settings window (hotkey rebinding + transcript manager),
/// creating it on first use and focusing it if it already exists. Callers
/// must not invoke this directly from the main thread (a tray/menu event
/// handler, for instance) — building a new webview window synchronously
/// there can deadlock waiting on its own async WebView2 setup to pump
/// through that same (blocked) main loop. Route through an async command
/// or `tauri::async_runtime::spawn` instead (see `tray/mod.rs`).
pub fn open_settings_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("settings") {
        window.show()?;
        window.set_focus()?;
        return Ok(());
    }

    WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("OpenVoiceFlow Settings")
        .inner_size(560.0, 440.0)
        .min_inner_size(420.0, 360.0)
        .resizable(true)
        .center()
        .build()?;

    Ok(())
}
