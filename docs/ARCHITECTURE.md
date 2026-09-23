# Architecture

OpenVoiceFlow is an open-source, privacy-first, local-first voice dictation
desktop app: hold a hotkey, speak, and get cleaned-up text at the cursor, with
local Whisper transcription by default and no required cloud dependency.

## Overview

```
OpenVoiceFlow
                         │
              ┌──────────┴──────────┐
              │                     │
          React UI              Rust Core (Tauri)
              │                     │
              └──────────┬──────────┘
                         │
       ┌─────────────────┼─────────────────┐
       │                 │                 │
     Audio            Speech            System APIs
  (cpal → channel   (Whisper via       (HotkeyManager,
   → worker: mono/  provider trait)     tray, clipboard,
   resample/hound)                      text injection)
       └─────────────────┼─────────────────┘
                         │
                Cursor Text Injection
```

The desktop shell is Tauri 2 (Rust backend + React/TypeScript/Vite frontend).
Everything runs on-device: speech-to-text and text injection are local,
period — there is no LLM anywhere in this product (no cloud provider, no
local LLM cleanup pass). Accuracy and speed are expected to come from the
local Whisper pipeline itself, not a post-processing model.

## Provider abstractions

Defined as Rust traits so alternate implementations can be swapped without
touching the callers:

- `SpeechEngine` — `initialize()`, `transcribe()`, `stream()`, `unload()`.
  Implemented in Phase 2 as `WhisperCppEngine` via `whisper-rs` (CPU-only,
  batch transcription; `stream()` is reserved for a later real-time phase).
  The trait exists so a different *local* engine could be swapped in later,
  not to leave room for a cloud or LLM-based provider.
- `TextInjector` — `insert_text()`. Implemented as `ClipboardPasteInjector`:
  clipboard-write + simulated Ctrl+V, restoring the user's previous
  clipboard contents afterward. Native (non-clipboard) insertion may come
  later if this proves limiting.
- `AudioProvider` — `list_devices()`, `start_recording()`, `stop_recording()`.
  Implemented in Phase 1 using `cpal`.

## Repository layout

```
OpenVoiceFlow/
├── apps/
│   └── desktop/                  # the complete Tauri desktop app
│       ├── src/                  # React + TS + Tailwind frontend
│       ├── src-tauri/            # Rust backend
│       │   ├── src/
│       │   │   ├── main.rs
│       │   │   ├── lib.rs
│       │   │   ├── audio/         # cpal capture, worker thread, hound WAV writer
│       │   │   ├── hotkeys/       # HotkeyManager (global-shortcut registration)
│       │   │   ├── speech/        # SpeechEngine trait, WhisperCppEngine, worker, model download
│       │   │   ├── injection/     # TextInjector trait, clipboard-paste simulation
│       │   │   ├── overlay/       # floating "main" window: position/show/hide, Settings window
│       │   │   ├── config.rs      # tiny persisted settings file (push-to-talk shortcut)
│       │   │   └── tray/          # tray icon + menu
│       │   ├── settings.html      # second Vite entry point, loaded by the Settings window
│       │   └── icons/
│       └── ...
├── packages/                     # shared TS types/utilities (stubs until needed)
├── models/                       # dev-convenience placeholder only — the runtime model
│                                  # lives in the OS app-data dir (see "App data paths"),
│                                  # not here; nothing here is loaded by the running app
├── docs/
└── tests/
```

`apps/desktop` is one complete application, Tauri backend included, so the
repo can later grow `apps/mobile` or `apps/web` without mixing a different
app's native backend into the root.

## Audio pipeline (Phase 1)

The `cpal` input callback runs on a dedicated, latency-sensitive audio
thread and must stay minimal — no WAV encoding, resampling, or
allocation-heavy work inside it:

```
Microphone
    │
    ▼
cpal input callback  (copies raw samples, pushes to a bounded channel — nothing else)
    │  raw samples
    ▼
Bounded channel (crossbeam)
    │
    ▼
Audio worker thread
    ├── stereo → mono
    ├── f32 → i16 conversion
    ├── resample native rate (e.g. 48kHz) → 16kHz   (via rubato)
    └── hound::WavWriter encoding
            │
            ▼
      recordings/<timestamp>.wav
```

