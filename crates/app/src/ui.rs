//! Consumer UI: floating toolbar, settings tabs, first-run wizard.

use audio::AudioDeviceInfo;
use capture::{
    display_hz, hz_label, resolution_label, CaptureMode, DeviceInfo, FormatDesc, Rational,
};
use egui::{Color32, Context, Key, Modifiers, RichText, Sense};

use crate::export::ExportStatus;
use crate::health::{CaptureHealthKind, CaptureHealthUi};
use crate::i18n::I18n;
use crate::lut::GradePrefs;
use crate::prefs::MonitorPrefs;
use crate::profiles::ProfilesPrefs;
use crate::update_check::UpdateStatus;

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

/// Severity for the top-center status toast / banner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToastKind {
    #[default]
    Neutral,
    Success,
    Error,
}

impl ToastKind {
    pub fn color(self) -> Color32 {
        match self {
            Self::Neutral => Color32::from_rgb(230, 230, 235),
            Self::Success => Color32::from_rgb(120, 220, 140),
            Self::Error => Color32::from_rgb(255, 140, 120),
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
    /// User prefers HDR output when the OS display is in HDR.
    pub hdr_output: bool,
    /// OS Advanced Color supported on an active path.
    pub hdr_supported: bool,
    /// OS Advanced Color currently enabled.
    pub hdr_enabled: bool,
    /// Swapchain is using the HDR (RGBA16F / scRGB) path.
    pub hdr_active: bool,
    /// DXGI / surface reports allow-tearing (VRR-capable).
    pub vrr_capable: bool,
    /// Active wgpu present mode label (Immediate / Mailbox / Fifo / …).
    pub present_mode: String,
    /// Active surface format label.
    pub surface_format: String,
    pub discord_presence: bool,
    pub discord_app_id: String,
    /// True when the Discord IPC worker is running (ID present).
    pub discord_active: bool,
    /// tracing level: error|warn|info|debug|trace
    pub log_level: String,
    /// Active log file path (for Infos / support).
    pub log_path: String,
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
    /// Visual severity for the status toast banner.
    pub toast_kind: ToastKind,
    /// Display pipeline delay (capture → present), milliseconds.
    pub pipeline_delay_ms: f32,
    pub one_percent_low_fps: f32,
    pub dropped_frames: u64,
    pub frame_times_ms: Vec<f32>,
    pub gpu_name: String,
    pub cpu_one_core: f32,
    pub ram_mb: f32,
    /// System RAM used / total (MiB).
    pub system_ram_used_mb: f32,
    pub system_ram_total_mb: f32,
    /// System-wide average CPU busy (0..=1).
    pub system_cpu_frac: f32,
    /// Per-logical-core busy fractions (0..=1).
    pub per_core_frac: Vec<f32>,
    /// Process affinity mask (bit i = core may run this process).
    pub affinity_mask: u64,
    pub logical_cores: u32,
    pub vram_used_mb: f32,
    pub vram_budget_mb: f32,
    pub io_read_mbps: f32,
    pub io_write_mbps: f32,
    /// Recent process CPU % (0..=100+) for the CPU graph.
    pub cpu_history: Vec<f32>,
    /// Recent process RAM MiB for the RAM graph.
    pub ram_history: Vec<f32>,
    /// Which monitor graphs / lines are enabled (from prefs).
    pub monitor: MonitorPrefs,
    pub mode_label: String,
    pub format_label: String,
    pub metrics_mode: MetricsMode,
    pub lag_test_running: bool,
    pub lag_test_ms: f32,
    pub lag_flash: bool,
    /// Compact HUD visible (F3), independent of F2 metrics mode.
    pub hud_enabled: bool,
    pub hud_always_on_top: bool,
    pub replay_enabled: bool,
    pub replay_seconds: u32,
    pub replay_frames: u32,
    pub grade: GradePrefs,
    pub cube_lut_path: String,
    pub health: CaptureHealthUi,
    pub health_auto_reinit: bool,
    pub health_stall_secs: u32,
    pub export: ExportStatus,
    pub spout_enabled: bool,
    pub ndi_enabled: bool,
    pub spout_name: String,
    pub ndi_name: String,
    pub update: UpdateStatus,
    pub current_version: String,
    pub profiles: ProfilesPrefs,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self {
            fps: 0.0,
            capture_fps: 0.0,
            frame_gen_fps: 0.0,
            display_fps_limit: 0,
            vsync_mode: "auto".into(),
            hdr_output: true,
            hdr_supported: false,
            hdr_enabled: false,
            hdr_active: false,
            vrr_capable: false,
            present_mode: String::new(),
            surface_format: String::new(),
            discord_presence: true,
            discord_app_id: String::new(),
            discord_active: false,
            log_level: "info".into(),
            log_path: String::new(),
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
            toast_kind: ToastKind::Neutral,
            pipeline_delay_ms: 0.0,
            one_percent_low_fps: 0.0,
            dropped_frames: 0,
            frame_times_ms: Vec::new(),
            gpu_name: String::new(),
            cpu_one_core: 0.0,
            ram_mb: 0.0,
            system_ram_used_mb: 0.0,
            system_ram_total_mb: 0.0,
            system_cpu_frac: 0.0,
            per_core_frac: Vec::new(),
            affinity_mask: 0,
            logical_cores: 0,
            vram_used_mb: 0.0,
            vram_budget_mb: 0.0,
            io_read_mbps: 0.0,
            io_write_mbps: 0.0,
            cpu_history: Vec::new(),
            ram_history: Vec::new(),
            monitor: MonitorPrefs::default(),
            mode_label: String::new(),
            format_label: String::new(),
            metrics_mode: MetricsMode::Off,
            lag_test_running: false,
            lag_test_ms: 0.0,
            lag_flash: false,
            hud_enabled: false,
            hud_always_on_top: false,
            replay_enabled: false,
            replay_seconds: 8,
            replay_frames: 0,
            grade: GradePrefs::default(),
            cube_lut_path: String::new(),
            health: CaptureHealthUi::default(),
            health_auto_reinit: false,
            health_stall_secs: 3,
            export: ExportStatus::default(),
            spout_enabled: false,
            ndi_enabled: false,
            spout_name: "KapturKard".into(),
            ndi_name: "KapturKard".into(),
            update: UpdateStatus::default(),
            current_version: env!("CARGO_PKG_VERSION").into(),
            profiles: ProfilesPrefs::default(),
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
    /// 0 = hidden, 1 = fully visible. Soft launch tips overlay opacity.
    pub launch_tips_alpha: f32,
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
            launch_tips_alpha: 0.0,
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
    pub dlss_boost_changed: Option<bool>,
    pub dlssg_multiplier: Option<&'static str>,
    pub interp_mode: Option<&'static str>,
    /// None = unchanged; Some(0..=1) Mid motion blur intensity.
    pub interp_motion_blur_changed: Option<f32>,
    pub deinterlace_mode: Option<&'static str>,
    /// None = unchanged; Some(0)=Auto, Some(9999)=unlimited, else Hz cap.
    pub display_fps_limit: Option<u32>,
    /// None = unchanged; Some("auto"|"on"|"off").
    pub vsync_mode: Option<&'static str>,
    /// None = unchanged; Some(prefer HDR output).
    pub hdr_output_changed: Option<bool>,
    pub discord_presence_changed: Option<bool>,
    pub discord_app_id_changed: Option<String>,
    pub log_level_changed: Option<&'static str>,
    /// UI language pref: "en" | "fr".
    pub language_changed: Option<&'static str>,
    pub color_range_changed: Option<&'static str>,
    pub color_matrix_changed: Option<&'static str>,
    pub color_range_toggle: bool,
    /// User dismissed the brief launch shortcuts overlay.
    pub dismiss_launch_tips: bool,
    /// Full replacement of monitor graph / HUD toggles (apply immediately).
    pub monitor_prefs_changed: Option<MonitorPrefs>,
    pub apply_profile: Option<String>,
    pub save_profile: Option<String>,
    pub toggle_hud: bool,
    pub hud_enabled_changed: Option<bool>,
    pub hud_always_on_top_changed: Option<bool>,
    pub replay_enabled_changed: Option<bool>,
    pub replay_seconds_changed: Option<u32>,
    pub save_replay_clip: bool,
    pub resolution_preset: Option<(u32, u32, u32, u32)>,
    pub grade_changed: Option<GradePrefs>,
    pub grade_reset: bool,
    pub cube_path_changed: Option<String>,
    pub cube_clear: bool,
    pub spout_enabled_changed: Option<bool>,
    pub ndi_enabled_changed: Option<bool>,
    pub spout_name_changed: Option<String>,
    pub ndi_name_changed: Option<String>,
    pub export_refresh: bool,
    pub ndi_open_download: bool,
    pub ndi_open_folder: bool,
    pub dlss_open_docs: bool,
    pub dlss_open_folder: bool,
    pub health_auto_reinit_changed: Option<bool>,
    pub health_stall_secs_changed: Option<u32>,
    pub update_check: bool,
    pub update_open: bool,
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
    /// Mid motion blur intensity when framegen is on (0 = off … 1 = strong).
    pub interp_motion_blur: f32,
    pub deinterlace_mode: String,
    pub source_interlaced: bool,
    pub deinterlace_active: bool,
    /// auto | on | off
    pub vsync_mode: String,
    /// Prefer HDR swapchain when OS HDR is on.
    pub hdr_output: bool,
    /// Mode C — DLSS 5 NR ready.
    pub dlss_ok: bool,
    /// i18n key explaining why Mode C is limited.
    pub dlss_reason_key: String,
    /// Mode C selected but NR not running (pass-through).
    pub dlss_fallback_nis: bool,
    /// Boost (half-res NR + composite).
    pub dlss_boost: bool,
    pub dlssg_ok: bool,
    pub dlssg_reason_key: String,
    pub dlssg_multiplier: String,
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
            interp_motion_blur: 0.0,
            deinterlace_mode: "auto".into(),
            source_interlaced: false,
            deinterlace_active: false,
            vsync_mode: "auto".into(),
            hdr_output: true,
            dlss_ok: false,
            dlss_reason_key: String::new(),
            dlss_fallback_nis: false,
            dlss_boost: true,
            dlssg_ok: false,
            dlssg_reason_key: String::new(),
            dlssg_multiplier: "off".into(),
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
    pub logo: Option<egui::TextureHandle>,
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
        if i.key_pressed(Key::F3) {
            actions.toggle_hud = true;
        }
        if i.key_pressed(Key::F9) {
            actions.save_replay_clip = true;
        }
        if i.key_pressed(Key::F11) {
            actions.toggle_fullscreen = true;
        }
        if i.key_pressed(Key::M) && !i.modifiers.matches_logically(Modifiers::CTRL) {
            actions.mute_toggled = Some(!input.audio.muted);
        }
        if i.key_pressed(Key::Escape) {
            // Launch tips → settings → leave fullscreen.
            if input.chrome.launch_tips_alpha > 0.01 {
                actions.dismiss_launch_tips = true;
            } else if input.chrome.settings_open {
                actions.close_settings = true;
            } else if input.chrome.fullscreen {
                actions.toggle_fullscreen = true;
            }
        }
    });

    if input.chrome.wizard_open {
        draw_wizard(
            ctx,
            i18n,
            input.devices,
            input.selection,
            input.outputs,
            input.audio,
            input.chrome,
            &mut actions,
            input.logo.as_ref(),
        );
        return actions;
    }

    if input.overlay.metrics_mode != MetricsMode::Off {
        draw_metrics_overlay(ctx, i18n, input.overlay, input.chrome, &mut actions);
    }
    if input.overlay.hud_enabled {
        draw_compact_hud(ctx, i18n, input.overlay);
    }

    if input.chrome.toolbar_visible {
        draw_toolbar(ctx, i18n, input.chrome, &mut actions);
    }

    if input.chrome.launch_tips_alpha > 0.01 {
        draw_launch_tips(
            ctx,
            i18n,
            input.chrome.launch_tips_alpha,
            input.logo.as_ref(),
            &mut actions,
        );
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
                            ui.colored_label(
                                input.overlay.toast_kind.color(),
                                RichText::new(&input.overlay.toast).strong(),
                            );
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

    draw_profiles_block(ui, i18n, &input.overlay.profiles, actions);

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
    draw_resolution_presets(ui, i18n, caps, actions);
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

    // Mode A/B/C stay selectable at native res — RCAS / NIS sharpen still apply.
    let blocked = !fx.compute_ok;
    let reason = if !fx.compute_ok {
        i18n.t("effect_reason_no_compute")
    } else if fx.auto_disabled {
        i18n.t("effect_reason_budget")
    } else {
        String::new()
    };

    ui.label(i18n.t("label_upscale_mode"));
    // Mode A/B on the first row; Mode C always on its own row so a long FR label
    // never clips off the panel. When unavailable we paint a muted chip ourselves
    // (egui add_enabled can be nearly invisible on dark themes).
    ui.horizontal_wrapped(|ui| {
        for (id, key) in [
            ("off", "effect_off"),
            ("fsr1", "effect_fsr"),
            ("nis", "effect_nis"),
        ] {
            let selected = fx.mode == id;
            ui.add_enabled_ui(!blocked, |ui| {
                if ui.selectable_label(selected, i18n.t(key)).clicked() {
                    actions.upscale_mode = Some(id);
                }
            });
        }
    });
    ui.horizontal(|ui| {
        let label = i18n.t("effect_dlss");
        if !blocked && fx.dlss_ok {
            let selected = fx.mode == "dlss";
            if ui.selectable_label(selected, label).clicked() {
                actions.upscale_mode = Some("dlss");
            }
        } else {
            // Always visible greyed Mode C — not egui-disabled (too faint on AMD dark UI).
            let chip = format!("{} · {}", label, i18n.t("label_unavailable"));
            ui.label(
                RichText::new(chip)
                    .color(ui.visuals().weak_text_color())
                    .strong(),
            );
        }
    });
    if blocked || fx.auto_disabled {
        ui.small(format!("{}: {}", i18n.t("label_unavailable"), reason));
    } else if fx.native_res && fx.mode != "off" && fx.mode != "dlss" {
        ui.small(i18n.t("tip_native_sharpen"));
    }
    // Always explain Mode C status (greyed AMD/missing DLL included).
    {
        let key = if fx.dlss_ok {
            "effect_reason_dlss_active"
        } else if fx.dlss_reason_key.is_empty() {
            "effect_reason_dlss_needs_rtx"
        } else {
            fx.dlss_reason_key.as_str()
        };
        ui.label(
            RichText::new(format!("{}: {}", i18n.t("label_dlss_status"), i18n.t(key))).small(),
        );
    }
    if fx.mode == "dlss" && fx.dlss_ok {
        ui.label(RichText::new(i18n.t("tip_dlss")).small().italics());
        let mut boost = fx.dlss_boost;
        if ui
            .checkbox(&mut boost, i18n.t("label_dlss_boost"))
            .changed()
        {
            actions.dlss_boost_changed = Some(boost);
        }
        ui.label(RichText::new(i18n.t("tip_dlss_boost")).small().italics());
    } else if !fx.dlss_ok {
        ui.label(RichText::new(i18n.t("tip_dlss_greyed")).small().italics());
        ui.label(RichText::new(i18n.t("tip_dlss_byo")).small().italics());
        ui.horizontal_wrapped(|ui| {
            if ui.button(i18n.t("btn_dlss_docs")).clicked() {
                actions.dlss_open_docs = true;
            }
            if ui.button(i18n.t("btn_dlss_open_folder")).clicked() {
                actions.dlss_open_folder = true;
            }
        });
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

    let interp_on = fx.interp_mode == "x2" || fx.interp_mode == "auto";
    option_block(
        ui,
        i18n,
        &i18n.t("label_interp_motion_blur"),
        &i18n.t("explain_interp_motion_blur"),
        &i18n.t("tip_interp_motion_blur"),
        Impact::Light,
        Impact::Light,
        |ui| {
            let mut blur = fx.interp_motion_blur;
            if ui
                .add_enabled(
                    !interp_blocked && interp_on,
                    egui::Slider::new(&mut blur, 0.0..=1.0),
                )
                .changed()
            {
                actions.interp_motion_blur_changed = Some(blur);
            }
        },
    );

    ui.add_space(10.0);
    ui.separator();
    ui.label(RichText::new(i18n.t("label_dlssg")).strong());
    ui.label(RichText::new(i18n.t("tip_dlssg")).small().italics());
    // Always show FG multipliers; grey ×2+ when RTX + BYO dlssg DLL is missing.
    ui.horizontal(|ui| {
        let selected_off = fx.dlssg_multiplier == "off";
        if ui
            .selectable_label(selected_off, i18n.t("effect_dlssg_off"))
            .clicked()
        {
            actions.dlssg_multiplier = Some("off");
        }
        for (id, key) in [
            ("x2", "effect_dlssg_x2"),
            ("x3", "effect_dlssg_x3"),
            ("x4", "effect_dlssg_x4"),
        ] {
            let selected = fx.dlssg_multiplier == id;
            ui.add_enabled_ui(fx.dlssg_ok, |ui| {
                if ui.selectable_label(selected, i18n.t(key)).clicked() {
                    actions.dlssg_multiplier = Some(id);
                }
            });
        }
    });
    {
        let key = if fx.dlssg_ok {
            "effect_reason_dlssg_active"
        } else if fx.dlssg_reason_key.is_empty() {
            "effect_reason_dlssg_needs_rtx"
        } else {
            fx.dlssg_reason_key.as_str()
        };
        let style = if fx.dlssg_ok {
            RichText::new(format!("{}: {}", i18n.t("label_dlssg_status"), i18n.t(key))).small()
        } else {
            RichText::new(format!("{}: {}", i18n.t("label_dlssg_status"), i18n.t(key)))
                .small()
                .weak()
        };
        ui.label(style);
    }

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
    ui.label(RichText::new(i18n.t("label_hdr")).strong());
    ui.label(RichText::new(i18n.t("tip_hdr")).small().italics());
    ui.horizontal(|ui| {
        let on = fx.hdr_output;
        if ui
            .selectable_label(on, i18n.t("hdr_on"))
            .clicked()
            && !on
        {
            actions.hdr_output_changed = Some(true);
        }
        if ui
            .selectable_label(!on, i18n.t("hdr_off"))
            .clicked()
            && on
        {
            actions.hdr_output_changed = Some(false);
        }
    });
    ui.small(hdr_status_line(i18n, input.overlay));
    ui.small(vrr_status_line(i18n, input.overlay));

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
    // Infos packs many toggles; keep them reachable without growing the window forever.
    let max_h = (ui.ctx().screen_rect().height() * 0.65).clamp(280.0, 640.0);
    egui::ScrollArea::vertical()
        .id_salt("info_tab_scroll")
        .max_height(max_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            draw_info_tab_body(ui, input, actions);
        });
}

fn draw_info_tab_body(ui: &mut egui::Ui, input: &DrawInput<'_>, actions: &mut UiActions) {
    let i18n = input.i18n;
    let o = input.overlay;
    draw_brand_header(ui, input.logo.as_ref(), i18n);
    ui.label(RichText::new(i18n.t("metrics_disclaimer")).small().weak());
    ui.add_space(4.0);
    draw_monitor_prefs(ui, i18n, &o.monitor, actions);
    ui.add_space(8.0);
    ui.separator();
    draw_hud_prefs(ui, i18n, o, actions);
    ui.add_space(6.0);
    draw_replay_prefs(ui, i18n, o, actions);
    ui.add_space(6.0);
    draw_grade_prefs(ui, i18n, o, actions);
    ui.add_space(6.0);
    draw_export_prefs(ui, i18n, o, actions);
    ui.add_space(6.0);
    draw_health_prefs(ui, i18n, o, actions);
    ui.add_space(6.0);
    draw_update_prefs(ui, i18n, o, actions);
    ui.add_space(8.0);
    ui.separator();
    draw_metrics_body(ui, i18n, o, true);
    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new(i18n.t("label_language")).strong());
    ui.label(RichText::new(i18n.t("tip_language")).small().italics());
    ui.horizontal(|ui| {
        let current = i18n.lang().as_str();
        for (id, label_key) in [("en", "lang_en"), ("fr", "lang_fr")] {
            if ui
                .selectable_label(current == id, i18n.t(label_key))
                .clicked()
                && current != id
            {
                actions.language_changed = Some(id);
            }
        }
    });
    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new(i18n.t("label_discord")).strong());
    ui.label(RichText::new(i18n.t("tip_discord")).small().italics());
    let mut enabled = o.discord_presence;
    if ui
        .checkbox(&mut enabled, i18n.t("label_discord_enable"))
        .changed()
    {
        actions.discord_presence_changed = Some(enabled);
    }
    ui.horizontal(|ui| {
        ui.label(i18n.t("label_discord_app_id"));
        let mut id = o.discord_app_id.clone();
        let resp = ui.add(
            egui::TextEdit::singleline(&mut id)
                .desired_width(220.0)
                .hint_text(crate::discord_presence::DEFAULT_APP_ID),
        );
        if resp.changed() {
            actions.discord_app_id_changed = Some(id);
        }
    });
    if o.discord_active {
        ui.small(i18n.t("discord_status_on"));
    } else if o.discord_presence {
        ui.small(i18n.t("discord_status_need_id"));
    }
    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new(i18n.t("label_logging")).strong());
    ui.label(RichText::new(i18n.t("tip_logging")).small().italics());
    ui.horizontal(|ui| {
        ui.label(i18n.t("label_log_level"));
        egui::ComboBox::from_id_salt("log_level")
            .selected_text(o.log_level.clone())
            .show_ui(ui, |ui| {
                for level in ["error", "warn", "info", "debug", "trace"] {
                    if ui
                        .selectable_label(o.log_level == level, level)
                        .clicked()
                    {
                        actions.log_level_changed = Some(level);
                    }
                }
            });
    });
    if !o.log_path.is_empty() {
        ui.small(format!("{}: {}", i18n.t("label_log_path"), o.log_path));
    }
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

