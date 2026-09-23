import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";
import type {
  ModelDownloadFailedPayload,
  ModelDownloadProgressPayload,
  ModelStatus,
  RecordingInfo,
} from "./types";

type Tab = "hotkey" | "transcripts";

const MODIFIER_CODES = new Set(["ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight"]);

function codeToKeyToken(code: string): string {
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  return code;
}

function captureShortcut(event: KeyboardEvent): string | null {
  if (MODIFIER_CODES.has(event.code)) return null;
  const tokens: string[] = [];
  if (event.ctrlKey) tokens.push("Ctrl");
  if (event.altKey) tokens.push("Alt");
  if (event.shiftKey) tokens.push("Shift");
  if (event.metaKey) tokens.push("Super");
  tokens.push(codeToKeyToken(event.code));
  return tokens.join("+");
}

function formatTimestamp(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString();
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb.toFixed(1)} KB`;
  return `${(kb / 1024).toFixed(1)} MB`;
}

function HotkeyTab() {
  const [current, setCurrent] = useState<string>("");
  const [capturing, setCapturing] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const capturingRef = useRef(capturing);
  capturingRef.current = capturing;

  useEffect(() => {
    invoke<string>("get_hotkey").then(setCurrent);
  }, []);

  useEffect(() => {
    if (!capturing) return;
    const onKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      const shortcut = captureShortcut(event);
      if (shortcut) {
        setPending(shortcut);
        setCapturing(false);
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [capturing]);

  const save = useCallback(async () => {
    if (!pending) return;
    setSaving(true);
    setError(null);
    try {
      await invoke("set_hotkey", { shortcut: pending });
      setCurrent(pending);
      setPending(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }, [pending]);

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h2 className="text-xs font-semibold uppercase tracking-wide text-neutral-500 dark:text-neutral-400">
          Push-to-talk shortcut
        </h2>
        <p className="mt-1 text-xs text-neutral-400 dark:text-neutral-500">
          Hold this key combination anywhere to dictate.
        </p>
      </div>

      <div className="flex items-center gap-3 rounded-xl border border-neutral-200 bg-white p-4 dark:border-neutral-800 dark:bg-neutral-900">
        <kbd className="rounded-lg bg-neutral-100 px-3 py-2 font-mono text-sm dark:bg-neutral-800">
          {pending ?? (current || "…")}
        </kbd>
        {!capturing && !pending && (
          <button
            type="button"
            onClick={() => setCapturing(true)}
            className="rounded-lg border border-neutral-300 px-3 py-1.5 text-xs font-medium hover:bg-neutral-100 dark:border-neutral-700 dark:hover:bg-neutral-800"
          >
            Change…
          </button>
        )}
        {capturing && (
          <span className="text-xs text-neutral-400">Press a key combination…</span>
        )}
        {pending && (
          <div className="flex gap-2">
            <button
              type="button"
              onClick={save}
              disabled={saving}
              className="rounded-lg bg-blue-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-blue-700 disabled:opacity-50"
            >
              Save
            </button>
            <button
              type="button"
              onClick={() => setPending(null)}
              className="rounded-lg border border-neutral-300 px-3 py-1.5 text-xs font-medium hover:bg-neutral-100 dark:border-neutral-700 dark:hover:bg-neutral-800"
            >
              Cancel
            </button>
          </div>
        )}
      </div>

      {error && <p className="text-xs text-red-600 dark:text-red-400">{error}</p>}
    </div>
  );
}

function TranscriptsTab() {
  const [recordings, setRecordings] = useState<RecordingInfo[]>([]);
  const [copiedPath, setCopiedPath] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const list = await invoke<RecordingInfo[]>("list_recordings");
      setRecordings(list);
    } catch (err) {
      console.error("Failed to list recordings", err);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const copy = useCallback(async (path: string, transcript: string) => {
    try {
      await navigator.clipboard.writeText(transcript);
      setCopiedPath(path);
      window.setTimeout(() => setCopiedPath((p) => (p === path ? null : p)), 1500);
    } catch (err) {
      console.error("Failed to copy transcript", err);
    }
  }, []);

  const remove = useCallback(
    async (path: string) => {
      try {
        await invoke("delete_recording", { path });
        refresh();
      } catch (err) {
        console.error("Failed to delete recording", err);
      }
    },
    [refresh],
  );

  return (
    <div className="flex flex-col gap-2">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-neutral-500 dark:text-neutral-400">
        Transcripts
      </h2>
      {recordings.length === 0 ? (
        <p className="rounded-xl border border-dashed border-neutral-200 p-4 text-center text-xs text-neutral-400 dark:border-neutral-800 dark:text-neutral-500">
          No recordings yet.
        </p>
      ) : (
        <ul className="flex flex-col gap-2">
          {recordings.map((rec) => (
            <li
              key={rec.path}
              className="rounded-xl border border-neutral-200 bg-white p-3 text-sm dark:border-neutral-800 dark:bg-neutral-900"
            >
              <div className="flex items-center justify-between">
                <span className="text-xs text-neutral-500 dark:text-neutral-400">
                  {formatTimestamp(rec.created_at)}
                </span>
                <div className="flex shrink-0 items-center gap-2">
                  <span className="text-xs text-neutral-400 dark:text-neutral-500">
                    {formatBytes(rec.size_bytes)}
                  </span>
                  {rec.transcript && (
                    <button
                      type="button"
                      onClick={() => copy(rec.path, rec.transcript ?? "")}
                      className="rounded-md border border-neutral-300 px-2 py-0.5 text-xs hover:bg-neutral-100 dark:border-neutral-700 dark:hover:bg-neutral-800"
                    >
                      {copiedPath === rec.path ? "Copied" : "Copy"}
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={() => remove(rec.path)}
                    className="rounded-md border border-red-300 px-2 py-0.5 text-xs text-red-600 hover:bg-red-50 dark:border-red-900 dark:text-red-400 dark:hover:bg-red-950"
                  >
                    Delete
                  </button>
                </div>
              </div>
              <p className="mt-1 text-sm text-neutral-700 dark:text-neutral-300">
                {rec.transcript ?? (
                  <span className="italic text-neutral-400 dark:text-neutral-500">
                    No transcript yet.
                  </span>
                )}
              </p>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function ModelBanner() {
  const [status, setStatus] = useState<ModelStatus | null>(null);
  const [progress, setProgress] = useState<{ downloaded: number; total: number } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    invoke<ModelStatus>("get_model_status").then(setStatus).catch(() => {});
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    const unlistenProgress = listen<ModelDownloadProgressPayload>(
      "model-download-progress",
      (event) =>
        setProgress({
          downloaded: event.payload.downloaded_bytes,
          total: event.payload.total_bytes,
        }),
    );
    const unlistenCompleted = listen("model-download-completed", () => {
      setProgress(null);
      refresh();
    });
    const unlistenFailed = listen<ModelDownloadFailedPayload>("model-download-failed", (event) => {
      setProgress(null);
      setError(event.payload.error);
    });
    return () => {
      unlistenProgress.then((f) => f());
      unlistenCompleted.then((f) => f());
      unlistenFailed.then((f) => f());
    };
  }, [refresh]);

  const startDownload = useCallback(async () => {
    setError(null);
    setProgress({ downloaded: 0, total: 0 });
    try {
      await invoke("download_model");
    } catch (err) {
      setProgress(null);
      setError(String(err));
    }
  }, []);

  if (!status || status.available) return null;

  return (
    <section className="rounded-xl border border-amber-300 bg-amber-50 p-4 text-sm dark:border-amber-800 dark:bg-amber-950">
      <p className="font-medium text-amber-800 dark:text-amber-200">Speech model not downloaded yet</p>
      <p className="mt-1 text-xs text-amber-700 dark:text-amber-300">
        Transcription needs a one-time ~148MB download before it can run.
      </p>
      {progress ? (
        <div className="mt-3">
          <div className="h-2 w-full overflow-hidden rounded-full bg-amber-200 dark:bg-amber-900">
            <div
              className="h-full rounded-full bg-amber-600 transition-[width]"
              style={{
                width:
                  progress.total > 0
                    ? `${Math.round((progress.downloaded / progress.total) * 100)}%`
                    : "10%",
              }}
            />
          </div>
        </div>
      ) : (
        <button
          type="button"
          onClick={startDownload}
          className="mt-3 rounded-lg bg-amber-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-amber-700"
        >
          Download model
        </button>
      )}
      {error && <p className="mt-2 text-xs text-red-600 dark:text-red-400">{error}</p>}
    </section>
  );
}

function SettingsApp() {
  const [tab, setTab] = useState<Tab>("hotkey");

  return (
    <main className="min-h-screen bg-neutral-50 text-neutral-900 dark:bg-neutral-950 dark:text-neutral-50">
      <div className="mx-auto flex min-h-screen max-w-lg flex-col gap-6 px-6 py-8">
        <header>
          <h1 className="text-lg font-semibold tracking-tight">Settings</h1>
        </header>

        <ModelBanner />

        <nav className="flex gap-1 rounded-lg bg-neutral-100 p-1 dark:bg-neutral-900">
          {(
            [
              ["hotkey", "Hotkey"],
              ["transcripts", "Transcripts"],
            ] as [Tab, string][]
          ).map(([value, label]) => (
            <button
              key={value}
              type="button"
              onClick={() => setTab(value)}
              className={`flex-1 rounded-md px-3 py-1.5 text-sm font-medium transition-colors ${
                tab === value
                  ? "bg-white text-neutral-900 shadow-sm dark:bg-neutral-800 dark:text-neutral-50"
                  : "text-neutral-500 hover:text-neutral-700 dark:text-neutral-400 dark:hover:text-neutral-200"
              }`}
            >
              {label}
            </button>
          ))}
        </nav>

        <div className="flex-1">{tab === "hotkey" ? <HotkeyTab /> : <TranscriptsTab />}</div>
      </div>
    </main>
  );
}

export default SettingsApp;
