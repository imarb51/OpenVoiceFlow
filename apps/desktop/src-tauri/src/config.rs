//! A tiny persisted settings file (`<app_data_dir>/config.json`) for the one
//! user-configurable setting Phase 1 anticipated: the push-to-talk shortcut.
//! Deliberately minimal — this is not a general settings store.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hotkeys::DEFAULT_SHORTCUT;

#[derive(Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub hotkey: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_SHORTCUT.to_string(),
        }
    }
}

fn config_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("config.json")
}

pub fn load(app_data_dir: &Path) -> AppConfig {
    let path = config_path(app_data_dir);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

pub fn save(app_data_dir: &Path, config: &AppConfig) -> std::io::Result<()> {
    std::fs::create_dir_all(app_data_dir)?;
    let contents = serde_json::to_string_pretty(config).unwrap_or_default();
    std::fs::write(config_path(app_data_dir), contents)
}
