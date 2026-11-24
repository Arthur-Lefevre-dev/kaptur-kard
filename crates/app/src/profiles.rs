//! Named quality profiles: Stream / Edit / Low-latency.
//! Distinct from CaptureMode (Auto/Game/Quality/Economy format picker).

use serde::{Deserialize, Serialize};

use crate::prefs::MonitorPrefs;

/// Snapshot of quality + monitoring toggles applied in one click.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NamedProfile {
    pub id: String,
    pub label_key: String,
    pub upscale_mode: String,
    pub sharpness: f32,
    pub interp_mode: String,
    pub interp_motion_blur: f32,
    pub dlssg_multiplier: String,
    pub display_fps_limit: u32,
    pub vsync_mode: String,
    pub hdr_output: bool,
    pub monitor: MonitorPrefs,
}

impl NamedProfile {
    pub fn stream() -> Self {
        Self {
            id: "stream".into(),
            label_key: "profile_stream".into(),
            upscale_mode: "nis".into(),
            sharpness: 0.45,
            interp_mode: "off".into(),
            interp_motion_blur: 0.0,
            dlssg_multiplier: "off".into(),
            display_fps_limit: 60,
            vsync_mode: "on".into(),
            hdr_output: false,
            monitor: MonitorPrefs {
                graph_cpu: true,
                graph_ram: false,
                graph_gpu: true,
                graph_vram: false,
                graph_fps_1pct: true,
                graph_latency: true,
                cpu_cores: false,
                vram: false,
                process_ram: false,
                fps_1pct: true,
                present_latency: true,
                framegen_rate: false,
                disk_io: false,
            },
        }
    }

    pub fn edit() -> Self {
        Self {
            id: "edit".into(),
            label_key: "profile_edit".into(),
            upscale_mode: "fsr1".into(),
            sharpness: 0.55,
            interp_mode: "off".into(),
            interp_motion_blur: 0.0,
            dlssg_multiplier: "off".into(),
            display_fps_limit: 0,
            vsync_mode: "on".into(),
            hdr_output: true,
            monitor: MonitorPrefs {
                graph_cpu: true,
                graph_ram: true,
                graph_gpu: true,
                graph_vram: true,
                graph_fps_1pct: true,
                graph_latency: true,
                cpu_cores: false,
                vram: true,
                process_ram: true,
                fps_1pct: true,
                present_latency: true,
                framegen_rate: true,
                disk_io: false,
            },
        }
    }

    pub fn low_latency() -> Self {
        Self {
            id: "low_latency".into(),
            label_key: "profile_low_latency".into(),
            upscale_mode: "off".into(),
            sharpness: 0.35,
            interp_mode: "off".into(),
            interp_motion_blur: 0.0,
            dlssg_multiplier: "off".into(),
            display_fps_limit: 0,
            vsync_mode: "off".into(),
            hdr_output: false,
            monitor: MonitorPrefs {
                graph_cpu: false,
                graph_ram: false,
                graph_gpu: false,
                graph_vram: false,
                graph_fps_1pct: true,
                graph_latency: true,
                cpu_cores: false,
                vram: false,
                process_ram: false,
                fps_1pct: true,
                present_latency: true,
                framegen_rate: false,
                disk_io: false,
            },
        }
    }

    /// Switch / Switch 2 dock: 1080p60 limited BT.709 feel via soft NIS, stereo audio prefs
    /// applied separately, Spout 60, VSync Auto.
    pub fn switch_dock() -> Self {
        Self {
            id: "switch_dock".into(),
            label_key: "profile_switch_dock".into(),
            upscale_mode: "nis".into(),
            sharpness: 0.35,
            interp_mode: "off".into(),
            interp_motion_blur: 0.0,
            dlssg_multiplier: "off".into(),
            display_fps_limit: 60,
            vsync_mode: "auto".into(),
            hdr_output: false,
            monitor: MonitorPrefs {
                graph_cpu: false,
                graph_ram: false,
                graph_gpu: false,
                graph_vram: false,
                graph_fps_1pct: true,
                graph_latency: true,
                cpu_cores: false,
                vram: false,
                process_ram: false,
                fps_1pct: true,
                present_latency: true,
                framegen_rate: false,
                disk_io: false,
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProfilesPrefs {
    /// Active named profile id, or "custom" when the user diverged.
    #[serde(default = "default_active")]
    pub active: String,
    #[serde(default = "default_profiles")]
    pub profiles: Vec<NamedProfile>,
}

fn default_active() -> String {
    "edit".into()
}

fn default_profiles() -> Vec<NamedProfile> {
    vec![
        NamedProfile::stream(),
        NamedProfile::edit(),
        NamedProfile::low_latency(),
        NamedProfile::switch_dock(),
    ]
}

impl Default for ProfilesPrefs {
    fn default() -> Self {
        Self {
            active: default_active(),
            profiles: default_profiles(),
        }
    }
}

impl ProfilesPrefs {
    pub fn ensure_builtins(&mut self) {
        let builtins = default_profiles();
        for b in builtins {
            if !self.profiles.iter().any(|p| p.id == b.id) {
                self.profiles.push(b);
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&NamedProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut NamedProfile> {
        self.profiles.iter_mut().find(|p| p.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_have_distinct_ids() {
        let p = ProfilesPrefs::default();
        assert_eq!(p.profiles.len(), 4);
        assert!(p.get("stream").is_some());
        assert!(p.get("edit").is_some());
        assert!(p.get("low_latency").is_some());
        assert!(p.get("switch_dock").is_some());
    }
}
