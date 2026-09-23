//! Local speech-to-text: a small provider trait plus the Phase 2 implementation
//! (`whisper_engine`), a persistent transcription worker (`worker`), and model
//! download/status handling (`model_manager`). See docs/ARCHITECTURE.md.

use std::path::Path;

mod model_manager;
mod whisper_engine;
mod worker;

pub use model_manager::{download_model, model_path, model_status, ModelDownloadGuard, ModelStatus};
pub use whisper_engine::WhisperCppEngine;
pub use worker::{spawn_worker, SpeechWorkerHandle};

#[derive(Debug, thiserror::Error)]
pub enum SpeechError {
    #[error("model not found at {0}")]
    ModelNotFound(String),
    #[error("failed to initialize speech engine: {0}")]
    InitFailed(String),
    #[error("transcription failed: {0}")]
    TranscribeFailed(String),
    /// Reserved for `SpeechEngine::stream()`, unused until a later phase
    /// implements real-time transcription.
    #[allow(dead_code)]
    #[error("not supported: {0}")]
    NotSupported(String),
}

/// A local speech-to-text provider. `WhisperCppEngine` is the only Phase 2
/// implementation; the trait exists so a different local engine could be
/// swapped in later without touching the worker/queue plumbing. This product
/// never ships a cloud or LLM-based provider behind this trait.
pub trait SpeechEngine: Send {
    fn initialize(&mut self) -> Result<(), SpeechError>;
    fn transcribe(&mut self, wav_path: &Path) -> Result<String, SpeechError>;
    /// Real-time streaming transcription. Not implemented in Phase 2 (batch-only);
    /// returns `SpeechError::NotSupported` until a later phase taps the same
    /// audio worker-thread output used for the batch pipeline. Unused (and
    /// thus not yet called) until that phase.
    #[allow(dead_code)]
    fn stream(&mut self) -> Result<(), SpeechError>;
    /// Releases the loaded model. Unused until a later phase adds
    /// idle-unload or engine-swap behavior.
    #[allow(dead_code)]
    fn unload(&mut self);
}
