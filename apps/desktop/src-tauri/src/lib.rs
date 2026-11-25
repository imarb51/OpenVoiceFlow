mod ai_config;
mod audio;
mod config;
mod engine;
mod hotkeys;
mod injection;
mod overlay;
mod speech;
mod tray;

use audio::{AudioRecorder, AudioRecorderHandle, RecordingInfo};
use hotkeys::{HotkeyManager, HotkeyManagerHandle};
use speech::{ModelDownloadGuard, ModelStatus, SpeechWorkerHandle, WhisperCppEngine};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{Listener, Manager};

fn recordings_dir(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .expect("failed to resolve app data dir")
        .join("recordings")
}

fn models_dir(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .expect("failed to resolve app data dir")
        .join("models")
}

#[tauri::command]
fn list_audio_devices() -> Vec<String> {
    audio::list_input_devices()
}

#[tauri::command]
fn get_recordings_dir(app: tauri::AppHandle) -> String {
    recordings_dir(&app).display().to_string()
}

#[tauri::command]
fn list_recordings(app: tauri::AppHandle) -> Vec<RecordingInfo> {
    audio::list_recordings(&recordings_dir(&app))
}

#[tauri::command]
fn delete_recording(path: String) -> Result<(), String> {
    let wav_path = PathBuf::from(&path);
    // Best-effort: a recording with no transcript yet has no sidecar to remove.
    let _ = std::fs::remove_file(wav_path.with_extension("txt"));
    std::fs::remove_file(&wav_path).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_model_status(app: tauri::AppHandle) -> ModelStatus {
    speech::model_status(&models_dir(&app))
}

#[tauri::command]
async fn download_model(app: tauri::AppHandle) -> Result<(), String> {
    let guard = app.state::<ModelDownloadGuard>().inner().clone();
    let dir = models_dir(&app);
    speech::download_model(app.clone(), dir, guard).await
}

#[tauri::command]
fn transcribe_recording(app: tauri::AppHandle, path: String) -> Result<(), String> {
    app.state::<SpeechWorkerHandle>()
        .enqueue(PathBuf::from(path))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_hotkey(app: tauri::AppHandle) -> String {
    app.state::<HotkeyManagerHandle>()
        .lock()
        .unwrap()
        .shortcut()
        .to_string()
}

#[tauri::command]
fn set_hotkey(app: tauri::AppHandle, shortcut: String) -> Result<(), String> {
    let manager = app.state::<HotkeyManagerHandle>().inner().clone();
    hotkeys::set_shortcut(&app, &manager, &shortcut)
}

#[tauri::command]
fn get_ai_settings(app: tauri::AppHandle) -> Result<ai_config::AiSettings, String> {
    ai_config::settings(&app, ai_config::load(&app)?)
}

#[tauri::command]
fn has_ai_api_key(provider: ai_config::AiProvider) -> Result<bool, String> {
    ai_config::has_api_key(provider)
}

#[tauri::command]
fn get_ai_api_key(provider: ai_config::AiProvider) -> Result<Option<String>, String> {
    ai_config::saved_api_key(provider)
}

#[tauri::command]
fn save_ai_settings(
    app: tauri::AppHandle,
    provider: ai_config::AiProvider,
    model: String,
    api_key: Option<String>,
) -> Result<ai_config::AiSettings, String> {
    ai_config::save(
        &app,
        ai_config::UpdateAiSettings {
            provider,
            model,
            api_key,
        },
    )
}

#[tauri::command]
fn remove_ai_api_key(provider: ai_config::AiProvider) -> Result<(), String> {
    ai_config::remove_api_key(provider)
}

#[tauri::command]
async fn test_ai_connection(provider: ai_config::AiProvider, model: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || speech::test_connection(provider, &model))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_ollama_models() -> Result<Vec<String>, String> {
    let url = format!("{}/api/tags", ai_config::OLLAMA_BASE_URL);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| format!("Could not create Ollama client: {e}"))?;
    let response = client.get(&url).send().await.map_err(|e| {
        format!(
            "Could not reach local Ollama at {}: {e}",
            ai_config::OLLAMA_BASE_URL
        )
    })?;
    if !response.status().is_success() {
        return Err(format!(
            "Ollama returned HTTP {} while listing models",
            response.status()
        ));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Ollama returned an invalid model list: {e}"))?;
    let models = body["models"]
        .as_array()
        .ok_or("Ollama response did not contain a models list")?;
    Ok(models
        .iter()
        .filter_map(|model| model["name"].as_str().map(str::to_string))
        .collect())
}

#[tauri::command]
async fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    overlay::open_settings_window(&app).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_engine_tasks(app: tauri::AppHandle) -> Vec<engine::types::TaskSnapshot> {
    app.state::<engine::EngineHandle>().tasks()
}
#[tauri::command]
fn cancel_engine_task(app: tauri::AppHandle, task_id: u64) -> Result<(), String> {
    app.state::<engine::EngineHandle>().cancel(task_id)
}
#[tauri::command]
fn resume_engine_task(
    app: tauri::AppHandle,
    task_id: u64,
    message: String,
    approved: bool,
) -> Result<(), String> {
    app.state::<engine::EngineHandle>()
        .resume(task_id, message, approved)
}
#[tauri::command]
fn submit_engine_task(app: tauri::AppHandle, transcript: String) -> Result<u64, String> {
    let target = engine::desktop::foreground().ok_or("No foreground window")?;
    app.state::<engine::EngineHandle>()
        .submit(transcript, target)
}
#[tauri::command]
fn get_engine_config(app: tauri::AppHandle) -> Result<engine::config::EngineConfig, String> {
    engine::config::load(&app)
}
#[tauri::command]
fn save_engine_config(
    app: tauri::AppHandle,
    config: engine::config::EngineConfig,
) -> Result<(), String> {
    engine::config::save(&app, config)
}

/// Mirrors the JSON shape of the (private) `RecordingStoppedPayload` emitted
/// by `audio::run_worker` — decoding the event's wire format, not reaching
/// into `audio`'s internals.
#[derive(serde::Deserialize)]
struct RecordingStoppedEvent {
    path: String,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    tauri::Builder::default()
        // Must be the first plugin registered. Without it, launching the app
        // while an instance is already running starts a second process that
        // fails to register the same global hotkey (Windows only allows one
        // owner per combination) — the new process runs, but silently never
        // responds to the hotkey. This makes a relaunch just refocus the
        // existing instance's overlay instead.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            overlay::show(app);
            overlay::schedule_hide(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            list_audio_devices,
            get_recordings_dir,
            list_recordings,
            delete_recording,
            get_model_status,
            download_model,
            transcribe_recording,
            get_hotkey,
            set_hotkey,
            get_ai_settings,
            has_ai_api_key,
            get_ai_api_key,
            save_ai_settings,
            remove_ai_api_key,
            test_ai_connection,
            list_ollama_models,
            open_settings_window,
            get_engine_tasks,
            cancel_engine_task,
            resume_engine_task,
            submit_engine_task,
            get_engine_config,
            save_engine_config,
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            let recorder: AudioRecorderHandle =
                Arc::new(Mutex::new(AudioRecorder::new(recordings_dir(&handle))));
            app.manage(recorder);

            tray::build(&handle)?;

            app.manage(overlay::OverlayState::default());
            app.manage(engine::Engine::start(handle.clone()));
            let capture_app = handle.clone();
            handle.listen("recording-started", move |event| {
                if let Ok(payload) = serde_json::from_str::<RecordingStoppedEvent>(event.payload())
                {
                    capture_app
                        .state::<engine::EngineHandle>()
                        .associate_recording(payload.path);
                }
            });

            let app_data_dir = handle
                .path()
                .app_data_dir()
                .expect("failed to resolve app data dir");
            let persisted_config = config::load(&app_data_dir);
            let hotkey_manager: HotkeyManagerHandle =
                Arc::new(Mutex::new(HotkeyManager::new(persisted_config.hotkey)));
            hotkeys::register(&handle, &hotkey_manager)?;
            app.manage(hotkey_manager);

            let whisper_engine = WhisperCppEngine::new(speech::model_path(&models_dir(&handle)));
            let speech_worker: SpeechWorkerHandle =
                speech::spawn_worker(handle.clone(), whisper_engine);
            app.manage(speech_worker.clone());
            app.manage(ModelDownloadGuard::default());

            // Decoupled from `audio`: rather than audio/mod.rs knowing about
            // transcription, it just emits `recording-stopped` as it always
            // has, and this listener bridges to the speech worker.
            handle.listen("recording-stopped", move |event| {
                let Ok(payload) = serde_json::from_str::<RecordingStoppedEvent>(event.payload())
                else {
                    return;
                };
                if let Err(e) = speech_worker.enqueue(PathBuf::from(payload.path)) {
                    log::error!("failed to enqueue transcription job: {e}");
                }
            });

            // Show the overlay once at launch so its position is visible,
            // then let the usual idle timer fade it if untouched.
            overlay::show(&handle);
            overlay::schedule_hide(&handle);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
