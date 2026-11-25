use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::thread;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use super::{SpeechEngine, SpeechError};
use tauri::Manager;

const JOB_QUEUE_CAPACITY: usize = 4;

#[derive(Serialize, Clone)]
struct TranscriptionQueuedPayload {
    recording_path: String,
}

#[derive(Serialize, Clone)]
struct TranscriptionStartedPayload {
    recording_path: String,
}

#[derive(Serialize, Clone)]
struct TranscriptionCompletedPayload {
    recording_path: String,
    transcript: String,
}

#[derive(Serialize, Clone)]
struct TranscriptionFailedPayload {
    recording_path: String,
    error: String,
}

/// A single persistent worker thread that owns the (expensive-to-load)
/// speech engine and processes transcription jobs strictly serially, in
/// contrast to `audio::AudioRecorder`'s per-recording capture/worker
/// threads — here the dominant one-time cost is loading the model, not
/// opening a device, so it must only happen once.
pub struct SpeechWorker {
    job_tx: SyncSender<PathBuf>,
    app: AppHandle,
}

pub type SpeechWorkerHandle = Arc<SpeechWorker>;

impl SpeechWorker {
    /// Queues a recording for transcription and emits `transcription-queued`.
    pub fn enqueue(&self, wav_path: PathBuf) -> Result<(), SpeechError> {
        let recording_path = wav_path.display().to_string();
        self.job_tx
            .send(wav_path)
            .map_err(|_| SpeechError::InitFailed("speech worker thread is not running".into()))?;
        let _ = self.app.emit(
            "transcription-queued",
            TranscriptionQueuedPayload { recording_path },
        );
        Ok(())
    }
}

pub fn spawn_worker(app: AppHandle, mut engine: impl SpeechEngine + 'static) -> SpeechWorkerHandle {
    let (job_tx, job_rx) = sync_channel::<PathBuf>(JOB_QUEUE_CAPACITY);

    let worker_app = app.clone();
    thread::spawn(move || {
        while let Ok(wav_path) = job_rx.recv() {
            let recording_path = wav_path.display().to_string();
            let _ = worker_app.emit(
                "transcription-started",
                TranscriptionStartedPayload {
                    recording_path: recording_path.clone(),
                },
            );

            match engine.transcribe(&wav_path) {
                Ok(transcript) => {
                    if let Err(e) = write_transcript_sidecar(&wav_path, &transcript) {
                        log::error!("failed to write transcript sidecar for {wav_path:?}: {e}");
                    }

                    // An empty result means no usable speech was recognized.
                    // Do not launch the intent router or action engine for it.
                    if !transcript.trim().is_empty() {
                        // Local intent routing preserves direct dictation and
                        // handles exact app launches before using the selected
                        // provider for complex desktop commands.
                        if let Err(e) = worker_app
                            .state::<crate::engine::EngineHandle>()
                            .submit_recording(&recording_path, transcript.clone())
                        {
                            log::error!("failed to enqueue engine task: {e}");
                        }
                    }

                    let _ = worker_app.emit(
                        "transcription-completed",
                        TranscriptionCompletedPayload {
                            recording_path,
                            transcript,
                        },
                    );
                }
                Err(e) => {
                    let _ = worker_app.emit(
                        "transcription-failed",
                        TranscriptionFailedPayload {
                            recording_path,
                            error: e.to_string(),
                        },
                    );
                }
            }

            // Extends the overlay's visibility a little so the result is
            // seen, then lets it fade on the usual idle timer.
            crate::overlay::schedule_hide(&worker_app);
        }
    });

    Arc::new(SpeechWorker { job_tx, app })
}

fn write_transcript_sidecar(wav_path: &std::path::Path, transcript: &str) -> std::io::Result<()> {
    std::fs::write(wav_path.with_extension("txt"), transcript)
}
