import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface EngineTask {
  id: number; transcript: string; status: string; message: string;
  application: string | null; steps: number;
}
type Provider = "ollama" | "open_ai" | "anthropic" | "xai" | "groq";
interface RegisteredApp { id: string; name: string; executable: string; arguments: string[] }
interface EngineConfig { vision: { provider: Provider; model: string } | null; applications: RegisteredApp[] }
const terminal = new Set(["completed", "failed", "cancelled"]);
const button = "rounded-lg border border-neutral-300 px-3 py-2 text-xs dark:border-neutral-700 disabled:opacity-50";
const input = "w-full rounded-lg border border-neutral-300 bg-transparent px-3 py-2 text-sm dark:border-neutral-700";

export default function EnginePanel() {
  const [tasks, setTasks] = useState<EngineTask[]>([]);
  const [config, setConfig] = useState<EngineConfig>({ vision: null, applications: [] });
  const [replies, setReplies] = useState<Record<number, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  useEffect(() => {
    const subscription = listen<EngineTask>("engine-task", ({ payload }) => {
      setTasks(current => [payload, ...current.filter(t => t.id !== payload.id)].sort((a, b) => b.id - a.id).slice(0, 100));
    });
    subscription.then(() => invoke<EngineTask[]>("get_engine_tasks")).then(initial => setTasks(current => {
      const merged = new Map(initial.map(task => [task.id, task]));
      current.forEach(task => merged.set(task.id, task));
      return [...merged.values()].sort((a, b) => b.id - a.id).slice(0, 100);
    })).catch(e => setError(String(e)));
    invoke<EngineConfig>("get_engine_config").then(setConfig).catch(e => setError(String(e)));
    return () => { void subscription.then(unlisten => unlisten()); };
  }, []);
  const call = async (command: string, args: Record<string, unknown>) => {
    setBusy(true); setError(""); setMessage("");
    try { await invoke(command, args); if (command === "save_engine_config") setMessage("Engine settings saved."); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  return <div className="flex flex-col gap-5">
    <section className="flex flex-col gap-3">
      <h2 className="font-semibold">Tasks</h2>
      <p className="text-xs text-neutral-500">Dictation inserts the exact transcript into the focused field and then stops. Routine desktop actions run directly; consequential actions such as sending or deleting still require approval.</p>
      {tasks.length === 0 && <p className="text-sm text-neutral-500">No tasks yet.</p>}
      {tasks.map(task => <article key={task.id} className="flex flex-col gap-2 rounded-lg border border-neutral-300 p-3 dark:border-neutral-700">
        <div className="flex justify-between gap-2"><strong className="text-xs">Task {task.id} · {task.status.replace(/_/g, " ")}</strong><span className="text-xs text-neutral-500">{task.steps} actions</span></div>
        <p className="break-words text-sm">{task.transcript}</p>
        {task.application && <p className="truncate text-xs text-neutral-500">{task.application}</p>}
        <p className="break-words text-xs" role="status">{task.message}</p>
        {["needs_clarification", "awaiting_confirmation"].includes(task.status) && <>
          <textarea aria-label={`Reply to task ${task.id}`} className={input} value={replies[task.id] ?? ""} onChange={e => setReplies(v => ({ ...v, [task.id]: e.target.value }))} placeholder="Answer the question, or resume after completing the manual step" />
          <button className={button} disabled={busy} onClick={() => void call("resume_engine_task", { taskId: task.id, message: replies[task.id] ?? "", approved: task.status === "awaiting_confirmation" })}>{task.status === "awaiting_confirmation" ? (task.steps === 0 ? "Approve command" : "Approve this action") : "Resume"}</button>
        </>}
        {!terminal.has(task.status) && <button className={button} disabled={busy} onClick={() => void call("cancel_engine_task", { taskId: task.id })}>Cancel task</button>}
      </article>)}
    </section>
    <section className="flex flex-col gap-3 border-t border-neutral-300 pt-4 dark:border-neutral-700">
      <h2 className="font-semibold">Vision fallback</h2>
      <p className="text-xs text-neutral-500">Accessibility controls are used first. When needed, a screenshot of the target window is sent to this model. Configure the provider and model in AI Provider. Screenshots are not saved.</p>
      <label className="text-xs"><input type="checkbox" checked={config.vision !== null} onChange={e => setConfig(c => ({ ...c, vision: e.target.checked ? { provider: "open_ai", model: "" } : null }))} /> Enable vision fallback</label>
      {config.vision && <>
        <label className="text-xs">Vision provider<select className={input} value={config.vision.provider} onChange={e => setConfig(c => ({ ...c, vision: { ...c.vision!, provider: e.target.value as Provider } }))}>
          <option value="ollama">Ollama (local)</option><option value="open_ai">OpenAI</option><option value="anthropic">Anthropic</option><option value="xai">Grok (xAI)</option><option value="groq">Groq</option>
        </select></label>
        <label className="text-xs">Vision-capable model ID<input className={input} value={config.vision.model} onChange={e => setConfig(c => ({ ...c, vision: { ...c.vision!, model: e.target.value } }))} placeholder="Model supporting images and tool calls" /></label>
      </>}
    </section>
    <section className="flex flex-col gap-3">
      <h2 className="font-semibold">Additional applications</h2>
      <p className="text-xs text-neutral-500">Installed applications are discovered automatically. Register another executable if it is missing.</p>
      {config.applications.map((application, index) => <div key={index} className="flex flex-col gap-2 rounded border border-neutral-300 p-3 dark:border-neutral-700">
        {(["name", "executable"] as const).map(field => <label className="text-xs" key={field}>{field === "name" ? "Application name" : "Absolute executable path"}<input className={input} value={application[field]} onChange={e => setConfig(c => ({ ...c, applications: c.applications.map((a, i) => i === index ? { ...a, [field]: e.target.value } : a) }))} /></label>)}
        <label className="text-xs">Startup arguments (one per line)<textarea className={input} value={application.arguments.join("\n")} onChange={e => setConfig(c => ({ ...c, applications: c.applications.map((a, i) => i === index ? { ...a, arguments: e.target.value ? e.target.value.split("\n") : [] } : a) }))} /></label>
        <button className={button} onClick={() => setConfig(c => ({ ...c, applications: c.applications.filter((_, i) => i !== index) }))}>Remove registration</button>
      </div>)}
      <button className={button} onClick={() => setConfig(c => ({ ...c, applications: [...c.applications, { id: `custom-${crypto.randomUUID()}`, name: "", executable: "", arguments: [] }] }))}>Register application</button>
      <button className={button} disabled={busy || (!!config.vision && !config.vision.model.trim())} onClick={() => void call("save_engine_config", { config })}>Save engine settings</button>
    </section>
    {error && <p role="alert" className="text-xs text-red-500">{error}</p>}
    {message && <p role="status" className="text-xs text-emerald-500">{message}</p>}
  </div>;
}