fn hdr_status_line(i18n: &I18n, o: &OverlayState) -> String {
    if o.hdr_active {
        format!(
            "{} · {}",
            i18n.t("hdr_status_active"),
            o.surface_format
        )
    } else if o.hdr_output && o.hdr_enabled {
        i18n.t("hdr_status_fallback")
    } else if o.hdr_output && o.hdr_supported && !o.hdr_enabled {
        i18n.t("hdr_status_os_off")
    } else if !o.hdr_supported {
        i18n.t("hdr_status_unsupported")
    } else {
        i18n.t("hdr_status_off")
    }
}

fn vrr_status_line(i18n: &I18n, o: &OverlayState) -> String {
    let mode = if o.present_mode.is_empty() {
        "—"
    } else {
        o.present_mode.as_str()
    };
    if o.vrr_capable {
        format!("{} ({})", i18n.t("vrr_status_capable"), mode)
    } else {
        format!("{} ({})", i18n.t("vrr_status_unavailable"), mode)
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

fn draw_monitor_prefs(
    ui: &mut egui::Ui,
    i18n: &I18n,
    prefs: &MonitorPrefs,
    actions: &mut UiActions,
) {
    ui.label(RichText::new(i18n.t("section_monitor_graphs")).strong());
    ui.label(
        RichText::new(i18n.t("tip_monitor_graphs"))
            .small()
            .italics(),
    );
    let mut next = prefs.clone();
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        changed |= ui
            .checkbox(&mut next.graph_cpu, i18n.t("mon_graph_cpu"))
            .changed();
        changed |= ui
            .checkbox(&mut next.graph_ram, i18n.t("mon_graph_ram"))
            .changed();
        changed |= ui
            .checkbox(&mut next.graph_gpu, i18n.t("mon_graph_gpu"))
            .changed();
    });
    ui.horizontal_wrapped(|ui| {
        changed |= ui
            .checkbox(&mut next.cpu_cores, i18n.t("mon_cpu_cores"))
            .changed();
        changed |= ui.checkbox(&mut next.vram, i18n.t("mon_vram")).changed();
        changed |= ui
            .checkbox(&mut next.process_ram, i18n.t("mon_process_ram"))
            .changed();
        changed |= ui
            .checkbox(&mut next.disk_io, i18n.t("mon_disk_io"))
            .changed();
    });
    ui.horizontal_wrapped(|ui| {
        changed |= ui
            .checkbox(&mut next.fps_1pct, i18n.t("mon_fps_1pct"))
            .changed();
        changed |= ui
            .checkbox(&mut next.present_latency, i18n.t("mon_present_latency"))
            .changed();
        changed |= ui
            .checkbox(&mut next.framegen_rate, i18n.t("mon_framegen_rate"))
            .changed();
    });
    if changed {
        actions.monitor_prefs_changed = Some(next);
    }
}

