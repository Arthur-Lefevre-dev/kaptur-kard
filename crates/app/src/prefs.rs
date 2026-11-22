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
    /// User confirmed (or skipped) the startup video-source picker.
    #[serde(default)]
    pub source_confirmed: bool,
    #[serde(default)]
    pub upscale_mode: String,
    #[serde(default = "default_sharpness")]
    pub sharpness: f32,
    /// DLSS 5 NR Boost (half-res + composite).
    #[serde(default = "default_true")]
    pub dlss_boost: bool,
    /// NVIDIA DLSS-G multiplier: off | x2 | x3 | x4 | x5 | x6
    #[serde(default)]
    pub dlssg_multiplier: String,
    #[serde(default)]
    pub interp_mode: String,
    #[serde(default = "default_deinterlace")]
    pub deinterlace_mode: String,
    /// Cap on display presents. 0 = Auto (capture, or 2× with frame gen). 9999 = unlimited.
    #[serde(default)]
    pub display_fps_limit: u32,
    /// VSync: "auto" (profile default), "on", or "off".
    #[serde(default = "default_vsync")]
    pub vsync_mode: String,
    /// Show **KapturKard** on Discord Rich Presence.
    #[serde(default = "default_true")]
    pub discord_presence: bool,
    /// Discord Developer Portal Application ID (public). Empty → env / built-in default.
    #[serde(default)]
    pub discord_app_id: String,
    /// tracing filter level: error | warn | info | debug | trace (RUST_LOG overrides when set).
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// UI language: "en" (default) or "fr".
    #[serde(default = "default_ui_language")]
    pub ui_language: String,
    #[serde(default)]
    pub metrics_pinned: bool,
    #[serde(default)]
    pub settings_pinned: bool,
    /// Last non-mock device id for hot-plug reconnect.
    #[serde(default)]
    pub last_device_id: String,
    #[serde(default)]
    pub last_width: u32,
    #[serde(default)]
    pub last_height: u32,
    #[serde(default)]
    pub last_rate_num: u32,
    #[serde(default)]
    pub last_rate_den: u32,
    /// Global color range: auto | limited | full
    #[serde(default = "default_color_range")]
    pub color_range: String,
    /// Global YUV matrix: auto | bt601 | bt709
    #[serde(default = "default_color_matrix")]
    pub color_matrix: String,
    /// Per-device overrides (id → range/matrix).
    #[serde(default)]
    pub device_colors: Vec<DeviceColorPrefs>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceColorPrefs {
    pub device_id: String,
    #[serde(default = "default_color_range")]
    pub color_range: String,
    #[serde(default = "default_color_matrix")]
    pub color_matrix: String,
}

fn default_sharpness() -> f32 {
    0.4
}

fn default_deinterlace() -> String {
    "auto".into()
}

fn default_vsync() -> String {
    "auto".into()
}

fn default_true() -> bool {
    true
}

fn default_log_level() -> String {
    "info".into()
}

fn default_ui_language() -> String {
    "en".into()
}

fn default_color_range() -> String {
    "auto".into()
}

fn default_color_matrix() -> String {
    "auto".into()
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            wizard_done: false,
            wizard_step: 0,
            source_confirmed: false,
            upscale_mode: "off".into(),
            sharpness: 0.4,
            dlss_boost: true,
            dlssg_multiplier: "off".into(),
            interp_mode: "off".into(),
            deinterlace_mode: "auto".into(),
            display_fps_limit: 0,
            vsync_mode: default_vsync(),
            discord_presence: true,
            discord_app_id: String::new(),
            log_level: default_log_level(),
            ui_language: default_ui_language(),
            metrics_pinned: false,
            settings_pinned: false,
            last_device_id: String::new(),
            last_width: 0,
            last_height: 0,
            last_rate_num: 0,
            last_rate_den: 1,
            color_range: "auto".into(),
            color_matrix: "auto".into(),
            device_colors: Vec::new(),
        }
    }
}

impl UiPrefs {
    pub fn color_for_device(&self, device_id: &str) -> (String, String) {
        if let Some(d) = self.device_colors.iter().find(|d| d.device_id == device_id) {
            return (d.color_range.clone(), d.color_matrix.clone());
        }
        (self.color_range.clone(), self.color_matrix.clone())
    }

    pub fn set_color_for_device(&mut self, device_id: &str, range: &str, matrix: &str) {
        if let Some(d) = self
            .device_colors
            .iter_mut()
            .find(|d| d.device_id == device_id)
        {
            d.color_range = range.into();
            d.color_matrix = matrix.into();
        } else {
            self.device_colors.push(DeviceColorPrefs {
                device_id: device_id.into(),
                color_range: range.into(),
                color_matrix: matrix.into(),
            });
        }
        // Keep globals in sync with the last edited device for simple launches.
        self.color_range = range.into();
        self.color_matrix = matrix.into();
    }
}

/// `%APPDATA%\CaptureView` (prefs + logs). Falls back to None off Windows / missing env.
pub fn app_data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA").or_else(|| {
        // Non-Windows: keep a predictable local folder for tests / Linux.
        std::env::var_os("XDG_CONFIG_HOME").or_else(|| {
            let home = std::env::var_os("HOME")?;
            let mut p = PathBuf::from(home);
            p.push(".config");
            Some(p.into_os_string())
        })
    })?;
    let mut p = PathBuf::from(base);
    p.push("CaptureView");
    Some(p)
}

/// `%APPDATA%\CaptureView\logs`
pub fn logs_dir() -> Option<PathBuf> {
    let mut p = app_data_dir()?;
    p.push("logs");
    Some(p)
}

fn audio_path() -> Option<PathBuf> {
    let mut p = app_data_dir()?;
    p.push("audio.toml");
    Some(p)
}

fn ui_path() -> Option<PathBuf> {
    let mut p = app_data_dir()?;
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
