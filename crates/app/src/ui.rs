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

/// F2 cycles metrics only (Off → Simple → Detailed → Off). F1 toggles settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MetricsMode {
    #[default]
    Off,
    Simple,
    Detailed,
}

impl MetricsMode {
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Simple,
            Self::Simple => Self::Detailed,
            Self::Detailed => Self::Off,
        }
    }
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
    /// Display / screen present rate.
    pub fps: f32,
    pub capture_fps: f32,
    /// Optical-flow generated frames per second (0 when interpolation is off).
    pub frame_gen_fps: f32,
    /// User display FPS cap (0 = Auto, 9999 = unlimited).
    pub display_fps_limit: u32,
    /// VSync preference: auto | on | off.
    pub vsync_mode: String,
    pub label: String,
    pub frame_w: u32,
    pub frame_h: u32,
    pub status: String,
    pub no_signal: bool,
    /// Device held by another app / MF HW MFT failed (distinct from HDMI no-signal).
    pub device_busy: bool,
    pub soft_black: bool,
    pub audio_notice: String,
    pub level: f32,
    pub toast: String,
    /// Display pipeline delay (capture → present), milliseconds.
    pub pipeline_delay_ms: f32,
    pub one_percent_low_fps: f32,
    pub dropped_frames: u64,
    pub frame_times_ms: Vec<f32>,
    pub gpu_name: String,
    pub cpu_one_core: f32,
    pub ram_mb: f32,
    pub mode_label: String,
    pub format_label: String,
    pub metrics_mode: MetricsMode,
    pub lag_test_running: bool,
    pub lag_test_ms: f32,
    pub lag_flash: bool,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self {
            fps: 0.0,
            capture_fps: 0.0,
            frame_gen_fps: 0.0,
            display_fps_limit: 0,
            vsync_mode: "auto".into(),
            label: String::new(),
            frame_w: 0,
            frame_h: 0,
            status: String::new(),
            no_signal: false,
            device_busy: false,
            soft_black: false,
            audio_notice: String::new(),
            level: 0.0,
            toast: String::new(),
            pipeline_delay_ms: 0.0,
            one_percent_low_fps: 0.0,
            dropped_frames: 0,
            frame_times_ms: Vec::new(),
            gpu_name: String::new(),
            cpu_one_core: 0.0,
            ram_mb: 0.0,
            mode_label: String::new(),
            format_label: String::new(),
            metrics_mode: MetricsMode::Off,
            lag_test_running: false,
            lag_test_ms: 0.0,
            lag_flash: false,
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
    /// Returning user: only the video-source step (Continue closes).
    pub source_only: bool,
    pub fullscreen: bool,
    /// Pinned windows stay put and are not auto-hidden with the toolbar.
    pub metrics_pinned: bool,
    pub settings_pinned: bool,
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
            source_only: false,
            fullscreen: false,
            metrics_pinned: false,
            settings_pinned: false,
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
    /// Select preferred capture card (Auto) in the startup source step.
    pub wizard_pick_auto: bool,
    /// User confirmed the startup source choice (Continue / Finish / Skip).
    pub source_confirmed: bool,
    pub show_toolbar: bool,
    pub cycle_metrics: bool,
    pub toggle_metrics_pin: bool,
    pub toggle_settings_pin: bool,
    pub lag_test_start: bool,
    pub lag_test_stop: bool,
    pub upscale_mode: Option<&'static str>,
    pub sharpness_changed: Option<f32>,
    pub interp_mode: Option<&'static str>,
    pub deinterlace_mode: Option<&'static str>,
    /// None = unchanged; Some(0)=Auto, Some(9999)=unlimited, else Hz cap.
    pub display_fps_limit: Option<u32>,
    /// None = unchanged; Some("auto"|"on"|"off").
    pub vsync_mode: Option<&'static str>,
    pub color_range_changed: Option<&'static str>,
    pub color_matrix_changed: Option<&'static str>,
    pub color_range_toggle: bool,
}

#[derive(Clone, Debug)]
pub struct EffectsUiState {
    pub mode: String,
    pub sharpness: f32,
    pub compute_ok: bool,
    pub native_res: bool,
    pub auto_disabled: bool,
    pub last_gpu_ms: f32,
    pub vendor: String,
    pub backend: String,
    pub interp_mode: String,
    pub interp_compute_ok: bool,
    pub interp_auto_disabled: bool,
    pub interp_delay_ms: f32,
    pub deinterlace_mode: String,
    pub source_interlaced: bool,
    pub deinterlace_active: bool,
    /// auto | on | off
    pub vsync_mode: String,
}

