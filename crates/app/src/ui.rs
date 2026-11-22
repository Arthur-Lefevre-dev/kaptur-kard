//! Consumer UI: floating toolbar, settings tabs, first-run wizard.

use audio::AudioDeviceInfo;
use capture::{
    display_hz, hz_label, resolution_label, CaptureMode, DeviceInfo, FormatDesc, Rational,
};
use egui::{Color32, Context, Key, Modifiers, RichText, Sense};

use crate::i18n::I18n;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    Image,
    Sound,
    Effects,
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Impact {
    None,
    Light,
    #[allow(dead_code)]
    Strong,
}

impl Impact {
    fn key(self) -> &'static str {
        match self {
            Impact::None => "label_none",
            Impact::Light => "label_light",
            Impact::Strong => "label_strong",
        }
    }

    fn color(self) -> Color32 {
        match self {
            Impact::None => Color32::from_rgb(120, 180, 120),
            Impact::Light => Color32::from_rgb(220, 180, 80),
            Impact::Strong => Color32::from_rgb(220, 100, 90),
        }
    }
}

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
    pub toast: String,
    pub estimated_delay_ms: f32,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self {
            fps: 0.0,
            label: String::new(),
            frame_w: 0,
            frame_h: 0,
            status: String::new(),
            no_signal: false,
            soft_black: false,
            audio_notice: String::new(),
            level: 0.0,
            toast: String::new(),
            estimated_delay_ms: 30.0,
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
}

#[derive(Clone, Debug)]
pub struct UiChrome {
    pub settings_open: bool,
    pub settings_tab: SettingsTab,
    pub advanced_open: bool,
    pub toolbar_visible: bool,
    pub wizard_open: bool,
    pub wizard_step: u8,
    pub fullscreen: bool,
}

impl Default for UiChrome {
    fn default() -> Self {
        Self {
            settings_open: false,
            settings_tab: SettingsTab::Image,
            advanced_open: false,
            toolbar_visible: true,
            wizard_open: false,
            wizard_step: 0,
            fullscreen: false,
        }
    }
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
    pub open_settings: Option<SettingsTab>,
    pub close_settings: bool,
    pub toggle_advanced: bool,
    pub toggle_fullscreen: bool,
    pub reset_auto: bool,
    pub retry: bool,
    pub copy_support: bool,
    pub wizard_next: bool,
    pub wizard_back: bool,
    pub wizard_skip: bool,
    pub wizard_finish: bool,
    pub show_toolbar: bool,
}

pub struct DrawInput<'a> {
    pub i18n: &'a I18n,
    pub devices: &'a [DeviceInfo],
    pub selection: &'a UiSelection,
    pub current: &'a FormatDesc,
    pub overlay: &'a OverlayState,
    pub inputs: &'a [AudioDeviceInfo],
    pub outputs: &'a [AudioDeviceInfo],
    pub audio: &'a AudioUiState,
    pub chrome: &'a UiChrome,
}

