use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WindowTarget {
    pub handle: i64,
    pub pid: u32,
    pub title: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Control {
    pub id: String,
    pub name: String,
    pub role: String,
    pub bounds: Bounds,
    pub editable: bool,
    pub value: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub id: u64,
    pub window: WindowTarget,
    pub bounds: Bounds,
    pub scale: f64,
    pub controls: Vec<Control>,
    pub accessibility_error: Option<String>,
    #[serde(skip)]
    pub screenshot: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Application {
    pub id: String,
    pub name: String,
    pub executable: String,
    #[serde(default)]
    pub arguments: Vec<String>,
}

/// Every input is grounded in an observation. Coordinate targets use physical
/// pixels relative to that observation's window, never global guessed positions.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    FindApps {
        query: String,
    },
    FindControls {
        query: String,
    },
    OpenApp {
        app_id: String,
    },
    FocusWindow {
        window: WindowTarget,
    },
    TypeText {
        observation_id: u64,
        control_id: Option<String>,
        text: String,
    },
    Key {
        observation_id: u64,
        keys: Vec<String>,
    },
    Pointer {
        observation_id: u64,
        control_id: Option<String>,
        x: Option<i32>,
        y: Option<i32>,
        operation: String,
        amount: Option<i32>,
    },
    Observe {
        screenshot: bool,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Decision {
    /// Exact speech transcription for the captured foreground field. This is
    /// terminal: paste it once and do not reinterpret the same transcript.
    Dictate,
    /// Local fast path for an unambiguous application launch request.
    LaunchApp {
        app_id: String,
        app_name: String,
    },
    Act {
        summary: String,
        consequential: bool,
        action: Action,
    },
    Completed {
        summary: String,
        evidence: String,
    },
    NeedsClarification {
        question: String,
    },
    Unable {
        reason: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub id: u64,
    pub transcript: String,
    pub status: String,
    pub message: String,
    pub application: Option<String>,
    pub steps: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Turn {
    pub decision: Option<Decision>,
    pub result: String,
}

pub trait Brain {
    fn decide(
        &mut self,
        transcript: &str,
        original: &WindowTarget,
        apps: &[Application],
        observation: &Observation,
        history: &[Turn],
    ) -> Result<Decision, String>;
}
pub trait Desktop {
    fn catalog(&self) -> Vec<Application>;
    fn observe(&mut self, target: &WindowTarget, screenshot: bool) -> Result<Observation, String>;
    fn execute(
        &mut self,
        action: &Action,
        observation: &Observation,
    ) -> Result<WindowTarget, String>;
    fn release_keys(&mut self);
}
