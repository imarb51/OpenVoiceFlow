use super::types::Application;
use crate::ai_config::AiProvider;
use serde::{Deserialize, Serialize};
use tauri::Manager;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelSelection {
    pub provider: AiProvider,
    pub model: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    pub vision: Option<ModelSelection>,
    pub applications: Vec<Application>,
}
pub fn load(app: &tauri::AppHandle) -> Result<EngineConfig, String> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("engine-config.json");
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(EngineConfig::default()),
        Err(e) => Err(e.to_string()),
    }
}
pub fn save(app: &tauri::AppHandle, config: EngineConfig) -> Result<(), String> {
    if let Some(v) = &config.vision {
        if v.provider == AiProvider::Jev || v.model.trim().is_empty() {
            return Err("Choose a tool-capable vision provider and model".into());
        }
    }
    let mut ids = std::collections::HashSet::new();
    for a in &config.applications {
        if a.id.trim().is_empty() || a.name.trim().is_empty() || !ids.insert(&a.id) {
            return Err("Application IDs and names must be present and IDs unique".into());
        }
        if !std::path::Path::new(&a.executable).is_absolute()
            || !std::path::Path::new(&a.executable).is_file()
        {
            return Err("Registered applications need an existing absolute executable path".into());
        }
        if !a.executable.to_ascii_lowercase().ends_with(".exe") {
            return Err("Register an .exe application".into());
        }
    }
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join("engine-config.json"),
        serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}
