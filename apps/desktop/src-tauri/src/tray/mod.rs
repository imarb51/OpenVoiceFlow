//! System tray icon, menu, and the Idle/Recording icon swap.

use crate::audio::{AudioRecorderHandle, RecordingErrorPayload, RecordingState};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

const IDLE_ICON_BYTES: &[u8] = include_bytes!("../../icons/tray-idle.png");
const RECORDING_ICON_BYTES: &[u8] = include_bytes!("../../icons/tray-recording.png");

/// Holds the tray icon and the menu item whose label changes with recording state,
/// managed as Tauri state so other parts of the app (the hotkey handler) can update
/// the tray without re-querying for it.
pub struct TrayHandle {
    tray: tauri::tray::TrayIcon,
    toggle_item: MenuItem<tauri::Wry>,
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let toggle_item = MenuItem::with_id(app, "toggle_recording", "Start Recording", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(
        app,
        &[&toggle_item, &separator, &settings_item, &separator, &quit_item],
    )?;

    let idle_icon = Image::from_bytes(IDLE_ICON_BYTES)?;

    let tray = TrayIconBuilder::with_id("main-tray")
        .icon(idle_icon)
        .menu(&menu)
        // Left-click summons the overlay instead of the menu (see
        // on_tray_icon_event below); right-click still shows the menu.
        .show_menu_on_left_click(false)
        .tooltip("OpenVoiceFlow — Ready")
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                crate::overlay::show(app);
                crate::overlay::schedule_hide(app);
            }
        })
        .on_menu_event(|app, event| match event.id.as_ref() {
            // Menu events run on the main thread, same as tray-icon click
            // events; building a new webview window synchronously from
            // there can deadlock waiting on its own async WebView2 setup
            // to pump through that same (blocked) main loop. Spawning it
            // onto Tauri's async runtime avoids that reentrancy.
            "settings" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::overlay::open_settings_window(&app) {
                        log::error!("failed to open settings window: {e}");
                    }
                });
            }
            "toggle_recording" => toggle_recording(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    app.manage(TrayHandle { tray, toggle_item });

    Ok(())
}

fn toggle_recording(app: &AppHandle) {
    let recorder = app.state::<AudioRecorderHandle>();
    let mut recorder = recorder.lock().unwrap();

    match recorder.state() {
        RecordingState::Idle => match recorder.start(app.clone()) {
            Ok(()) => set_recording_icon(app, true),
            Err(e) => {
                log::error!("failed to start recording from tray: {e}");
                let _ = app.emit(
                    "recording-error",
                    RecordingErrorPayload {
                        message: e.user_message(),
                    },
                );
                drop(recorder);
                crate::overlay::schedule_hide(app);
            }
        },
        RecordingState::Recording => match recorder.stop() {
            Ok(()) => set_recording_icon(app, false),
            Err(e) => log::error!("failed to stop recording from tray: {e}"),
        },
        _ => {}
    }
}

/// Swaps the tray icon and updates its tooltip/menu label. Called from the hotkey
/// handler and the tray's own menu handler so both input paths keep the tray in sync.
pub fn set_recording_icon(app: &AppHandle, recording: bool) {
    let Some(handle) = app.try_state::<TrayHandle>() else {
        return;
    };

    let icon_bytes = if recording {
        RECORDING_ICON_BYTES
    } else {
        IDLE_ICON_BYTES
    };

    if let Ok(icon) = Image::from_bytes(icon_bytes) {
        let _ = handle.tray.set_icon(Some(icon));
    }
    let _ = handle
        .tray
        .set_tooltip(Some(if recording {
            "OpenVoiceFlow — Listening…"
        } else {
            "OpenVoiceFlow — Ready"
        }));
    let _ = handle
        .toggle_item
        .set_text(if recording { "Stop Recording" } else { "Start Recording" });
}