fn draw_profiles_block(
    ui: &mut egui::Ui,
    i18n: &I18n,
    profiles: &ProfilesPrefs,
    actions: &mut UiActions,
) {
    ui.label(RichText::new(i18n.t("section_profiles")).strong());
    ui.label(RichText::new(i18n.t("tip_profiles")).small().italics());
    ui.horizontal_wrapped(|ui| {
        for p in &profiles.profiles {
            let selected = profiles.active == p.id;
            let label = i18n.t(&p.label_key);
            if ui.selectable_label(selected, label).clicked() {
                actions.apply_profile = Some(p.id.clone());
            }
        }
    });
    if profiles.active != "custom" {
        if ui
            .button(i18n.t("btn_profile_save"))
            .on_hover_text(i18n.t("tip_profiles"))
            .clicked()
        {
            actions.save_profile = Some(profiles.active.clone());
        }
    }
    ui.add_space(6.0);
}

fn draw_resolution_presets(
    ui: &mut egui::Ui,
    i18n: &I18n,
    caps: &[FormatDesc],
    actions: &mut UiActions,
) {
    ui.label(RichText::new(i18n.t("section_presets_res")).strong());
    ui.label(RichText::new(i18n.t("tip_presets_res")).small().italics());
    let presets = [
        (1920, 1080, 60, 1, "preset_1080p60"),
        (1920, 1080, 30, 1, "preset_1080p30"),
        (1280, 720, 60, 1, "preset_720p60"),
        (640, 480, 60, 1, "preset_480p60"),
    ];
    ui.horizontal_wrapped(|ui| {
        for (w, h, num, den, key) in presets {
            let rate = Rational::new(num, den);
            let available = capture::modes::pick_format_at(caps, w, h, rate).is_some()
                || caps.iter().any(|f| f.width == w && f.height == h);
            ui.add_enabled_ui(available, |ui| {
                if ui.button(i18n.t(key)).clicked() {
                    actions.resolution_preset = Some((w, h, num, den));
                }
            });
        }
    });
    ui.add_space(6.0);
}

