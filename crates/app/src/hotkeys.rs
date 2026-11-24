//! Remappable keyboard shortcuts (persisted in ui.toml).

use egui::Key;
use serde::{Deserialize, Serialize};

/// Logical actions the user can rebind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HotkeyAction {
    ToggleSettings,
    CycleMetrics,
    ToggleHud,
    ToggleAudioHud,
    SaveClip,
    ToggleFullscreen,
    Mute,
}

impl HotkeyAction {
    pub const ALL: [HotkeyAction; 7] = [
        Self::ToggleSettings,
        Self::CycleMetrics,
        Self::ToggleHud,
        Self::ToggleAudioHud,
        Self::SaveClip,
        Self::ToggleFullscreen,
        Self::Mute,
    ];

    pub fn label_key(self) -> &'static str {
        match self {
            Self::ToggleSettings => "hotkey_settings",
            Self::CycleMetrics => "hotkey_metrics",
            Self::ToggleHud => "hotkey_hud",
            Self::ToggleAudioHud => "hotkey_audio_hud",
            Self::SaveClip => "hotkey_save_clip",
            Self::ToggleFullscreen => "hotkey_fullscreen",
            Self::Mute => "hotkey_mute",
        }
    }

    fn default_key(self) -> Key {
        match self {
            Self::ToggleSettings => Key::F1,
            Self::CycleMetrics => Key::F2,
            Self::ToggleHud => Key::F3,
            Self::ToggleAudioHud => Key::F4,
            Self::SaveClip => Key::F9,
            Self::ToggleFullscreen => Key::F11,
            Self::Mute => Key::M,
        }
    }
}

/// Persisted hotkey map (string key names for TOML readability).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HotkeysPrefs {
    #[serde(default = "default_f1")]
    pub toggle_settings: String,
    #[serde(default = "default_f2")]
    pub cycle_metrics: String,
    #[serde(default = "default_f3")]
    pub toggle_hud: String,
    #[serde(default = "default_f4")]
    pub toggle_audio_hud: String,
    #[serde(default = "default_f9")]
    pub save_clip: String,
    #[serde(default = "default_f11")]
    pub toggle_fullscreen: String,
    #[serde(default = "default_m")]
    pub mute: String,
}

fn default_f1() -> String {
    key_name(Key::F1)
}
fn default_f2() -> String {
    key_name(Key::F2)
}
fn default_f3() -> String {
    key_name(Key::F3)
}
fn default_f4() -> String {
    key_name(Key::F4)
}
fn default_f9() -> String {
    key_name(Key::F9)
}
fn default_f11() -> String {
    key_name(Key::F11)
}
fn default_m() -> String {
    key_name(Key::M)
}

impl Default for HotkeysPrefs {
    fn default() -> Self {
        Self {
            toggle_settings: default_f1(),
            cycle_metrics: default_f2(),
            toggle_hud: default_f3(),
            toggle_audio_hud: default_f4(),
            save_clip: default_f9(),
            toggle_fullscreen: default_f11(),
            mute: default_m(),
        }
    }
}

impl HotkeysPrefs {
    pub fn get(&self, action: HotkeyAction) -> &str {
        match action {
            HotkeyAction::ToggleSettings => &self.toggle_settings,
            HotkeyAction::CycleMetrics => &self.cycle_metrics,
            HotkeyAction::ToggleHud => &self.toggle_hud,
            HotkeyAction::ToggleAudioHud => &self.toggle_audio_hud,
            HotkeyAction::SaveClip => &self.save_clip,
            HotkeyAction::ToggleFullscreen => &self.toggle_fullscreen,
            HotkeyAction::Mute => &self.mute,
        }
    }

