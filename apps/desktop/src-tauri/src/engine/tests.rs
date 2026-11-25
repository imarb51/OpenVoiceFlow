use super::*;
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
fn target() -> WindowTarget {
    WindowTarget {
        handle: 1,
        pid: 10,
        title: "Editor".into(),
    }
}
fn observation(id: u64, w: WindowTarget) -> Observation {
    Observation {
        id,
        window: w,
        bounds: Bounds {
            x: -500,
            y: 50,
            width: 1000,
            height: 700,
        },
        scale: 1.5,
        controls: vec![Control {
            id: format!("{id}:edit"),
            name: "Document".into(),
            role: "50004".into(),
            bounds: Bounds {
                x: -480,
                y: 80,
                width: 900,
                height: 500,
            },
            editable: true,
            value: Some(String::new()),
        }],
        accessibility_error: None,
        screenshot: None,
    }
}
struct FakeDesktop {
    sequence: u64,
    actions: Vec<Action>,
    text: String,
    released: bool,
    fail: usize,
}
impl FakeDesktop {
    fn new() -> Self {
        Self {
            sequence: 0,
            actions: vec![],
            text: String::new(),
            released: false,
            fail: 0,
        }
    }
}
impl Desktop for FakeDesktop {
    fn catalog(&self) -> Vec<Application> {
        vec![Application {
            id: "editor".into(),
            name: "Editor".into(),
            executable: "editor.exe".into(),
            arguments: vec![],
        }]
    }
    fn observe(&mut self, w: &WindowTarget, image: bool) -> Result<Observation, String> {
        self.sequence += 1;
        let mut o = observation(self.sequence, w.clone());
        o.controls[0].value = Some(self.text.clone());
        if image {
            o.screenshot = Some("png".into())
        }
        Ok(o)
    }
    fn execute(&mut self, a: &Action, o: &Observation) -> Result<WindowTarget, String> {
        self.actions.push(a.clone());
        if self.fail > 0 {
            self.fail -= 1;
            return Err("Uncertain result".into());
        }
        match a {
            Action::TypeText {
                observation_id,
                text,
                ..
            } => {
                if *observation_id != o.id {
                    return Err("Stale observation".into());
                }
                self.text.push_str(text)
            }
            Action::Key { observation_id, .. } => {
                if *observation_id != o.id {
                    return Err("Stale observation".into());
                }
            }
            _ => {}
        }
        Ok(o.window.clone())
    }
    fn release_keys(&mut self) {
        self.released = true;
    }
}
struct Script {
    decisions: VecDeque<Result<Decision, String>>,
    cancel_on_decide: Option<Arc<AtomicBool>>,
    seen_history: usize,
}
impl Script {
    fn new(d: Vec<Decision>) -> Self {
        Self {
            decisions: d.into_iter().map(Ok).collect(),
            cancel_on_decide: None,
            seen_history: 0,
        }
    }
}
impl Brain for Script {
    fn decide(
        &mut self,
        _: &str,
        _: &WindowTarget,
        _: &[Application],
        o: &Observation,
        h: &[Turn],
    ) -> Result<Decision, String> {
        self.seen_history = h.len();
        if let Some(c) = &self.cancel_on_decide {
            c.store(true, Ordering::SeqCst)
        }
        let mut d = self.decisions.pop_front().ok_or("Script exhausted")??;
        if let Decision::Completed { evidence, .. } = &mut d {
            if evidence == "Observed field contains requested text" {
                *evidence = format!("Control {} contains requested text", o.controls[0].id);
            }
        }
        if let Decision::Act {
            action:
                Action::TypeText {
                    observation_id,
                    control_id,
                    ..
                },
            ..
        } = &mut d
        {
            if *observation_id == u64::MAX {
                *observation_id = o.id;
                *control_id = Some(o.controls[0].id.clone())
            }
        }
        Ok(d)
    }
}
struct ControlStub {
    cancel: Arc<AtomicBool>,
    pauses: Cell<usize>,
    statuses: RefCell<Vec<String>>,
}
impl ControlStub {
    fn new() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            pauses: Cell::new(0),
            statuses: RefCell::new(vec![]),
        }
    }
}
impl TaskControl for ControlStub {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    fn update(&self, s: &str, _: &str, _: Option<&Observation>, _: usize) {
        self.statuses.borrow_mut().push(s.into())
    }
    fn pause(&self, _: &str, _: bool) -> Result<String, String> {
        self.pauses.set(self.pauses.get() + 1);
        Ok("User resumed".into())
    }
}
fn text(s: &str) -> Decision {
    Decision::Act {
        summary: "Type requested text".into(),
        consequential: false,
        action: Action::TypeText {
            observation_id: u64::MAX,
            control_id: None,
            text: s.into(),
        },
    }
}
#[test]
fn dictation_inserts_exact_transcript_once_and_stops() {
    let mut b = Script::new(vec![
        Decision::Dictate,
        Decision::Act {
            summary: "This must not run after dictation".into(),
            consequential: false,
            action: Action::OpenApp {
                app_id: "editor".into(),
            },
        },
    ]);
    let mut d = FakeDesktop::new();
    let c = ControlStub::new();
    assert!(run_task(
        &mut b,
        &mut d,
        &c,
        "Hi there, can you tell me what is happening?",
        target(),
        Limits::default()
    )
    .is_ok());
    assert_eq!(d.text, "Hi there, can you tell me what is happening?");
    assert_eq!(d.actions.len(), 1);
    assert!(matches!(d.actions[0], Action::TypeText { .. }));
    assert_eq!(c.pauses.get(), 0);
}

