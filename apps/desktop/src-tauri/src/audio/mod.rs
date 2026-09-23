//! Microphone capture -> downmix/resample -> 16kHz mono WAV, per docs/ARCHITECTURE.md.
//!
//! Two dedicated OS threads per recording:
//! - capture thread: owns the cpal::Device/Stream for its whole lifetime (built and
//!   dropped on the same thread, since WASAPI streams are not meant to hop threads).
//!   Its audio callback only copies samples into a bounded channel.
//! - worker thread: downmixes, resamples to 16kHz, encodes to WAV, and emits a
//!   throttled RMS/peak level event for the UI.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver as StdReceiver, Sender as StdSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver as CbReceiver, Sender as CbSender};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

const TARGET_SAMPLE_RATE: u32 = 16_000;
const RESAMPLE_CHUNK_SIZE: usize = 1024;
const LEVEL_EMIT_INTERVAL: Duration = Duration::from_millis(75);
const CAPTURE_CHANNEL_CAPACITY: usize = 64;
const DEVICE_READY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    Idle,
    Initializing,
    Recording,
    Stopping,
    FinalizingWav,
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("invalid recording state transition (currently {0:?})")]
    InvalidTransition(RecordingState),
    #[error("audio device error: {0}")]
    Device(String),
    #[error("io error: {0}")]
    Io(String),
}

#[derive(Serialize, Clone)]
pub struct RecordingErrorPayload {
    pub message: String,
}

impl AudioError {
    /// A short, user-facing message for the overlay — the raw error text
    /// (WASAPI HRESULTs, etc.) isn't meaningful to someone dictating.
    pub fn user_message(&self) -> String {
        match self {
            AudioError::Device(reason) if reason.contains("0x8889000A") => {
                "Another application is using the microphone".to_string()
            }
            AudioError::Device(_) => "Couldn't access the microphone".to_string(),
            AudioError::InvalidTransition(_) => "Already recording".to_string(),
            AudioError::Io(_) => "Couldn't save the recording".to_string(),
        }
    }
}

#[derive(Serialize, Clone)]
struct RecordingStartedPayload {
    path: String,
}

#[derive(Serialize, Clone)]
struct RecordingStoppedPayload {
    path: String,
}

#[derive(Serialize, Clone)]
struct AudioLevelPayload {
    rms: f32,
    peak: f32,
}

struct CaptureReady {
    channels: u16,
    sample_rate: u32,
}

/// Owns the state machine and threads for a single in-progress recording.
/// Wrap in `Arc<Mutex<AudioRecorder>>` and manage via Tauri state so the
/// hotkey handler, tray menu, and (future) UI commands can all drive it.
pub struct AudioRecorder {
    state: Arc<Mutex<RecordingState>>,
    stop_tx: Option<StdSender<()>>,
    capture_thread: Option<JoinHandle<()>>,
    worker_thread: Option<JoinHandle<()>>,
    recordings_dir: PathBuf,
}

impl AudioRecorder {
    pub fn new(recordings_dir: PathBuf) -> Self {
        Self {
            state: Arc::new(Mutex::new(RecordingState::Idle)),
            stop_tx: None,
            capture_thread: None,
            worker_thread: None,
            recordings_dir,
        }
    }

    pub fn state(&self) -> RecordingState {
        *self.state.lock().unwrap()
    }

    /// Begins recording. Blocks briefly (bounded by `DEVICE_READY_TIMEOUT`) while the
    /// capture thread opens the input device, so callers know immediately whether the
    /// device is usable, then returns while the recording continues in the background.
    pub fn start(&mut self, app: AppHandle) -> Result<(), AudioError> {
        {
            let mut state = self.state.lock().unwrap();
            if *state != RecordingState::Idle {
                return Err(AudioError::InvalidTransition(*state));
            }
            *state = RecordingState::Initializing;
        }

        std::fs::create_dir_all(&self.recordings_dir)
            .map_err(|e| AudioError::Io(e.to_string()))?;
        let filename = format!(
            "{}.wav",
            chrono::Local::now().format("%Y%m%d_%H%M%S%3f")
        );
        let output_path = self.recordings_dir.join(filename);

        let (raw_tx, raw_rx) = crossbeam_channel::bounded::<Vec<f32>>(CAPTURE_CHANNEL_CAPACITY);
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<CaptureReady, String>>();

        let capture_thread = spawn_capture_thread(raw_tx, stop_rx, ready_tx);

        let ready = match ready_rx.recv_timeout(DEVICE_READY_TIMEOUT) {
            Ok(Ok(ready)) => ready,
            Ok(Err(reason)) => {
                *self.state.lock().unwrap() = RecordingState::Idle;
                let _ = capture_thread.join();
                return Err(AudioError::Device(reason));
            }
            Err(_) => {
                *self.state.lock().unwrap() = RecordingState::Idle;
                return Err(AudioError::Device(
                    "timed out waiting for audio device to initialize".into(),
                ));
            }
        };

        let worker_thread = {
            let state_for_worker = Arc::clone(&self.state);
            let app_for_worker = app.clone();
            let output_path_for_worker = output_path.clone();
            thread::spawn(move || {
                run_worker(
                    raw_rx,
                    ready.channels,
                    ready.sample_rate,
                    output_path_for_worker,
                    app_for_worker,
                    state_for_worker,
                );
            })
        };

        self.stop_tx = Some(stop_tx);
        self.capture_thread = Some(capture_thread);
        self.worker_thread = Some(worker_thread);
        *self.state.lock().unwrap() = RecordingState::Recording;

        let _ = app.emit(
            "recording-started",
            RecordingStartedPayload {
                path: output_path.display().to_string(),
            },
        );

        Ok(())
    }

