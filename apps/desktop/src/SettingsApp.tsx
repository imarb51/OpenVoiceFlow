import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";
import EnginePanel from "./EnginePanel";
import type {
  ModelDownloadFailedPayload,
  ModelDownloadProgressPayload,
  ModelStatus,
  RecordingInfo,
} from "./types";

type Tab = "hotkey" | "transcripts";
type AiProvider = "jev" | "ollama" | "open_ai" | "anthropic" | "xai" | "groq";

interface AiSettings {
  provider: AiProvider;
  model: string;
  has_api_key: boolean;
}

const PROVIDER_OPTIONS: { id: AiProvider; label: string; model: string }[] = [
  { id: "jev", label: "Jev", model: "jev-latest" },
  { id: "ollama", label: "Ollama (local)", model: "gemma4:12b" },
  { id: "open_ai", label: "OpenAI", model: "gpt-4.1-mini" },
  { id: "anthropic", label: "Claude (Anthropic)", model: "claude-haiku-4-5-20251001" },
  { id: "xai", label: "Grok (xAI)", model: "grok-4.7" },
  { id: "groq", label: "Groq", model: "openai/gpt-oss-20b" },
];

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

function AiProviderTab() {
  const [provider, setProvider] = useState<AiProvider>("jev");
  const [model, setModel] = useState("jev-latest");
  const [apiKey, setApiKey] = useState("");
  const [revealedKey, setRevealedKey] = useState<string | null>(null);
  const [loadingSavedKey, setLoadingSavedKey] = useState(false);
  const currentProviderRef = useRef<AiProvider>("jev");
  const [keyStatusByProvider, setKeyStatusByProvider] = useState<Partial<Record<AiProvider, boolean>>>({});
  const keyStatus = keyStatusByProvider[provider];
  const hasApiKey = keyStatus === true;
  const requiresApiKey = provider !== "ollama";
  const providerLabel = PROVIDER_OPTIONS.find((option) => option.id === provider)?.label ?? provider;
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [localModels, setLocalModels] = useState<string[]>([]);
  const [loadingLocalModels, setLoadingLocalModels] = useState(false);
  const [localModelsError, setLocalModelsError] = useState<string | null>(null);

  const refreshLocalModels = useCallback(async () => {
    setLoadingLocalModels(true);
    setLocalModelsError(null);
    try {
      setLocalModels(await invoke<string[]>("list_ollama_models"));
    } catch (err) {
      setLocalModelsError(String(err));
    } finally {
      setLoadingLocalModels(false);
    }
  }, []);

  useEffect(() => {
    invoke<AiSettings>("get_ai_settings")
      .then((settings) => {
        currentProviderRef.current = settings.provider;
        setProvider(settings.provider);
        setModel(settings.model);
        setKeyStatusByProvider((current) => ({ ...current, [settings.provider]: settings.has_api_key }));
        if (settings.provider === "ollama") void refreshLocalModels();
      })
      .catch((err) => setError(String(err)));
  }, [refreshLocalModels]);

  const selectProvider = useCallback((next: AiProvider) => {
    currentProviderRef.current = next;
    setProvider(next);
    setModel(PROVIDER_OPTIONS.find((option) => option.id === next)?.model ?? "");
    if (next === "ollama") void refreshLocalModels();
    setApiKey("");
    setRevealedKey(null);
    invoke<boolean>("has_ai_api_key", { provider: next })
      .then((hasKey) => setKeyStatusByProvider((current) => ({ ...current, [next]: hasKey })))
      .catch((err) => setError(String(err)));
    setMessage(null);
    setError(null);
  }, [refreshLocalModels]);

  const toggleSavedKeyVisibility = useCallback(async () => {
    if (revealedKey !== null) {
      setRevealedKey(null);
      return;
    }

    const requestedProvider = provider;
    setLoadingSavedKey(true);
    setError(null);
    try {
      const savedKey = await invoke<string | null>("get_ai_api_key", { provider: requestedProvider });
      if (currentProviderRef.current !== requestedProvider) return;
      if (savedKey === null) {
        setKeyStatusByProvider((current) => ({ ...current, [requestedProvider]: false }));
        setError("No saved API key was found for this provider.");
        return;
      }
      setRevealedKey(savedKey);
    } catch (err) {
      if (currentProviderRef.current === requestedProvider) setError(String(err));
    } finally {
      setLoadingSavedKey(false);
    }
  }, [provider, revealedKey]);

  const save = useCallback(async () => {
    setBusy(true);
    setMessage(null);
    setError(null);
    try {
      const settings = await invoke<AiSettings>("save_ai_settings", {
        provider,
        model,
        apiKey: apiKey.trim() || null,
      });
      setKeyStatusByProvider((current) => ({ ...current, [provider]: settings.has_api_key }));
      setApiKey("");
      setRevealedKey(null);
      setMessage(provider === "ollama" ? "Local Ollama settings saved." : "Provider settings saved securely.");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }, [apiKey, model, provider]);

  const testConnection = useCallback(async () => {
    setBusy(true);
    setMessage(null);
    setError(null);
    try {
      const settings = await invoke<AiSettings>("save_ai_settings", {
        provider,
        model,
        apiKey: apiKey.trim() || null,
      });
      setKeyStatusByProvider((current) => ({ ...current, [provider]: settings.has_api_key }));
      setApiKey("");
      setRevealedKey(null);
      await invoke("test_ai_connection", { provider, model });
      setMessage("Connection successful.");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }, [apiKey, model, provider]);

  const removeKey = useCallback(async () => {
    setBusy(true);
    setMessage(null);
    setError(null);
    try {
      await invoke("remove_ai_api_key", { provider });
      setKeyStatusByProvider((current) => ({ ...current, [provider]: false }));
      setApiKey("");
      setRevealedKey(null);
      setMessage("Saved API key removed.");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }, [provider]);

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h2 className="text-xs font-semibold uppercase tracking-wide text-neutral-500 dark:text-neutral-400">
          AI provider
        </h2>
        <p className="mt-1 text-xs text-neutral-400 dark:text-neutral-500">
          Choose the service that routes voice requests to OpenVoiceFlow actions.
        </p>
        {provider === "jev" && <p className="mt-2 text-xs text-amber-600">Jev's classification interface cannot run the action engine. Choose Ollama, OpenAI, Groq, Grok, or Anthropic.</p>}
        {provider === "ollama" && <p className="mt-2 text-xs text-neutral-500">Connects to the local Ollama server at http://localhost:11434. No API key is required; use a model name shown by `ollama list`.</p>}
      </div>

      <label className="flex flex-col gap-1 text-xs font-medium">
        Provider
        <select
          value={provider}
          onChange={(event) => selectProvider(event.target.value as AiProvider)}
          className="rounded-lg border border-neutral-300 bg-white px-3 py-2 text-sm dark:border-neutral-700 dark:bg-neutral-900"
        >
          {PROVIDER_OPTIONS.map((option) => (
            <option key={option.id} value={option.id}>{option.label}</option>
          ))}
        </select>
      </label>

      <label className="flex flex-col gap-1 text-xs font-medium">
        Model ID
        {provider === "ollama" ? (
          <div className="flex gap-2">
            <select
              value={model}
              onChange={(event) => setModel(event.target.value)}
              className="min-w-0 flex-1 rounded-lg border border-neutral-300 bg-white px-3 py-2 font-mono text-sm dark:border-neutral-700 dark:bg-neutral-900"
            >
              {model && !localModels.includes(model) && <option value={model}>{model} (not installed)</option>}
              {localModels.map((name) => <option key={name} value={name}>{name}</option>)}
              {!model && localModels.length === 0 && <option value="">No local models found</option>}
            </select>
            <button
              type="button"
              onClick={() => void refreshLocalModels()}
              disabled={loadingLocalModels}
              className="rounded-lg border border-neutral-300 px-3 py-2 text-xs dark:border-neutral-700 disabled:opacity-50"
            >
              {loadingLocalModels ? "Refreshing…" : "Refresh"}
            </button>
          </div>
        ) : (
          <input
            value={model}
            onChange={(event) => setModel(event.target.value)}
            spellCheck={false}
            className="rounded-lg border border-neutral-300 bg-white px-3 py-2 font-mono text-sm dark:border-neutral-700 dark:bg-neutral-900"
            placeholder="Provider model ID"
          />
        )}
        {provider === "ollama" && localModelsError && <span role="alert" className="font-normal text-red-500">{localModelsError}</span>}
        {provider === "ollama" && !loadingLocalModels && !localModelsError && localModels.length === 0 && <span className="font-normal text-neutral-500">No local models found. Pull a model in Ollama, then refresh.</span>}
      </label>

      {requiresApiKey && <div className="flex flex-col gap-1 text-xs">
        <p className={hasApiKey ? "font-medium text-emerald-600 dark:text-emerald-400" : "font-medium text-neutral-500 dark:text-neutral-400"}>
          {keyStatus === undefined
            ? `Checking saved API key for ${providerLabel}…`
            : hasApiKey
              ? `API key saved securely for ${providerLabel}.`
              : `No API key saved for ${providerLabel}.`}
        </p>
        {hasApiKey && <p className="text-neutral-400 dark:text-neutral-500">To replace it, enter a new key below and save. Use “Show saved key” to view or copy the current key.</p>}
      </div>}

      {requiresApiKey && <label className="flex flex-col gap-1 text-xs font-medium">
        {hasApiKey ? "Replace API key" : "API key"}
        <input
          type={revealedKey === null ? "password" : "text"}
          autoComplete="new-password"
          value={revealedKey ?? apiKey}
          onChange={(event) => {
            setApiKey(event.target.value);
            setRevealedKey(null);
          }}
          className="rounded-lg border border-neutral-300 bg-white px-3 py-2 text-sm dark:border-neutral-700 dark:bg-neutral-900"
          placeholder={hasApiKey ? "Enter a new key to replace the saved key" : "Paste this provider's API key"}
        />
        <span className="font-normal text-neutral-400">
          Stored in Windows Credential Manager. Leave blank to keep the saved key. Ordinary dictation and direct app launches stay local; complex desktop commands are sent to the selected provider.
        </span>
      </label>}

      <div className="flex flex-wrap gap-2">
        {requiresApiKey && hasApiKey && (
          <button
            type="button"
            disabled={busy || loadingSavedKey}
            onClick={toggleSavedKeyVisibility}
            className="rounded-lg border border-neutral-300 px-3 py-1.5 text-xs font-medium hover:bg-neutral-100 dark:border-neutral-700 dark:hover:bg-neutral-800 disabled:opacity-50"
          >
            {loadingSavedKey ? "Loading key…" : revealedKey === null ? "Show saved key" : "Hide saved key"}
          </button>
        )}
        <button
          type="button"
          disabled={busy || !model.trim()}
          onClick={save}
          className="rounded-lg bg-blue-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-blue-700 disabled:opacity-50"
        >
          Save provider
        </button>
        <button
          type="button"
          disabled={busy || !model.trim() || (requiresApiKey && !hasApiKey && !apiKey.trim())}
          onClick={testConnection}
          className="rounded-lg border border-neutral-300 px-3 py-1.5 text-xs font-medium hover:bg-neutral-100 dark:border-neutral-700 dark:hover:bg-neutral-800 disabled:opacity-50"
        >
          Test connection
        </button>
        {requiresApiKey && hasApiKey && (
          <button
            type="button"
            disabled={busy}
            onClick={removeKey}
            className="rounded-lg border border-red-300 px-3 py-1.5 text-xs font-medium text-red-600 hover:bg-red-50 dark:border-red-900 dark:text-red-400 disabled:opacity-50"
          >
            Remove key
          </button>
        )}
      </div>

      {message && <p className="text-xs text-emerald-600 dark:text-emerald-400">{message}</p>}
      {error && <p className="text-xs text-red-600 dark:text-red-400">{error}</p>}
      <p className="text-xs text-neutral-400 dark:text-neutral-500">
        Action execution is limited to the apps supported by OpenVoiceFlow. The app does not execute model-generated shell commands.
      </p>
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
  const [tab, setTab] = useState<Tab | "ai" | "engine">("hotkey");
  useEffect(() => {
    const subscription = listen("engine-needs-attention", () => setTab("engine"));
    invoke<import("./EnginePanel").EngineTask[]>("get_engine_tasks").then(tasks => {
      if (tasks.some(t => ["needs_clarification", "awaiting_confirmation"].includes(t.status))) setTab("engine");
    }).catch(() => {});
    return () => { void subscription.then(unlisten => unlisten()); };
  }, []);

  return (
    <main className="min-h-screen bg-neutral-50 text-neutral-900 dark:bg-neutral-950 dark:text-neutral-50">
      <div className="mx-auto flex min-h-screen max-w-lg flex-col gap-6 px-6 py-8">
        <header>
          <h1 className="text-lg font-semibold tracking-tight">Settings</h1>
        </header>

        <ModelBanner />

        <nav className="grid grid-cols-2 gap-1 rounded-lg bg-neutral-100 p-1 dark:bg-neutral-900">
          {(
            [
              ["hotkey", "Hotkey"],
              ["ai", "AI Provider"],
              ["engine", "Engine"],
              ["transcripts", "Transcripts"],
            ] as [Tab | "ai" | "engine", string][]
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

        <div className="flex-1">
          {tab === "hotkey" ? <HotkeyTab /> : tab === "ai" ? <AiProviderTab /> : tab === "engine" ? <EnginePanel /> : <TranscriptsTab />}
        </div>
      </div>
    </main>
  );
}

export default SettingsApp;