fn draw_compact_hud(ctx: &Context, i18n: &I18n, overlay: &OverlayState) {
    let frame = egui::Frame::NONE
        .fill(Color32::from_rgba_unmultiplied(12, 14, 18, 190))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(60, 70, 90)));
    egui::Window::new("hud_compact")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::LEFT_TOP, [12.0, 12.0])
        .frame(frame)
        .show(ctx, |ui| {
            ui.label(
                RichText::new(format!(
                    "{} {:.0}  ·  {} {:.0}  ·  {} {:.1} ms",
                    i18n.t("hud_fps"),
                    overlay.fps,
                    i18n.t("hud_one_pct"),
                    overlay.one_percent_low_fps,
                    i18n.t("hud_latency"),
                    overlay.pipeline_delay_ms
                ))
                .monospace()
                .size(13.0),
            );
            if overlay.health.kind != CaptureHealthKind::Ok {
                ui.colored_label(
                    Color32::from_rgb(255, 180, 100),
                    health_label(i18n, overlay.health.kind),
                );
            }
        });
}

fn health_label(i18n: &I18n, kind: CaptureHealthKind) -> String {
    let key = match kind {
        CaptureHealthKind::Ok => "health_ok",
        CaptureHealthKind::WaitingForSignal => "health_waiting",
        CaptureHealthKind::DeviceBusy => "health_busy",
        CaptureHealthKind::StreamError => "health_error",
        CaptureHealthKind::Stall => "health_stall",
    };
    format!("{}: {}", i18n.t("label_health_status"), i18n.t(key))
}

