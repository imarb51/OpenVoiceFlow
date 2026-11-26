import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { currentMonitor, getCurrentWindow, LogicalPosition, LogicalSize } from "@tauri-apps/api/window";
import "./App.css";
import type { EngineTask } from "./EnginePanel";
import type {
  AudioLevelPayload,
  ModelStatus,
  RecordingErrorPayload,
  RecordingStartedPayload,
  RecordingStoppedPayload,
  TranscriptionCompletedPayload,
  TranscriptionFailedPayload,
  TranscriptionStartedPayload,
} from "./types";

type Phase = "idle" | "listening" | "transcribing";
type ChatMessage = { id: string; role: "user" | "assistant"; text: string; state: "active" | "leaving" };
const BAR_COUNT = 28;
const TERMINAL = new Set(["completed", "failed", "cancelled"]);
const ATTENTION = new Set(["awaiting_confirmation", "needs_clarification"]);

const BAR_CONFIG = Array.from({ length: BAR_COUNT }, (_, index) => ({
  weight: 0.18 + 0.82 * Math.pow(Math.sin((index + 1) / (BAR_COUNT + 1) * Math.PI), 0.8),
  phase: index * 0.34,
}));

function useLatest<T>(value: T) {
  const ref = useRef(value);
  ref.current = value;
  return ref;
}

function Waveform({ level }: { level: number }) {
  const levelRef = useLatest(level);
  const barRefs = useRef<Array<HTMLSpanElement | null>>([]);
  const displayed = useRef<number[]>(new Array(BAR_COUNT).fill(0.12));

  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      for (let i = 0; i < BAR_COUNT; i++) {
        const bar = barRefs.current[i];
        if (bar) bar.style.transform = `scaleY(${0.2 + BAR_CONFIG[i].weight * 0.25})`;
      }
      return;
    }
    let raf = 0;
    const start = performance.now();
    const tick = (now: number) => {
      const t = (now - start) / 1000;
      const amplitude = levelRef.current;
      for (let i = 0; i < BAR_COUNT; i++) {
        const { weight, phase } = BAR_CONFIG[i];
        const pulse = 0.08 + 0.12 * (0.5 + 0.5 * Math.sin(t * 4.2 + phase));
        const target = Math.min(1, pulse + amplitude * weight * 1.8);
        displayed.current[i] += (target - displayed.current[i]) * 0.22;
        const bar = barRefs.current[i];
        if (bar) bar.style.transform = `scaleY(${Math.max(0.12, displayed.current[i])})`;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [levelRef]);

  return (
    <div className="voice-wave" role="img" aria-label="Listening to your voice">
      {BAR_CONFIG.map((_, i) => (
        <span key={i} ref={(el) => { barRefs.current[i] = el; }} />
      ))}
    </div>
  );
}