    /// Signals the capture thread to stop and waits for it to release the device.
    /// The worker thread keeps running in the background to flush/encode the WAV
    /// file; it flips the state back to `Idle` and emits `recording-stopped` itself
    /// once finalization completes, so this call does not block on that.
    pub fn stop(&mut self) -> Result<(), AudioError> {
        {
            let mut state = self.state.lock().unwrap();
            if *state != RecordingState::Recording {
                return Err(AudioError::InvalidTransition(*state));
            }
            *state = RecordingState::Stopping;
        }

        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.capture_thread.take() {
            let _ = handle.join();
        }

        *self.state.lock().unwrap() = RecordingState::FinalizingWav;
        // Dropping the handle without joining lets the worker finish in the
        // background; it owns the app handle it needs to emit completion.
        self.worker_thread = None;

        Ok(())
    }
}

pub type AudioRecorderHandle = Arc<Mutex<AudioRecorder>>;

fn spawn_capture_thread(
    raw_tx: CbSender<Vec<f32>>,
    stop_rx: StdReceiver<()>,
    ready_tx: StdSender<Result<CaptureReady, String>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let host = cpal::default_host();
        let device = match host.default_input_device() {
            Some(device) => device,
            None => {
                let _ = ready_tx.send(Err("no input device available".into()));
                return;
            }
        };

        let supported_config = match device.default_input_config() {
            Ok(config) => config,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("failed to read input config: {e}")));
                return;
            }
        };

        let channels = supported_config.channels();
        let sample_rate = supported_config.sample_rate().0;
        let sample_format = supported_config.sample_format();
        let stream_config: cpal::StreamConfig = supported_config.into();

        let err_fn = |err| log::error!("cpal input stream error: {err}");

        let stream_result = match sample_format {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let _ = raw_tx.try_send(data.to_vec());
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &stream_config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let converted: Vec<f32> =
                        data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                    let _ = raw_tx.try_send(converted);
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                &stream_config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    let converted: Vec<f32> = data
                        .iter()
                        .map(|&s| (s as f32 - 32768.0) / 32768.0)
                        .collect();
                    let _ = raw_tx.try_send(converted);
                },
                err_fn,
                None,
            ),
            other => {
                let _ = ready_tx.send(Err(format!("unsupported sample format: {other:?}")));
                return;
            }
        };

        let stream = match stream_result {
            Ok(stream) => stream,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("failed to build input stream: {e}")));
                return;
            }
        };

        if let Err(e) = stream.play() {
            let _ = ready_tx.send(Err(format!("failed to start input stream: {e}")));
            return;
        }

        let _ = ready_tx.send(Ok(CaptureReady {
            channels,
            sample_rate,
        }));

        // Block until told to stop; the stream (and its device handle) is then
        // dropped on this same thread when the function returns.
        let _ = stop_rx.recv();
    })
}

fn downmix_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let channels = channels as usize;
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

fn build_resampler(input_rate: u32, output_rate: u32) -> Option<SincFixedIn<f32>> {
    if input_rate == output_rate {
        return None;
    }
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    SincFixedIn::<f32>::new(
        output_rate as f64 / input_rate as f64,
        2.0,
        params,
        RESAMPLE_CHUNK_SIZE,
        1,
    )
    .ok()
}