fn draw_hud_prefs(ui: &mut egui::Ui, i18n: &I18n, o: &OverlayState, actions: &mut UiActions) {
    ui.label(RichText::new(i18n.t("section_hud")).strong());
    ui.label(RichText::new(i18n.t("tip_hud")).small().italics());
    let mut hud = o.hud_enabled;
    if ui.checkbox(&mut hud, i18n.t("label_hud_enable")).changed() {
        actions.hud_enabled_changed = Some(hud);
    }
    let mut top = o.hud_always_on_top;
    if ui
        .checkbox(&mut top, i18n.t("label_hud_always_on_top"))
        .changed()
    {
        actions.hud_always_on_top_changed = Some(top);
    }
}

fn draw_replay_prefs(ui: &mut egui::Ui, i18n: &I18n, o: &OverlayState, actions: &mut UiActions) {
    ui.label(RichText::new(i18n.t("section_replay")).strong());
    ui.label(RichText::new(i18n.t("tip_replay")).small().italics());
    let mut on = o.replay_enabled;
    if ui
        .checkbox(&mut on, i18n.t("label_replay_enable"))
        .changed()
    {
        actions.replay_enabled_changed = Some(on);
    }
    let mut secs = o.replay_seconds;
    if ui
        .add(egui::Slider::new(&mut secs, 3..=30).text(i18n.t("label_replay_seconds")))
        .changed()
    {
        actions.replay_seconds_changed = Some(secs);
    }
    ui.small(format!("{}: {}", i18n.t("btn_replay_save"), o.replay_frames));
    if ui.button(i18n.t("btn_replay_save")).clicked() {
        actions.save_replay_clip = true;
    }
}

fn draw_grade_prefs(ui: &mut egui::Ui, i18n: &I18n, o: &OverlayState, actions: &mut UiActions) {
    ui.label(RichText::new(i18n.t("section_grade")).strong());
    ui.label(RichText::new(i18n.t("tip_grade")).small().italics());
    let mut g = o.grade;
    let mut changed = false;
    changed |= ui
        .add(egui::Slider::new(&mut g.brightness, -0.5..=0.5).text(i18n.t("label_brightness")))
        .changed();
    changed |= ui
        .add(egui::Slider::new(&mut g.contrast, 0.5..=2.0).text(i18n.t("label_contrast")))
        .changed();
    changed |= ui
        .add(egui::Slider::new(&mut g.gamma, 0.4..=2.5).text(i18n.t("label_gamma")))
        .changed();
    if changed {
        actions.grade_changed = Some(g.clamp());
    }
    ui.horizontal(|ui| {
        if ui.button(i18n.t("btn_grade_reset")).clicked() {
            actions.grade_reset = true;
        }
        if ui.button(i18n.t("btn_cube_clear")).clicked() {
            actions.cube_clear = true;
        }
    });
    let mut path = o.cube_lut_path.clone();
    ui.horizontal(|ui| {
        ui.label(i18n.t("label_cube_path"));
        if ui
            .add(egui::TextEdit::singleline(&mut path).desired_width(220.0))
            .lost_focus()
            && path != o.cube_lut_path
        {
            actions.cube_path_changed = Some(path);
        }
    });
}