pub fn draw_ui(ctx: &Context, input: DrawInput<'_>) -> UiActions {
    let mut actions = UiActions::default();
    let i18n = input.i18n;

    // Global pointer activity → show toolbar.
    let pointer_moved = ctx.input(|i| i.pointer.motion().is_some());
    if pointer_moved {
        actions.show_toolbar = true;
    }

    // Keyboard shortcuts.
    ctx.input(|i| {
        if i.key_pressed(Key::F1) {
            actions.open_settings = Some(SettingsTab::Image);
        }
        if i.key_pressed(Key::F2) {
            actions.open_settings = Some(SettingsTab::Info);
        }
        if i.key_pressed(Key::F11) {
            actions.toggle_fullscreen = true;
        }
        if i.key_pressed(Key::M) && !i.modifiers.matches_logically(Modifiers::CTRL) {
            actions.mute_toggled = Some(!input.audio.muted);
        }
        if i.key_pressed(Key::Escape) && input.chrome.settings_open {
            actions.close_settings = true;
        }
    });

    if input.chrome.wizard_open {
        draw_wizard(ctx, i18n, input.devices, input.selection, input.outputs, input.audio, input.chrome, &mut actions);
        return actions;
    }

    if input.chrome.toolbar_visible {
        draw_toolbar(ctx, i18n, input.chrome, &mut actions);
    }

    // Status toasts / signal hints (non-blocking).
    if input.overlay.no_signal
        || input.overlay.soft_black
        || !input.overlay.status.is_empty()
        || !input.overlay.toast.is_empty()
        || !input.overlay.audio_notice.is_empty()
    {
        egui::Area::new(egui::Id::new("status_banner"))
            .anchor(egui::Align2::CENTER_TOP, [0.0, 16.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(Color32::from_rgba_unmultiplied(0, 0, 0, 170))
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        if input.overlay.no_signal {
                            ui.colored_label(Color32::LIGHT_RED, i18n.t("label_no_signal"));
                        } else if input.overlay.soft_black {
                            ui.colored_label(Color32::LIGHT_GRAY, i18n.t("label_soft_black"));
                        }
                        if !input.overlay.status.is_empty() {
                            ui.colored_label(Color32::from_rgb(255, 200, 120), &input.overlay.status);
                            if ui.button(i18n.t("btn_retry")).clicked() {
                                actions.retry = true;
                            }
                        }
                        if !input.overlay.audio_notice.is_empty() {
                            ui.colored_label(
                                Color32::from_rgb(255, 200, 120),
                                &input.overlay.audio_notice,
                            );
                        }
                        if !input.overlay.toast.is_empty() {
                            ui.label(&input.overlay.toast);
                        }
                    });
            });
    }

    if input.chrome.settings_open {
        draw_settings(ctx, &input, &mut actions);
    }

    actions
}

fn draw_toolbar(ctx: &Context, i18n: &I18n, chrome: &UiChrome, actions: &mut UiActions) {
    egui::Area::new(egui::Id::new("floating_toolbar"))
        .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -28.0])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(Color32::from_rgba_unmultiplied(18, 18, 22, 210))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        toolbar_btn(
                            ui,
                            "🖼",
                            &i18n.t("toolbar_image"),
                            &i18n.t("tip_device"),
                            chrome.settings_tab == SettingsTab::Image && chrome.settings_open,
                            || actions.open_settings = Some(SettingsTab::Image),
                        );
                        toolbar_btn(
                            ui,
                            "🔊",
                            &i18n.t("toolbar_sound"),
                            &i18n.t("tip_audio_out"),
                            chrome.settings_tab == SettingsTab::Sound && chrome.settings_open,
                            || actions.open_settings = Some(SettingsTab::Sound),
                        );
                        toolbar_btn(
                            ui,
                            "✨",
                            &i18n.t("toolbar_effects"),
                            &i18n.t("tip_fsr"),
                            chrome.settings_tab == SettingsTab::Effects && chrome.settings_open,
                            || actions.open_settings = Some(SettingsTab::Effects),
                        );
                        toolbar_btn(
                            ui,
                            "ℹ",
                            &i18n.t("toolbar_info"),
                            &i18n.t("tip_info"),
                            chrome.settings_tab == SettingsTab::Info && chrome.settings_open,
                            || actions.open_settings = Some(SettingsTab::Info),
                        );
                        toolbar_btn(
                            ui,
                            "⛶",
                            &i18n.t("toolbar_fullscreen"),
                            &i18n.t("tip_fullscreen"),
                            chrome.fullscreen,
                            || actions.toggle_fullscreen = true,
                        );
                    });
                });
        });
}

fn toolbar_btn(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    tip: &str,
    active: bool,
    on_click: impl FnOnce(),
) {
    let text = format!("{icon}  {label}");
    let rich = if active {
        RichText::new(text).strong().color(Color32::WHITE)
    } else {
        RichText::new(text).color(Color32::from_gray(220))
    };
    let response = ui.add(egui::Button::new(rich).frame(false));
    if response.clicked() {
        on_click();
    }
    response.on_hover_text(tip);
}