impl Default for EffectsUiState {
    fn default() -> Self {
        Self {
            mode: "off".into(),
            sharpness: 0.4,
            compute_ok: true,
            native_res: false,
            auto_disabled: false,
            last_gpu_ms: 0.0,
            vendor: String::new(),
            backend: String::new(),
            interp_mode: "off".into(),
            interp_compute_ok: true,
            interp_auto_disabled: false,
            interp_delay_ms: 0.0,
            deinterlace_mode: "auto".into(),
            source_interlaced: false,
            deinterlace_active: false,
            vsync_mode: "auto".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ColorUiState {
    pub range_mode: String,
    pub matrix_mode: String,
    pub resolved_limited: bool,
    pub resolved_bt709: bool,
}

impl Default for ColorUiState {
    fn default() -> Self {
        Self {
            range_mode: "auto".into(),
            matrix_mode: "auto".into(),
            resolved_limited: true,
            resolved_bt709: true,
        }
    }
}

pub struct DrawInput<'a> {
    pub i18n: &'a I18n,
    pub devices: &'a [DeviceInfo],
    pub selection: &'a UiSelection,
    #[allow(dead_code)]
    pub current: &'a FormatDesc,
    pub overlay: &'a OverlayState,
    pub inputs: &'a [AudioDeviceInfo],
    pub outputs: &'a [AudioDeviceInfo],
    pub audio: &'a AudioUiState,
    pub chrome: &'a UiChrome,
    pub effects: &'a EffectsUiState,
    pub color: &'a ColorUiState,
}

pub fn draw_ui(ctx: &Context, input: DrawInput<'_>) -> UiActions {
    let mut actions = UiActions::default();
    let i18n = input.i18n;

    // Global pointer activity → show toolbar.
    let pointer_moved = ctx.input(|i| i.pointer.motion().is_some());
    if pointer_moved {
        actions.show_toolbar = true;
    }

    // Keyboard shortcuts: F1 = settings menu, F2 = metrics only (no toolbar).
    ctx.input(|i| {
        if i.key_pressed(Key::F1) {
            if input.chrome.settings_open {
                actions.close_settings = true;
            } else {
                actions.open_settings = Some(SettingsTab::Image);
            }
        }
        if i.key_pressed(Key::F2) {
            actions.cycle_metrics = true;
        }
        if i.key_pressed(Key::F11) {
            actions.toggle_fullscreen = true;
        }
        if i.key_pressed(Key::M) && !i.modifiers.matches_logically(Modifiers::CTRL) {
            actions.mute_toggled = Some(!input.audio.muted);
        }
        if i.key_pressed(Key::Escape) {
            // Settings first, then leave fullscreen (Esc must exit plein écran).
            if input.chrome.settings_open {
                actions.close_settings = true;
            } else if input.chrome.fullscreen {
                actions.toggle_fullscreen = true;
            }
        }
    });

    if input.chrome.wizard_open {
        draw_wizard(ctx, i18n, input.devices, input.selection, input.outputs, input.audio, input.chrome, &mut actions);
        return actions;
    }

    if input.overlay.metrics_mode != MetricsMode::Off {
        draw_metrics_overlay(ctx, i18n, input.overlay, input.chrome, &mut actions);
    }

    if input.chrome.toolbar_visible {
        draw_toolbar(ctx, i18n, input.chrome, &mut actions);
    }

    // Status toasts / signal hints (non-blocking).
    if input.overlay.no_signal
        || input.overlay.device_busy
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
                        if input.overlay.device_busy {
                            ui.colored_label(
                                Color32::from_rgb(255, 160, 80),
                                i18n.t("label_device_busy"),
                            );
                            ui.label(RichText::new(i18n.t("hint_device_busy")).small());
                            if ui.button(i18n.t("btn_retry")).clicked() {
                                actions.retry = true;
                            }
                        } else if input.overlay.no_signal {
                            ui.colored_label(Color32::LIGHT_RED, i18n.t("label_no_signal"));
                        } else if input.overlay.soft_black {
                            ui.colored_label(Color32::LIGHT_GRAY, i18n.t("label_soft_black"));
                        }
                        if !input.overlay.status.is_empty() && !input.overlay.device_busy {
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
    let pinned = input.chrome.settings_pinned;
    let mut win = egui::Window::new(i18n.t("app_title"))
        .id(egui::Id::new("settings_window"))
        .open(&mut open)
        .resizable(true)
        .default_width(460.0)
        .default_pos(egui::pos2(920.0, 80.0))
        .movable(!pinned);
    if pinned {
        // Keep last position while pinned; do not re-anchor every frame.
        win = win.movable(false);
    }
    win.show(ctx, |ui| {
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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let pin_label = if pinned {
                        i18n.t("btn_unpin")
                    } else {
                        i18n.t("btn_pin")
                    };
                    if ui
                        .selectable_label(pinned, pin_label)
                        .on_hover_text(i18n.t("tip_pin_window"))
                        .clicked()
                    {
                        actions.toggle_settings_pin = true;
                    }
                });
            });
            ui.separator();

            match input.chrome.settings_tab {
                SettingsTab::Image => draw_image_tab(ui, input, actions),
                SettingsTab::Sound => draw_sound_tab(ui, input, actions),
                SettingsTab::Effects => draw_effects_tab(ui, input, actions),
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
                .map(capture::device_list_label)
                .unwrap_or_else(|| "—".into());
            egui::ComboBox::from_id_salt("set_device")
                .selected_text(label)
                .width(320.0)
                .show_ui(ui, |ui| {
                    ui.set_min_width(320.0);
                    for (i, d) in devices.iter().enumerate() {
                        let row = capture::device_list_label(d);
                        if ui
                            .selectable_label(i == selection.device_idx, row)
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

        ui.add_space(8.0);
        ui.label(RichText::new(i18n.t("label_color_section")).strong());
        ui.label(RichText::new(i18n.t("tip_color_range")).small().italics());

        let color = input.color;
        let range_label = if color.resolved_limited {
            i18n.t("color_range_limited")
        } else {
            i18n.t("color_range_full")
        };
        ui.label(format!(
            "{}: {} ({})",
            i18n.t("label_color_range"),
            color.range_mode,
            range_label
        ));
        ui.horizontal(|ui| {
            for (id, key) in [
                ("auto", "color_range_auto"),
                ("limited", "color_range_limited"),
                ("full", "color_range_full"),
            ] {
                if ui
                    .selectable_label(color.range_mode == id, i18n.t(key))
                    .clicked()
                {
                    actions.color_range_changed = Some(id);
                }
            }
        });
        if ui.button(i18n.t("btn_color_toggle")).clicked() {
            actions.color_range_toggle = true;
        }

        ui.add_space(6.0);
        let matrix_label = if color.resolved_bt709 {
            i18n.t("color_matrix_bt709")
        } else {
            i18n.t("color_matrix_bt601")
        };
        ui.label(format!(
            "{}: {} ({})",
            i18n.t("label_color_matrix"),
            color.matrix_mode,
            matrix_label
        ));
        ui.horizontal(|ui| {
            for (id, key) in [
                ("auto", "color_matrix_auto"),
                ("bt601", "color_matrix_bt601"),
                ("bt709", "color_matrix_bt709"),
            ] {
                if ui
                    .selectable_label(color.matrix_mode == id, i18n.t(key))
                    .clicked()
                {
                    actions.color_matrix_changed = Some(id);
                }
            }
        });
        ui.label(RichText::new(i18n.t("explain_color")).small());
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

fn draw_effects_tab(ui: &mut egui::Ui, input: &DrawInput<'_>, actions: &mut UiActions) {
    let i18n = input.i18n;
    let fx = input.effects;
    ui.label(RichText::new(i18n.t("label_why_fsr")).strong());
    ui.label(i18n.t("why_fsr_body"));
    ui.add_space(8.0);

    // Mode A/B stay selectable at native res — RCAS / NIS sharpen still apply.
    let blocked = !fx.compute_ok;
    let reason = if !fx.compute_ok {
        i18n.t("effect_reason_no_compute")
    } else if fx.auto_disabled {
        i18n.t("effect_reason_budget")
    } else {
        String::new()
    };

    ui.label(i18n.t("label_upscale_mode"));
    ui.add_enabled_ui(!blocked, |ui| {
        ui.horizontal(|ui| {
            for (id, key) in [
                ("off", "effect_off"),
                ("fsr1", "effect_fsr"),
                ("nis", "effect_nis"),
            ] {
                let selected = fx.mode == id;
                if ui.selectable_label(selected, i18n.t(key)).clicked() {
                    actions.upscale_mode = Some(id);
                }
            }
        });
    });
    if blocked || fx.auto_disabled {
        ui.small(format!("{}: {}", i18n.t("label_unavailable"), reason));
    } else if fx.native_res && fx.mode != "off" {
        ui.small(i18n.t("tip_native_sharpen"));
    }
    ui.label(RichText::new(i18n.t("tip_upscale")).small().italics());
    ui.add_space(8.0);

    option_block(
        ui,
        i18n,
        &i18n.t("label_sharpness"),
        &i18n.t("explain_sharpness"),
        &i18n.t("tip_sharpness"),
        Impact::Light,
        Impact::Light,
        |ui| {
            let mut s = fx.sharpness;
            if ui
                .add_enabled(!blocked, egui::Slider::new(&mut s, 0.0..=1.0))
                .changed()
            {
                actions.sharpness_changed = Some(s);
            }
        },
    );

    ui.add_space(10.0);
    ui.separator();
    ui.label(RichText::new(i18n.t("label_interp_mode")).strong());
    let interp_blocked = !fx.interp_compute_ok;
    ui.add_enabled_ui(!interp_blocked, |ui| {
        ui.horizontal(|ui| {
            for (id, key) in [
                ("off", "effect_interp_off"),
                ("x2", "effect_interp_x2"),
                ("auto", "effect_interp_auto"),
            ] {
                let selected = fx.interp_mode == id;
                if ui.selectable_label(selected, i18n.t(key)).clicked() {
                    actions.interp_mode = Some(id);
                }
            }
        });
    });
    if interp_blocked {
        ui.small(format!(
            "{}: {}",
            i18n.t("label_unavailable"),
            i18n.t("effect_reason_no_compute")
        ));
    } else if fx.interp_auto_disabled {
        ui.small(format!(
            "{}: {}",
            i18n.t("label_unavailable"),
            i18n.t("effect_reason_interp_budget")
        ));
    }
    if fx.interp_delay_ms > 0.0 {
        ui.label(format!(
            "{}: ~{:.0} ms",
            i18n.t("label_interp_delay"),
            fx.interp_delay_ms
        ));
    }
    ui.label(RichText::new(i18n.t("tip_interp")).small().italics());
    ui.label(RichText::new(i18n.t("explain_interp")).small());

    ui.add_space(10.0);
    ui.label(RichText::new(i18n.t("label_display_fps_limit")).strong());
    ui.label(
        RichText::new(i18n.t("tip_display_fps_limit"))
            .small()
            .italics(),
    );
    ui.horizontal_wrapped(|ui| {
        for (hz, key) in [
            (0u32, "fps_limit_auto"),
            (60, "fps_limit_60"),
            (120, "fps_limit_120"),
            (144, "fps_limit_144"),
            (240, "fps_limit_240"),
            (9999, "fps_limit_unlimited"),
        ] {
            let selected = input.overlay.display_fps_limit == hz;
            if ui.selectable_label(selected, i18n.t(key)).clicked() {
                actions.display_fps_limit = Some(hz);
            }
        }
    });

    ui.add_space(10.0);
    ui.label(RichText::new(i18n.t("label_vsync")).strong());
    ui.label(RichText::new(i18n.t("tip_vsync")).small().italics());
    ui.horizontal(|ui| {
        for (id, key) in [
            ("auto", "vsync_auto"),
            ("on", "vsync_on"),
            ("off", "vsync_off"),
        ] {
            let selected = fx.vsync_mode == id;
            if ui.selectable_label(selected, i18n.t(key)).clicked() {
                actions.vsync_mode = Some(id);
            }
        }
    });

    ui.add_space(10.0);
    ui.separator();
    ui.label(RichText::new(i18n.t("label_deinterlace")).strong());
    ui.horizontal(|ui| {
        for (id, key) in [
            ("auto", "effect_deint_auto"),
            ("off", "effect_deint_off"),
            ("bob", "effect_deint_bob"),
        ] {
            let selected = fx.deinterlace_mode == id;
            if ui.selectable_label(selected, i18n.t(key)).clicked() {
                actions.deinterlace_mode = Some(id);
            }
        }
    });
    if fx.source_interlaced {
        ui.small(i18n.t("label_source_interlaced"));
    }
    if fx.deinterlace_active {
        ui.small(i18n.t("label_deint_active"));
    }
    ui.label(RichText::new(i18n.t("tip_deinterlace")).small().italics());
    ui.label(RichText::new(i18n.t("explain_deinterlace")).small());

    ui.add_space(10.0);
    ui.separator();
    ui.small(format!(
        "{}: {} · {}",
        i18n.t("label_gpu"),
        fx.vendor,
        fx.backend
    ));
    if fx.last_gpu_ms > 0.0 {
        ui.small(format!(
            "{}: {:.1} ms",
            i18n.t("label_effect_cost"),
            fx.last_gpu_ms
        ));
    }
}

fn draw_info_tab(ui: &mut egui::Ui, input: &DrawInput<'_>, actions: &mut UiActions) {
    let i18n = input.i18n;
    let o = input.overlay;
    ui.label(RichText::new(i18n.t("metrics_disclaimer")).small().weak());
    ui.add_space(4.0);
    draw_metrics_body(ui, i18n, o, true);
    ui.add_space(8.0);
    ui.separator();
    ui.label(i18n.t("lag_test_title"));
    ui.label(RichText::new(i18n.t("lag_test_body")).small());
    ui.horizontal(|ui| {
        if !o.lag_test_running {
            if ui.button(i18n.t("btn_lag_start")).clicked() {
                actions.lag_test_start = true;
            }
        } else {
            ui.label(format!(
                "{}: {:.0} ms",
                i18n.t("label_lag_timer"),
                o.lag_test_ms
            ));
            if ui.button(i18n.t("btn_lag_stop")).clicked() {
                actions.lag_test_stop = true;
            }
        }
    });
    ui.add_space(8.0);
    if ui.button(i18n.t("btn_copy_support")).clicked() {
        actions.copy_support = true;
    }
    if ui.button(i18n.t("btn_retry")).clicked() {
        actions.retry = true;
    }
}

fn vsync_mode_label(i18n: &I18n, mode: &str) -> String {
    match mode {
        "on" => i18n.t("vsync_on"),
        "off" => i18n.t("vsync_off"),
        _ => i18n.t("vsync_auto"),
    }
}

fn display_fps_limit_label(i18n: &I18n, limit: u32) -> String {
    match limit {
        0 => i18n.t("fps_limit_auto"),
        9999 => i18n.t("fps_limit_unlimited"),
        n => format!("{n}"),
    }
}

fn draw_metrics_overlay(
    ctx: &Context,
    i18n: &I18n,
    overlay: &OverlayState,
    chrome: &UiChrome,
    actions: &mut UiActions,
) {
    let detailed = overlay.metrics_mode == MetricsMode::Detailed;
    let title = if detailed {
        i18n.t("metrics_detailed")
    } else {
        i18n.t("metrics_simple")
    };
    let pinned = chrome.metrics_pinned;
    egui::Window::new(title)
        .id(egui::Id::new("metrics_overlay"))
        .collapsible(false)
        .resizable(false)
        .default_pos(egui::pos2(12.0, 12.0))
        .movable(!pinned)
        .frame(
            egui::Frame::NONE
                .fill(Color32::from_rgba_unmultiplied(12, 14, 18, 200))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(10, 10)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(i18n.t("hint_f2_metrics")).small().weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let pin_label = if pinned {
                        i18n.t("btn_unpin")
                    } else {
                        i18n.t("btn_pin")
                    };
                    if ui
                        .selectable_label(pinned, pin_label)
                        .on_hover_text(i18n.t("tip_pin_window"))
                        .clicked()
                    {
                        actions.toggle_metrics_pin = true;
                    }
                });
            });
            draw_metrics_body(ui, i18n, overlay, detailed);
            ui.add_space(4.0);
            ui.label(
                RichText::new(i18n.t("metrics_disclaimer"))
                    .small()
                    .weak(),
            );
        });
}

fn draw_metrics_body(ui: &mut egui::Ui, i18n: &I18n, o: &OverlayState, detailed: bool) {
    let gen_label = if o.frame_gen_fps > 0.5 {
        format!("{:.0}", o.frame_gen_fps)
    } else {
        "—".into()
    };
    ui.label(format!(
        "{}: {:.0}  ·  {}: {}  ·  {}: {:.0}",
        i18n.t("label_capture_fps"),
        o.capture_fps,
        i18n.t("label_frame_gen_fps"),
        gen_label,
        i18n.t("label_display_fps"),
        o.fps
    ));
    ui.label(format!(
        "{}: {:.0} ms  ·  {}: {}",
        i18n.t("label_pipeline_delay"),
        o.pipeline_delay_ms,
        i18n.t("label_display_fps_limit"),
        display_fps_limit_label(i18n, o.display_fps_limit)
    ));
    ui.label(format!(
        "{}: {}",
        i18n.t("label_vsync"),
        vsync_mode_label(i18n, &o.vsync_mode)
    ));
    if !detailed {
        return;
    }
    ui.label(format!(
        "{}: {:.0}  ·  {}: {}",
        i18n.t("label_one_percent_low"),
        o.one_percent_low_fps,
        i18n.t("label_dropped"),
        o.dropped_frames
    ));
    ui.label(format!(
        "{}: {:.0}%  ·  {}: {:.0} Mo",
        i18n.t("label_cpu"),
        o.cpu_one_core * 100.0,
        i18n.t("label_ram"),
        o.ram_mb
    ));
    ui.label(format!("{}: {}", i18n.t("label_gpu"), o.gpu_name));
    ui.label(format!("{}: {}", i18n.t("label_active_mode"), o.mode_label));
    ui.label(format!("{}: {}", i18n.t("label_active_format"), o.format_label));
    ui.add_space(4.0);
    ui.label(i18n.t("label_frame_graph"));
    let times = &o.frame_times_ms;
    if times.is_empty() {
        ui.weak("—");
    } else {
        let max_ms = times.iter().copied().fold(1.0f32, f32::max).max(20.0);
        let (rect, _resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width().min(280.0), 48.0),
            Sense::hover(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, Color32::from_rgb(24, 28, 34));
        let n = times.len().max(1) as f32;
        for (i, &ms) in times.iter().enumerate() {
            let x0 = rect.left() + rect.width() * (i as f32 / n);
            let x1 = rect.left() + rect.width() * ((i as f32 + 1.0) / n);
            let h = (ms / max_ms).clamp(0.0, 1.0) * rect.height();
            let bar = egui::Rect::from_min_max(
                egui::pos2(x0, rect.bottom() - h),
                egui::pos2(x1.max(x0 + 1.0), rect.bottom()),
            );
            let color = if ms > 25.0 {
                Color32::from_rgb(220, 100, 90)
            } else if ms > 18.0 {
                Color32::from_rgb(220, 180, 80)
            } else {
                Color32::from_rgb(90, 180, 140)
            };
            painter.rect_filled(bar, 0.0, color);
        }
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
    let title = if chrome.source_only {
        i18n.t("wizard_source_title")
    } else {
        i18n.t("wizard_title")
    };
    egui::Window::new(title)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .resizable(false)
        .show(ctx, |ui| {
            let step = if chrome.source_only {
                0
            } else {
                chrome.wizard_step
            };
            match step {
                0 => {
                    ui.heading(i18n.t("wizard_step1_title"));
                    ui.label(i18n.t("wizard_step1_body"));
                    ui.add_space(8.0);
                    if ui.button(i18n.t("btn_source_auto")).clicked() {
                        actions.wizard_pick_auto = true;
                    }
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            ui.set_min_width(360.0);
                            for (i, d) in devices.iter().enumerate() {
                                let row = capture::device_list_label(d);
                                let selected = i == selection.device_idx;
                                if ui.selectable_label(selected, row).clicked() {
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
                    actions.source_confirmed = true;
                }
                if !chrome.source_only && chrome.wizard_step > 0 {
                    if ui.button(i18n.t("btn_back")).clicked() {
                        actions.wizard_back = true;
                    }
                }
                if chrome.source_only {
                    if ui.button(i18n.t("btn_continue")).clicked() {
                        actions.wizard_finish = true;
                        actions.source_confirmed = true;
                    }
                } else if chrome.wizard_step < 2 {
                    if ui.button(i18n.t("btn_next")).clicked() {
                        actions.wizard_next = true;
                        if chrome.wizard_step == 0 {
                            actions.source_confirmed = true;
                        }
                    }
                } else if ui.button(i18n.t("btn_finish")).clicked() {
                    actions.wizard_finish = true;
                    actions.source_confirmed = true;
                }
            });
        });
}