The capture thread owns the `cpal::Device`/`Stream` for its entire lifetime
(built and dropped on the same OS thread — WASAPI streams are not meant to
hop threads on Windows) and only ever copies samples into the channel. The
worker thread does the downmix/resample/encode work, and also aggregates a
throttled RMS/peak level per ~75ms window, emitted as an `audio-level` Tauri
event (~13Hz) for the UI's level meter — not once per raw callback.

The input device's actual `cpal::StreamConfig` (channel count, sample rate)
is always queried and run through the downmix/resample step; the pipeline
never assumes the device is already 16kHz mono. This also sets up cleanly
for a later real-time streaming Whisper phase, which can tap the same
worker-thread output.

### Recording state machine

`Idle → Start → Initializing → Recording → Stopping → FinalizingWav → Idle`

`start()`/`stop()` on `AudioRecorder` drive this and return errors (not
panics) on invalid transitions, e.g. calling `start()` while already
`Recording`.

## Hotkey handling (Phase 1)

`HotkeyManager` holds a configurable push-to-talk shortcut (default
`Ctrl+Space`, with `F8` documented as a fallback for a later Settings UI,
since Ctrl-based combos can collide with OS/IME shortcuts — especially on
macOS). Phase 1 hardcodes the default but keeps the manager structured so a
later Settings UI can swap the binding without a rewrite.

`tauri-plugin-global-shortcut` reports press/release, but OS-level key-repeat
can emit multiple "pressed" events before a single "released", so the
handler is a guarded state check rather than a direct pass-through:

```
on shortcut event:
    if state == Pressed  && recorder.state == Idle      → recorder.start()
    if state == Released && recorder.state == Recording → recorder.stop()
    (any other combination is ignored — already recording / already idle)
```

Recording is driven directly from Rust, so it works regardless of window
focus.

## Speech pipeline (Phase 2)

Transcription is batch, not streaming: it runs once per completed recording,
triggered off the existing `recording-stopped` event. `audio/mod.rs` stays
completely unaware that transcription exists — `lib.rs` listens for
`recording-stopped` itself and forwards the path to the speech worker. Real
streaming transcription (tapping the same audio worker-thread output word by
word) is left for a later phase.

Unlike `audio/mod.rs`'s per-recording capture/worker threads, speech uses a
**single persistent worker thread with a job queue**, spawned once at
startup: the per-recording cost in `audio/` is opening the device, but here
the dominant one-time cost is loading the ~148MB GGML model into memory, so
that must happen once (lazily, on first job), not per recording. Jobs are
processed strictly serially — whisper.cpp already multithreads internally
per transcription.

```
recording-stopped {path}
        │
        ▼
speech worker thread (persistent, loads the model lazily on first job)
        ├── WhisperCppEngine::transcribe(path)   (whisper-rs / whisper.cpp, CPU-only)
        └── writes <timestamp>.txt sidecar next to <timestamp>.wav
        │
        ▼
transcription-completed {recording_path, transcript}
```

Events: `transcription-queued`, `transcription-started`,
`transcription-completed {transcript}`, `transcription-failed {error}` —
mirrors the `recording-started`/`recording-stopped` pairing style.

No history database yet: each transcript is just a plain-text `.txt`
sidecar next to its `.wav`, and `list_recordings()` picks it up on scan by
matching filename. `RecordingInfo` gained a `transcript: Option<String>`
field for this.

**Model acquisition**: the default model (`ggml-base.en.bin`, ~148MB) is
downloaded on demand — not bundled, not auto-fetched at every startup —
streamed to a `.part` file and renamed into place only on success, so an
interrupted download is never mistaken for a valid model.
`get_model_status()` lets the UI show a "model missing" banner proactively;
`download_model()` drives the download with throttled
`model-download-progress` events.

## Text injection

