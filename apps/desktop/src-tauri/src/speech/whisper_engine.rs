use std::path::{Path, PathBuf};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::{SpeechEngine, SpeechError};

/// `SpeechEngine` implementation backed by whisper.cpp via `whisper-rs`.
/// CPU-only (no GPU feature flags enabled), so it runs on any machine with
/// no required cloud dependency. Loads the model lazily on first use so a
/// missing/not-yet-downloaded model doesn't block anything until it's
/// actually needed.
pub struct WhisperCppEngine {
    model_path: PathBuf,
    context: Option<WhisperContext>,
}

impl WhisperCppEngine {
    pub fn new(model_path: PathBuf) -> Self {
        Self {
            model_path,
            context: None,
        }
    }
}

impl SpeechEngine for WhisperCppEngine {
    fn initialize(&mut self) -> Result<(), SpeechError> {
        if self.context.is_some() {
            return Ok(());
        }
        if !self.model_path.is_file() {
            return Err(SpeechError::ModelNotFound(
                self.model_path.display().to_string(),
            ));
        }
        let model_path_str = self
            .model_path
            .to_str()
            .ok_or_else(|| SpeechError::InitFailed("model path is not valid UTF-8".into()))?;

        let context =
            WhisperContext::new_with_params(model_path_str, WhisperContextParameters::default())
                .map_err(|e| SpeechError::InitFailed(e.to_string()))?;

        self.context = Some(context);
        Ok(())
    }

    fn transcribe(&mut self, wav_path: &Path) -> Result<String, SpeechError> {
        self.initialize()?;
        let context = self
            .context
            .as_ref()
            .expect("context is Some after initialize()");

        let samples =
            read_wav_as_f32_mono(wav_path).map_err(|e| SpeechError::TranscribeFailed(e.to_string()))?;

        let mut state = context
            .create_state()
            .map_err(|e| SpeechError::TranscribeFailed(e.to_string()))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state
            .full(params, &samples)
            .map_err(|e| SpeechError::TranscribeFailed(e.to_string()))?;

        let num_segments = state.full_n_segments();

        let mut transcript = String::new();
        for i in 0..num_segments {
            let Some(segment) = state.get_segment(i) else {
                continue;
            };
            let text = segment
                .to_str()
                .map_err(|e| SpeechError::TranscribeFailed(e.to_string()))?;
            if !transcript.is_empty() {
                transcript.push(' ');
            }
            transcript.push_str(text.trim());
        }

        Ok(transcript)
    }

    fn stream(&mut self) -> Result<(), SpeechError> {
        Err(SpeechError::NotSupported(
            "streaming transcription is not implemented until a later phase".into(),
        ))
    }

    fn unload(&mut self) {
        self.context = None;
    }
}

fn read_wav_as_f32_mono(path: &Path) -> Result<Vec<f32>, hound::Error> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect::<Result<_, _>>()?,
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
    };

    Ok(samples)
}