fn draw_settings(ctx: &Context, input: &DrawInput<'_>, actions: &mut UiActions) {
    let i18n = input.i18n;
    let mut open = true;
    egui::Window::new(i18n.t("app_title"))
        .id(egui::Id::new("settings_window"))
        .open(&mut open)
        .resizable(true)
        .default_width(460.0)
        .anchor(egui::Align2::RIGHT_CENTER, [-24.0, 0.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (tab, key) in [
                    (SettingsTab::Image, "tab_image"),
                    (SettingsTab::Sound, "tab_sound"),
                    (SettingsTab::Effects, "tab_effects"),
                    (SettingsTab::Info, "tab_info"),
                ] {
                    let selected = input.chrome.settings_tab == tab;
                    if ui.selectable_label(selected, i18n.t(key)).clicked() {
                        actions.open_settings = Some(tab);
                    }
                }
            });
            ui.separator();

            match input.chrome.settings_tab {
                SettingsTab::Image => draw_image_tab(ui, input, actions),
                SettingsTab::Sound => draw_sound_tab(ui, input, actions),
                SettingsTab::Effects => draw_effects_tab(ui, input),
                SettingsTab::Info => draw_info_tab(ui, input, actions),
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(i18n.t("btn_reset_auto")).clicked() {
                    actions.reset_auto = true;
                }
                if ui.button(i18n.t("btn_copy_support")).clicked() {
                    actions.copy_support = true;
                }
                if ui.button(i18n.t("btn_close")).clicked() {
                    actions.close_settings = true;
                }
            });
        });
    if !open {
        actions.close_settings = true;
    }
}

fn option_block(
    ui: &mut egui::Ui,
    i18n: &I18n,
    title: &str,
    explain: &str,
    tip: &str,
    delay: Impact,
    gpu: Impact,
    add_controls: impl FnOnce(&mut egui::Ui),
) {
    ui.group(|ui| {
        ui.label(RichText::new(title).strong());
        ui.label(RichText::new(explain).small().color(Color32::LIGHT_GRAY));
        ui.horizontal(|ui| {
            ui.small(format!(
                "{}: ",
                i18n.t("label_impact_delay")
            ));
            ui.colored_label(delay.color(), i18n.t(delay.key()));
            ui.small(format!(" · {}: ", i18n.t("label_impact_gpu")));
            ui.colored_label(gpu.color(), i18n.t(gpu.key()));
        });
        add_controls(ui);
        ui.allocate_response(egui::vec2(0.0, 0.0), Sense::hover())
            .on_hover_text(tip);
    });
    ui.add_space(6.0);
}

fn draw_image_tab(ui: &mut egui::Ui, input: &DrawInput<'_>, actions: &mut UiActions) {
    let i18n = input.i18n;
    let devices = input.devices;
    let selection = input.selection;

    option_block(
        ui,
        i18n,
        &i18n.t("label_device"),
        &i18n.t("explain_device"),
        &i18n.t("tip_device"),
        Impact::None,
        Impact::None,
        |ui| {
            let label = devices
                .get(selection.device_idx)
                .map(|d| d.display_name.as_str())
                .unwrap_or("—");
            egui::ComboBox::from_id_salt("set_device")
                .selected_text(label)
                .width(280.0)
                .show_ui(ui, |ui| {
                    for (i, d) in devices.iter().enumerate() {
                        if ui
                            .selectable_label(i == selection.device_idx, &d.display_name)
                            .clicked()
                        {
                            actions.device_changed = Some(i);
                        }
                    }
                });
        },
    );

    let caps = devices
        .get(selection.device_idx)
        .map(|d| d.formats.as_slice())
        .unwrap_or(&[]);
    let resolutions = capture::modes::unique_resolutions(caps);

    option_block(
        ui,
        i18n,
        &i18n.t("label_resolution"),
        &i18n.t("explain_resolution"),
        &i18n.t("tip_resolution"),
        Impact::Light,
        Impact::Light,
        |ui| {
            egui::ComboBox::from_id_salt("set_res")
                .selected_text(resolution_label(selection.width, selection.height))
                .width(280.0)
                .show_ui(ui, |ui| {
                    for &(w, h) in &resolutions {
                        if ui
                            .selectable_label(
                                w == selection.width && h == selection.height,
                                resolution_label(w, h),
                            )
                            .clicked()
                        {
                            actions.resolution_changed = Some((w, h));
                        }
                    }
                });
        },
    );

    let rates = capture::modes::rates_for_resolution(caps, selection.width, selection.height);
    option_block(
        ui,
        i18n,
        &i18n.t("label_hz"),
        &i18n.t("explain_hz"),
        &i18n.t("tip_hz"),
        Impact::Light,
        Impact::Light,
        |ui| {
            egui::ComboBox::from_id_salt("set_hz")
                .selected_text(hz_label(selection.rate))
                .width(280.0)
                .show_ui(ui, |ui| {
                    for r in &rates {
                        if ui
                            .selectable_label(
                                display_hz(*r) == display_hz(selection.rate),
                                hz_label(*r),
                            )
                            .clicked()
                        {
                            actions.rate_changed = Some(*r);
                        }
                    }
                });
        },
    );

    ui.collapsing(i18n.t("section_advanced"), |ui| {
        option_block(
            ui,
            i18n,
            &i18n.t("label_mode"),
            &i18n.t("explain_mode"),
            &i18n.t("tip_mode"),
            Impact::None,
            Impact::None,
            |ui| {
                for (m, key) in [
                    (CaptureMode::Auto, "label_mode_auto"),
                    (CaptureMode::Game, "label_mode_game"),
                    (CaptureMode::Quality, "label_mode_quality"),
                    (CaptureMode::Economy, "label_mode_economy"),
                ] {
                    if ui
                        .selectable_label(selection.mode == m, i18n.t(key))
                        .clicked()
                    {
                        actions.mode_changed = Some(m);
                    }
                }
            },
        );
    });
}