    pub fn set(&mut self, action: HotkeyAction, name: String) {
        match action {
            HotkeyAction::ToggleSettings => self.toggle_settings = name,
            HotkeyAction::CycleMetrics => self.cycle_metrics = name,
            HotkeyAction::ToggleHud => self.toggle_hud = name,
            HotkeyAction::ToggleAudioHud => self.toggle_audio_hud = name,
            HotkeyAction::SaveClip => self.save_clip = name,
            HotkeyAction::ToggleFullscreen => self.toggle_fullscreen = name,
            HotkeyAction::Mute => self.mute = name,
        }
    }

    pub fn key_for(&self, action: HotkeyAction) -> Key {
        parse_key(self.get(action)).unwrap_or_else(|| action.default_key())
    }

    /// Bind `key` to `action`, swapping if another action already owns it.
    pub fn bind(&mut self, action: HotkeyAction, key: Key) {
        let name = key_name(key);
        // Swap if conflict.
        for other in HotkeyAction::ALL {
            if other == action {
                continue;
            }
            if parse_key(self.get(other)) == Some(key) {
                let old = self.get(action).to_string();
                self.set(other, old);
                break;
            }
        }
        self.set(action, name);
    }

    pub fn reset_defaults(&mut self) {
        *self = Self::default();
    }
}

/// Human-readable key label for UI (F1, F4, M, …).
pub fn key_name(key: Key) -> String {
    match key {
        Key::F1 => "F1".into(),
        Key::F2 => "F2".into(),
        Key::F3 => "F3".into(),
        Key::F4 => "F4".into(),
        Key::F5 => "F5".into(),
        Key::F6 => "F6".into(),
        Key::F7 => "F7".into(),
        Key::F8 => "F8".into(),
        Key::F9 => "F9".into(),
        Key::F10 => "F10".into(),
        Key::F11 => "F11".into(),
        Key::F12 => "F12".into(),
        Key::A => "A".into(),
        Key::B => "B".into(),
        Key::C => "C".into(),
        Key::D => "D".into(),
        Key::E => "E".into(),
        Key::F => "F".into(),
        Key::G => "G".into(),
        Key::H => "H".into(),
        Key::I => "I".into(),
        Key::J => "J".into(),
        Key::K => "K".into(),
        Key::L => "L".into(),
        Key::M => "M".into(),
        Key::N => "N".into(),
        Key::O => "O".into(),
        Key::P => "P".into(),
        Key::Q => "Q".into(),
        Key::R => "R".into(),
        Key::S => "S".into(),
        Key::T => "T".into(),
        Key::U => "U".into(),
        Key::V => "V".into(),
        Key::W => "W".into(),
        Key::X => "X".into(),
        Key::Y => "Y".into(),
        Key::Z => "Z".into(),
        Key::Num0 => "0".into(),
        Key::Num1 => "1".into(),
        Key::Num2 => "2".into(),
        Key::Num3 => "3".into(),
        Key::Num4 => "4".into(),
        Key::Num5 => "5".into(),
        Key::Num6 => "6".into(),
        Key::Num7 => "7".into(),
        Key::Num8 => "8".into(),
        Key::Num9 => "9".into(),
        Key::Space => "Space".into(),
        Key::Enter => "Enter".into(),
        Key::Tab => "Tab".into(),
        Key::Backspace => "Backspace".into(),
        Key::Insert => "Insert".into(),
        Key::Delete => "Delete".into(),
        Key::Home => "Home".into(),
        Key::End => "End".into(),
        Key::PageUp => "PageUp".into(),
        Key::PageDown => "PageDown".into(),
        Key::ArrowUp => "Up".into(),
        Key::ArrowDown => "Down".into(),
        Key::ArrowLeft => "Left".into(),
        Key::ArrowRight => "Right".into(),
        other => format!("{other:?}"),
    }
}

