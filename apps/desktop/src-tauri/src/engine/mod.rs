pub mod config;
pub mod desktop;
pub mod intent;
pub mod model;
#[cfg(test)]
mod tests;
pub mod types;

use std::{
    collections::HashMap,
    io::Write,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use types::*;

pub trait TaskControl {
    fn record_action(&self, _action: &Action) {}
    fn cancelled(&self) -> bool;
    fn update(&self, status: &str, message: &str, observation: Option<&Observation>, steps: usize);
    fn pause(&self, message: &str, confirmation: bool) -> Result<String, String>;
}
#[derive(Clone, Copy)]
pub struct Limits {
    pub actions: usize,
    pub active_time: Duration,
    pub recoveries: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            actions: 40,
            active_time: Duration::from_secs(300),
            recoveries: 2,
        }
    }
}

pub fn run_task(
    brain: &mut dyn Brain,
    desktop: &mut dyn Desktop,
    control: &dyn TaskControl,
    transcript: &str,
    original: WindowTarget,
    limits: Limits,
) -> Result<String, String> {
    let result = run_inner(brain, desktop, control, transcript, original, limits);
    desktop.release_keys();
    result
}
fn run_inner(
    brain: &mut dyn Brain,
    desktop: &mut dyn Desktop,
    control: &dyn TaskControl,
    transcript: &str,
    original: WindowTarget,
    limits: Limits,
) -> Result<String, String> {
    let started = Instant::now();
    let mut paused = Duration::ZERO;
    let mut steps = 0;
    let mut failures = 0;
    let mut turns = 0;
    let mut provider_retries = 0;
    let mut observation = desktop.observe(&original, false)?;
    let mut apps = desktop.catalog();
    let mut history = Vec::new();
    loop {
        if control.cancelled() {
            return Err("Cancelled".into());
        }
        if started.elapsed().saturating_sub(paused) >= limits.active_time
            || turns >= limits.actions * 3
        {
            return Err("Task execution limit reached".into());
        }
        control.update(
            "thinking",
            "Deciding next action",
            Some(&observation),
            steps,
        );
        turns += 1;
        let decision = match brain.decide(transcript, &original, &apps, &observation, &history) {
            Ok(d) => {
                provider_retries = 0;
                d
            }
            Err(e) if e.starts_with("RATE_LIMIT:") => {
                provider_retries += 1;
                if provider_retries > 2 {
                    return Err("Provider rate limit persisted after two waits. Retry later or use a provider with sufficient quota".into());
                }
                let seconds = e
                    .trim_start_matches("RATE_LIMIT:")
                    .parse::<u64>()
                    .unwrap_or(15)
                    .clamp(1, 60);
                control.update(
                    "waiting_provider",
                    &format!("Provider rate limit; retrying in {seconds} seconds"),
                    Some(&observation),
                    steps,
                );
                let deadline = Instant::now() + Duration::from_secs(seconds);
                while Instant::now() < deadline {
                    if control.cancelled() {
                        return Err("Cancelled".into());
                    }
                    if started.elapsed().saturating_sub(paused) >= limits.active_time {
                        return Err(
                            "Task time limit reached while waiting for provider quota".into()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                observation =
                    desktop.observe(&observation.window, observation.screenshot.is_some())?;
                continue;
            }
            Err(e) if e.starts_with("NEEDS_CONFIGURATION:") => {
                let start = Instant::now();
                let reply = control.pause(&e, false)?;
                paused += start.elapsed();
                history.push(Turn {
                    decision: None,
                    result: format!("User resumed after configuration: {reply}"),
                });
                observation =
                    desktop.observe(&observation.window, observation.screenshot.is_some())?;
                continue;
            }
            Err(e) => return Err(e),
        };
        if control.cancelled() {
            return Err("Cancelled".into());
        }
        if started.elapsed().saturating_sub(paused) >= limits.active_time {
            return Err("Task execution time limit reached".into());
        }
        match &decision {
            Decision::Dictate => {
                let action = Action::TypeText {
                    observation_id: observation.id,
                    control_id: None,
                    text: transcript.to_string(),
                };
                if steps >= limits.actions {
                    return Err("Task action limit reached".into());
                }
                control.update(
                    "executing",
                    "Inserting the exact transcript",
                    Some(&observation),
                    steps,
                );
                control.record_action(&action);
                steps += 1;
                desktop.execute(&action, &observation)?;
                control.update(
                    "completed",
                    "Transcript inserted; voice task stopped",
                    Some(&observation),
                    steps,
                );
                return Ok("Transcript inserted".into());
            }
            Decision::LaunchApp { app_id, app_name } => {
                if steps >= limits.actions {
                    return Err("Task action limit reached".into());
                }
                let action = Action::OpenApp {
                    app_id: app_id.clone(),
                };
                control.update(
                    "executing",
                    &format!("Opening {app_name}"),
                    Some(&observation),
                    steps,
                );
                control.record_action(&action);
                steps += 1;
                let target = desktop.execute(&action, &observation)?;
                observation = desktop.observe(&target, false)?;
                control.update(
                    "completed",
                    &format!("Opened {app_name}"),
                    Some(&observation),
                    steps,
                );
                return Ok(format!("Opened {app_name}"));
            }
            Decision::Completed { summary, evidence } => {
                if evidence.trim().is_empty() {
                    return Err("Model claimed completion without evidence".into());
                }
                if observation.screenshot.is_none()
                    && !observation
                        .controls
                        .iter()
                        .any(|c| evidence.contains(&c.id))
                {
                    failures += 1;
                    if failures > limits.recoveries {
                        return Err("Completion could not be grounded in the current application observation".into());
                    }
                    history.push(Turn{decision:Some(decision.clone()),result:"Completion rejected: cite current control IDs and their observed state proving the requested outcome. Use screenshot observation if needed.".into()});
                    continue;
                }
                control.update(
                    "completed",
                    &format!("{summary} — {evidence}"),
                    Some(&observation),
                    steps,
                );
                return Ok(summary.clone());
            }
            Decision::Unable { reason } => return Err(reason.clone()),
            Decision::NeedsClarification { question } => {
                let start = Instant::now();
                let reply = control.pause(question, false)?;
                paused += start.elapsed();
                history.push(Turn {
                    decision: Some(decision.clone()),
                    result: format!("User response: {reply}"),
                });
                observation =
                    desktop.observe(&observation.window, observation.screenshot.is_some())?;
            }
            Decision::Act {
                summary,
                consequential,
                action,
            } => {
                let mut approved_action = action.clone();
                if requires_confirmation(action, &observation, *consequential) {
                    let start = Instant::now();
                    control.pause(summary, true)?;
                    paused += start.elapsed();
                    let fresh =
                        desktop.observe(&observation.window, observation.screenshot.is_some())?;
                    approved_action = rebind(action, &observation, &fresh)?;
                    observation = fresh;
                }
                if control.cancelled() {
                    return Err("Cancelled".into());
                }
                if steps >= limits.actions {
                    return Err("Task action limit reached".into());
                }
                control.update("executing", summary, Some(&observation), steps);
                control.record_action(&approved_action);
                steps += 1;
                let screenshot = matches!(approved_action, Action::Observe { screenshot: true });
                match desktop.execute(&approved_action, &observation) {
                    Ok(target) => {
                        apps = desktop.catalog();
                        history.push(Turn {
                            decision: Some(decision.clone()),
                            result: if let Action::FindApps { query } = &approved_action {
                                serde_json::to_string(
                                    &apps
                                        .iter()
                                        .filter(|a| {
                                            a.name.to_lowercase().contains(&query.to_lowercase())
                                        })
                                        .map(|a| (&a.id, &a.name))
                                        .take(15)
                                        .collect::<Vec<_>>(),
                                )
                                .unwrap_or_default()
                            } else {
                                "Action executed. Inspect the new observation to verify its effect."
                                    .into()
                            },
                        });
                        observation = desktop.observe(&target, screenshot)?;
                        if let Action::FindControls { query } = &approved_action {
                            let query = query.to_lowercase();
                            observation.controls.retain(|c| {
                                c.name.to_lowercase().contains(&query)
                                    || c.value
                                        .as_deref()
                                        .unwrap_or("")
                                        .to_lowercase()
                                        .contains(&query)
                            });
                        }
                        failures = 0;
                    }
                    Err(error) => {
                        failures += 1;
                        history.push(Turn{decision:Some(decision.clone()),result:format!("Action failed or result uncertain: {error}. Inspect before retrying; text may already have been inserted.")});
                        if failures > limits.recoveries {
                            return Err(format!("Action recovery limit reached: {error}"));
                        }
                        observation = desktop.observe(&observation.window, false)?;
                    }
                }
            }
        }
    }
}

fn requires_confirmation(action: &Action, observation: &Observation, declared: bool) -> bool {
    if !declared {
        return false;
    }
    match action {
        Action::Key { keys, .. } => {
            let normalized = keys
                .iter()
                .map(|k| k.to_lowercase().replace("control", "ctrl"))
                .collect::<Vec<_>>();
            let navigation = normalized.len() == 1
                && [
                    "tab", "escape", "up", "down", "left", "right", "home", "end",
                ]
                .contains(&normalized[0].as_str());
            let shortcut = normalized.len() == 2
                && ((normalized[0] == "ctrl" && ["l", "f", "a"].contains(&normalized[1].as_str()))
                    || (normalized[0] == "alt" && normalized[1] == "d"));
            !(navigation || shortcut)
        }
        Action::Pointer {
            operation,
            control_id,
            ..
        } => {
            if ["move", "scroll"].contains(&operation.as_str()) {
                return false;
            }
            !control_id.as_ref().is_some_and(|id| {
                observation
                    .controls
                    .iter()
                    .any(|c| c.id == *id && c.role == "50004")
            })
        }
        _ => false,
    }
}
fn rebind(action: &Action, old: &Observation, new: &Observation) -> Result<Action, String> {
    if old.window.handle != new.window.handle || old.window.pid != new.window.pid {
        return Err("Approved target window changed".into());
    }
    let mut action = action.clone();
    match &mut action {
        Action::TypeText {
            observation_id,
            control_id,
            ..
        }
        | Action::Pointer {
            observation_id,
            control_id,
            ..
        } => {
            *observation_id = new.id;
            if let Some(id) = control_id {
                let old_control = old
                    .controls
                    .iter()
                    .find(|c| c.id == *id)
                    .ok_or("Approved control is missing")?;
                let matches: Vec<_> = new
                    .controls
                    .iter()
                    .filter(|c| {
                        c.name == old_control.name
                            && c.role == old_control.role
                            && c.bounds == old_control.bounds
                    })
                    .collect();
                if matches.len() != 1 {
                    return Err(
                        "Approved control changed or is ambiguous; request the task again".into(),
                    );
                }
                *id = matches[0].id.clone();
            } else if old.bounds != new.bounds {
                return Err("Approved window geometry changed".into());
            }
        }
        Action::Key { observation_id, .. } => {
            *observation_id = new.id;
        }
        _ => {}
    }
    Ok(action)
}

struct TaskEntry {
    snapshot: TaskSnapshot,
    cancel: Arc<AtomicBool>,
    reply: mpsc::Sender<String>,
}
pub struct Engine {
    app: tauri::AppHandle,
    queue: mpsc::Sender<Job>,
    tasks: Mutex<Vec<TaskEntry>>,
    sequence: AtomicU64,
    capture: Mutex<Option<WindowTarget>>,
    recordings: Mutex<HashMap<String, WindowTarget>>,
}
pub type EngineHandle = Arc<Engine>;
struct Job {
    id: u64,
    transcript: String,
    target: WindowTarget,
    replies: mpsc::Receiver<String>,
    cancel: Arc<AtomicBool>,
}
struct FixedBrain(Decision);
impl Brain for FixedBrain {
    fn decide(
        &mut self,
        _: &str,
        _: &WindowTarget,
        _: &[Application],
        _: &Observation,
        _: &[Turn],
    ) -> Result<Decision, String> {
        Ok(self.0.clone())
    }
}
impl Engine {
    pub fn start(app: tauri::AppHandle) -> EngineHandle {
        let (tx, rx) = mpsc::channel::<Job>();
        let engine = Arc::new(Self {
            app: app.clone(),
            queue: tx,
            tasks: Mutex::new(Vec::new()),
            sequence: AtomicU64::new(chrono::Utc::now().timestamp_millis() as u64),
            capture: Mutex::new(None),
            recordings: Mutex::new(HashMap::new()),
        });
        let owner = Arc::downgrade(&engine);
        std::thread::spawn(move || {
            for job in rx {
                let Some(engine) = owner.upgrade() else { break };
                let control = LiveControl {
                    engine: engine.clone(),
                    id: job.id,
                    cancel: job.cancel.clone(),
                    replies: Mutex::new(job.replies),
                };
                let result = (|| {
                    if control.cancelled() {
                        return Err("Cancelled".into());
                    }
                    let config = config::load(&app)?;
                    control.update("starting", "Discovering applications", None, 0);
                    let apps = desktop::discover(&config.applications);
                    let mut desktop = desktop::NativeDesktop::new(apps, job.cancel.clone())?;
                    let route = intent::route(&job.transcript, &desktop.catalog());
                    let mut brain: Box<dyn Brain> = match route {
                        intent::Route::Dictate => Box::new(FixedBrain(Decision::Dictate)),
                        intent::Route::LaunchApp(application) => {
                            Box::new(FixedBrain(Decision::LaunchApp {
                                app_id: application.id,
                                app_name: application.name,
                            }))
                        }
                        intent::Route::Reasoning => {
                            let ai = crate::ai_config::load(&app)?;
                            if ai.provider == crate::ai_config::AiProvider::Jev {
                                return Err("Jev supports classification only. Select a tool-capable provider in AI Provider settings".into());
                            }
                            crate::ai_config::api_key(ai.provider)?;
                            Box::new(model::ProviderBrain { app: app.clone() })
                        }
                    };
                    run_task(
                        brain.as_mut(),
                        &mut desktop,
                        &control,
                        &job.transcript,
                        job.target,
                        Limits::default(),
                    )
                })();
                if let Err(e) = result {
                    control.update(
                        if control.cancelled() {
                            "cancelled"
                        } else {
                            "failed"
                        },
                        &e,
                        None,
                        usize::MAX,
                    )
                }
            }
        });
        engine
    }
    pub fn capture_target(&self) {
        *self.capture.lock().unwrap() = desktop::foreground();
    }
    pub fn associate_recording(&self, path: String) {
        if let Some(target) = self
            .capture
            .lock()
            .unwrap()
            .take()
            .or_else(desktop::foreground)
        {
            self.recordings.lock().unwrap().insert(path, target);
        }
    }
    pub fn submit_recording(&self, path: &str, transcript: String) -> Result<u64, String> {
        let target = self
            .recordings
            .lock()
            .unwrap()
            .remove(path)
            .or_else(desktop::foreground)
            .ok_or("No target application window")?;
        self.submit(transcript, target)
    }
    pub fn submit(&self, transcript: String, target: WindowTarget) -> Result<u64, String> {
        if transcript.trim().is_empty() {
            return Err("Empty instruction".into());
        }
        let id = self.sequence.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let snapshot = TaskSnapshot {
            id,
            transcript: transcript.clone(),
            status: "queued".into(),
            message: "Waiting for the desktop executor".into(),
            application: Some(target.title.clone()),
            steps: 0,
        };
        self.tasks.lock().unwrap().push(TaskEntry {
            snapshot: snapshot.clone(),
            cancel: cancel.clone(),
            reply: tx,
        });
        let _ = self.app.emit("engine-task", snapshot);
        self.queue
            .send(Job {
                id,
                transcript,
                target,
                replies: rx,
                cancel,
            })
            .map_err(|_| "Engine worker stopped".to_string())?;
        Ok(id)
    }
    pub fn tasks(&self) -> Vec<TaskSnapshot> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .rev()
            .take(100)
            .map(|t| t.snapshot.clone())
            .collect()
    }
    pub fn cancel(&self, id: u64) -> Result<(), String> {
        let mut tasks = self.tasks.lock().unwrap();
        let t = tasks
            .iter_mut()
            .find(|t| t.snapshot.id == id)
            .ok_or("Unknown task")?;
        t.cancel.store(true, Ordering::SeqCst);
        t.snapshot.status = "cancelled".into();
        t.snapshot.message = "Cancelled; no further actions will execute".into();
        let _ = self.app.emit("engine-task", &t.snapshot);
        Ok(())
    }
    pub fn resume(&self, id: u64, message: String, approved: bool) -> Result<(), String> {
        let mut tasks = self.tasks.lock().unwrap();
        let t = tasks
            .iter_mut()
            .find(|t| t.snapshot.id == id)
            .ok_or("Unknown task")?;
        if t.snapshot.status != "needs_clarification"
            && t.snapshot.status != "awaiting_confirmation"
        {
            return Err("Task is not paused".into());
        }
        if t.snapshot.status == "awaiting_confirmation" && !approved {
            return Err("Explicit approval is required".into());
        }
        t.reply
            .send(message)
            .map_err(|_| "Task is no longer waiting".to_string())?;
        t.snapshot.status = "resuming".into();
        let _ = self.app.emit("engine-task", &t.snapshot);
        Ok(())
    }
}
struct LiveControl {
    engine: EngineHandle,
    id: u64,
    cancel: Arc<AtomicBool>,
    replies: Mutex<mpsc::Receiver<String>>,
}
impl TaskControl for LiveControl {
    fn record_action(&self, action: &Action) {
        let serialized = serde_json::to_value(action).unwrap_or_default();
        if let Ok(dir) = self.engine.app.path().app_data_dir() {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("engine-events.jsonl"))
            {
                let _ = writeln!(
                    f,
                    "{}",
                    serde_json::json!({"task_id":self.id,"action":serialized["kind"],"text_characters":serialized.get("text").and_then(|v|v.as_str()).map(|s|s.chars().count()),"at":chrono::Utc::now().to_rfc3339()})
                );
            }
        }
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    fn update(&self, status: &str, message: &str, o: Option<&Observation>, steps: usize) {
        if self.cancelled() && status != "cancelled" {
            return;
        }
        let mut tasks = self.engine.tasks.lock().unwrap();
        if let Some(t) = tasks.iter_mut().find(|t| t.snapshot.id == self.id) {
            t.snapshot.status = status.into();
            t.snapshot.message = message.into();
            if let Some(o) = o {
                t.snapshot.application = Some(o.window.title.clone())
            }
            if steps != usize::MAX {
                t.snapshot.steps = steps
            }
            let _ = self.engine.app.emit("engine-task", &t.snapshot);
            crate::overlay::show(&self.engine.app);
            if ["completed", "failed", "cancelled"].contains(&status) {
                crate::overlay::schedule_hide(&self.engine.app);
            }
            if let Ok(dir) = self.engine.app.path().app_data_dir() {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join("engine-events.jsonl"))
                {
                    let _ = writeln!(
                        f,
                        "{}",
                        serde_json::json!({"task_id":self.id,"status":status,"steps":t.snapshot.steps,"at":chrono::Utc::now().to_rfc3339()})
                    );
                }
            }
        }
    }
    fn pause(&self, message: &str, confirmation: bool) -> Result<String, String> {
        self.update(
            if confirmation {
                "awaiting_confirmation"
            } else {
                "needs_clarification"
            },
            message,
            None,
            usize::MAX,
        );
        let _ = crate::overlay::open_settings_window(&self.engine.app);
        let _ = self.engine.app.emit("engine-needs-attention", self.id);
        loop {
            if self.cancelled() {
                return Err("Cancelled".into());
            }
            match self
                .replies
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_millis(100))
            {
                Ok(s) => return Ok(s),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err("Task reply channel closed".into()),
            }
        }
    }
}