fn draw_sound_tab(ui: &mut egui::Ui, input: &DrawInput<'_>, actions: &mut UiActions) {
    let i18n = input.i18n;
    let audio = input.audio;

    option_block(
        ui,
        i18n,
        &i18n.t("label_audio_in"),
        &i18n.t("explain_audio_in"),
        &i18n.t("tip_audio_in"),
        Impact::None,
        Impact::None,
        |ui| {
            let label = input
                .inputs
                .iter()
                .find(|d| d.id == audio.input_id)
                .map(|d| d.display_name.as_str())
                .unwrap_or("—");
            egui::ComboBox::from_id_salt("set_ain")
                .selected_text(label)
                .width(280.0)
                .show_ui(ui, |ui| {
                    for d in input.inputs {
                        if ui
                            .selectable_label(d.id == audio.input_id, &d.display_name)
                            .clicked()
                        {
                            actions.audio_input_changed = Some(d.id.clone());
                        }
                    }
                });
        },
    );

    option_block(
        ui,
        i18n,
        &i18n.t("label_audio_out"),
        &i18n.t("explain_audio_out"),
        &i18n.t("tip_audio_out"),
        Impact::None,
        Impact::None,
        |ui| {
            let label = input
                .outputs
                .iter()
                .find(|d| d.id == audio.output_id)
                .map(|d| d.display_name.as_str())
                .unwrap_or("—");
            egui::ComboBox::from_id_salt("set_aout")
                .selected_text(label)
                .width(280.0)
                .show_ui(ui, |ui| {
                    for d in input.outputs {
                        if ui
                            .selectable_label(d.id == audio.output_id, &d.display_name)
                            .clicked()
                        {
                            actions.audio_output_changed = Some(d.id.clone());
                        }
                    }
                });
        },
    );

    ui.horizontal(|ui| {
        let mut muted = audio.muted;
        if ui
            .checkbox(&mut muted, i18n.t("label_mute"))
            .on_hover_text(i18n.t("tip_mute"))
            .changed()
        {
            actions.mute_toggled = Some(muted);
        }
        let mut vol = audio.volume;
        if ui
            .add(egui::Slider::new(&mut vol, 0.0..=1.25).text(i18n.t("label_volume")))
            .changed()
        {
            actions.volume_changed = Some(vol);
        }
        ui.add(
            egui::ProgressBar::new(input.overlay.level.clamp(0.0, 1.0))
                .desired_width(70.0)
                .text(i18n.t("label_level")),
        );
    });

    ui.collapsing(i18n.t("section_advanced"), |ui| {
        option_block(
            ui,
            i18n,
            &i18n.t("label_delay"),
            &i18n.t("explain_delay"),
            &i18n.t("tip_delay"),
            Impact::Light,
            Impact::None,
            |ui| {
                let mut delay = audio.delay_ms;
                if ui
                    .add(egui::Slider::new(&mut delay, 0.0..=500.0).suffix(" ms"))
                    .changed()
                {
                    actions.delay_changed = Some(delay);
                }
            },
        );
        option_block(
            ui,
            i18n,
            &i18n.t("label_buffer"),
            &i18n.t("explain_buffer"),
            &i18n.t("tip_buffer"),
            Impact::Light,
            Impact::None,
            |ui| {
                let mut buf = audio.buffer_ms;
                if ui
                    .add(egui::Slider::new(&mut buf, 20.0..=40.0).suffix(" ms"))
                    .changed()
                {
                    actions.buffer_changed = Some(buf);
                }
            },
        );
    });
}