#[test]
fn direct_app_launch_runs_without_approval_or_provider_roundtrip() {
    let mut b = Script::new(vec![
        Decision::LaunchApp {
            app_id: "editor".into(),
            app_name: "Notepad".into(),
        },
        Decision::Unable {
            reason: "This must not be requested after launching".into(),
        },
    ]);
    let mut d = FakeDesktop::new();
    let c = ControlStub::new();
    assert!(run_task(
        &mut b,
        &mut d,
        &c,
        "Open Notepad",
        target(),
        Limits::default()
    )
    .is_ok());
    assert!(matches!(d.actions.as_slice(), [Action::OpenApp { app_id }] if app_id == "editor"));
    assert_eq!(c.pauses.get(), 0);
}

#[test]
fn routine_desktop_command_does_not_wait_for_approval() {
    let mut b = Script::new(vec![
        Decision::Act {
            summary: "Open the browser".into(),
            consequential: false,
            action: Action::OpenApp {
                app_id: "editor".into(),
            },
        },
        done(),
    ]);
    let mut d = FakeDesktop::new();
    let c = ControlStub::new();
    assert!(run_task(
        &mut b,
        &mut d,
        &c,
        "Open the browser",
        target(),
        Limits::default()
    )
    .is_ok());
    assert_eq!(c.pauses.get(), 0);
    assert_eq!(d.actions.len(), 1);
}