pub fn parse_key(name: &str) -> Option<Key> {
    let n = name.trim();
    match n.to_ascii_uppercase().as_str() {
        "F1" => Some(Key::F1),
        "F2" => Some(Key::F2),
        "F3" => Some(Key::F3),
        "F4" => Some(Key::F4),
        "F5" => Some(Key::F5),
        "F6" => Some(Key::F6),
        "F7" => Some(Key::F7),
        "F8" => Some(Key::F8),
        "F9" => Some(Key::F9),
        "F10" => Some(Key::F10),
        "F11" => Some(Key::F11),
        "F12" => Some(Key::F12),
        "A" => Some(Key::A),
        "B" => Some(Key::B),
        "C" => Some(Key::C),
        "D" => Some(Key::D),
        "E" => Some(Key::E),
        "F" => Some(Key::F),
        "G" => Some(Key::G),
        "H" => Some(Key::H),
        "I" => Some(Key::I),
        "J" => Some(Key::J),
        "K" => Some(Key::K),
        "L" => Some(Key::L),
        "M" => Some(Key::M),
        "N" => Some(Key::N),
        "O" => Some(Key::O),
        "P" => Some(Key::P),
        "Q" => Some(Key::Q),
        "R" => Some(Key::R),
        "S" => Some(Key::S),
        "T" => Some(Key::T),
        "U" => Some(Key::U),
        "V" => Some(Key::V),
        "W" => Some(Key::W),
        "X" => Some(Key::X),
        "Y" => Some(Key::Y),
        "Z" => Some(Key::Z),
        "0" | "NUM0" => Some(Key::Num0),
        "1" | "NUM1" => Some(Key::Num1),
        "2" | "NUM2" => Some(Key::Num2),
        "3" | "NUM3" => Some(Key::Num3),
        "4" | "NUM4" => Some(Key::Num4),
        "5" | "NUM5" => Some(Key::Num5),
        "6" | "NUM6" => Some(Key::Num6),
        "7" | "NUM7" => Some(Key::Num7),
        "8" | "NUM8" => Some(Key::Num8),
        "9" | "NUM9" => Some(Key::Num9),
        "SPACE" => Some(Key::Space),
        "ENTER" | "RETURN" => Some(Key::Enter),
        "TAB" => Some(Key::Tab),
        "BACKSPACE" => Some(Key::Backspace),
        "INSERT" => Some(Key::Insert),
        "DELETE" | "DEL" => Some(Key::Delete),
        "HOME" => Some(Key::Home),
        "END" => Some(Key::End),
        "PAGEUP" | "PGUP" => Some(Key::PageUp),
        "PAGEDOWN" | "PGDN" => Some(Key::PageDown),
        "UP" | "ARROWUP" => Some(Key::ArrowUp),
        "DOWN" | "ARROWDOWN" => Some(Key::ArrowDown),
        "LEFT" | "ARROWLEFT" => Some(Key::ArrowLeft),
        "RIGHT" | "ARROWRIGHT" => Some(Key::ArrowRight),
        _ => None,
    }
}

/// Keys that must not be rebound (Esc dismisses modals).
pub fn is_reserved(key: Key) -> bool {
    matches!(key, Key::Escape)
}

/// First pressed key this frame (for click-to-rebind capture). Esc cancels.
pub fn first_pressed_key(i: &egui::InputState) -> Option<Key> {
    for event in &i.events {
        if let egui::Event::Key {
            key,
            pressed: true,
            repeat: false,
            ..
        } = event
        {
            return Some(*key);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_defaults() {
        let p = HotkeysPrefs::default();
        assert_eq!(p.key_for(HotkeyAction::ToggleAudioHud), Key::F4);
        assert_eq!(p.key_for(HotkeyAction::Mute), Key::M);
        assert_eq!(parse_key("f4"), Some(Key::F4));
        assert_eq!(key_name(Key::F9), "F9");
    }

    #[test]
    fn bind_swaps_conflict() {
        let mut p = HotkeysPrefs::default();
        p.bind(HotkeyAction::Mute, Key::F2);
        assert_eq!(p.key_for(HotkeyAction::Mute), Key::F2);
        assert_eq!(p.key_for(HotkeyAction::CycleMetrics), Key::M);
    }
}
