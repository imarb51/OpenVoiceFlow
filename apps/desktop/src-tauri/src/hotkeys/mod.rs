//! Push-to-talk global shortcut handling.
//!
//! The binding is user-configurable (persisted via `crate::config`) and can be
//! changed at runtime through `set_shortcut` without restarting the app.
//! `F8` is documented here as a fallback suggestion for the Settings UI, since
//! Ctrl-based combos can collide with OS/IME shortcuts (especially on macOS).

use std::sync::{Arc, Mutex};

use crate::audio::{AudioRecorderHandle, RecordingErrorPayload, RecordingState};
use crate::{overlay, tray};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

pub const DEFAULT_SHORTCUT: &str = "Ctrl+Space";
#[allow(dead_code)]
pub const FALLBACK_SHORTCUT: &str = "F8";

pub struct HotkeyManager {
    shortcut: String,
}

pub type HotkeyManagerHandle = Arc<Mutex<HotkeyManager>>;

impl HotkeyManager {
    pub fn new(shortcut: String) -> Self {
        Self { shortcut }
    }

    pub fn shortcut(&self) -> &str {
        &self.shortcut
    }
}

/// Registers the initial push-to-talk shortcut (loaded from persisted config,
/// falling back to `DEFAULT_SHORTCUT`) and wires press/release events to the
/// audio recorder's state machine.
pub fn register(
    app: &AppHandle,
    manager: &HotkeyManagerHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    let shortcut_str = manager.lock().unwrap().shortcut().to_string();
    let shortcut: Shortcut = shortcut_str
        .parse()
        .map_err(|_| format!("invalid shortcut string: {shortcut_str}"))?;

    app.global_shortcut()
        .on_shortcut(shortcut, |app, _shortcut, event| {
            handle_shortcut_event(app, event)
        })?;

    log::info!("registered push-to-talk shortcut: {shortcut_str}");

    Ok(())
}

/// Changes the active push-to-talk shortcut at runtime: unregisters the old
/// binding, registers the new one with the same handler, updates in-memory
/// state, and persists the change so it survives restarts.
pub fn set_shortcut(
    app: &AppHandle,
    manager: &HotkeyManagerHandle,
    new_shortcut: &str,
) -> Result<(), String> {
    let new_parsed: Shortcut = new_shortcut
        .parse()
        .map_err(|_| format!("'{new_shortcut}' is not a valid shortcut"))?;

    let old_shortcut = manager.lock().unwrap().shortcut().to_string();

    app.global_shortcut()
        .unregister(
            old_shortcut
                .parse::<Shortcut>()
                .map_err(|_| "failed to re-parse current shortcut".to_string())?,
        )
        .map_err(|e| e.to_string())?;

    app.global_shortcut()
        .on_shortcut(new_parsed, |app, _shortcut, event| {
            handle_shortcut_event(app, event)
        })
        .map_err(|e| e.to_string())?;

    manager.lock().unwrap().shortcut = new_shortcut.to_string();

    let app_data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let _ = crate::config::save(
        &app_data_dir,
        &crate::config::AppConfig {
            hotkey: new_shortcut.to_string(),
        },
    );

    Ok(())
}

/// `tauri-plugin-global-shortcut` can report multiple "pressed" events during
/// OS key-repeat before a single "released", so transitions are guarded by
/// the recorder's current state rather than passed straight through.
fn handle_shortcut_event(app: &AppHandle, event: ShortcutEvent) {
    log::debug!("hotkey event received: {:?}", event.state());
    let recorder = app.state::<AudioRecorderHandle>();

    match event.state() {
        ShortcutState::Pressed => {
            let mut recorder = recorder.lock().unwrap();
            if recorder.state() == RecordingState::Idle {
                app.state::<crate::engine::EngineHandle>().capture_target();
                overlay::show(app);
                match recorder.start(app.clone()) {
                    Ok(()) => tray::set_recording_icon(app, true),
                    Err(e) => {
                        log::error!("failed to start recording: {e}");
                        let _ = app.emit(
                            "recording-error",
                            RecordingErrorPayload {
                                message: e.user_message(),
                            },
                        );
                        drop(recorder);
                        overlay::schedule_hide(app);
                    }
                }
            }
        }
        ShortcutState::Released => {
            let mut recorder = recorder.lock().unwrap();
            if recorder.state() == RecordingState::Recording {
                match recorder.stop() {
                    Ok(()) => tray::set_recording_icon(app, false),
                    Err(e) => log::error!("failed to stop recording: {e}"),
                }
            }
            drop(recorder);
            overlay::schedule_hide(app);
        }
    }
}