fn done() -> Decision {
    Decision::Completed {
        summary: "Finished".into(),
        evidence: "Observed field contains requested text".into(),
    }
}
#[test]
fn ordinary_dictation_and_multiple_actions_are_model_driven() {
    let mut b = Script::new(vec![
        Decision::Act {
            summary: "Open editor".into(),
            consequential: false,
            action: Action::OpenApp {
                app_id: "editor".into(),
            },
        },
        text("Hello"),
        done(),
    ]);
    let mut d = FakeDesktop::new();
    let c = ControlStub::new();
    assert!(run_task(
        &mut b,
        &mut d,
        &c,
        "Open editor and write Hello",
        target(),
        Limits::default()
    )
    .is_ok());
    assert_eq!(d.text, "Hello");
    assert_eq!(d.actions.len(), 2);
    assert_eq!(b.seen_history, 2);
    assert!(d.released);
}
#[test]
fn clarification_resumes_with_history() {
    let mut b = Script::new(vec![
        Decision::NeedsClarification {
            question: "Which track?".into(),
        },
        text("Arzu"),
        done(),
    ]);
    let mut d = FakeDesktop::new();
    let c = ControlStub::new();
    assert!(run_task(&mut b, &mut d, &c, "Play Arzu", target(), Limits::default()).is_ok());
    assert_eq!(c.pauses.get(), 1);
    assert_eq!(d.text, "Arzu");
}
#[test]
fn consequential_action_executes_only_after_exact_action_approval() {
    let action = Decision::Act {
        summary: "Send requested message".into(),
        consequential: true,
        action: Action::Key {
            observation_id: 1,
            keys: vec!["Enter".into()],
        },
    };
    let mut b = Script::new(vec![action, done()]);
    let mut d = FakeDesktop::new();
    let c = ControlStub::new();
    assert!(run_task(&mut b, &mut d, &c, "Send", target(), Limits::default()).is_ok());
    assert_eq!(c.pauses.get(), 1);
    assert_eq!(d.actions.len(), 1);
}
#[test]
fn late_model_response_cannot_act_after_cancel() {
    let c = ControlStub::new();
    let mut b = Script::new(vec![text("no")]);
    b.cancel_on_decide = Some(c.cancel.clone());
    let mut d = FakeDesktop::new();
    assert_eq!(
        run_task(&mut b, &mut d, &c, "dictation", target(), Limits::default()).unwrap_err(),
        "Cancelled"
    );
    assert!(d.actions.is_empty());
    assert!(d.released);
}
#[test]
fn provider_failure_never_pastes_transcript() {
    let mut b = Script::new(vec![]);
    b.decisions.push_back(Err("Provider unavailable".into()));
    let mut d = FakeDesktop::new();
    assert!(run_task(
        &mut b,
        &mut d,
        &ControlStub::new(),
        "open app",
        target(),
        Limits::default()
    )
    .is_err());
    assert!(d.text.is_empty());
    assert!(d.released);
}
#[test]
fn stale_targets_are_reobserved_and_can_be_corrected() {
    let mut b = Script::new(vec![
        Decision::Act {
            summary: "stale".into(),
            consequential: false,
            action: Action::TypeText {
                observation_id: 0,
                control_id: None,
                text: "wrong".into(),
            },
        },
        text("correct"),
        done(),
    ]);
    let mut d = FakeDesktop::new();
    assert!(run_task(
        &mut b,
        &mut d,
        &ControlStub::new(),
        "type",
        target(),
        Limits::default()
    )
    .is_ok());
    assert_eq!(d.text, "correct");
    assert!(d.sequence >= 3);
}
#[test]
fn action_recovery_is_bounded() {
    let mut b = Script::new(vec![text("x"), text("x"), text("x"), text("x")]);
    let mut d = FakeDesktop::new();
    d.fail = 10;
    assert!(run_task(
        &mut b,
        &mut d,
        &ControlStub::new(),
        "type",
        target(),
        Limits::default()
    )
    .unwrap_err()
    .contains("recovery limit"));
    assert_eq!(d.actions.len(), 3);
}
#[test]
fn action_and_time_limits_stop_execution() {
    let mut b = Script::new(vec![text("a"), text("b")]);
    let mut d = FakeDesktop::new();
    assert!(run_task(
        &mut b,
        &mut d,
        &ControlStub::new(),
        "type",
        target(),
        Limits {
            actions: 1,
            ..Limits::default()
        }
    )
    .is_err());
    assert_eq!(d.text, "a");
    let mut b = Script::new(vec![text("never")]);
    assert!(run_task(
        &mut b,
        &mut d,
        &ControlStub::new(),
        "type",
        target(),
        Limits {
            active_time: Duration::ZERO,
            ..Limits::default()
        }
    )
    .is_err());
    assert_eq!(d.text, "a");
}
#[test]
fn completion_requires_evidence() {
    let mut b = Script::new(vec![Decision::Completed {
        summary: "Done".into(),
        evidence: "".into(),
    }]);
    assert!(run_task(
        &mut b,
        &mut FakeDesktop::new(),
        &ControlStub::new(),
        "test",
        target(),
        Limits::default()
    )
    .is_err());
}
#[test]
fn malformed_actions_and_unknown_tools_are_rejected() {
    use crate::ai_config::AiProvider;
    for args in [
        json!({"kind":"act","summary":"test","consequential":false,"action":{"kind":"shell","command":"bad"}}),
        json!({"kind":"act","summary":"test","consequential":false,"action":{"kind":"type_text","observation_id":1,"text":"x","extra":true}}),
    ] {
        let response = json!({"choices":[{"message":{"tool_calls":[{"function":{"name":"engine_decision","arguments":args.to_string()}}]}}]});
        assert!(model::parse(AiProvider::Groq, response).is_err());
    }
}
#[test]
fn adapters_preserve_tool_results_and_images() {
    use crate::ai_config::AiProvider;
    let mut o = observation(1, target());
    o.screenshot = Some("png-base64".into());
    let history = vec![Turn {
        decision: Some(text("Hello")),
        result: "Inserted".into(),
    }];
    for p in [AiProvider::Ollama, AiProvider::Groq, AiProvider::Anthropic] {
        let (_, body) = model::request(p, "model", json!({"task":"test"}), &o, &history).unwrap();
        let s = body.to_string();
        assert!(s.contains("Inserted"));
        assert!(s.contains("png-base64"));
        assert!(s.contains(if p == AiProvider::Anthropic {
            "tool_result"
        } else {
            "tool_call_id"
        }));
    }
    assert!(model::request(AiProvider::Jev, "model", json!({}), &o, &[]).is_err());
}