Right after a transcription completes (`speech/worker.rs`, before emitting
`transcription-completed`), `ClipboardPasteInjector::insert_text()` copies
the transcript to the clipboard, simulates Ctrl+V (via `enigo`), and
restores whatever text was previously on the clipboard. This delivers text
system-wide, wherever OS input focus currently is — not just inside this
app — with no detection of whether a real text field is focused. It's a
best-effort side effect only (`log::error!` on failure, no event payload
change): the transcript is already durably saved via the `.txt` sidecar and
the Settings window's Transcripts tab regardless of whether the paste lands
anywhere useful.

## Overlay, tray, and Settings windows

Two Tauri windows: the small floating **overlay** (label `"main"`,
`overlay/mod.rs`) and a normal **Settings** window created on demand
(`overlay::open_settings_window()`), each its own Vite entry point
(`index.html` / `settings.html`) sharing one React codebase. Both windows
must be listed in `capabilities/default.json`'s `"windows"` array — Tauri's
ACL/capability system is scoped per window label, so a window left out of
that list gets no command permissions at all (an easy thing to miss when
adding a new window).

The overlay is frameless, transparent, always-on-top, and `focus: false` —
it never steals OS input focus, so whatever app the user is dictating into
keeps focus throughout recording and transcription (this is also what
makes text injection above land in the right place). It's positioned
bottom-center of the current monitor in logical units (accounting for
display scaling), with a fixed approximate margin above the taskbar (Tauri
has no direct taskbar-bounds API). `overlay::show()`/`schedule_hide()` are
called from: the hotkey handler (show on press, schedule-hide on release),
the speech worker (schedule-hide again after transcription settles, so the
result is visible briefly before fading), and the tray icon's left-click
(show + schedule-hide, for manually summoning it back after it's
auto-hidden). `HIDE_DELAY` (10s) governs how long it stays visible once
idle; the app process itself keeps running regardless of overlay
visibility.

Tray click behavior: **left-click** shows the overlay; **right-click**
shows the context menu (Start/Stop Recording, Settings…, Quit) — driven by
`TrayIconBuilder::on_tray_icon_event` matching `TrayIconEvent::Click {
button: MouseButton::Left, button_state: MouseButtonState::Up, .. }`, with
`show_menu_on_left_click(false)` so the two don't conflict.

## App data paths

Recordings and models (and later, database/logs) are resolved via Tauri's
own `app.path().app_data_dir()`, not the `dirs` crate directly, to keep path
resolution consistent with the rest of the framework. The model lives at
`<app_data_dir>/models/ggml-base.en.bin` — this is the actual runtime path;
the repo-level `models/` directory is a dev-convenience placeholder only and
is never read by the running app (see "Repository layout" above).

## Status

**Phase 1 — complete.** Desktop shell (Tauri 2 + React + Tailwind), system
tray with Idle/Recording icon states, global push-to-talk hotkey
(`Ctrl+Space`) that works while unfocused, and microphone recording to a
16kHz mono WAV file in the app data directory.

**Phase 2 — complete.** Local batch transcription via `whisper-rs`
(`ggml-base.en.bin`, CPU-only), a persistent speech worker thread, on-demand
model download with progress UI, and `.txt` transcript sidecars.

**Phase 3 slice — complete.** System-wide text injection (clipboard-paste
simulation), a floating auto-hiding overlay (replacing the original full-size
dashboard window), a separate Settings window (hotkey rebinding + transcript
manager), and tray left-click-to-summon behavior.

Out of scope, by design, forever: any LLM-based formatting, cleanup, or
LLM/NLU-driven features (voice commands, etc.) — this product stays a pure
local STT pipeline.

Out of scope for now (deferred to later phases): VAD, real-time streaming
transcription, a real model manager UI (switch/re-download/delete models),
functional input-device selection, history database beyond `.txt` sidecars,
custom vocabulary, multi-language, packaging/installers, CI, and
macOS/Linux verification (development has been Windows-only since Phase 1).

Next: real-time streaming transcription + further Settings features.
