//! Audio device enumeration via cpal.

use cpal::traits::{DeviceTrait, HostTrait};
use thiserror::Error;

/// Sentinel id for "Aucun son" (no capture).
pub const NONE_INPUT_ID: &str = "none";

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("audio host unavailable: {0}")]
    Host(String),
    #[error("audio device not found: {0}")]
    DeviceNotFound(String),
    #[error("audio stream error: {0}")]
    Stream(String),
    #[error("unsupported config: {0}")]
    Config(String),
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub display_name: String,
    pub is_input: bool,
    /// Best-effort USB VID/PID extracted from the name/id (lowercase hex).
    pub vid: Option<String>,
    pub pid: Option<String>,
}

impl AudioDeviceInfo {
    pub fn none_input() -> Self {
        Self {
            id: NONE_INPUT_ID.into(),
            display_name: "Aucun son".into(),
            is_input: true,
            vid: None,
            pid: None,
        }
    }
}

fn extract_vid_pid(text: &str) -> (Option<String>, Option<String>) {
    let lower = text.to_ascii_lowercase();
    let vid = lower
        .find("vid_")
        .and_then(|i| lower.get(i + 4..i + 8).map(|s| s.to_string()));
    let pid = lower
        .find("pid_")
        .and_then(|i| lower.get(i + 4..i + 8).map(|s| s.to_string()));
    (vid, pid)
}

fn host() -> Result<cpal::Host, AudioError> {
    Ok(cpal::default_host())
}

/// List capture (input) devices, with "Aucun son" first.
pub fn list_inputs() -> Result<Vec<AudioDeviceInfo>, AudioError> {
    let host = host()?;
    let mut out = vec![AudioDeviceInfo::none_input()];
    let devices = host
        .input_devices()
        .map_err(|e| AudioError::Host(e.to_string()))?;
    for (idx, dev) in devices.enumerate() {
        let name = dev
            .name()
            .unwrap_or_else(|_| format!("Input {idx}"));
        let id = format!("in:{idx}:{name}");
        let (vid, pid) = extract_vid_pid(&name);
        out.push(AudioDeviceInfo {
            id,
            display_name: name,
            is_input: true,
            vid,
            pid,
        });
    }
    Ok(out)
}

/// List playback (output) devices.
pub fn list_outputs() -> Result<Vec<AudioDeviceInfo>, AudioError> {
    let host = host()?;
    let mut out = Vec::new();
    let default_name = host
        .default_output_device()
        .and_then(|d| d.name().ok());
    let devices = host
        .output_devices()
        .map_err(|e| AudioError::Host(e.to_string()))?;
    for (idx, dev) in devices.enumerate() {
        let name = dev
            .name()
            .unwrap_or_else(|_| format!("Output {idx}"));
        let id = format!("out:{idx}:{name}");
        let (vid, pid) = extract_vid_pid(&name);
        let display_name = if default_name.as_ref() == Some(&name) {
            format!("{name} (défaut système)")
        } else {
            name
        };
        out.push(AudioDeviceInfo {
            id,
            display_name,
            is_input: false,
            vid,
            pid,
        });
    }
    Ok(out)
}

pub fn default_output_id() -> Option<String> {
    let host = cpal::default_host();
    let name = host.default_output_device()?.name().ok()?;
    // Match against enumerated list naming scheme.
    let devices = host.output_devices().ok()?;
    for (idx, dev) in devices.enumerate() {
        if let Ok(n) = dev.name() {
            if n == name {
                return Some(format!("out:{idx}:{n}"));
            }
        }
    }
    Some(format!("out:0:{name}"))
}

pub(crate) fn find_input_device(id: &str) -> Result<cpal::Device, AudioError> {
    if id == NONE_INPUT_ID {
        return Err(AudioError::DeviceNotFound(id.into()));
    }
    let host = host()?;
    let want_name = id.splitn(3, ':').nth(2).unwrap_or(id);
    for dev in host
        .input_devices()
        .map_err(|e| AudioError::Host(e.to_string()))?
    {
        if dev.name().map(|n| n == want_name).unwrap_or(false) {
            return Ok(dev);
        }
    }
    Err(AudioError::DeviceNotFound(id.into()))
}

pub(crate) fn find_output_device(id: &str) -> Result<cpal::Device, AudioError> {
    let host = host()?;
    if let Some(want_name) = id.splitn(3, ':').nth(2) {
        for dev in host
            .output_devices()
            .map_err(|e| AudioError::Host(e.to_string()))?
        {
            if dev.name().map(|n| n == want_name).unwrap_or(false) {
                return Ok(dev);
            }
        }
    }
    host.default_output_device()
        .ok_or_else(|| AudioError::DeviceNotFound("default output".into()))
}
