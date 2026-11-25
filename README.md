# OpenVoiceFlow

Open-source, local-first voice dictation and desktop actions. Hold a hotkey,
speak, and the transcribed text is typed at your cursor — wherever you're
working. Transcription runs entirely on-device via
[Whisper](https://github.com/openai/whisper) (through
[whisper.cpp](https://github.com/ggerganov/whisper.cpp)). Ordinary dictation is
inserted directly and stops. Clear app-launch requests are resolved locally
and run directly. Complex desktop tasks use the selected AI provider. Audio
is never sent; accessible application context is sent, and screenshots are
sent to an optional vision model when needed.

## Features

- **Global push-to-talk hotkey** (`Ctrl+Space` by default, rebindable) —
  works system-wide, regardless of which window has focus.
- **Local speech-to-text** — runs on your machine, CPU-only, via
  `whisper-rs`/whisper.cpp. The selected AI provider receives recognized text
  only when configured.
- **Automatic text injection** — the transcript is typed at your cursor in
  whatever app you're focused on, via clipboard-paste simulation (your
  existing clipboard contents are preserved and restored afterward).
- **A small floating overlay**, not a full window — appears near the
  taskbar when you start dictating, shows a live waveform while recording,
  and auto-hides after a few seconds idle.
- **System tray** — left-click to summon the overlay on demand, right-click
  for the menu (start/stop recording, settings, quit).
- **Settings window** — rebind the push-to-talk shortcut, and browse/copy/
  delete past recordings and their transcripts.
- **Local history** — every recording and its transcript are saved
  alongside each other on disk; audio remains on the device.
- **LLM action engine** — discover/open/focus applications, type text, send
  keyboard shortcuts, and move/click/scroll the pointer. The model observes
  results after each step; common direct app launches use a local fast path.
- **Engine task controls** — view queued/running tasks, cancel, answer questions,
  approve consequential actions, and resume after manual login.

## Installation

Download the latest installer (`OpenVoiceFlow_x.y.z_x64-setup.exe`) from
the [Releases](../../releases) page and run it. Windows only for now (see
[Roadmap](#roadmap)).

## Usage

1. Launch OpenVoiceFlow — it lives in the system tray, no window opens.
2. The first time you dictate, you'll be prompted to download the speech
   model (~148MB, one-time, local from then on).
3. Hold `Ctrl+Space`, speak, release. The transcript is typed at your
   cursor a moment later.
4. Right-click the tray icon (or the overlay itself) to open **Settings**
   and change the hotkey or review past transcripts.

Open **Settings → AI Provider** and select Ollama, OpenAI, Anthropic, xAI, or
Groq. Ollama connects to `http://localhost:11434` and needs no API key; enter a
model name available from `ollama list`. Hosted providers need a model supporting
tool calls and an API key, stored per provider in Windows Credential Manager.
Save the settings and test the connection. Jev's classification interface
cannot run the engine. Provider failures preserve the transcript and report an
error rather than pasting the instruction into an arbitrary field.

**Settings → Engine** shows tasks and controls, optional vision provider/model
selection, and additional executable registrations. Accessibility context is
used first; screenshots require a configured model supporting images and tools.
Sign-in and protected prompts are handled manually. Application content is
untrusted context; model-declared sending/deleting/purchasing actions require
explicit confirmation. Completion is model-evaluated from observed context.

The engine defaults to 40 actions, five minutes of active execution, and two
recovery attempts per failed step. Screenshots stay in memory. Local
`engine-events.jsonl` contains task IDs/status/step counts, without credentials
or screenshots. Provider diagnostics in the dev log include provider/model,
elapsed time, status, and error class, without transcripts or screenshots.
Cancelling prevents further actions; an outstanding provider request may take
up to 30 seconds for hosted providers or 120 seconds for local Ollama to return
and is then discarded.

## Development

Requires:

- [Node.js](https://nodejs.org) ≥ 20.19
- Rust (stable, MSVC target on Windows) via [rustup](https://rustup.rs)
- CMake and LLVM/Clang on `PATH` (needed to build whisper.cpp — set
  `LIBCLANG_PATH` to your LLVM `bin` directory)
- The Visual Studio "Desktop development with C++" workload (Windows)

```sh
npm install
npm run dev      # launches the app in dev mode (hot-reloading frontend)
npm run build    # type-checks and builds the frontend only
```

To build a release installer:

```sh
cd apps/desktop
npm run tauri build
```

The desktop app lives in [apps/desktop](apps/desktop); its Rust backend is
in [apps/desktop/src-tauri](apps/desktop/src-tauri), organized as one
module per concern (`audio/`, `speech/`, `hotkeys/`, `injection/`,
`overlay/`, `tray/`). See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for
how the pieces fit together — the audio capture pipeline, the speech
worker, the overlay/settings window setup, and the design decisions behind
each.

## Roadmap

Not yet built, in rough priority order:

- Real-time streaming transcription (today's transcription runs after you
  release the hotkey, not word-by-word while you speak)
- Voice activity detection (auto-stop on silence)
- A proper model manager UI (switch model size, re-download, delete)
- macOS and Linux support (development and testing so far is Windows-only)
- Packaging for other platforms / auto-update

Speech recognition and action execution remain local. A selected hosted
provider plans actions from application context; Rust validates typed tool
requests and executes desktop operations. The engine has no shell-execution tool.

## Contributing

Issues and pull requests are welcome. The codebase is small and organized
by concern (see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)) — a good
first step is reading that file, then the module you want to change.

## License

[MIT](LICENSE)