fn write_mono_f32_as_i16(writer: &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>, samples: &[f32]) {
    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let value = (clamped * i16::MAX as f32) as i16;
        let _ = writer.write_sample(value);
    }
}

fn run_worker(
    raw_rx: CbReceiver<Vec<f32>>,
    channels: u16,
    input_sample_rate: u32,
    output_path: PathBuf,
    app: AppHandle,
    state: Arc<Mutex<RecordingState>>,
) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = match hound::WavWriter::create(&output_path, spec) {
        Ok(writer) => writer,
        Err(e) => {
            log::error!("failed to create WAV writer for {output_path:?}: {e}");
            *state.lock().unwrap() = RecordingState::Idle;
            return;
        }
    };

    let mut resampler = build_resampler(input_sample_rate, TARGET_SAMPLE_RATE);
    let mut pending_mono: Vec<f32> = Vec::new();

    let mut level_sum_sq: f64 = 0.0;
    let mut level_peak: f32 = 0.0;
    let mut level_count: usize = 0;
    let mut last_emit = Instant::now();

    while let Ok(chunk) = raw_rx.recv() {
        let mono = downmix_to_mono(&chunk, channels);

        for &sample in &mono {
            level_sum_sq += (sample as f64) * (sample as f64);
            level_peak = level_peak.max(sample.abs());
        }
        level_count += mono.len();

        if last_emit.elapsed() >= LEVEL_EMIT_INTERVAL && level_count > 0 {
            let rms = (level_sum_sq / level_count as f64).sqrt() as f32;
            let _ = app.emit(
                "audio-level",
                AudioLevelPayload {
                    rms,
                    peak: level_peak,
                },
            );
            level_sum_sq = 0.0;
            level_peak = 0.0;
            level_count = 0;
            last_emit = Instant::now();
        }

        pending_mono.extend_from_slice(&mono);

        if let Some(resampler) = resampler.as_mut() {
            while pending_mono.len() >= RESAMPLE_CHUNK_SIZE {
                let in_chunk: Vec<f32> = pending_mono.drain(..RESAMPLE_CHUNK_SIZE).collect();
                match resampler.process(&[in_chunk], None) {
                    Ok(out) => write_mono_f32_as_i16(&mut writer, &out[0]),
                    Err(e) => log::error!("resample error: {e}"),
                }
            }
        } else {
            write_mono_f32_as_i16(&mut writer, &pending_mono);
            pending_mono.clear();
        }
    }

    if let Some(resampler) = resampler.as_mut() {
        if !pending_mono.is_empty() {
            match resampler.process_partial(Some(&[pending_mono.clone()]), None) {
                Ok(out) => write_mono_f32_as_i16(&mut writer, &out[0]),
                Err(e) => log::error!("resample flush error: {e}"),
            }
            pending_mono.clear();
        }
        if let Ok(out) = resampler.process_partial::<Vec<f32>>(None, None) {
            if let Some(tail) = out.first() {
                write_mono_f32_as_i16(&mut writer, tail);
            }
        }
    }

    if let Err(e) = writer.finalize() {
        log::error!("failed to finalize WAV file {output_path:?}: {e}");
    }

    *state.lock().unwrap() = RecordingState::Idle;
    let _ = app.emit(
        "recording-stopped",
        RecordingStoppedPayload {
            path: output_path.display().to_string(),
        },
    );
}

pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devices) => devices.filter_map(|d| d.name().ok()).collect(),
        Err(e) => {
            log::error!("failed to enumerate input devices: {e}");
            Vec::new()
        }
    }
}

#[derive(Serialize)]
pub struct RecordingInfo {
    pub name: String,
    pub path: String,
    pub created_at: String,
    pub size_bytes: u64,
    /// Populated from a `<timestamp>.txt` sidecar written by the speech
    /// worker (see `speech::worker`), if transcription has completed.
    pub transcript: Option<String>,
}

pub fn list_recordings(recordings_dir: &Path) -> Vec<RecordingInfo> {
    let entries = match std::fs::read_dir(recordings_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut recordings: Vec<RecordingInfo> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("wav"))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            let created = metadata
                .created()
                .or_else(|_| metadata.modified())
                .ok()?;
            let created_at: chrono::DateTime<chrono::Local> = created.into();
            let transcript = std::fs::read_to_string(entry.path().with_extension("txt")).ok();
            Some(RecordingInfo {
                name: entry.file_name().to_string_lossy().to_string(),
                path: entry.path().display().to_string(),
                created_at: created_at.to_rfc3339(),
                size_bytes: metadata.len(),
                transcript,
            })
        })
        .collect();

    recordings.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    recordings
}