#[test]
fn ollama_uses_local_openai_compatible_tool_endpoint() {
    use crate::ai_config::{AiProvider, OLLAMA_BASE_URL};

    let (url, body) = model::request(
        AiProvider::Ollama,
        "gemma4:12b",
        json!({"task":"connection test"}),
        &observation(1, target()),
        &[],
    )
    .unwrap();

    assert_eq!(url, format!("{OLLAMA_BASE_URL}/v1/chat/completions"));
    assert_eq!(body["model"], "gemma4:12b");
    assert_eq!(body["tool_choice"], "required");
    assert_eq!(body["parallel_tool_calls"], false);
}

#[test]
#[ignore = "requires local Ollama; set OPENVOICEFLOW_OLLAMA_TEST_MODEL to a local model tag"]
fn ollama_local_connection_acceptance() {
    use crate::ai_config::AiProvider;
    let model = std::env::var("OPENVOICEFLOW_OLLAMA_TEST_MODEL")
        .unwrap_or_else(|_| "gemma4:12b".to_string());
    model::connection_test(AiProvider::Ollama, &model).unwrap();
}
#[test]
fn adapter_http_roundtrip_with_mock_provider() {
    use crate::ai_config::AiProvider;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    for provider in [AiProvider::Ollama, AiProvider::Groq, AiProvider::Anthropic] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let response=if provider==AiProvider::Anthropic{json!({"content":[{"type":"tool_use","name":"engine_decision","input":{"kind":"unable","reason":"mock"}}]})}else{json!({"choices":[{"message":{"tool_calls":[{"function":{"name":"engine_decision","arguments":"{\"kind\":\"unable\",\"reason\":\"mock\"}"}}]}}]})}.to_string();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 8192];
            let _ = stream.read(&mut buffer).unwrap();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).unwrap();
        });
        let result = model::post(
            &format!("http://{address}"),
            "test-key",
            provider,
            json!({"model":"mock"}),
        )
        .unwrap();
        assert!(matches!(
            model::parse(provider, result).unwrap(),
            Decision::Unable { .. }
        ));
        server.join().unwrap();
    }
}

