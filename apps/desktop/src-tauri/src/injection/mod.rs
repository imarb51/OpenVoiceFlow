//! System-wide text delivery: after a transcription completes, put the text
//! wherever OS input focus currently is, via clipboard-write + simulated
//! paste. No detection of whether a real text field is focused — if the
//! paste doesn't land anywhere useful, the transcript is still durably
//! saved (the `.txt` sidecar and the Settings window's Transcripts tab),
//! so this is a best-effort side effect, not the only way to get the text.

use std::thread;
use std::time::Duration;

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

/// Gap between writing the clipboard and sending the paste keystroke (and
/// again before restoring the old clipboard contents) — some target apps
/// (browser address bars, Electron apps) need a moment to register new
/// clipboard content before a paste is processed reliably.
const PASTE_DELAY: Duration = Duration::from_millis(80);

#[derive(Debug, thiserror::Error)]
pub enum InjectionError {
    #[error("clipboard error: {0}")]
    Clipboard(String),
    #[error("input simulation error: {0}")]
    Input(String),
}

pub trait TextInjector: Send {
    fn insert_text(&self, text: &str) -> Result<(), InjectionError>;
    fn insert_text_checked(
        &self,
        text: &str,
        validate: &dyn Fn() -> Result<(), InjectionError>,
    ) -> Result<(), InjectionError> {
        validate()?;
        self.insert_text(text)
    }
}

/// Copies `text` to the clipboard and simulates Ctrl+V, restoring whatever
/// text was previously on the clipboard afterward so dictation doesn't
/// silently clobber the user's last copy.
pub struct ClipboardPasteInjector;

impl TextInjector for ClipboardPasteInjector {
    fn insert_text(&self, text: &str) -> Result<(), InjectionError> {
        self.insert_text_checked(text, &|| Ok(()))
    }
    fn insert_text_checked(
        &self,
        text: &str,
        validate: &dyn Fn() -> Result<(), InjectionError>,
    ) -> Result<(), InjectionError> {
        let mut clipboard =
            Clipboard::new().map_err(|e| InjectionError::Clipboard(e.to_string()))?;

        let previous = clipboard.get_text().ok();
        let previous_image = if previous.is_none() {
            clipboard.get_image().ok()
        } else {
            None
        };

        let result = (|| {
            clipboard
                .set_text(text.to_string())
                .map_err(|e| InjectionError::Clipboard(e.to_string()))?;
            thread::sleep(PASTE_DELAY);
            validate()?;
            let mut enigo = Enigo::new(&Settings::default())
                .map_err(|e| InjectionError::Input(e.to_string()))?;
            let pasted = (|| {
                enigo.key(Key::Control, Direction::Press)?;
                enigo.key(Key::Unicode('v'), Direction::Click)
            })();
            let released = enigo.key(Key::Control, Direction::Release);
            pasted
                .and(released)
                .map_err(|e| InjectionError::Input(e.to_string()))
        })();

        thread::sleep(PASTE_DELAY);
        let restored = if let Some(previous) = previous {
            clipboard.set_text(previous)
        } else if let Some(previous) = previous_image {
            clipboard.set_image(previous)
        } else {
            clipboard.clear()
        };
        result?;
        restored.map_err(|e| InjectionError::Clipboard(e.to_string()))
    }
}
