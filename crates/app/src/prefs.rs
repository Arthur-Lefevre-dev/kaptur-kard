//! Lightweight audio preference persistence (%APPDATA%/CaptureView/audio.toml).

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

fn prefs_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    let mut p = PathBuf::from(base);
    p.push("CaptureView");
    p.push("audio.toml");
    Some(p)
}

pub fn load_audio_prefs() -> AudioPrefs {
    let Some(path) = prefs_path() else {
        return AudioPrefs::default();
    };
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).unwrap_or_default(),
        Err(_) => AudioPrefs::default(),
    }
}

pub fn save_audio_prefs(prefs: &AudioPrefs) {
    let Some(path) = prefs_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(text) = toml::to_string_pretty(prefs) {
        let _ = fs::write(path, text);
    }
}