fn draw_effects_tab(ui: &mut egui::Ui, input: &DrawInput<'_>) {
    let i18n = input.i18n;
    ui.label(RichText::new(i18n.t("label_why_fsr")).strong());
    ui.label(i18n.t("why_fsr_body"));
    ui.add_space(10.0);

    for (name_key, tip_key) in [
        ("effect_fsr", "tip_fsr"),
        ("effect_nis", "tip_nis"),
        ("effect_interp", "tip_interp"),
    ] {
        ui.add_enabled_ui(false, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut false, i18n.t(name_key));
                ui.weak(format!("— {}", i18n.t("label_soon")));
            });
        });
        ui.small(format!(
            "{}: {}",
            i18n.t("label_unavailable"),
            i18n.t("effect_reason_soon")
        ));
        ui.label(RichText::new(i18n.t(tip_key)).small().italics());
        ui.add_space(6.0);
    }
}

fn draw_info_tab(ui: &mut egui::Ui, input: &DrawInput<'_>, actions: &mut UiActions) {
    let i18n = input.i18n;
    let o = input.overlay;
    ui.label(format!("{}: {:.0}", i18n.t("label_fps"), o.fps));
    ui.label(format!(
        "{}: ~{:.0} ms",
        i18n.t("label_latency"),
        o.estimated_delay_ms
    ));
    ui.label(format!("{}×{} · {}", o.frame_w, o.frame_h, o.label));
    ui.label(format!("{:?}", input.current.pixel_format));
    ui.add_space(8.0);
    if ui.button(i18n.t("btn_copy_support")).clicked() {
        actions.copy_support = true;
    }
    if ui.button(i18n.t("btn_retry")).clicked() {
        actions.retry = true;
    }
}

fn draw_wizard(
    ctx: &Context,
    i18n: &I18n,
    devices: &[DeviceInfo],
    selection: &UiSelection,
    outputs: &[AudioDeviceInfo],
    audio: &AudioUiState,
    chrome: &UiChrome,
    actions: &mut UiActions,
) {
    egui::Window::new(i18n.t("wizard_title"))
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .resizable(false)
        .show(ctx, |ui| {
            match chrome.wizard_step {
                0 => {
                    ui.heading(i18n.t("wizard_step1_title"));
                    ui.label(i18n.t("wizard_step1_body"));
                    let label = devices
                        .get(selection.device_idx)
                        .map(|d| d.display_name.as_str())
                        .unwrap_or("—");
                    egui::ComboBox::from_id_salt("wiz_dev")
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            for (i, d) in devices.iter().enumerate() {
                                if ui
                                    .selectable_label(i == selection.device_idx, &d.display_name)
                                    .clicked()
                                {
                                    actions.device_changed = Some(i);
                                }
                            }
                        });
                }
                1 => {
                    ui.heading(i18n.t("wizard_step2_title"));
                    ui.label(i18n.t("wizard_step2_body"));
                    let label = outputs
                        .iter()
                        .find(|d| d.id == audio.output_id)
                        .map(|d| d.display_name.as_str())
                        .unwrap_or("—");
                    egui::ComboBox::from_id_salt("wiz_out")
                        .selected_text(label)
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
                }
                _ => {
                    ui.heading(i18n.t("wizard_step3_title"));
                    ui.label(i18n.t("wizard_step3_body"));
                }
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(i18n.t("btn_skip")).clicked() {
                    actions.wizard_skip = true;
                }
                if chrome.wizard_step > 0 && ui.button(i18n.t("btn_back")).clicked() {
                    actions.wizard_back = true;
                }
                if chrome.wizard_step < 2 {
                    if ui.button(i18n.t("btn_next")).clicked() {
                        actions.wizard_next = true;
                    }
                } else if ui.button(i18n.t("btn_finish")).clicked() {
                    actions.wizard_finish = true;
                }
            });
        });
}