fn draw_export_prefs(ui: &mut egui::Ui, i18n: &I18n, o: &OverlayState, actions: &mut UiActions) {
    ui.label(RichText::new(i18n.t("section_export")).strong());
    ui.label(RichText::new(i18n.t("tip_export")).small().italics());

    // Spout — bundled on Windows; no “missing” when DLL ships with the app.
    let mut spout = o.spout_enabled;
    ui.add_enabled_ui(o.export.spout_available || !cfg!(windows), |ui| {
        if ui.checkbox(&mut spout, i18n.t("label_spout")).changed() {
            actions.spout_enabled_changed = Some(spout);
        }
    });
    let spout_key = if o.export.spout_detail_key.is_empty() {
        "export_spout_missing"
    } else {
        o.export.spout_detail_key.as_str()
    };
    ui.label(RichText::new(i18n.t(spout_key)).small());
    if !o.export.spout_dll_path.is_empty() {
        ui.label(
            RichText::new(format!(
                "{}: {}",
                i18n.t("label_export_dll"),
                o.export.spout_dll_path
            ))
            .small()
            .weak(),
        );
    }
    ui.label(RichText::new(i18n.t("tip_spout_send")).small().italics());
    let mut spout_name = o.spout_name.clone();
    if ui
        .add(
            egui::TextEdit::singleline(&mut spout_name)
                .desired_width(160.0)
                .hint_text(i18n.t("label_export_name")),
        )
        .changed()
    {
        actions.spout_name_changed = Some(spout_name);
    }

    ui.add_space(6.0);
    // NDI — BYO only; never redistribute NDI DLLs.
    let mut ndi = o.ndi_enabled;
    ui.add_enabled_ui(o.export.ndi_available, |ui| {
        if ui.checkbox(&mut ndi, i18n.t("label_ndi")).changed() {
            actions.ndi_enabled_changed = Some(ndi);
        }
    });
    let ndi_key = if o.export.ndi_detail_key.is_empty() {
        "export_ndi_missing"
    } else {
        o.export.ndi_detail_key.as_str()
    };
    ui.label(RichText::new(i18n.t(ndi_key)).small());
    ui.label(RichText::new(i18n.t("tip_ndi_setup")).small().italics());
    ui.horizontal_wrapped(|ui| {
        if ui.button(i18n.t("btn_ndi_download")).clicked() {
            actions.ndi_open_download = true;
        }
        if ui.button(i18n.t("btn_export_refresh")).clicked() {
            actions.export_refresh = true;
        }
        if !o.export.ndi_folder.is_empty()
            && ui.button(i18n.t("btn_ndi_open_folder")).clicked()
        {
            actions.ndi_open_folder = true;
        }
    });
    let mut ndi_name = o.ndi_name.clone();
    if ui
        .add(
            egui::TextEdit::singleline(&mut ndi_name)
                .desired_width(160.0)
                .hint_text(i18n.t("label_export_name")),
        )
        .changed()
    {
        actions.ndi_name_changed = Some(ndi_name);
    }
}

fn draw_health_prefs(ui: &mut egui::Ui, i18n: &I18n, o: &OverlayState, actions: &mut UiActions) {
    ui.label(RichText::new(i18n.t("section_health")).strong());
    ui.label(RichText::new(i18n.t("tip_health")).small().italics());
    ui.label(health_label(i18n, o.health.kind));
    ui.small(format!(
        "{}: {} · reinit ×{}",
        i18n.t("label_dropped"),
        o.health.dropped,
        o.health.reinit_count
    ));
    let mut auto = o.health_auto_reinit;
    if ui
        .checkbox(&mut auto, i18n.t("label_health_auto"))
        .changed()
    {
        actions.health_auto_reinit_changed = Some(auto);
    }
    let mut stall = o.health_stall_secs;
    if ui
        .add(egui::Slider::new(&mut stall, 2..=15).text(i18n.t("label_health_stall")))
        .changed()
    {
        actions.health_stall_secs_changed = Some(stall);
    }
}

fn draw_update_prefs(ui: &mut egui::Ui, i18n: &I18n, o: &OverlayState, actions: &mut UiActions) {
    ui.label(RichText::new(i18n.t("section_update")).strong());
    ui.label(RichText::new(i18n.t("tip_update")).small().italics());
    ui.small(format!(
        "{}: v{}",
        i18n.t("label_current_version"),
        o.current_version
    ));
    if o.update.checking {
        ui.label(i18n.t("update_checking"));
    } else if !o.update.error.is_empty() {
        ui.colored_label(
            Color32::from_rgb(220, 120, 90),
            format!("{} {}", i18n.t("update_error"), o.update.error),
        );
    } else if o.update.newer {
        ui.colored_label(
            Color32::from_rgb(120, 200, 140),
            format!("{}: {}", i18n.t("update_available"), o.update.latest_tag),
        );
        if !o.update.body.is_empty() {
            ui.small(o.update.body.chars().take(240).collect::<String>());
        }
    } else if !o.update.latest_tag.is_empty() {
        ui.label(i18n.t("update_up_to_date"));
    }
    ui.horizontal(|ui| {
        if ui.button(i18n.t("btn_update_check")).clicked() {
            actions.update_check = true;
        }
        if ui.button(i18n.t("btn_update_open")).clicked() {
            actions.update_open = true;
        }
    });
}

fn draw_sparkline(ui: &mut egui::Ui, values: &[f32], max_hint: f32, color_fn: impl Fn(f32) -> Color32) {
    if values.is_empty() {
        ui.weak("—");
        return;
    }
    let max_v = values
        .iter()
        .copied()
        .fold(max_hint, f32::max)
        .max(max_hint)
        .max(1.0);
    let (rect, _resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().min(280.0), 40.0),
        Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, Color32::from_rgb(24, 28, 34));
    let n = values.len().max(1) as f32;
    for (i, &v) in values.iter().enumerate() {
        let x0 = rect.left() + rect.width() * (i as f32 / n);
        let x1 = rect.left() + rect.width() * ((i as f32 + 1.0) / n);
        let h = (v / max_v).clamp(0.0, 1.0) * rect.height();
        let bar = egui::Rect::from_min_max(
            egui::pos2(x0, rect.bottom() - h),
            egui::pos2(x1.max(x0 + 1.0), rect.bottom()),
        );
        painter.rect_filled(bar, 0.0, color_fn(v));
    }
}

