//! Preference persistence (%APPDATA%/CaptureView/).

use std::fs;
use std::path::PathBuf;

use audio::{default_output_id, NONE_INPUT_ID};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioPrefs {
    pub input_id: String,
    pub output_id: String,
    pub volume: f32,
    pub muted: bool,
    pub delay_ms: f32,
    pub buffer_ms: f32,
}

impl Default for AudioPrefs {
    fn default() -> Self {
        Self {
            input_id: NONE_INPUT_ID.into(),
            output_id: default_output_id().unwrap_or_else(|| "default".into()),
            volume: 1.0,
            muted: false,
            delay_ms: 0.0,
            buffer_ms: 30.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UiPrefs {
    pub wizard_done: bool,
    pub wizard_step: u8,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            wizard_done: false,
            wizard_step: 0,
        }
    }
}

fn captureview_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    let mut p = PathBuf::from(base);
    p.push("CaptureView");
    Some(p)
}

fn audio_path() -> Option<PathBuf> {
    let mut p = captureview_dir()?;
    p.push("audio.toml");
    Some(p)
}

fn ui_path() -> Option<PathBuf> {
    let mut p = captureview_dir()?;
    p.push("ui.toml");
    Some(p)
}

fn load_toml<T: for<'de> Deserialize<'de> + Default>(path: &PathBuf) -> T {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).unwrap_or_default(),
        Err(_) => T::default(),
    }
}

fn save_toml<T: Serialize>(path: &PathBuf, value: &T) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(text) = toml::to_string_pretty(value) {
        let _ = fs::write(path, text);
    }
}

pub fn load_audio_prefs() -> AudioPrefs {
    audio_path()
        .map(|p| load_toml(&p))
        .unwrap_or_default()
}

pub fn save_audio_prefs(prefs: &AudioPrefs) {
    if let Some(path) = audio_path() {
        save_toml(&path, prefs);
    }
}

pub fn load_ui_prefs() -> UiPrefs {
    ui_path().map(|p| load_toml(&p)).unwrap_or_default()
}

pub fn save_ui_prefs(prefs: &UiPrefs) {
    if let Some(path) = ui_path() {
        save_toml(&path, prefs);
    }
}
