# OpenVoiceFlow

Open-source, privacy-first, local-first voice dictation for the desktop.
Hold a hotkey, speak, and the transcribed text is typed at your cursor —
wherever you're working. Transcription runs entirely on-device via
[Whisper](https://github.com/openai/whisper) (through
[whisper.cpp](https://github.com/ggerganov/whisper.cpp)); no account, no
cloud API, no network required after the one-time model download.

## Features

- **Global push-to-talk hotkey** (`Ctrl+Space` by default, rebindable) —
  works system-wide, regardless of which window has focus.
- **Local speech-to-text** — runs entirely on your machine, CPU-only, via
  `whisper-rs`/whisper.cpp. No LLM, no cloud dependency, ever.
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
  alongside each other on disk; nothing is uploaded anywhere.

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

No LLM-based text formatting or cloud provider is planned — this project's
whole premise is a local speech pipeline that's accurate and fast enough
on its own.

## Contributing

Issues and pull requests are welcome. The codebase is small and organized
by concern (see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)) — a good
first step is reading that file, then the module you want to change.

## License

[MIT](LICENSE)
