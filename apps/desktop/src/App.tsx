import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";
import type { EngineTask } from "./EnginePanel";
import type {
  AudioLevelPayload,
  ModelStatus,
  RecordingErrorPayload,
  RecordingStartedPayload,
  RecordingStoppedPayload,
  TranscriptionFailedPayload,
  TranscriptionStartedPayload,
} from "./types";

type Phase = "idle" | "listening" | "transcribing";

const BAR_COUNT = 18;
// Precomputed once (not per render) so each bar has a fixed personality —
// real audio level drives the target, this just keeps them from all moving
// in perfect lockstep like a single shared value would.
const BAR_CONFIG = Array.from({ length: BAR_COUNT }, () => ({
  weight: 0.5 + Math.random() * 0.5,
  phase: Math.random() * Math.PI * 2,
}));

function useLatest<T>(value: T) {
  const ref = useRef(value);
  ref.current = value;
  return ref;
}

function Waveform({ level }: { level: number }) {
  const levelRef = useLatest(level);
  const barRefs = useRef<Array<HTMLSpanElement | null>>([]);
  const displayed = useRef<number[]>(new Array(BAR_COUNT).fill(0.1));

  useEffect(() => {
    let raf = 0;
    const start = performance.now();

    const tick = (now: number) => {
      const t = (now - start) / 1000;
      const lvl = levelRef.current;
      for (let i = 0; i < BAR_COUNT; i++) {
        const { weight, phase } = BAR_CONFIG[i];
        const jitter = 0.08 * Math.sin(t * 6 + phase);
        const target = Math.min(1, Math.max(0.08, lvl * weight + jitter));
        displayed.current[i] += (target - displayed.current[i]) * 0.35;
        const el = barRefs.current[i];
        if (el) el.style.height = `${Math.round(displayed.current[i] * 100)}%`;
      }
      raf = requestAnimationFrame(tick);
    };

    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [levelRef]);

  return (
    <div className="flex h-5 w-full items-end gap-[2px]">
      {BAR_CONFIG.map((_, i) => (
        <span
          key={i}
          ref={(el) => {
            barRefs.current[i] = el;
          }}
          className="flex-1 rounded-full bg-red-400"
          style={{ height: "10%" }}
        />
      ))}
    </div>
  );
}

function App() {
  const [engineTask, setEngineTask] = useState<EngineTask | null>(null);
  useEffect(() => {
    const subscription = listen<EngineTask>("engine-task", ({ payload }) => {
      if (payload.status !== "queued") setEngineTask(payload);
    });
    void subscription.then(() => invoke<EngineTask[]>("get_engine_tasks")).then(tasks => {
      setEngineTask(current => current ?? tasks.find(t => !["queued", "completed", "failed", "cancelled"].includes(t.status)) ?? tasks[0] ?? null);
    }).catch(() => {});
    return () => { void subscription.then(unlisten => unlisten()); };
  }, []);
  const [phase, setPhase] = useState<Phase>("idle");
  const [level, setLevel] = useState(0);
  const [modelMissing, setModelMissing] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const levelDecayRef = useRef<number | null>(null);
  const errorClearRef = useRef<number | null>(null);

  useEffect(() => {
    invoke<ModelStatus>("get_model_status")
      .then((status) => setModelMissing(!status.available))
      .catch(() => {});
  }, []);

  useEffect(() => {
    const unlistenStarted = listen<RecordingStartedPayload>("recording-started", () => {
      setPhase("listening");
      setLevel(0);
    });

    const unlistenStopped = listen<RecordingStoppedPayload>("recording-stopped", () => {
      setPhase("transcribing");
      setLevel(0);
    });

    const unlistenTranscriptionStarted = listen<TranscriptionStartedPayload>(
      "transcription-started",
      () => setPhase("transcribing"),
    );

    const unlistenTranscriptionCompleted = listen("transcription-completed", () => {
      setPhase("idle");
    });

    const unlistenTranscriptionFailed = listen<TranscriptionFailedPayload>(
      "transcription-failed",
      () => setPhase("idle"),
    );

    const unlistenError = listen<RecordingErrorPayload>("recording-error", (event) => {
      setErrorMessage(event.payload.message);
      if (errorClearRef.current !== null) {
        window.clearTimeout(errorClearRef.current);
      }
      errorClearRef.current = window.setTimeout(() => setErrorMessage(null), 4000);
    });

    const unlistenLevel = listen<AudioLevelPayload>("audio-level", (event) => {
      setLevel(Math.min(1, event.payload.rms * 6));
      if (levelDecayRef.current !== null) {
        window.clearTimeout(levelDecayRef.current);
      }
      levelDecayRef.current = window.setTimeout(() => setLevel(0), 400);
    });

    return () => {
      unlistenStarted.then((f) => f());
      unlistenStopped.then((f) => f());
      unlistenTranscriptionStarted.then((f) => f());
      unlistenTranscriptionCompleted.then((f) => f());
      unlistenTranscriptionFailed.then((f) => f());
      unlistenError.then((f) => f());
      unlistenLevel.then((f) => f());
    };
  }, []);

  const openSettings = useCallback((event: React.MouseEvent) => {
    event.preventDefault();
    invoke("open_settings_window").catch((err) => console.error("Failed to open settings", err));
  }, []);

  return (
    <div
      onContextMenu={openSettings}
      className="flex h-screen w-screen items-center justify-center bg-transparent"
    >
      <div className="flex h-full w-full items-center gap-2 rounded-xl border border-white/10 bg-neutral-900/95 px-3 shadow-xl">
        <div className="min-w-0 flex-1 overflow-hidden">
          {phase === "idle" && (
            <>
              <p className="truncate text-[11px] font-semibold leading-tight text-white">
                {engineTask && !["completed", "cancelled", "failed"].includes(engineTask.status) ? engineTask.status.replace(/_/g, " ") : "OpenVoiceFlow"}
              </p>
              <p
                className={`truncate text-[9px] leading-tight ${
                  errorMessage ? "text-red-400" : "text-white/40"
                }`}
              >
                {errorMessage ?? engineTask?.message ??
                  (modelMissing ? "Model missing — right-click" : "Hold Ctrl+Space")}
              </p>
            </>
          )}
          {phase === "listening" && <Waveform level={level} />}
          {phase === "transcribing" && (
            <p className="truncate text-[11px] font-medium text-amber-300">Transcribing…</p>
          )}
        </div>

        {engineTask && !["completed", "cancelled", "failed"].includes(engineTask.status) && <button
          type="button" title="Cancel current task" aria-label="Cancel current task"
          className="text-xs text-red-300" onClick={() => void invoke("cancel_engine_task", { taskId: engineTask.id })}>Stop</button>}

        {modelMissing && phase === "idle" && (
          <span
            className="h-1.5 w-1.5 shrink-0 rounded-full bg-amber-400"
            title="Speech model not downloaded"
          />
        )}
      </div>
    </div>
  );
}

export default App;
