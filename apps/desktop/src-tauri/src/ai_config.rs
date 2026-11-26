use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiProvider {
    Jev,
    Ollama,
    OpenAi,
    Gemini,
    Anthropic,
    Xai,
    Groq,
    OpenRouter,
}

impl AiProvider {
    pub fn id(self) -> &'static str {
        match self {
            Self::Jev => "jev",
            Self::Ollama => "ollama",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
            Self::Anthropic => "anthropic",
            Self::Xai => "xai",
            Self::Groq => "groq",
            Self::OpenRouter => "openrouter",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Self::Jev => "jev-latest",
            Self::Ollama => "gemma4:12b",
            Self::OpenAi => "gpt-4.1-mini",
            Self::Gemini => "gemini-3.8-flash",
            Self::Anthropic => "claude-haiku-4-5-20251001",
            Self::Xai => "grok-4.7",
            Self::Groq => "openai/gpt-oss-20b",
            Self::OpenRouter => "openrouter/free",
        }
    }
}

pub const OLLAMA_BASE_URL: &str = "http://localhost:11434";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    pub provider: AiProvider,
    pub model: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: AiProvider::Jev,
            model: AiProvider::Jev.default_model().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AiSettings {
    pub provider: AiProvider,
    pub model: String,
    pub has_api_key: bool,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAiSettings {
    pub provider: AiProvider,
    pub model: String,
    pub api_key: Option<String>,
}

fn config_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("ai-config.json")
}

pub fn load(app: &AppHandle) -> Result<AiConfig, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(std::fs::read_to_string(config_path(&dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default())
}

pub fn save(app: &AppHandle, update: UpdateAiSettings) -> Result<AiSettings, String> {
    let model = update.model.trim();
    if model.is_empty() {
        return Err("Enter a model ID for the selected provider".to_string());
    }

    if update.provider != AiProvider::Ollama {
        if let Some(key) = update.api_key.filter(|key| !key.trim().is_empty()) {
            credentials::store(update.provider, key.trim())?;
        }
    }

    let config = AiConfig {
        provider: update.provider,
        model: model.to_string(),
    };
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let contents = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(config_path(&dir), contents).map_err(|e| e.to_string())?;
    settings(app, config)
}

pub fn settings(_app: &AppHandle, config: AiConfig) -> Result<AiSettings, String> {
    Ok(AiSettings {
        provider: config.provider,
        model: config.model,
        has_api_key: if config.provider == AiProvider::Ollama {
            false
        } else {
            credentials::read(config.provider)?.is_some()
        },
    })
}

pub fn has_api_key(provider: AiProvider) -> Result<bool, String> {
    if provider == AiProvider::Ollama {
        return Ok(false);
    }
    Ok(credentials::read(provider)?.is_some())
}

pub fn saved_api_key(provider: AiProvider) -> Result<Option<String>, String> {
    if provider == AiProvider::Ollama {
        return Ok(None);
    }
    credentials::read(provider)
}

pub fn api_key(provider: AiProvider) -> Result<String, String> {
    if provider == AiProvider::Ollama {
        return Ok(String::new());
    }
    credentials::read(provider)?
        .ok_or_else(|| format!("Add an API key for {} in Settings", provider.id()))
}

pub fn remove_api_key(provider: AiProvider) -> Result<(), String> {
    if provider == AiProvider::Ollama {
        return Ok(());
    }
    credentials::remove(provider)?;
    Ok(())
}

mod credentials {
    use super::AiProvider;

    #[cfg(windows)]
    fn target(provider: AiProvider) -> Vec<u16> {
        format!("OpenVoiceFlow.AI.{}", provider.id())
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }

    #[cfg(windows)]
    pub fn store(provider: AiProvider, secret: &str) -> Result<(), String> {
        use std::mem::zeroed;
        use windows_sys::Win32::Security::Credentials::{
            CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
        };

        let target = target(provider);
        let username: Vec<u16> = "OpenVoiceFlow"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let blob = secret.as_bytes();
        if blob.len() > 2560 {
            return Err("API key is too long to store in Windows Credential Manager".to_string());
        }

        let mut credential: CREDENTIALW = unsafe { zeroed() };
        credential.Type = CRED_TYPE_GENERIC;
        credential.TargetName = target.as_ptr() as *mut u16;
        credential.UserName = username.as_ptr() as *mut u16;
        credential.CredentialBlob = blob.as_ptr() as *mut u8;
        credential.CredentialBlobSize = blob.len() as u32;
        credential.Persist = CRED_PERSIST_LOCAL_MACHINE;

        let ok = unsafe { CredWriteW(&credential, 0) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn store(_provider: AiProvider, _secret: &str) -> Result<(), String> {
        Err("Secure API-key storage is currently supported on Windows only".to_string())
    }

    #[cfg(windows)]
    pub fn read(provider: AiProvider) -> Result<Option<String>, String> {
        use std::ptr::null_mut;
        use windows_sys::Win32::Security::Credentials::{
            CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
        };

        let target = target(provider);
        let mut credential_ptr: *mut CREDENTIALW = null_mut();
        let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential_ptr) };
        if ok == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(1168) {
                return Ok(None);
            }
            return Err(error.to_string());
        }

        let result = unsafe {
            let credential = &*credential_ptr;
            if credential.CredentialBlob.is_null() || credential.CredentialBlobSize == 0 {
                Err("Stored API key is empty or invalid".to_string())
            } else {
                let blob = std::slice::from_raw_parts(
                    credential.CredentialBlob,
                    credential.CredentialBlobSize as usize,
                );
                String::from_utf8(blob.to_vec()).map_err(|e| e.to_string())
            }
        };
        unsafe { CredFree(credential_ptr as *const std::ffi::c_void) };
        result.map(Some)
    }

    #[cfg(not(windows))]
    pub fn read(_provider: AiProvider) -> Result<Option<String>, String> {
        Err("Secure API-key storage is currently supported on Windows only".to_string())
    }

    #[cfg(windows)]
    pub fn remove(provider: AiProvider) -> Result<(), String> {
        use windows_sys::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};

        let target = target(provider);
        let ok = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(1168) {
                return Err(error.to_string());
            }
        }
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn remove(_provider: AiProvider) -> Result<(), String> {
        Err("Secure API-key storage is currently supported on Windows only".to_string())
    }
}
