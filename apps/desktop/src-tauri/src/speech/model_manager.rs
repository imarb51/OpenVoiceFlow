use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub const MODEL_FILENAME: &str = "ggml-base.en.bin";
pub const MODEL_DOWNLOAD_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin";

const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Serialize, Clone)]
pub struct ModelStatus {
    pub available: bool,
    pub path: String,
}

#[derive(Serialize, Clone)]
struct DownloadProgressPayload {
    downloaded_bytes: u64,
    total_bytes: u64,
}

#[derive(Serialize, Clone)]
struct DownloadFailedPayload {
    error: String,
}

pub fn model_path(models_dir: &Path) -> PathBuf {
    models_dir.join(MODEL_FILENAME)
}

pub fn model_status(models_dir: &Path) -> ModelStatus {
    let path = model_path(models_dir);
    ModelStatus {
        available: path.is_file(),
        path: path.display().to_string(),
    }
}

/// Guards against concurrent download attempts; managed as Tauri state
/// alongside `SpeechWorkerHandle`.
#[derive(Clone, Default)]
pub struct ModelDownloadGuard(pub Arc<AtomicBool>);

/// Downloads the default model to `<models_dir>/ggml-base.en.bin`, streaming
/// to a `.part` file and renaming into place only on success so a crash or
/// interrupted download never leaves a half-written file mistaken for valid.
/// Emits throttled `model-download-progress` events plus a terminal
/// `model-download-completed`/`model-download-failed` event.
pub async fn download_model(
    app: AppHandle,
    models_dir: PathBuf,
    guard: ModelDownloadGuard,
) -> Result<(), String> {
    if guard.0.swap(true, Ordering::SeqCst) {
        return Err("a model download is already in progress".into());
    }

    let result = download_model_inner(&app, &models_dir).await;
    guard.0.store(false, Ordering::SeqCst);

    match &result {
        Ok(()) => {
            let _ = app.emit("model-download-completed", ());
        }
        Err(error) => {
            let _ = app.emit(
                "model-download-failed",
                DownloadFailedPayload {
                    error: error.clone(),
                },
            );
        }
    }

    result
}

async fn download_model_inner(app: &AppHandle, models_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(models_dir).map_err(|e| e.to_string())?;
    let final_path = model_path(models_dir);
    let part_path = final_path.with_extension("part");

    let response = reqwest::get(MODEL_DOWNLOAD_URL)
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "model download failed with HTTP status {}",
            response.status()
        ));
    }
    let total_bytes = response.content_length().unwrap_or(0);

    let mut file = std::fs::File::create(&part_path).map_err(|e| e.to_string())?;
    let mut downloaded: u64 = 0;
    let mut last_emit = Instant::now();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;

        if last_emit.elapsed() >= PROGRESS_EMIT_INTERVAL {
            let _ = app.emit(
                "model-download-progress",
                DownloadProgressPayload {
                    downloaded_bytes: downloaded,
                    total_bytes,
                },
            );
            last_emit = Instant::now();
        }
    }
    drop(file);

    std::fs::rename(&part_path, &final_path).map_err(|e| e.to_string())?;
    Ok(())
}