/// Explicit opt-in desktop acceptance checks. These use the configured provider
/// and real application context. Never run as part of the default unit suite.
struct LiveBrain;
impl Brain for LiveBrain {
    fn decide(
        &mut self,
        t: &str,
        w: &WindowTarget,
        a: &[Application],
        o: &Observation,
        h: &[Turn],
    ) -> Result<Decision, String> {
        let dir = std::path::PathBuf::from(std::env::var("APPDATA").map_err(|e| e.to_string())?)
            .join("com.openvoiceflow.app");
        let primary: crate::ai_config::AiConfig = serde_json::from_str(
            &std::fs::read_to_string(dir.join("ai-config.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let engine: config::EngineConfig = std::fs::read_to_string(dir.join("engine-config.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        model::decide_with_settings(
            config::ModelSelection {
                provider: primary.provider,
                model: primary.model,
            },
            engine.vision,
            t,
            w,
            a,
            o,
            h,
        )
    }
}
struct LiveControl {
    last: RefCell<Option<Observation>>,
}
impl TaskControl for LiveControl {
    fn record_action(&self, action: &Action) {
        match action {
            Action::FindApps { query } => eprintln!("Application lookup: {query}"),
            Action::OpenApp { app_id } => eprintln!("Opening catalog ID: {app_id}"),
            _ => {}
        }
    }
    fn cancelled(&self) -> bool {
        false
    }
    fn update(&self, status: &str, message: &str, o: Option<&Observation>, steps: usize) {
        eprintln!("{status} ({steps} actions): {message}");
        if let Some(o) = o {
            *self.last.borrow_mut() = Some(o.clone());
        }
    }
    fn pause(&self, message: &str, _: bool) -> Result<String, String> {
        Err(format!("Manual input required: {message}"))
    }
}
fn live(instruction: &str) -> (String, Observation) {
    let apps = desktop::discover(&[]);
    let target = desktop::windows()
        .into_iter()
        .find(|w| w.title.contains("Google Chrome"))
        .or_else(|| {
            desktop::windows()
                .into_iter()
                .find(|w| w.title.contains("Notepad"))
        })
        .or_else(desktop::foreground)
        .expect("An application window is required");
    let control = LiveControl {
        last: RefCell::new(None),
    };
    let mut desktop = desktop::NativeDesktop::new(apps, Arc::new(AtomicBool::new(false))).unwrap();
    let summary = run_task(
        &mut LiveBrain,
        &mut desktop,
        &control,
        instruction,
        target,
        Limits::default(),
    )
    .expect("Live acceptance task did not finish");
    (
        summary,
        control
            .last
            .into_inner()
            .expect("Missing final observation"),
    )
}
#[test]
#[ignore = "uses real desktop and configured paid provider"]
fn live_notepad_acceptance() {
    let marker = format!(
        "OpenVoiceFlow engine acceptance check {}",
        chrono::Utc::now().timestamp_millis()
    );
    let text = format!(
        "{} {marker}",
        std::env::var("OVF_LIVE_INSTRUCTION").unwrap_or_else(|_| "Open Notepad and type".into())
    );
    let clipboard_before = arboard::Clipboard::new().unwrap().get_text().ok();
    let (_, o) = live(&text);
    assert!(
        o.controls
            .iter()
            .any(|c| c.value.as_deref().unwrap_or("").contains(&marker)),
        "Requested text must be observed in Notepad"
    );
    assert!(
        arboard::Clipboard::new().unwrap().get_text().ok() == clipboard_before,
        "Clipboard text must be restored after typing"
    );
}
#[test]
#[ignore = "uses real Chrome, Spotify account and configured paid provider"]
fn live_spotify_acceptance() {
    let(_,o)=live("Open Chrome, open Spotify in Chrome's web player, use my existing signed-in account, and play Arzu. Keep this workflow in Chrome. If the track is ambiguous, search first and ask which artist instead of guessing.");
    let visible = o
        .controls
        .iter()
        .map(|c| format!("{} {}", c.name, c.value.as_deref().unwrap_or("")))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    assert!(
        desktop::process_path(o.window.pid)
            .to_lowercase()
            .ends_with("\\chrome.exe"),
        "The requested workflow must stay inside Chrome"
    );
    assert!(
        visible.contains("arzu") && visible.contains("pause"),
        "Must observe the matching track and active playback, not merely a successful click"
    );
}

#[test]
fn pointer_coordinates_handle_negative_monitors_and_physical_scaling() {
    let o = observation(1, target());
    assert_eq!(
        desktop::screen_point(&o.bounds, 100, 200).unwrap(),
        (-400, 250)
    );
    assert!(desktop::screen_point(&o.bounds, -1, 0).is_err());
    assert!(desktop::screen_point(&o.bounds, 1000, 0).is_err());
    assert!(desktop::screen_point(&o.bounds, 0, 700).is_err());
}
#[test]
fn packaged_apps_match_process_identity_not_window_titles() {
    let app = Application {
        id: "installed-1".into(),
        name: "Notepad".into(),
        executable: "explorer.exe".into(),
        arguments: vec!["shell:AppsFolder\\Microsoft.WindowsNotepad_8wekyb3d8bbwe!App".into()],
    };
    assert!(!desktop::application_matches_process(
        &app,
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
    ));
    assert!(desktop::application_matches_process(
        &app,
        r"C:\Program Files\WindowsApps\Microsoft.WindowsNotepad_11.0_x64__8wekyb3d8bbwe\Notepad\Notepad.exe"
    ));
    assert!(!desktop::application_matches_process(&app, ""));
}
#[test]
fn vision_requires_configuration_before_reading_any_credentials() {
    let mut o = observation(1, target());
    o.screenshot = Some("image".into());
    let error = model::decide_with_settings(
        config::ModelSelection {
            provider: crate::ai_config::AiProvider::Groq,
            model: "text-model".into(),
        },
        None,
        "task",
        &target(),
        &[],
        &o,
        &[],
    )
    .unwrap_err();
    assert!(error.starts_with("NEEDS_CONFIGURATION:"));
}
#[test]
fn flat_tools_decode_without_permissive_extra_arguments() {
    let dictate = json!({"choices":[{"message":{"tool_calls":[{"function":{"name":"dictate","arguments":"{}"}}]}}]});
    assert!(matches!(
        model::parse(crate::ai_config::AiProvider::Groq, dictate).unwrap(),
        Decision::Dictate
    ));
    let response = json!({"choices":[{"message":{"tool_calls":[{"function":{"name":"type_text","arguments":json!({"summary":"Type","consequential":false,"control_id":null,"text":"Hello"}).to_string()}}]}}]});
    assert!(matches!(
        model::parse_for_observation(crate::ai_config::AiProvider::Groq, response, 42).unwrap(),
        Decision::Act {
            action: Action::TypeText {
                observation_id: 42,
                ..
            },
            ..
        }
    ));
    let stale = json!({"choices":[{"message":{"tool_calls":[{"function":{"name":"type_text","arguments":json!({"summary":"Type","consequential":false,"observation_id":1,"text":"Hello"}).to_string()}}]}}]});
    assert!(model::parse_for_observation(crate::ai_config::AiProvider::Groq, stale, 42).is_err());
    let response = json!({"choices":[{"message":{"tool_calls":[{"function":{"name":"unknown_command","arguments":"{}"}}]}}]});
    assert!(model::parse(crate::ai_config::AiProvider::Groq, response).is_err());
}
