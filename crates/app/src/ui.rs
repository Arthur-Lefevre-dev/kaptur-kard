//! egui selectors + sound panel.

use audio::AudioDeviceInfo;
use capture::{
    display_hz, hz_label, resolution_label, CaptureMode, DeviceInfo, FormatDesc, Rational,
};

#[derive(Clone, Debug)]
pub struct OverlayState {
    pub fps: f32,
    pub label: String,
    pub frame_w: u32,
    pub frame_h: u32,
    pub status: String,
    pub no_signal: bool,
    pub soft_black: bool,
    pub audio_notice: String,
    pub level: f32,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self {
            fps: 0.0,
            label: "Mock".to_string(),
            frame_w: 0,
            frame_h: 0,
            status: String::new(),
            no_signal: false,
            soft_black: false,
            audio_notice: String::new(),
            level: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct UiSelection {
    pub device_idx: usize,
    pub width: u32,
    pub height: u32,
    pub rate: Rational,
    pub mode: CaptureMode,
}

#[derive(Clone, Debug)]
pub struct AudioUiState {
    pub input_id: String,
    pub output_id: String,
    pub volume: f32,
    pub muted: bool,
    pub delay_ms: f32,
    pub buffer_ms: f32,
    pub show_advanced: bool,
}

#[derive(Debug, Default)]
pub struct UiActions {
    pub device_changed: Option<usize>,
    pub resolution_changed: Option<(u32, u32)>,
    pub rate_changed: Option<Rational>,
    pub mode_changed: Option<CaptureMode>,
    pub audio_input_changed: Option<String>,
    pub audio_output_changed: Option<String>,
    pub volume_changed: Option<f32>,
    pub mute_toggled: Option<bool>,
    pub delay_changed: Option<f32>,
    pub buffer_changed: Option<f32>,
    pub toggle_advanced: bool,
}

pub fn draw_ui(
    ctx: &egui::Context,
    devices: &[DeviceInfo],
    selection: &UiSelection,
    current: &FormatDesc,
    overlay: &OverlayState,
    inputs: &[AudioDeviceInfo],
    outputs: &[AudioDeviceInfo],
    audio: &AudioUiState,
) -> UiActions {
    let mut actions = UiActions::default();

    egui::TopBottomPanel::top("captureview_bar").show(ctx, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.heading("CaptureView");

            let mut device_idx = selection.device_idx.min(devices.len().saturating_sub(1));
            let device_label = devices
                .get(device_idx)
                .map(|d| d.display_name.as_str())
                .unwrap_or("(aucun)");
            egui::ComboBox::from_id_salt("device")
                .selected_text(device_label)
                .show_ui(ui, |ui| {
                    for (i, d) in devices.iter().enumerate() {
                        if ui
                            .selectable_label(i == device_idx, &d.display_name)
                            .clicked()
                        {
                            device_idx = i;
                            actions.device_changed = Some(i);
                        }
                    }
                });

            let caps = devices
                .get(selection.device_idx)
                .map(|d| d.formats.as_slice())
                .unwrap_or(&[]);
            let resolutions = capture::modes::unique_resolutions(caps);

            let res_text = resolution_label(selection.width, selection.height);
            egui::ComboBox::from_id_salt("resolution")
                .selected_text(res_text)
                .show_ui(ui, |ui| {
                    for &(w, h) in &resolutions {
                        let label = resolution_label(w, h);
                        if ui
                            .selectable_label(
                                w == selection.width && h == selection.height,
                                label,
                            )
                            .clicked()
                        {
                            actions.resolution_changed = Some((w, h));
                        }
                    }
                });

            let rates =
                capture::modes::rates_for_resolution(caps, selection.width, selection.height);
            let hz_text = hz_label(selection.rate);
            egui::ComboBox::from_id_salt("hz")
                .selected_text(hz_text)
                .show_ui(ui, |ui| {
                    for r in &rates {
                        let label = hz_label(*r);
                        if ui
                            .selectable_label(
                                display_hz(*r) == display_hz(selection.rate),
                                label,
                            )
                            .clicked()
                        {
                            actions.rate_changed = Some(*r);
                        }
                    }
                });

            let mode_text = match selection.mode {
                CaptureMode::Auto => "Auto",
                CaptureMode::Game => "Game",
                CaptureMode::Quality => "Quality",
                CaptureMode::Economy => "Economy",
            };
            egui::ComboBox::from_id_salt("mode")
                .selected_text(mode_text)
                .show_ui(ui, |ui| {
                    for (m, label) in [
                        (CaptureMode::Auto, "Auto"),
                        (CaptureMode::Game, "Game"),
                        (CaptureMode::Quality, "Quality"),
                        (CaptureMode::Economy, "Economy"),
                    ] {
                        if ui.selectable_label(selection.mode == m, label).clicked() {
                            actions.mode_changed = Some(m);
                        }
                    }
                });

            ui.separator();
            ui.label(format!(
                "{:.0} FPS · {} · {}×{}",
                overlay.fps, overlay.label, current.width, current.height
            ));
        });

        if !overlay.status.is_empty() {
            ui.colored_label(egui::Color32::from_rgb(255, 200, 120), &overlay.status);
        }
        if overlay.no_signal {
            ui.colored_label(egui::Color32::LIGHT_RED, "Pas de signal");
        } else if overlay.soft_black {
            ui.colored_label(egui::Color32::GRAY, "Signal noir / source faible");
        }
    });

    egui::TopBottomPanel::bottom("captureview_sound").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Son");
            let input_label = inputs
                .iter()
                .find(|d| d.id == audio.input_id)
                .map(|d| d.display_name.as_str())
                .unwrap_or("Source du son");
            egui::ComboBox::from_id_salt("audio_in")
                .selected_text(input_label)
                .width(180.0)
                .show_ui(ui, |ui| {
                    for d in inputs {
                        if ui
                            .selectable_label(d.id == audio.input_id, &d.display_name)
                            .clicked()
                        {
                            actions.audio_input_changed = Some(d.id.clone());
                        }
                    }
                });

            let output_label = outputs
                .iter()
                .find(|d| d.id == audio.output_id)
                .map(|d| d.display_name.as_str())
                .unwrap_or("Sortie");
            egui::ComboBox::from_id_salt("audio_out")
                .selected_text(output_label)
                .width(180.0)
                .show_ui(ui, |ui| {
                    for d in outputs {
                        if ui
                            .selectable_label(d.id == audio.output_id, &d.display_name)
                            .clicked()
                        {
                            actions.audio_output_changed = Some(d.id.clone());
                        }
                    }
                });

            let mut muted = audio.muted;
            if ui.checkbox(&mut muted, "Muet").changed() {
                actions.mute_toggled = Some(muted);
            }
            let mut vol = audio.volume;
            if ui
                .add(egui::Slider::new(&mut vol, 0.0..=1.25).text("Vol"))
                .changed()
            {
                actions.volume_changed = Some(vol);
            }
            // Level meter
            let level = overlay.level.clamp(0.0, 1.0);
            ui.add(
                egui::ProgressBar::new(level)
                    .desired_width(80.0)
                    .text("Niveau"),
            );

            if ui
                .button(if audio.show_advanced {
                    "Masquer avancé"
                } else {
                    "Avancé"
                })
                .clicked()
            {
                actions.toggle_advanced = true;
            }
        });

        if audio.show_advanced {
            ui.horizontal(|ui| {
                let mut delay = audio.delay_ms;
                if ui
                    .add(egui::Slider::new(&mut delay, 0.0..=500.0).text("Délai audio (ms)"))
                    .changed()
                {
                    actions.delay_changed = Some(delay);
                }
                let mut buf = audio.buffer_ms;
                if ui
                    .add(egui::Slider::new(&mut buf, 20.0..=40.0).text("Buffer (ms)"))
                    .changed()
                {
                    actions.buffer_changed = Some(buf);
                }
            });
        }

        if !overlay.audio_notice.is_empty() {
            ui.colored_label(
                egui::Color32::from_rgb(255, 200, 120),
                &overlay.audio_notice,
            );
        }
    });

    actions
}