fn draw_cpu_cores(ui: &mut egui::Ui, i18n: &I18n, o: &OverlayState) {
    ui.label(i18n.t("label_cpu_cores"));
    let cores = o.per_core_frac.len().max(o.logical_cores as usize).max(1);
    let fracs = if o.per_core_frac.is_empty() {
        vec![0.0f32; cores]
    } else {
        o.per_core_frac.clone()
    };
    let cols = cores.min(16).max(1);
    let bar_w = (ui.available_width().min(280.0) / cols as f32).clamp(8.0, 18.0);
    let bar_h = 28.0;
    ui.horizontal_wrapped(|ui| {
        for (i, &frac) in fracs.iter().enumerate().take(64) {
            let allowed = if i < 64 {
                (o.affinity_mask >> i) & 1 == 1
            } else {
                true
            };
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(bar_w, bar_h), Sense::hover());
            let painter = ui.painter_at(rect);
            let bg = if allowed {
                Color32::from_rgb(28, 34, 42)
            } else {
                Color32::from_rgb(18, 18, 22)
            };
            painter.rect_filled(rect, 2.0, bg);
            let h = frac.clamp(0.0, 1.0) * rect.height();
            let fill = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.bottom() - h),
                egui::pos2(rect.right(), rect.bottom()),
            );
            let color = if !allowed {
                Color32::from_rgb(60, 60, 70)
            } else if frac > 0.85 {
                Color32::from_rgb(220, 100, 90)
            } else if frac > 0.55 {
                Color32::from_rgb(220, 180, 80)
            } else {
                Color32::from_rgb(90, 180, 140)
            };
            painter.rect_filled(fill, 0.0, color);
            if allowed {
                painter.rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(1.0_f32, Color32::from_rgb(70, 140, 200)),
                    egui::StrokeKind::Inside,
                );
            }
            resp.on_hover_text(format!(
                "{} {} · {:.0}%{}",
                i18n.t("label_core"),
                i,
                frac * 100.0,
                if allowed {
                    format!(" · {}", i18n.t("label_core_affinity"))
                } else {
                    String::new()
                }
            ));
        }
    });
    ui.label(
        RichText::new(i18n.t("tip_cpu_cores_affinity"))
            .small()
            .weak(),
    );
}

fn draw_metrics_body(ui: &mut egui::Ui, i18n: &I18n, o: &OverlayState, detailed: bool) {
    let m = &o.monitor;
    if m.framegen_rate {
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
    } else {
        ui.label(format!(
            "{}: {:.0}  ·  {}: {:.0}",
            i18n.t("label_capture_fps"),
            o.capture_fps,
            i18n.t("label_display_fps"),
            o.fps
        ));
    }
    if m.present_latency {
        ui.label(format!(
            "{}: {:.0} ms  ·  {}: {}",
            i18n.t("label_pipeline_delay"),
            o.pipeline_delay_ms,
            i18n.t("label_display_fps_limit"),
            display_fps_limit_label(i18n, o.display_fps_limit)
        ));
    } else {
        ui.label(format!(
            "{}: {}",
            i18n.t("label_display_fps_limit"),
            display_fps_limit_label(i18n, o.display_fps_limit)
        ));
    }
    ui.label(format!(
        "{}: {}",
        i18n.t("label_vsync"),
        vsync_mode_label(i18n, &o.vsync_mode)
    ));
    ui.label(vrr_status_line(i18n, o));
    ui.label(hdr_status_line(i18n, o));
    if !detailed {
        // Simple mode still shows enabled graphs so toggles are useful without F2 detail.
        draw_monitor_graphs(ui, i18n, o);
        return;
    }
    if m.fps_1pct {
        ui.label(format!(
            "{}: {:.0}  ·  {}: {}",
            i18n.t("label_one_percent_low"),
            o.one_percent_low_fps,
            i18n.t("label_dropped"),
            o.dropped_frames
        ));
    } else {
        ui.label(format!("{}: {}", i18n.t("label_dropped"), o.dropped_frames));
    }
    ui.label(health_label(i18n, o.health.kind));

    if m.process_ram {
        ui.label(format!(
            "{}: {:.0}%  ·  {}: {:.0} Mo",
            i18n.t("label_cpu"),
            o.cpu_one_core * 100.0,
            i18n.t("label_ram"),
            o.ram_mb
        ));
    } else {
        ui.label(format!(
            "{}: {:.0}%",
            i18n.t("label_cpu"),
            o.cpu_one_core * 100.0
        ));
    }
    if m.vram && o.vram_budget_mb > 0.0 {
        ui.label(format!(
            "{}: {:.0} / {:.0} Mo",
            i18n.t("label_vram"),
            o.vram_used_mb,
            o.vram_budget_mb
        ));
    }
    if m.disk_io {
        ui.label(format!(
            "{}: ↓ {:.1} · ↑ {:.1} Mo/s",
            i18n.t("label_disk_io"),
            o.io_read_mbps,
            o.io_write_mbps
        ));
    }
    ui.label(format!("{}: {}", i18n.t("label_gpu"), o.gpu_name));
    ui.label(format!("{}: {}", i18n.t("label_active_mode"), o.mode_label));
    ui.label(format!("{}: {}", i18n.t("label_active_format"), o.format_label));
    draw_monitor_graphs(ui, i18n, o);
}

fn draw_monitor_graphs(ui: &mut egui::Ui, i18n: &I18n, o: &OverlayState) {
    let m = &o.monitor;
    if m.graph_cpu {
        ui.add_space(4.0);
        ui.label(format!(
            "{} · {:.0}%",
            i18n.t("label_graph_cpu"),
            o.cpu_one_core * 100.0
        ));
        draw_sparkline(ui, &o.cpu_history, 50.0, |v| {
            if v > 80.0 {
                Color32::from_rgb(220, 100, 90)
            } else if v > 40.0 {
                Color32::from_rgb(220, 180, 80)
            } else {
                Color32::from_rgb(90, 160, 220)
            }
        });
    }
    if m.graph_ram {
        ui.add_space(4.0);
        let sys = if o.system_ram_total_mb > 0.0 {
            format!(
                " · {} {:.0}/{:.0} Mo",
                i18n.t("label_system_ram"),
                o.system_ram_used_mb,
                o.system_ram_total_mb
            )
        } else {
            String::new()
        };
        ui.label(format!(
            "{} · {:.0} Mo{}",
            i18n.t("label_graph_ram"),
            o.ram_mb,
            sys
        ));
        let max_hint = o.ram_history.iter().copied().fold(256.0f32, f32::max);
        draw_sparkline(ui, &o.ram_history, max_hint, |_| {
            Color32::from_rgb(160, 120, 220)
        });
    }
    if m.graph_gpu {
        ui.add_space(4.0);
        ui.label(i18n.t("label_graph_gpu"));
        let times = &o.frame_times_ms;
        if times.is_empty() {
            ui.weak("—");
        } else {
            draw_sparkline(ui, times, 20.0, |ms| {
                if ms > 25.0 {
                    Color32::from_rgb(220, 100, 90)
                } else if ms > 18.0 {
                    Color32::from_rgb(220, 180, 80)
                } else {
                    Color32::from_rgb(90, 180, 140)
                }
            });
        }
    }
    if m.cpu_cores {
        ui.add_space(4.0);
        draw_cpu_cores(ui, i18n, o);
    }
}