function App() {
  const rootRef = useRef<HTMLDivElement>(null);
  const [engineTask, setEngineTask] = useState<EngineTask | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [level, setLevel] = useState(0);
  const [modelMissing, setModelMissing] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [resultText, setResultText] = useState<string | null>(null);
  const [resultKind, setResultKind] = useState<"transcript" | "notice" | "status">("transcript");
  const [resultVersion, setResultVersion] = useState(0);
  const [reply, setReply] = useState("");
  const [busy, setBusy] = useState(false);
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([]);
  const chatMessagesRef = useRef<ChatMessage[]>([]);
  const levelDecayRef = useRef<number | null>(null);
  const resultClearRef = useRef<number | null>(null);
  const errorClearRef = useRef<number | null>(null);

  const showResult = useCallback((text: string | null, kind: "transcript" | "notice" | "status" = "transcript") => {
    if (resultClearRef.current !== null) window.clearTimeout(resultClearRef.current);
    setResultText(text);
    setResultKind(kind);
    setResultVersion(version => version + 1);
    if (text) resultClearRef.current = window.setTimeout(() => setResultText(null), 6000);
  }, []);

  useEffect(() => {
    const subscription = listen<EngineTask>("engine-task", ({ payload }) => {
      const previous = chatMessagesRef.current;
      const hasUserMessage = previous.some(message => message.id === `task:${payload.id}:user`);
      const next = hasUserMessage ? previous.filter(message => message.state === "active" || message.id === `task:${payload.id}:user`) : [
        { id: `task:${payload.id}:user`, role: "user" as const, text: payload.transcript, state: "active" as const },
      ];
      const assistantMessage: ChatMessage = {
        id: `task:${payload.id}:assistant`, role: "assistant", text: payload.message, state: "active",
      };
      // Keep one user bubble and one up-to-date assistant bubble. Engine
      // progress emits repeatedly; reusing the assistant key avoids duplicate
      // React children and lets its text update in place.
      chatMessagesRef.current = [...next.filter(message => message.role === "user"), assistantMessage].slice(-3);
      setChatMessages(chatMessagesRef.current);
      setEngineTask(payload);
      setReply("");
      if (TERMINAL.has(payload.status)) {
        // Completion is already represented by the assistant chat bubble.
        // Clear the earlier transcription preview so it cannot reappear as
        // the legacy single-card fallback after the bubbles fade away.
        showResult(null);
        // Let the completed conversation remain readable before it drifts
        // upward. Previously it began fading at the same moment completion
        // arrived and was removed after only 900 ms.
        window.setTimeout(() => {
          const taskPrefix = `task:${payload.id}:`;
          // A newer task may have replaced these bubbles during the hold.
          if (!chatMessagesRef.current.some(message => message.id.startsWith(taskPrefix))) return;
          chatMessagesRef.current = chatMessagesRef.current.map(message => message.id.startsWith(taskPrefix)
            ? { ...message, state: "leaving" }
            : message);
          setChatMessages(chatMessagesRef.current);
          window.setTimeout(() => {
            // Do not let an old completion timer clear a newer conversation.
            if (!chatMessagesRef.current.some(message => message.id.startsWith(taskPrefix))) return;
            setEngineTask(current => current?.id === payload.id ? null : current);
            chatMessagesRef.current = chatMessagesRef.current.filter(message => !message.id.startsWith(taskPrefix));
            setChatMessages(chatMessagesRef.current);
          }, 900);
        }, 2200);
      }
    });
    void subscription.then(() => invoke<EngineTask[]>("get_engine_tasks")).then(tasks => {
      const restoredTask = tasks.find(task => !TERMINAL.has(task.status) && task.status !== "queued");
      if (!restoredTask) return;
      setEngineTask(current => current ?? restoredTask);
      if (chatMessagesRef.current.length === 0) {
        const restoredMessages: ChatMessage[] = [
          { id: `task:${restoredTask.id}:user`, role: "user", text: restoredTask.transcript, state: "active" },
          { id: `task:${restoredTask.id}:assistant`, role: "assistant", text: restoredTask.message, state: "active" },
        ];
        chatMessagesRef.current = restoredMessages;
        setChatMessages(restoredMessages);
      }
    }).catch(() => {});
    return () => { void subscription.then(unlisten => unlisten()); };
  }, [showResult]);

  useEffect(() => {
    invoke<ModelStatus>("get_model_status")
      .then((status) => setModelMissing(!status.available))
      .catch(() => {});
  }, []);

  useEffect(() => {
    const unlistenStarted = listen<RecordingStartedPayload>("recording-started", () => {
      showResult(null);
      setErrorMessage(null);
      setPhase("listening");
      setLevel(0);
    });
    const unlistenStopped = listen<RecordingStoppedPayload>("recording-stopped", () => {
      setPhase("transcribing");
      setLevel(0);
    });
    const unlistenTranscriptionStarted = listen<TranscriptionStartedPayload>("transcription-started", () => setPhase("transcribing"));
    const unlistenTranscriptionCompleted = listen<TranscriptionCompletedPayload>("transcription-completed", ({ payload }) => {
      setPhase("idle");
      showResult(payload.transcript.trim() || "No speech recognized", payload.transcript.trim() ? "transcript" : "notice");
    });
    const unlistenTranscriptionFailed = listen<TranscriptionFailedPayload>("transcription-failed", ({ payload }) => {
      setPhase("idle");
      showResult(`Transcription failed · ${payload.error}`, "notice");
    });
    const unlistenError = listen<RecordingErrorPayload>("recording-error", (event) => {
      setErrorMessage(event.payload.message);
      if (errorClearRef.current !== null) window.clearTimeout(errorClearRef.current);
      errorClearRef.current = window.setTimeout(() => setErrorMessage(null), 5000);
    });
    const unlistenLevel = listen<AudioLevelPayload>("audio-level", (event) => {
      setLevel(Math.min(1, event.payload.rms * 6));
      if (levelDecayRef.current !== null) window.clearTimeout(levelDecayRef.current);
      levelDecayRef.current = window.setTimeout(() => setLevel(0), 400);
    });
    return () => {
      unlistenStarted.then(f => f()); unlistenStopped.then(f => f());
      unlistenTranscriptionStarted.then(f => f()); unlistenTranscriptionCompleted.then(f => f());
      unlistenTranscriptionFailed.then(f => f()); unlistenError.then(f => f()); unlistenLevel.then(f => f());
    };
  }, [showResult]);

  // The main Tauri window begins at the compact control-bar size. Content
  // can grow after it is shown (for example, when an engine status card is
  // added), so size from the layout dimensions rather than the viewport. A
  // ResizeObserver alone is not enough here: viewport changes and late font
  // loads do not necessarily change the observed element's border box.
  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    let pending = 0;
    let resizing = false;
    let resizeAgain = false;
    let lastSize = { width: 0, height: 0 };

    const resize = () => {
      if (resizing) {
        resizeAgain = true;
        return;
      }
      if (pending) return;
      pending = requestAnimationFrame(async () => {
        pending = 0;
        const children = Array.from(root.children) as HTMLElement[];
        const contentHeight = children.reduce((total, child) => total + child.getBoundingClientRect().height, 0);
        const style = getComputedStyle(root);
        const gap = Number.parseFloat(style.rowGap) || 0;
        const verticalPadding = (Number.parseFloat(style.paddingTop) || 0) + (Number.parseFloat(style.paddingBottom) || 0);
        // Do not use root.scrollHeight or its viewport bounds here: both can
        // report the old native window height and prevent reliable shrinking
        // or growth. Child boxes are their natural, unclipped layout sizes.
        const messageIsVisible = chatMessages.length > 0 || !!resultText || !!errorMessage
          || (!!engineTask && !TERMINAL.has(engineTask.status));
        // Reserve real native-window space before showing any chat/result.
        // This avoids depending on WebView measurements made while the old,
        // compact viewport is clipping the content.
        const height = Math.max(messageIsVisible ? 96 : 56, Math.ceil(contentHeight + Math.max(0, children.length - 1) * gap + verticalPadding + 12));
        const width = 360;
        if (!width || !height || (width === lastSize.width && height === lastSize.height)) return;
        resizing = true;
        let revealAfterResize = false;
        try {
          const nativeWindow = getCurrentWindow();
          const monitor = await currentMonitor();
          const nativeSize = await nativeWindow.innerSize();
          const scale = monitor?.scaleFactor ?? 1;
          revealAfterResize = height > nativeSize.height / scale + 1;
          if (revealAfterResize) root.style.visibility = "hidden";
          await nativeWindow.setSize(new LogicalSize(width, height));
          if (monitor) {
            const x = monitor.position.x / scale + (monitor.size.width / scale - width) / 2;
            const y = monitor.position.y / scale + monitor.size.height / scale - height - 72;
            await nativeWindow.setPosition(new LogicalPosition(x, y));
          }
          lastSize = { width, height };
        } catch { /* The native window may not be available during hot reload. */ }
        finally {
          resizing = false;
          if (revealAfterResize) requestAnimationFrame(() => { root.style.visibility = "visible"; });
          if (resizeAgain) {
            resizeAgain = false;
            resize();
          }
        }
      });
    };
    const observer = new ResizeObserver(resize);
    observer.observe(root);
    for (const child of Array.from(root.children)) observer.observe(child);
    const mutations = new MutationObserver(resize);
    mutations.observe(root, { childList: true, subtree: true, characterData: true });
    document.fonts?.ready.then(resize).catch(() => {});
    void resize();
    return () => {
      observer.disconnect();
      mutations.disconnect();
      cancelAnimationFrame(pending);
    };
  }, [chatMessages, resultText, errorMessage, engineTask, phase]);

  const openSettings = useCallback((event: React.MouseEvent) => {
    event.preventDefault();
    invoke("open_settings_window").catch((err) => console.error("Failed to open settings", err));
  }, []);

  const runTaskCommand = async (command: string, args: Record<string, unknown>) => {
    if (!engineTask) return;
    setBusy(true);
    try {
      await invoke(command, args);
      setReply("");
    } catch (error) {
      setErrorMessage(String(error));
    } finally {
      setBusy(false);
    }
  };

  const attention = !!engineTask && ATTENTION.has(engineTask.status);
  const activeTask = !!engineTask && !TERMINAL.has(engineTask.status);
  // Keep the recognized text visible briefly after transcription even if the
  // task engine has already emitted its first progress update.
  const dictationAdded = !!engineTask && engineTask.status === "completed"
    && engineTask.message.toLowerCase().startsWith("transcript inserted");
  const cardText = errorMessage ?? (dictationAdded ? engineTask.transcript : activeTask ? engineTask.message : resultText);
  const cardVisible = !!cardText || attention;
  const cardHeading = attention
    ? (engineTask?.status === "awaiting_confirmation" ? "Approval required" : "Needs your input")
    : errorMessage || resultKind === "notice" ? "Notice"
      : activeTask ? "Emo is working"
        : dictationAdded || resultKind === "transcript" ? "Dictation added"
          : "Task update";
  const cardKey = attention
    ? `attention:${engineTask?.id}:${engineTask?.status}:${engineTask?.message}`
    : `${engineTask?.id ?? "result"}:${resultKind}:${dictationAdded ? engineTask?.transcript : cardText}:${resultVersion}`;

  return (
    <div ref={rootRef} onContextMenu={openSettings} className="hud-shell">
      {chatMessages.length > 0 ? chatMessages.map(message => (
        <section key={message.id} className={`message-card message-card--chat ${message.role === "user" ? "message-card--user" : "message-card--assistant"} ${message.role === "assistant" && activeTask ? "message-card--active" : ""} ${message.state === "leaving" ? "message-card--leaving" : ""}`} aria-live={attention ? "assertive" : "polite"}>
          <div className="card-kicker">
            <span className="status-dot" />
            <span>{message.role === "user" ? "You" : "OpenVoiceFlow"}</span>
          </div>
          <p className="card-copy">{message.text}</p>
          {message.role === "assistant" && activeTask && message.state === "active" && (
            <div className="working-dots" aria-label="OpenVoiceFlow is working">
              <span /><span /><span />
            </div>
          )}
          {message.role === "assistant" && attention && engineTask?.status === "needs_clarification" && (
            <textarea className="reply-input" aria-label="Reply to Emo" value={reply} onChange={e => {
              const input = e.currentTarget;
              setReply(input.value);
              input.style.height = "auto";
              input.style.height = `${input.scrollHeight}px`;
            }} placeholder="Type your answer…" rows={1} />
          )}
          {message.role === "assistant" && attention && engineTask && (
            <div className="card-actions">
              {engineTask.status === "awaiting_confirmation" ? (
                <button className="button-primary" disabled={busy} onClick={() => void runTaskCommand("resume_engine_task", { taskId: engineTask.id, message: "Approved", approved: true })}>Approve</button>
              ) : (
                <button className="button-primary" disabled={busy || !reply.trim()} onClick={() => void runTaskCommand("resume_engine_task", { taskId: engineTask.id, message: reply.trim(), approved: false })}>Reply</button>
              )}
              <button className="button-quiet" disabled={busy} onClick={() => void runTaskCommand("cancel_engine_task", { taskId: engineTask.id })}>Cancel task</button>
            </div>
          )}
        </section>
      )) : cardVisible && <section key={cardKey} className="message-card message-card--chat message-card--assistant"><div className="card-kicker"><span className="status-dot" /><span>{cardHeading}</span></div><p className="card-copy">{cardText}</p></section>}

      <div className={`control-bar ${phase === "listening" ? "control-bar--listening" : ""}`}>
        <div className="bar-content">
          {phase === "listening" ? <Waveform level={level} /> : phase === "transcribing" ? (
            <div className="bar-subtitle"><span className="loader" />Transcribing voice</div>
          ) : activeTask ? (
            <div className="bar-subtitle">{engineTask.application ? `Working in ${engineTask.application}` : engineTask.status.replace(/_/g, " ")}</div>
          ) : (
            <div className="bar-subtitle">{modelMissing ? "Speech model missing" : "Hold Ctrl + Space to speak"}</div>
          )}
        </div>
        {activeTask && engineTask && !attention && <button className="bar-stop" title="Cancel task" aria-label="Cancel task" onClick={() => void runTaskCommand("cancel_engine_task", { taskId: engineTask.id })}>STOP</button>}
        {modelMissing && phase === "idle" && <span className="model-indicator" title="Speech model not downloaded" />}
      </div>
    </div>
  );
}

export default App;