fn draw_brand_header(
    ui: &mut egui::Ui,
    logo: Option<&egui::TextureHandle>,
    i18n: &I18n,
) {
    ui.horizontal(|ui| {
        if let Some(tex) = logo {
            ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(40.0, 40.0)));
        }
        ui.vertical(|ui| {
            ui.label(RichText::new(i18n.t("app_title")).strong().size(18.0));
        });
    });
    ui.add_space(6.0);
}

/// Soft, centered shortcuts card shown once after launch (auto-fades; Esc/click dismiss).
fn draw_launch_tips(
    ctx: &Context,
    i18n: &I18n,
    alpha: f32,
    logo: Option<&egui::TextureHandle>,
    actions: &mut UiActions,
) {
    let a = alpha.clamp(0.0, 1.0);
    // Smooth fade without a busy loop (display FPS still caps presents).
    ctx.request_repaint_after(std::time::Duration::from_millis(16));
    // Any click dismisses (button / card / empty space).
    if ctx.input(|i| i.pointer.any_click()) {
        actions.dismiss_launch_tips = true;
    }

    let fill = Color32::from_rgba_unmultiplied(12, 14, 18, (200.0 * a) as u8);
    let text = Color32::from_rgba_unmultiplied(235, 238, 245, (255.0 * a) as u8);
    let muted = Color32::from_rgba_unmultiplied(180, 186, 198, (220.0 * a) as u8);

    let rows = [
        ("launch_tips_key_f1", "launch_tips_desc_f1"),
        ("launch_tips_key_f2", "launch_tips_desc_f2"),
        ("launch_tips_key_f3", "launch_tips_desc_f3"),
        ("launch_tips_key_f9", "launch_tips_desc_f9"),
        ("launch_tips_key_f11", "launch_tips_desc_f11"),
        ("launch_tips_key_esc", "launch_tips_desc_esc"),
        ("launch_tips_key_pin", "launch_tips_desc_pin"),
        ("launch_tips_key_mouse", "launch_tips_desc_mouse"),
    ];

    egui::Area::new(egui::Id::new("launch_tips"))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let frame = egui::Frame::NONE
                .fill(fill)
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(20, 16))
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    Color32::from_rgba_unmultiplied(80, 90, 110, (140.0 * a) as u8),
                ));
            frame.show(ui, |ui| {
                ui.set_min_width(360.0);
                ui.horizontal(|ui| {
                    if let Some(tex) = logo {
                        ui.add(
                            egui::Image::new(tex)
                                .fit_to_exact_size(egui::vec2(28.0, 28.0))
                                .tint(Color32::from_rgba_unmultiplied(255, 255, 255, (255.0 * a) as u8)),
                        );
                    }
                    ui.label(
                        RichText::new(i18n.t("launch_tips_title"))
                            .strong()
                            .color(text)
                            .size(16.0),
                    );
                });
                ui.add_space(12.0);
                for (key_id, desc_id) in rows {
                    draw_launch_tip_row(ui, a, &i18n.t(key_id), &i18n.t(desc_id), text);
                    ui.add_space(6.0);
                }
                ui.add_space(6.0);
                ui.label(RichText::new(i18n.t("launch_tips_hint")).small().color(muted));
                ui.add_space(8.0);
                let _ = ui.add_enabled(a > 0.4, egui::Button::new(i18n.t("launch_tips_dismiss")));
            });
        });
}

/// Keyboard-key style chip + purpose label (matches CaptureView dark chrome).
fn draw_launch_tip_row(ui: &mut egui::Ui, alpha: f32, key: &str, purpose: &str, text: Color32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        draw_keycap(ui, alpha, key);
        ui.vertical(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(purpose).color(text).size(14.0));
        });
    });
}

fn draw_keycap(ui: &mut egui::Ui, alpha: f32, label: &str) {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui.fonts(|f| f.layout_no_wrap(label.to_owned(), font, Color32::WHITE));
    let pad_x = 12.0_f32;
    let pad_y = 6.0_f32;
    let min_w = 52.0_f32;
    let size = egui::vec2(
        (galley.size().x + pad_x * 2.0).max(min_w),
        (galley.size().y + pad_y * 2.0).max(28.0),
    );
    let (rect, _resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);

    let radius = egui::CornerRadius::same(5);
    // Soft “raised key” stack: shadow lip → face → top highlight.
    let shadow = Color32::from_rgba_unmultiplied(0, 0, 0, (110.0 * alpha) as u8);
    let face = Color32::from_rgba_unmultiplied(36, 42, 54, (235.0 * alpha) as u8);
    let edge = Color32::from_rgba_unmultiplied(120, 135, 160, (200.0 * alpha) as u8);
    let label_c = Color32::from_rgba_unmultiplied(245, 248, 255, (255.0 * alpha) as u8);

    let lip = rect.translate(egui::vec2(0.0, 2.0));
    painter.rect_filled(lip, radius, shadow);
    painter.rect_filled(rect, radius, face);
    painter.rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0_f32, edge),
        egui::StrokeKind::Inside,
    );
    // Subtle top sheen.
    let sheen = egui::Rect::from_min_max(
        rect.left_top() + egui::vec2(2.0, 2.0),
        egui::pos2(rect.right() - 2.0, rect.top() + 5.0),
    );
    painter.rect_filled(
        sheen,
        egui::CornerRadius {
            nw: 3,
            ne: 3,
            sw: 0,
            se: 0,
        },
        Color32::from_rgba_unmultiplied(255, 255, 255, (22.0 * alpha) as u8),
    );

    let text_pos = egui::pos2(
        rect.center().x - galley.size().x * 0.5,
        rect.center().y - galley.size().y * 0.5 - 0.5,
    );
    painter.galley(text_pos, galley, label_c);
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
    logo: Option<&egui::TextureHandle>,
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
            draw_brand_header(ui, logo, i18n);
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
