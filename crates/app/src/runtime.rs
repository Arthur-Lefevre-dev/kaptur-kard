//! winit + wgpu + egui runtime with Mock and platform video sources.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, TryRecvError};

use audio::{
    list_inputs, list_outputs, suggest_input_for_video, AudioDeviceInfo, AudioEngine,
    AudioEngineConfig, NONE_INPUT_ID,
};
use capture::{
    hz_label, modes, CaptureError, CaptureMode, DeviceInfo, FormatDesc, FrameReceiver, FrameWake,
    MockVideoSource, PixelFormat, Rational, StreamHealth, StreamStatus, VideoFrame, VideoSource,
};
use egui_wgpu::ScreenDescriptor;
use egui_winit::State as EguiWinitState;
use metrics::{sample_process, SharedMetrics};
use render::pipeline::VideoPipeline;
use render::upload::upload_frame;
use render::{
    is_native_or_downscale, pick_present_mode, pick_surface_format, resolve_color_convert,
    ColorMatrixMode, ColorRangeMode, DeinterlaceEngine, DeinterlaceMode, DlssGMultiplier,
    InterpBlockReason, InterpMode, InterpolateEngine, PresentCaps, UpscaleBlockReason,
    UpscaleEngine, UpscaleMode,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::window::{Window, WindowId};

/// Hide the OS cursor after this much pointer idle (capture-preview UX).
const CURSOR_IDLE_HIDE: Duration = Duration::from_millis(1500);

/// Wakes the UI when the capture thread queues a frame (capture-driven present).
#[derive(Debug, Clone, Copy)]
pub enum UserEvent {
    FrameReady,
}

use crate::discord_presence::{resolve_app_id, DiscordPresence};
use crate::display_caps::OsDisplayCaps;
use crate::export::FrameExport;
use crate::health::HealthMonitor;
use crate::i18n::{I18n, Lang};
use crate::lut::{CubeLut1d, GradePrefs};
use crate::prefs::{
    load_audio_prefs, load_ui_prefs, save_audio_prefs, save_ui_prefs, AudioPrefs, UiPrefs,
};
use crate::profiles::NamedProfile;
use crate::replay::ReplayBuffer;
use crate::ui::{
    self, AudioUiState, ColorUiState, EffectsUiState, OverlayState, ToastKind, UiChrome,
    UiSelection,
};
use crate::update_check::UpdateChecker;
use winit::window::WindowLevel;

/// Result of a background Media Foundation presence / enum poll.
enum HotplugResult {
    Unchanged,
    Changed {
        before_count: usize,
        devices: Vec<DeviceInfo>,
    },
}

enum ActiveSource {
    Mock(MockVideoSource),
    #[cfg(windows)]
    Mf(capture::MfVideoSource),
    #[cfg(target_os = "linux")]
    V4l2(capture::V4l2VideoSource),
    #[cfg(target_os = "macos")]
    Avf(capture::AvfVideoSource),
}

impl ActiveSource {
    fn info(&self) -> &DeviceInfo {
        match self {
            Self::Mock(s) => s.info(),
            #[cfg(windows)]
            Self::Mf(s) => s.info(),
            #[cfg(target_os = "linux")]
            Self::V4l2(s) => s.info(),
            #[cfg(target_os = "macos")]
            Self::Avf(s) => s.info(),
        }
    }

    fn set_frame_wake(&mut self, wake: FrameWake) {
        match self {
            Self::Mock(s) => s.set_frame_wake(wake),
            #[cfg(windows)]
            Self::Mf(s) => s.set_frame_wake(wake),
            #[cfg(target_os = "linux")]
            Self::V4l2(s) => s.set_frame_wake(wake),
            #[cfg(target_os = "macos")]
            Self::Avf(s) => s.set_frame_wake(wake),
        }
    }

    fn start(&mut self, selected: &FormatDesc) -> Result<(), capture::CaptureError> {
        match self {
            Self::Mock(s) => s.start(selected),
            #[cfg(windows)]
            Self::Mf(s) => s.start(selected),
            #[cfg(target_os = "linux")]
            Self::V4l2(s) => s.start(selected),
            #[cfg(target_os = "macos")]
            Self::Avf(s) => s.start(selected),
        }
    }

    fn stop(&mut self) -> Result<(), capture::CaptureError> {
        match self {
            Self::Mock(s) => s.stop(),
            #[cfg(windows)]
            Self::Mf(s) => s.stop(),
            #[cfg(target_os = "linux")]
            Self::V4l2(s) => s.stop(),
            #[cfg(target_os = "macos")]
            Self::Avf(s) => s.stop(),
        }
    }

    fn receiver(&self) -> FrameReceiver {
        match self {
            Self::Mock(s) => s.receiver(),
            #[cfg(windows)]
            Self::Mf(s) => s.receiver(),
            #[cfg(target_os = "linux")]
            Self::V4l2(s) => s.receiver(),
            #[cfg(target_os = "macos")]
            Self::Avf(s) => s.receiver(),
        }
    }

    fn stream_status(&self) -> StreamStatus {
        match self {
            Self::Mock(s) => s.stream_status(),
            #[cfg(windows)]
            Self::Mf(s) => s.stream_status(),
            #[cfg(target_os = "linux")]
            Self::V4l2(s) => s.stream_status(),
            #[cfg(target_os = "macos")]
            Self::Avf(s) => s.stream_status(),
        }
    }
}

struct GpuState {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: wgpu::Adapter,
    backend: wgpu::Backend,
    config: wgpu::SurfaceConfiguration,
    pipeline: VideoPipeline,
    upscale: UpscaleEngine,
    interpolate: InterpolateEngine,
    deinterlace: DeinterlaceEngine,
    window: Arc<Window>,
    egui_renderer: egui_wgpu::Renderer,
    egui_state: EguiWinitState,
    egui_ctx: egui::Context,
    adapter_name: String,
    present_caps: PresentCaps,
}

/// Presence-only check interval. Full MF probe runs only when the id set changes.
/// Presence poll cadence. Actual MF enum runs on a worker — never blocks the UI thread.
const HOTPLUG_INTERVAL: Duration = Duration::from_secs(8);
/// Skip starting a new presence poll when a present is imminent (protects 1% low).
const HOTPLUG_PRESENT_SLACK: Duration = Duration::from_millis(10);

pub struct App {
    gpu: Option<GpuState>,
    devices: Vec<DeviceInfo>,
    source: Option<ActiveSource>,
    selection: UiSelection,
    current_format: FormatDesc,
    metrics: Arc<SharedMetrics>,
    overlay: OverlayState,
    fps_window_start: Instant,
    frames_in_window: u32,
    captures_in_window: u32,
    gen_frames_in_window: u32,
    last_frame_at: Instant,
    last_present_at: Option<Instant>,
    /// Absolute present phase (deadline pacing). Avoids WaitUntil slip → ~59 fps.
    next_present_at: Instant,
    last_redraw_at: Instant,
    last_channel_drops: u64,
    black_since: Option<Instant>,
    running: bool,
    audio: AudioEngine,
    audio_inputs: Vec<AudioDeviceInfo>,
    audio_outputs: Vec<AudioDeviceInfo>,
    audio_ui: AudioUiState,
    effects_ui: EffectsUiState,
    color_ui: ColorUiState,
    i18n: I18n,
    chrome: UiChrome,
    ui_prefs: UiPrefs,
    last_pointer_at: Instant,
    /// True while we forced the OS cursor hidden for idle preview.
    cursor_hidden: bool,
    toast_until: Option<Instant>,
    support_log: Vec<String>,
    process_sample_at: Instant,
    lag_test_started: Option<Instant>,
    lag_flash_until: Option<Instant>,
    last_hotplug_at: Instant,
    /// Non-blocking hot-plug result. MF enum never runs on the UI thread.
    hotplug_rx: Option<Receiver<HotplugResult>>,
    hotplug_inflight: bool,
    /// Throttle expensive metrics percentile / graph copy (UI only).
    last_metrics_ui_at: Instant,
    cached_one_percent_low: f32,
    cached_frame_times_ms: Vec<f32>,
    /// True while stop/start runs — keep showing the last GPU frame (audio stays up).
    capture_restarting: bool,
    prefs_dirty: bool,
    prefs_dirty_at: Instant,
    /// Per-device last FormatDesc for reconnect-after-unplug.
    device_settings: HashMap<String, FormatDesc>,
    discord: DiscordPresence,
    last_discord_push: Instant,
    logo_tex: Option<egui::TextureHandle>,
    /// Wakes `about_to_wait` / redraw when capture queues a frame.
    proxy: Option<EventLoopProxy<UserEvent>>,
    /// Brief shortcuts overlay after the wizard (or immediately if wizard already done).
    launch_tips_started: Option<Instant>,
    launch_tips_dismissed: bool,
    /// Startup update modal dismissed for this process (once per session).
    update_popup_dismissed: bool,
    /// Cached Windows HDR / DXGI tearing probe (refreshed ~1 Hz).
    os_display_caps: OsDisplayCaps,
    last_display_caps_at: Instant,
    replay: ReplayBuffer,
    health: HealthMonitor,
    export: FrameExport,
    update: UpdateChecker,
    /// Optional 1D LUT loaded from .cube (CPU preview / future GPU upload).
    cube_lut: Option<CubeLut1d>,
}

impl Default for App {
    fn default() -> Self {
        let mock_info = MockVideoSource::device_info();
        // Real devices first (already sorted: cards → others → virtual); Mock last.
        let mut devices = match capture::list_devices() {
            Ok(real) => {
                for d in &real {
                    tracing::info!(
                        name = %d.display_name,
                        id = %d.id.as_str(),
                        backend = d.backend,
                        formats = d.formats.len(),
                        score = capture::device_capture_score_for(&d.display_name, d.id.as_str()),
                        virtual = capture::is_virtual_camera(&d.display_name, d.id.as_str()),
                        "enumerated video device"
                    );
                }
                tracing::info!(count = real.len(), "platform devices enumerated");
                real
            }
            Err(e) => {
                tracing::warn!(error = %e, "device enumeration failed; mock only");
                Vec::new()
            }
        };
        devices.push(mock_info.clone());

        let ui_prefs_early = load_ui_prefs();
        let preferred = capture::pick_preferred_device_index(&devices).unwrap_or(0);
        let device_idx = if !ui_prefs_early.last_device_id.is_empty() {
            match devices
                .iter()
                .position(|d| d.id.as_str() == ui_prefs_early.last_device_id)
            {
                Some(idx) => {
                    let remembered = &devices[idx];
                    let preferred_dev = devices.get(preferred);
                    // If prefs stuck on a virtual cam while a real capture card exists, prefer the card.
                    let remembered_virtual = capture::is_virtual_camera(
                        &remembered.display_name,
                        remembered.id.as_str(),
                    );
                    let preferred_is_card = preferred_dev
                        .map(|d| {
                            d.backend != "mock"
                                && !capture::is_virtual_camera(&d.display_name, d.id.as_str())
                                && capture::device_capture_score_for(&d.display_name, d.id.as_str())
                                    > 0
                        })
                        .unwrap_or(false);
                    if remembered_virtual && preferred_is_card {
                        tracing::info!(
                            remembered = %remembered.display_name,
                            preferred = %preferred_dev.map(|d| d.display_name.as_str()).unwrap_or("?"),
                            "overriding virtual-camera prefs with capture card"
                        );
                        preferred
                    } else {
                        idx
                    }
                }
                None => preferred,
            }
        } else {
            preferred
        };

        let caps = &devices[device_idx].formats;
        let format = modes::select_format(CaptureMode::Auto, caps)
            .or_else(|| caps.first().cloned())
            .unwrap_or_else(|| mock_info.formats[0].clone());

        let audio_inputs = list_inputs().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "audio input enum failed");
            vec![AudioDeviceInfo::none_input()]
        });
        let audio_outputs = list_outputs().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "audio output enum failed");
            Vec::new()
        });
        let prefs = load_audio_prefs();
        let mut audio_ui = AudioUiState {
            input_id: prefs.input_id.clone(),
            output_id: prefs.output_id.clone(),
            volume: prefs.volume,
            muted: prefs.muted,
            delay_ms: prefs.delay_ms,
            buffer_ms: prefs.buffer_ms,
        };
        let ui_prefs = ui_prefs_early;
        let i18n = I18n::load_pref(&ui_prefs.ui_language);
        let mut chrome = UiChrome::default();
        let real_device_count = devices.iter().filter(|d| d.backend != "mock").count();
        let multi_sources = real_device_count >= 2;
        // First-run wizard, or one-shot source picker when several capture devices exist.
        let need_source_pick = multi_sources && !ui_prefs.source_confirmed;
        chrome.source_only = ui_prefs.wizard_done && need_source_pick;
        chrome.wizard_open = !ui_prefs.wizard_done || need_source_pick;
        chrome.wizard_step = if need_source_pick {
            0
        } else {
            ui_prefs.wizard_step.min(2)
        };
        chrome.metrics_pinned = ui_prefs.metrics_pinned;
        chrome.settings_pinned = ui_prefs.settings_pinned;

        // Prefer last resolution/rate for this device when still advertised.
        let format = if ui_prefs.last_width > 0
            && ui_prefs.last_height > 0
            && ui_prefs.last_rate_den > 0
        {
            let rate = Rational::new(ui_prefs.last_rate_num.max(1), ui_prefs.last_rate_den);
            modes::pick_format_at(
                &devices[device_idx].formats,
                ui_prefs.last_width,
                ui_prefs.last_height,
                rate,
            )
            .unwrap_or(format)
        } else {
            format
        };
        // Validate remembered ids still exist.
        if !audio_inputs.iter().any(|d| d.id == audio_ui.input_id) {
            audio_ui.input_id = NONE_INPUT_ID.into();
        }
        if !audio_outputs.iter().any(|d| d.id == audio_ui.output_id) {
            if let Some(d) = audio_outputs.first() {
                audio_ui.output_id = d.id.clone();
            }
        }
        // If still "none" and video looks like a capture card, suggest association.
        if audio_ui.input_id == NONE_INPUT_ID {
            let video = &devices[device_idx];
            if let Some(idx) =
                suggest_input_for_video(&video.display_name, video.id.as_str(), &audio_inputs)
            {
                audio_ui.input_id = audio_inputs[idx].id.clone();
            }
        }

        let color_ui = {
            let dev = &devices[device_idx];
            let (range, matrix) = ui_prefs.color_for_device(dev.id.as_str());
            let resolved = resolve_color_convert(
                ColorRangeMode::from_str_lossy(&range),
                ColorMatrixMode::from_str_lossy(&matrix),
                format.width,
                format.height,
                &dev.display_name,
                dev.id.as_str(),
            );
            ColorUiState {
                range_mode: range,
                matrix_mode: matrix,
                resolved_limited: resolved.limited,
                resolved_bt709: resolved.bt709,
            }
        };

        let mut replay = ReplayBuffer::default();
        replay.set_enabled(ui_prefs.replay_enabled);
        replay.set_seconds(ui_prefs.replay_seconds);
        let mut health = HealthMonitor::default();
        health.prefs = ui_prefs.health.clone();
        let mut export = FrameExport::default();
        export.prefs = ui_prefs.export.clone();
        export.refresh_probe();
        let cube_lut = if ui_prefs.cube_lut_path.trim().is_empty() {
            None
        } else {
            CubeLut1d::load_path(std::path::Path::new(&ui_prefs.cube_lut_path)).ok()
        };
        let mut profiles = ui_prefs.profiles.clone();
        profiles.ensure_builtins();
        // Keep prefs in sync with ensured builtins.
        let mut ui_prefs = ui_prefs;
        ui_prefs.profiles = profiles;

        Self {
            gpu: None,
            devices,
            source: None,
            selection: UiSelection {
                device_idx,
                width: format.width,
                height: format.height,
                rate: format
                    .frame_rates
                    .first()
                    .copied()
                    .unwrap_or(Rational::new(60, 1)),
                mode: CaptureMode::Auto,
            },
            current_format: format.clone(),
            metrics: SharedMetrics::new(),
            overlay: OverlayState {
                monitor: ui_prefs.monitor.clone(),
                ..OverlayState::default()
            },
            fps_window_start: Instant::now(),
            frames_in_window: 0,
            captures_in_window: 0,
            gen_frames_in_window: 0,
            last_frame_at: Instant::now(),
            last_present_at: None,
            next_present_at: Instant::now(),
            last_redraw_at: Instant::now(),
            last_channel_drops: 0,
            black_since: None,
            running: false,
            audio: AudioEngine::new(),
            audio_inputs,
            audio_outputs,
            audio_ui,
            effects_ui: EffectsUiState {
                mode: ui_prefs.upscale_mode.clone(),
                sharpness: ui_prefs.sharpness.clamp(0.0, 1.0),
                interp_mode: ui_prefs.interp_mode.clone(),
                interp_motion_blur: ui_prefs.interp_motion_blur.clamp(0.0, 1.0),
                deinterlace_mode: ui_prefs.deinterlace_mode.clone(),
                source_interlaced: format.interlaced,
                vsync_mode: normalize_vsync_mode(&ui_prefs.vsync_mode),
                hdr_output: ui_prefs.hdr_output,
                dlss_boost: ui_prefs.dlss_boost,
                dlssg_multiplier: ui_prefs.dlssg_multiplier.clone(),
                ..EffectsUiState::default()
            },
            color_ui,
            i18n,
            chrome,
            ui_prefs,
            last_pointer_at: Instant::now(),
            cursor_hidden: false,
            toast_until: None,
            support_log: Vec::new(),
            process_sample_at: Instant::now(),
            lag_test_started: None,
            lag_flash_until: None,
            last_hotplug_at: Instant::now(),
            hotplug_rx: None,
            hotplug_inflight: false,
            last_metrics_ui_at: Instant::now()
                .checked_sub(Duration::from_secs(1))
                .unwrap_or_else(Instant::now),
            cached_one_percent_low: 0.0,
            cached_frame_times_ms: Vec::new(),
            capture_restarting: false,
            prefs_dirty: false,
            prefs_dirty_at: Instant::now(),
            device_settings: HashMap::new(),
            discord: DiscordPresence::start(false, ""),
            last_discord_push: Instant::now()
                .checked_sub(Duration::from_secs(60))
                .unwrap_or_else(Instant::now),
            logo_tex: None,
            proxy: None,
            launch_tips_started: None,
            launch_tips_dismissed: false,
            update_popup_dismissed: false,
            os_display_caps: OsDisplayCaps::default(),
            last_display_caps_at: Instant::now()
                .checked_sub(Duration::from_secs(2))
                .unwrap_or_else(Instant::now),
            replay,
            health,
            export,
            update: UpdateChecker::default(),
            cube_lut,
        }
    }
}

/// Soft fade-in / hold / fade-out for the launch shortcuts card.
fn push_history(buf: &mut Vec<f32>, value: f32, cap: usize) {
    buf.push(value);
    if buf.len() > cap {
        let drain = buf.len() - cap;
        buf.drain(0..drain);
    }
}


fn tags_match(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let norm = |s: &str| s.trim().trim_start_matches('v').to_ascii_lowercase();
    norm(a) == norm(b)
}

fn launch_tips_alpha(elapsed: Duration) -> f32 {
    const FADE_IN: f32 = 0.35;
    const HOLD: f32 = 4.5;
    const FADE_OUT: f32 = 0.85;
    let t = elapsed.as_secs_f32();
    if t < FADE_IN {
        (t / FADE_IN).clamp(0.0, 1.0)
    } else if t < FADE_IN + HOLD {
        1.0
    } else if t < FADE_IN + HOLD + FADE_OUT {
        1.0 - ((t - FADE_IN - HOLD) / FADE_OUT).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

impl App {
    pub fn with_proxy(proxy: EventLoopProxy<UserEvent>) -> Self {
        let mut app = Self::default();
        app.proxy = Some(proxy);
        app
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::FrameReady => {
                // Present as soon as a capture frame lands (min gap avoids double-present).
                let period = Duration::from_secs_f64(1.0 / self.target_display_hz());
                let now = Instant::now();
                let since = self
                    .last_present_at
                    .map(|t| now.duration_since(t))
                    .unwrap_or(Duration::from_secs(1));
                let framegen = self.framegen_wants_2x();
                let min_gap = Duration::from_millis(1);
                let due = if framegen {
                    // Keep Mid/Curr on the 2× deadline grid.
                    now + Duration::from_millis(1) >= self.next_present_at
                } else {
                    since >= min_gap
                };
                if due {
                    if let Some(gpu) = self.gpu.as_ref() {
                        gpu.window.request_redraw();
                    }
                } else if !framegen {
                    // Too soon after last present — schedule the remainder of the period.
                    self.next_present_at = self
                        .last_present_at
                        .unwrap_or(now)
                        .checked_add(period)
                        .unwrap_or(now);
                }
            }
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }

        let mut attrs = Window::default_attributes()
            .with_title(self.i18n.t("app_title"))
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        // ICON_SMALL → title bar / Alt-Tab; without ICON_BIG the taskbar shows the
        // generic Windows window glyph (winit class registers hIcon=0).
        if let Some(icon) = crate::brand::window_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }
        #[cfg(windows)]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            if let Some(icon) = crate::brand::taskbar_icon() {
                attrs = attrs.with_taskbar_icon(Some(icon));
            }
        }
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("create window"),
        );
        // Belt-and-suspenders: re-apply after HWND exists (some shells cache class icons).
        #[cfg(windows)]
        {
            use winit::platform::windows::WindowExtWindows;
            if let Some(icon) = crate::brand::window_icon() {
                window.set_window_icon(Some(icon));
            }
            if let Some(icon) = crate::brand::taskbar_icon() {
                window.set_taskbar_icon(Some(icon));
            }
        }

        self.refresh_os_display_caps(true);
        let prefer_hdr = self.want_hdr_swapchain();
        let mut gpu =
            pollster::block_on(init_gpu(window.clone(), prefer_hdr)).expect("init gpu");
        // Respect OS DPI / egui style scaling.
        let scale = window.scale_factor() as f32;
        gpu.egui_ctx.set_pixels_per_point(scale);
        gpu.upscale
            .set_mode(UpscaleMode::from_str_lossy(&self.effects_ui.mode));
        gpu.upscale.set_sharpness(self.effects_ui.sharpness);
        gpu.upscale.dlss.set_boost(self.effects_ui.dlss_boost);
        gpu.upscale.dlss.set_fg_multiplier(DlssGMultiplier::from_str_lossy(
            &self.effects_ui.dlssg_multiplier,
        ));
        gpu.interpolate
            .set_mode(InterpMode::from_str_lossy(&self.effects_ui.interp_mode));
        gpu.interpolate
            .set_motion_blur(self.effects_ui.interp_motion_blur);
        gpu.deinterlace
            .set_mode(DeinterlaceMode::from_str_lossy(&self.effects_ui.deinterlace_mode));
        gpu.deinterlace
            .set_source_interlaced(self.current_format.interlaced);
        self.effects_ui.vendor = gpu.upscale.vendor.clone();
        self.effects_ui.backend = gpu.upscale.backend.clone();
        self.effects_ui.compute_ok = gpu.upscale.compute_ok;
        self.effects_ui.interp_compute_ok = gpu.interpolate.compute_ok;
        self.effects_ui.hdr_output = self.ui_prefs.hdr_output;
        self.gpu = Some(gpu);
        self.apply_color_convert();
        self.apply_present_mode_for_profile();
        self.sync_present_overlay();
        self.apply_grade_to_gpu();
        self.apply_window_level();
        if self.ui_prefs.update_check {
            self.update.request_check(false);
        }

        if let Err(e) = self.open_and_start(self.selection.device_idx, self.current_format.clone()) {
            tracing::error!(error = %e, "failed to start capture; falling back to mock");
            let busy = matches!(e, CaptureError::DeviceBusy(_));
            self.apply_capture_err(&e);
            self.selection.device_idx = 0;
            let fmt = self.devices[0].formats[0].clone();
            self.current_format = fmt.clone();
            self.selection.width = fmt.width;
            self.selection.height = fmt.height;
            self.selection.rate = fmt.frame_rates[0];
            let _ = self.open_and_start(0, fmt);
            // Keep the human busy message after mock fallback so Arthur knows why NZXT failed.
            if busy {
                self.overlay.status = self.i18n.t("err_device_busy");
                self.overlay.device_busy = true;
            } else {
                self.overlay.status = self.i18n.t("err_device_start");
            }
        }

        self.restart_audio();
        if self.devices.iter().all(|d| d.backend == "mock") {
            self.overlay.status = self.i18n.t("status_no_card");
        }
        self.restart_discord_presence();
        self.push_discord_activity(true);
        tracing::info!("CaptureView ready");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Compute before borrowing GPU (egui repaint must honor the same FPS cap).
        let redraw_due = Instant::now() >= self.next_present_at;

        // Cursor idle tracking must run before the exclusive GPU borrow.
        match &event {
            WindowEvent::CursorMoved { .. } => self.note_pointer_activity(),
            WindowEvent::CursorLeft { .. } => self.show_os_cursor(),
            _ => {}
        }

        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };

        let egui_response = gpu.egui_state.on_window_event(&gpu.window, &event);
        // Defer egui repaints through the display-FPS cap (Mesures used to
        // request_redraw unbound and fight WaitUntil pacing).
        if egui_response.repaint && redraw_due {
            gpu.window.request_redraw();
        }

        match event {
            WindowEvent::CloseRequested => {
                self.discord.shutdown();
                self.persist_audio_prefs();
                self.persist_ui_prefs();
                self.flush_prefs_now();
                self.audio.stop();
                self.stop_source();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    gpu.config.width = size.width;
                    gpu.config.height = size.height;
                    gpu.surface.configure(&gpu.device, &gpu.config);
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Capture-driven (no framegen): FrameReady wakes us when a frame is queued.
        // Framegen: absolute 2× deadline so Mid then Curr each get a present.
        let now = Instant::now();
        let framegen = self.framegen_wants_2x();
        let pending = self
            .source
            .as_ref()
            .map(|s| !s.receiver().is_empty())
            .unwrap_or(false);
        let since_present = self
            .last_present_at
            .map(|t| now.duration_since(t))
            .unwrap_or(Duration::from_secs(1));

        // Min gap ~1ms (not 0.92×period): old gate still aged frames on the bus → sawtooth.
        let min_gap = Duration::from_millis(1);
        let capture_due = !framegen && pending && since_present >= min_gap;
        let due = now >= self.next_present_at || capture_due;
        let until_due = self.next_present_at.saturating_duration_since(now);

        // Apply finished presence polls; only start a new one when not near a present.
        self.poll_hotplug_async(until_due > HOTPLUG_PRESENT_SLACK && !due);
        self.flush_prefs_if_due();
        self.push_discord_activity(false);

        if due {
            if let Some(gpu) = self.gpu.as_ref() {
                gpu.window.request_redraw();
            }
            // Park until RedrawRequested runs. A 1ms WaitUntil here busy-looped
            // under NoVsync (Arthur: FPS écran ~101, CPU ~82%, 1% low ~31).
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        // Without framegen, FrameReady is the primary wake; deadline is a stall fallback.
        let wake_at = self.next_present_at;
        event_loop.set_control_flow(ControlFlow::WaitUntil(wake_at));
    }
}

impl App {
    fn stop_source(&mut self) {
        if let Some(mut source) = self.source.take() {
            let _ = source.stop();
        }
        self.running = false;
    }

    fn note_pointer_activity(&mut self) {
        self.last_pointer_at = Instant::now();
        self.show_os_cursor();
    }

    fn hide_os_cursor(&mut self) {
        if self.cursor_hidden {
            return;
        }
        if let Some(gpu) = self.gpu.as_ref() {
            gpu.window.set_cursor_visible(false);
        }
        self.cursor_hidden = true;
    }

    fn show_os_cursor(&mut self) {
        if !self.cursor_hidden {
            return;
        }
        if let Some(gpu) = self.gpu.as_ref() {
            gpu.window.set_cursor_visible(true);
        }
        self.cursor_hidden = false;
    }

    /// True when optical-flow x2 should drive a 2× present cadence (LS-style).
    fn framegen_wants_2x(&self) -> bool {
        let mode = InterpMode::from_str_lossy(&self.effects_ui.interp_mode);
        if !mode.wants_effect() {
            return false;
        }
        match self.gpu.as_ref() {
            Some(g) => {
                g.interpolate.compute_ok
                    && g.interpolate.status().block != InterpBlockReason::AutoDisabled
            }
            None => true,
        }
    }

    /// Target on-screen present rate from prefs + capture / frame-gen.
    fn target_display_hz(&self) -> f64 {
        let capture_hz = self
            .current_format
            .frame_rates
            .first()
            .map(|r| r.as_f64())
            .unwrap_or(60.0)
            .clamp(24.0, 240.0);
        let limit = self.ui_prefs.display_fps_limit;
        // Optical framegen inserts one Mid per capture — needs ~2× swapchain presents
        // (Lossless Scaling does the same). A 60 Hz VSync/monitor still caps it.
        let framegen_hz = (capture_hz * 2.0).min(240.0);
        let fg_mult = match self.effects_ui.dlssg_multiplier.as_str() {
            "x2" => 2.0,
            "x3" => 3.0,
            "x4" => 4.0,
            "x5" => 5.0,
            "x6" => 6.0,
            _ => 1.0,
        };
        let dlssg_active = self.effects_ui.dlssg_ok && fg_mult > 1.0;
        let dlssg_hz = if dlssg_active {
            (capture_hz * fg_mult).min(240.0)
        } else {
            capture_hz
        };
        let auto = if self.framegen_wants_2x() {
            framegen_hz.max(dlssg_hz)
        } else if dlssg_active {
            dlssg_hz
        } else {
            capture_hz
        };
        let hz = match limit {
            0 => auto,
            9999 => 360.0,
            // Explicit 60 while framegen is on would silently drop Curr — bump.
            n if self.framegen_wants_2x() && (n as f64) + 0.5 < framegen_hz => framegen_hz,
            n if dlssg_active && (n as f64) + 0.5 < dlssg_hz => dlssg_hz,
            n => n as f64,
        };
        hz.clamp(24.0, 360.0)
    }

    /// Apply swapchain present mode from the VSync pref (auto/on/off).
    /// Auto: Auto/Game → NoVsync (avoids Fifo phase-beat with software pacing);
    /// Quality/Economy → VSync. Framegen x2 forces NoVsync under Auto (needs >monitor
    /// refresh when the panel is 60 Hz — same idea as Lossless Scaling).
    /// Explicit on/off overrides the profile.
    ///
    /// NoVsync path prefers Immediate (DXGI allow-tearing / VRR) then Mailbox —
    /// capture_wake + display FPS limit still pace presents.
    fn apply_present_mode_for_profile(&mut self) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let mode = normalize_vsync_mode(&self.ui_prefs.vsync_mode);
        let profile_low_latency = matches!(
            self.selection.mode,
            CaptureMode::Game | CaptureMode::Auto
        );
        let framegen = InterpMode::from_str_lossy(&self.effects_ui.interp_mode).wants_effect();
        let use_vsync = match mode.as_str() {
            "on" => true,
            "off" => false,
            // Auto: NoVsync for Game/Auto profiles, or whenever framegen needs 2× presents.
            _ => !(profile_low_latency || framegen),
        };
        let caps = gpu.surface.get_capabilities(&gpu.adapter);
        let want = pick_present_mode(use_vsync, &caps);
        let latency = if use_vsync { 2 } else { 1 };
        if gpu.config.present_mode == want && gpu.config.desired_maximum_frame_latency == latency {
            gpu.present_caps = PresentCaps::from_surface(&caps, want, gpu.config.format);
            return;
        }
        gpu.config.present_mode = want;
        gpu.config.desired_maximum_frame_latency = latency;
        gpu.surface.configure(&gpu.device, &gpu.config);
        gpu.present_caps = PresentCaps::from_surface(&caps, want, gpu.config.format);
        tracing::info!(
            ?want,
            latency,
            vsync = %mode,
            tearing = gpu.present_caps.tearing_supported,
            "present mode updated"
        );
    }

    fn want_hdr_swapchain(&self) -> bool {
        self.ui_prefs.hdr_output && self.os_display_caps.hdr_enabled
    }

    fn refresh_os_display_caps(&mut self, force: bool) {
        if !force && self.last_display_caps_at.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.os_display_caps = OsDisplayCaps::probe();
        self.last_display_caps_at = Instant::now();
        // Merge DXGI factory tearing with surface Immediate when we already have GPU.
        if let Some(gpu) = self.gpu.as_ref() {
            if gpu.present_caps.tearing_supported {
                self.os_display_caps.tearing_supported = true;
            }
        }
    }

    fn sync_present_overlay(&mut self) {
        self.refresh_os_display_caps(false);
        self.overlay.hdr_output = self.ui_prefs.hdr_output;
        self.overlay.hdr_supported = self.os_display_caps.hdr_supported;
        self.overlay.hdr_enabled = self.os_display_caps.hdr_enabled;
        self.effects_ui.hdr_output = self.ui_prefs.hdr_output;
        if let Some(gpu) = self.gpu.as_ref() {
            self.overlay.hdr_active = gpu.present_caps.hdr_active;
            self.overlay.vrr_capable =
                gpu.present_caps.tearing_supported || self.os_display_caps.tearing_supported;
            self.overlay.present_mode = gpu.present_caps.present_mode.clone();
            self.overlay.surface_format = gpu.present_caps.surface_format.clone();
        } else {
            self.overlay.hdr_active = false;
            self.overlay.vrr_capable = self.os_display_caps.tearing_supported;
        }
    }

    /// Reconfigure surface format for HDR on/off without recreating the device.
    fn apply_hdr_output(&mut self) {
        let prefer = self.want_hdr_swapchain();
        let os_hdr = self.os_display_caps.hdr_enabled;
        let mode = UpscaleMode::from_str_lossy(&self.effects_ui.mode);
        let sharpness = self.effects_ui.sharpness;
        let dlss_boost = self.effects_ui.dlss_boost;
        let dlssg = DlssGMultiplier::from_str_lossy(&self.effects_ui.dlssg_multiplier);
        let interp = InterpMode::from_str_lossy(&self.effects_ui.interp_mode);
        let blur = self.effects_ui.interp_motion_blur;

        let mut rebuilt_meta: Option<(String, String, bool, bool)> = None;
        let rebuilt = {
            let Some(gpu) = self.gpu.as_mut() else {
                return;
            };
            let caps = gpu.surface.get_capabilities(&gpu.adapter);
            let format = pick_surface_format(&caps, prefer);
            if format == gpu.config.format {
                gpu.present_caps =
                    PresentCaps::from_surface(&caps, gpu.config.present_mode, format);
                false
            } else {
                tracing::info!(
                    ?format,
                    prefer_hdr = prefer,
                    os_hdr,
                    "reconfiguring surface for HDR/SDR"
                );
                gpu.config.format = format;
                gpu.surface.configure(&gpu.device, &gpu.config);
                gpu.pipeline = VideoPipeline::new(&gpu.device, format);
                gpu.upscale = UpscaleEngine::new(
                    &gpu.device,
                    &gpu.queue,
                    format,
                    &gpu.adapter,
                    gpu.backend,
                );
                gpu.interpolate = InterpolateEngine::new(&gpu.device, format, &gpu.adapter);
                gpu.egui_renderer =
                    egui_wgpu::Renderer::new(&gpu.device, format, None, 1, false);
                gpu.upscale.set_mode(mode);
                gpu.upscale.set_sharpness(sharpness);
                gpu.upscale.dlss.set_boost(dlss_boost);
                gpu.upscale.dlss.set_fg_multiplier(dlssg);
                gpu.interpolate.set_mode(interp);
                gpu.interpolate.set_motion_blur(blur);
                rebuilt_meta = Some((
                    gpu.upscale.vendor.clone(),
                    gpu.upscale.backend.clone(),
                    gpu.upscale.compute_ok,
                    gpu.interpolate.compute_ok,
                ));
                gpu.present_caps =
                    PresentCaps::from_surface(&caps, gpu.config.present_mode, format);
                true
            }
        };

        if let Some((vendor, backend, compute_ok, interp_ok)) = rebuilt_meta {
            self.effects_ui.vendor = vendor;
            self.effects_ui.backend = backend;
            self.effects_ui.compute_ok = compute_ok;
            self.effects_ui.interp_compute_ok = interp_ok;
        }
        if rebuilt {
            self.logo_tex = None; // egui renderer rebuilt — drop stale texture handle
            self.apply_color_convert();
            self.apply_present_mode_for_profile();
        }
        self.sync_present_overlay();
    }

    fn capture_err_status(&self, e: &CaptureError) -> String {
        match e {
            CaptureError::DeviceBusy(_) => self.i18n.t("err_device_busy"),
            _ => self.i18n.t("err_device_start"),
        }
    }

    fn apply_capture_err(&mut self, e: &CaptureError) {
        self.overlay.status = self.capture_err_status(e);
        self.overlay.device_busy = matches!(e, CaptureError::DeviceBusy(_));
        self.push_support(format!("capture error: {e}"));
    }

    fn open_and_start(
        &mut self,
        device_idx: usize,
        format: FormatDesc,
    ) -> Result<(), CaptureError> {
        // Hold last presented frame through stop/join/start so the picture does not
        // flash "Pas de signal" while audio keeps playing.
        self.capture_restarting = true;
        self.last_frame_at = Instant::now();
        self.stop_source();
        let info = self.devices[device_idx].clone();
        let result = (|| -> Result<ActiveSource, CaptureError> {
            let mut source = match info.backend {
                "mock" => ActiveSource::Mock(MockVideoSource::new()),
                #[cfg(windows)]
                "mf" => ActiveSource::Mf(capture::MfVideoSource::open(info.clone())?),
                #[cfg(target_os = "linux")]
                "v4l2" => ActiveSource::V4l2(capture::V4l2VideoSource::open(info.clone())?),
                #[cfg(target_os = "macos")]
                "avf" => ActiveSource::Avf(capture::AvfVideoSource::open(info.clone())?),
                other => {
                    return Err(CaptureError::Unsupported(format!(
                        "backend '{other}' not available on this OS"
                    )));
                }
            };
            // Wake UI on each queued frame so presents follow capture (kills latency sawtooth).
            if let Some(proxy) = self.proxy.clone() {
                source.set_frame_wake(Arc::new(move || {
                    let _ = proxy.send_event(UserEvent::FrameReady);
                }));
            }
            source.start(&format)?;
            Ok(source)
        })();
        let source = match result {
            Ok(s) => s,
            Err(e) => {
                self.capture_restarting = false;
                return Err(e);
            }
        };
        self.overlay.device_busy = false;
        if info.backend != "mock" {
            self.device_settings
                .insert(info.id.as_str().to_string(), format.clone());
            self.ui_prefs.last_device_id = info.id.as_str().to_string();
            self.ui_prefs.last_width = format.width;
            self.ui_prefs.last_height = format.height;
            if let Some(r) = format.frame_rates.first() {
                self.ui_prefs.last_rate_num = r.num;
                self.ui_prefs.last_rate_den = r.den;
            }
            self.persist_ui_prefs();
        }
        self.source = Some(source);
        self.current_format = format;
        self.effects_ui.source_interlaced = self.current_format.interlaced;
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.deinterlace
                .set_source_interlaced(self.current_format.interlaced);
        }
        // Reload per-device color prefs when switching sources.
        let id = self.devices[device_idx].id.as_str();
        let (range, matrix) = self.ui_prefs.color_for_device(id);
        self.color_ui.range_mode = range;
        self.color_ui.matrix_mode = matrix;
        self.apply_color_convert();
        self.running = true;
        self.last_frame_at = Instant::now();
        self.black_since = None;
        self.capture_restarting = false;
        Ok(())
    }

    fn apply_color_convert(&mut self) {
        let device = &self.devices[self.selection.device_idx];
        let resolved = resolve_color_convert(
            ColorRangeMode::from_str_lossy(&self.color_ui.range_mode),
            ColorMatrixMode::from_str_lossy(&self.color_ui.matrix_mode),
            self.current_format.width,
            self.current_format.height,
            &device.display_name,
            device.id.as_str(),
        );
        self.color_ui.resolved_limited = resolved.limited;
        self.color_ui.resolved_bt709 = resolved.bt709;
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.pipeline.set_color_convert(resolved);
        }
        tracing::info!(
            device = %device.display_name,
            range = %self.color_ui.range_mode,
            matrix = %self.color_ui.matrix_mode,
            limited = resolved.limited,
            bt709 = resolved.bt709,
            "color convert params"
        );
    }

    /// Non-blocking hot-plug: MF presence (+ optional full probe) on a worker.
    /// Blocking `list_device_presence()` / `run_on_mta` on the UI thread caused
    /// periodic ~30 fps 1% lows (Arthur: lag spikes every few seconds).
    fn poll_hotplug_async(&mut self, may_start: bool) {
        if let Some(rx) = self.hotplug_rx.as_ref() {
            match rx.try_recv() {
                Ok(result) => {
                    self.hotplug_rx = None;
                    self.hotplug_inflight = false;
                    if let HotplugResult::Changed {
                        before_count,
                        devices,
                    } = result
                    {
                        self.apply_hotplug_devices(before_count, devices);
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.hotplug_rx = None;
                    self.hotplug_inflight = false;
                }
            }
        }

        if !may_start || self.hotplug_inflight {
            return;
        }
        if self.last_hotplug_at.elapsed() < HOTPLUG_INTERVAL {
            return;
        }
        self.last_hotplug_at = Instant::now();
        let before_ids: std::collections::HashSet<String> = self
            .devices
            .iter()
            .filter(|d| d.backend != "mock")
            .map(|d| d.id.as_str().to_string())
            .collect();
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.hotplug_rx = Some(rx);
        self.hotplug_inflight = true;
        let _ = thread::Builder::new().name("hotplug-mf".into()).spawn(move || {
            let t0 = Instant::now();
            let presence = capture::list_device_presence().unwrap_or_default();
            let after_ids: std::collections::HashSet<String> =
                presence.iter().map(|(id, _)| id.clone()).collect();
            if after_ids == before_ids {
                tracing::debug!(
                    ms = t0.elapsed().as_secs_f32() * 1000.0,
                    devices = after_ids.len(),
                    "hotplug presence OK (worker, UI not blocked)"
                );
                let _ = tx.send(HotplugResult::Unchanged);
                return;
            }
            tracing::info!(
                before = before_ids.len(),
                after = after_ids.len(),
                "device set changed — full MF re-probe (worker)"
            );
            let real = capture::list_devices().unwrap_or_default();
            tracing::info!(
                ms = t0.elapsed().as_secs_f32() * 1000.0,
                devices = real.len(),
                "hotplug full probe done (worker)"
            );
            let _ = tx.send(HotplugResult::Changed {
                before_count: before_ids.len(),
                devices: real,
            });
        });
    }

    fn apply_hotplug_devices(&mut self, before_count: usize, real: Vec<DeviceInfo>) {
        let ids_before: std::collections::HashSet<String> = self
            .devices
            .iter()
            .filter(|d| d.backend != "mock")
            .map(|d| d.id.as_str().to_string())
            .collect();
        let mut next = real;
        next.push(MockVideoSource::device_info());

        let current_id = self
            .devices
            .get(self.selection.device_idx)
            .map(|d| d.id.as_str().to_string());
        let was_real = self
            .devices
            .get(self.selection.device_idx)
            .map(|d| d.backend != "mock")
            .unwrap_or(false);

        let ids_after: std::collections::HashSet<String> =
            next.iter().map(|d| d.id.as_str().to_string()).collect();

        tracing::info!(
            before = before_count,
            after = ids_after.len().saturating_sub(1),
            "applying hot-plug device list"
        );

        self.devices = next;

        // Active real device disappeared → fall back to mock.
        if was_real {
            if let Some(id) = current_id.as_ref() {
                if !ids_after.contains(id) {
                    tracing::warn!(%id, "capture device removed");
                    let fallback = self
                        .devices
                        .iter()
                        .position(|d| d.backend == "mock")
                        .or_else(|| capture::pick_preferred_device_index(&self.devices))
                        .unwrap_or(0);
                    self.selection.device_idx = fallback;
                    if let Some(fmt) = self.devices.get(fallback).and_then(|d| d.formats.first()) {
                        let _ = self.open_and_start(fallback, fmt.clone());
                    }
                    self.show_toast(self.i18n.t("msg_device_gone"));
                    return;
                }
                // Refresh index if list order changed.
                if let Some(idx) = self.devices.iter().position(|d| d.id.as_str() == id) {
                    self.selection.device_idx = idx;
                }
            }
        }

        // Preferred device reappeared while on mock → reconnect last settings.
        let preferred = self.ui_prefs.last_device_id.clone();
        if preferred.is_empty() {
            return;
        }
        let on_mock = self
            .devices
            .get(self.selection.device_idx)
            .map(|d| d.backend == "mock")
            .unwrap_or(true);
        if on_mock && !ids_before.contains(&preferred) && ids_after.contains(&preferred) {
            if let Some(idx) = self.devices.iter().position(|d| d.id.as_str() == preferred) {
                let fmt = self
                    .device_settings
                    .get(&preferred)
                    .cloned()
                    .or_else(|| {
                        let caps = &self.devices[idx].formats;
                        if self.ui_prefs.last_width > 0 {
                            modes::pick_format_at(
                                caps,
                                self.ui_prefs.last_width,
                                self.ui_prefs.last_height,
                                Rational::new(
                                    self.ui_prefs.last_rate_num.max(1),
                                    self.ui_prefs.last_rate_den.max(1),
                                ),
                            )
                        } else {
                            None
                        }
                    })
                    .or_else(|| modes::select_format(CaptureMode::Auto, &self.devices[idx].formats))
                    .or_else(|| self.devices[idx].formats.first().cloned());
                if let Some(fmt) = fmt {
                    self.selection.device_idx = idx;
                    self.selection.width = fmt.width;
                    self.selection.height = fmt.height;
                    self.selection.rate = fmt
                        .frame_rates
                        .first()
                        .copied()
                        .unwrap_or(self.selection.rate);
                    if self.open_and_start(idx, fmt).is_ok() {
                        self.show_toast(self.i18n.t("msg_device_back"));
                    }
                }
            }
        }
    }

    fn restart_with_fallback(
        &mut self,
        new_format: FormatDesc,
        on_fail_msg: &str,
    ) -> bool {
        let previous = self.current_format.clone();
        let device_idx = self.selection.device_idx;
        match self.open_and_start(device_idx, new_format.clone()) {
            Ok(()) => {
                self.current_format = new_format;
                self.selection.width = self.current_format.width;
                self.selection.height = self.current_format.height;
                self.selection.rate = self
                    .current_format
                    .frame_rates
                    .first()
                    .copied()
                    .unwrap_or(self.selection.rate);
                self.overlay.status.clear();
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "format change failed; restoring previous");
                let _ = self.open_and_start(device_idx, previous);
                self.overlay.status = if matches!(e, CaptureError::DeviceBusy(_)) {
                    self.i18n.t("err_device_busy")
                } else {
                    on_fail_msg.to_string()
                };
                self.overlay.device_busy = matches!(e, CaptureError::DeviceBusy(_));
                false
            }
        }
    }

    fn audio_config_from_ui(&self) -> AudioEngineConfig {
        AudioEngineConfig {
            input_id: self.audio_ui.input_id.clone(),
            output_id: self.audio_ui.output_id.clone(),
            volume: self.audio_ui.volume,
            muted: self.audio_ui.muted,
            delay_ms: self.audio_ui.delay_ms,
            buffer_ms: self.audio_ui.buffer_ms,
        }
    }

    fn restart_audio(&mut self) {
        match self.audio.start(self.audio_config_from_ui()) {
            Ok(()) => {
                let notice = self.audio.status().notice.clone();
                self.overlay.audio_notice = if notice.is_empty() {
                    String::new()
                } else {
                    self.i18n.t("err_audio_out_fallback")
                };
            }
            Err(e) => {
                tracing::warn!(error = %e, "audio start failed");
                self.push_support(format!("audio start failed: {e}"));
                self.overlay.audio_notice = self.i18n.t("err_audio");
                self.audio_ui.input_id = NONE_INPUT_ID.into();
            }
        }
        self.persist_audio_prefs();
    }

    fn persist_audio_prefs(&self) {
        save_audio_prefs(&AudioPrefs {
            input_id: self.audio_ui.input_id.clone(),
            output_id: self.audio_ui.output_id.clone(),
            volume: self.audio_ui.volume,
            muted: self.audio_ui.muted,
            delay_ms: self.audio_ui.delay_ms,
            buffer_ms: self.audio_ui.buffer_ms,
        });
    }

    fn suggest_audio_for_current_video(&mut self) {
        let video = &self.devices[self.selection.device_idx];
        if let Some(idx) =
            suggest_input_for_video(&video.display_name, video.id.as_str(), &self.audio_inputs)
        {
            self.audio_ui.input_id = self.audio_inputs[idx].id.clone();
        } else if video.backend == "mock" {
            self.audio_ui.input_id = NONE_INPUT_ID.into();
        }
    }

    fn apply_ui_actions(&mut self, actions: ui::UiActions) {
        if let Some(idx) = actions.device_changed {
            self.selection.device_idx = idx;
            let caps = &self.devices[idx].formats;
            if caps.is_empty() && self.devices[idx].backend != "mock" {
                self.overlay.status = self.i18n.t("err_device_no_modes");
                self.push_support(format!(
                    "device has no probed modes: {} ({})",
                    self.devices[idx].display_name,
                    self.devices[idx].id.as_str()
                ));
                // Still switch selection so the user can see the empty mode lists.
                self.show_toast(self.i18n.t("err_device_no_modes"));
            } else {
                let format = modes::select_format(self.selection.mode, caps)
                    .or_else(|| caps.first().cloned())
                    .unwrap_or_else(|| self.current_format.clone());
                self.selection.width = format.width;
                self.selection.height = format.height;
                self.selection.rate = format
                    .frame_rates
                    .first()
                    .copied()
                    .unwrap_or(Rational::new(60, 1));
                let msg = self.i18n.t("err_device_start");
                let _ = self.restart_with_fallback(format, &msg);
                self.suggest_audio_for_current_video();
                self.restart_audio();
            }
        }

        if let Some((w, h)) = actions.resolution_changed {
            let caps = &self.devices[self.selection.device_idx].formats;
            let rates = modes::rates_for_resolution(caps, w, h);
            let rate = rates.first().copied().unwrap_or(self.selection.rate);
            if let Some(fmt) = modes::pick_format_at(caps, w, h, rate) {
                let msg = self.i18n.t("err_resolution");
                if self.restart_with_fallback(fmt, &msg) {
                    // Match client area to capture so windowed 1080 can stay 1:1 crisp.
                    self.request_window_for_capture();
                }
            }
            // Keep dropdown in sync with the live MF format (never a stale preset).
            self.selection.width = self.current_format.width;
            self.selection.height = self.current_format.height;
            self.selection.rate = self
                .current_format
                .frame_rates
                .first()
                .copied()
                .unwrap_or(self.selection.rate);
        }

        if let Some(rate) = actions.rate_changed {
            let caps = &self.devices[self.selection.device_idx].formats;
            if let Some(fmt) =
                modes::pick_format_at(caps, self.selection.width, self.selection.height, rate)
            {
                let msg = self.i18n.t("err_rate");
                if !self.restart_with_fallback(fmt, &msg) {
                    self.selection.rate = self
                        .current_format
                        .frame_rates
                        .first()
                        .copied()
                        .unwrap_or(self.selection.rate);
                }
            } else {
                self.selection.rate = self
                    .current_format
                    .frame_rates
                    .first()
                    .copied()
                    .unwrap_or(self.selection.rate);
            }
        }

        if let Some(mode) = actions.mode_changed {
            let previous_mode = self.selection.mode;
            self.selection.mode = mode;
            self.apply_present_mode_for_profile();
            let caps = &self.devices[self.selection.device_idx].formats;
            if let Some(fmt) = modes::select_format(mode, caps) {
                let msg = self.i18n.t("err_mode");
                if !self.restart_with_fallback(fmt, &msg) {
                    self.selection.mode = previous_mode;
                    self.apply_present_mode_for_profile();
                }
            }
        }

        let mut audio_restart = false;
        if let Some(id) = actions.audio_input_changed {
            self.audio_ui.input_id = id;
            audio_restart = true;
        }
        if let Some(id) = actions.audio_output_changed {
            self.audio_ui.output_id = id;
            audio_restart = true;
        }
        if let Some(v) = actions.volume_changed {
            self.audio_ui.volume = v;
            self.audio.set_volume(v);
            self.persist_audio_prefs();
        }
        if let Some(m) = actions.mute_toggled {
            self.audio_ui.muted = m;
            self.audio.set_muted(m);
            self.persist_audio_prefs();
        }
        if let Some(d) = actions.delay_changed {
            self.audio_ui.delay_ms = d;
            self.audio.set_delay_ms(d);
            self.persist_audio_prefs();
        }
        if let Some(b) = actions.buffer_changed {
            self.audio_ui.buffer_ms = b;
            self.audio.set_buffer_ms(b);
            self.persist_audio_prefs();
        }
        if audio_restart {
            self.restart_audio();
        }

        if let Some(tab) = actions.open_settings {
            self.chrome.settings_open = true;
            self.chrome.settings_tab = tab;
            // F1 / settings: show toolbar for quick tabs; F2 must not open this path.
            self.chrome.toolbar_visible = true;
            self.note_pointer_activity();
        }
        if actions.close_settings {
            self.chrome.settings_open = false;
        }
        if actions.toggle_settings_pin {
            self.chrome.settings_pinned = !self.chrome.settings_pinned;
            self.ui_prefs.settings_pinned = self.chrome.settings_pinned;
            self.persist_ui_prefs();
        }
        if actions.toggle_metrics_pin {
            self.chrome.metrics_pinned = !self.chrome.metrics_pinned;
            self.ui_prefs.metrics_pinned = self.chrome.metrics_pinned;
            self.persist_ui_prefs();
        }
        if actions.toggle_advanced {
            self.chrome.advanced_open = !self.chrome.advanced_open;
        }
        if actions.toggle_fullscreen {
            self.toggle_fullscreen();
        }
        if actions.show_toolbar {
            self.chrome.toolbar_visible = true;
            self.note_pointer_activity();
        }
        if actions.reset_auto {
            self.reset_to_auto();
            self.effects_ui.mode = "off".into();
            self.effects_ui.sharpness = 0.4;
            self.effects_ui.interp_mode = "off".into();
            self.effects_ui.interp_motion_blur = 0.0;
            self.effects_ui.deinterlace_mode = "auto".into();
            self.effects_ui.vsync_mode = "auto".into();
            self.ui_prefs.vsync_mode = "auto".into();
            self.overlay.vsync_mode = "auto".into();
            self.color_ui.range_mode = "auto".into();
            self.color_ui.matrix_mode = "auto".into();
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.upscale.set_mode(UpscaleMode::Off);
                gpu.upscale.set_sharpness(0.4);
                gpu.interpolate.set_mode(InterpMode::Off);
                gpu.interpolate.set_motion_blur(0.0);
                gpu.deinterlace.set_mode(DeinterlaceMode::Auto);
            }
            self.apply_present_mode_for_profile();
            let id = self.devices[self.selection.device_idx].id.as_str().to_string();
            self.ui_prefs.set_color_for_device(&id, "auto", "auto");
            self.apply_color_convert();
            self.persist_ui_prefs();
        }
        if actions.retry {
            let fmt = self.current_format.clone();
            let idx = self.selection.device_idx;
            if let Err(e) = self.open_and_start(idx, fmt) {
                self.apply_capture_err(&e);
            } else {
                self.overlay.status.clear();
            }
            self.restart_audio();
        }
        if actions.copy_support {
            self.copy_support_report();
        }
        if actions.wizard_pick_auto {
            if let Some(idx) = capture::pick_preferred_device_index(&self.devices) {
                // Reuse the same path as a manual device change.
                let caps = &self.devices[idx].formats;
                if let Some(format) = modes::select_format(self.selection.mode, caps)
                    .or_else(|| caps.first().cloned())
                {
                    self.selection.device_idx = idx;
                    self.selection.width = format.width;
                    self.selection.height = format.height;
                    self.selection.rate = format
                        .frame_rates
                        .first()
                        .copied()
                        .unwrap_or(Rational::new(60, 1));
                    let msg = self.i18n.t("err_device_start");
                    let _ = self.restart_with_fallback(format, &msg);
                    self.suggest_audio_for_current_video();
                    self.restart_audio();
                } else {
                    self.selection.device_idx = idx;
                }
            }
        }
        if actions.wizard_next {
            self.chrome.wizard_step = (self.chrome.wizard_step + 1).min(2);
            self.persist_ui_prefs();
        }
        if actions.wizard_back {
            self.chrome.wizard_step = self.chrome.wizard_step.saturating_sub(1);
            self.persist_ui_prefs();
        }
        if actions.source_confirmed {
            self.ui_prefs.source_confirmed = true;
            self.persist_ui_prefs();
        }
        if actions.wizard_skip || actions.wizard_finish {
            self.chrome.wizard_open = false;
            self.chrome.source_only = false;
            self.ui_prefs.wizard_done = true;
            self.ui_prefs.source_confirmed = true;
            self.persist_ui_prefs();
            // Start tips right after the welcome / source picker closes.
            if self.launch_tips_started.is_none() && !self.launch_tips_dismissed {
                self.launch_tips_started = Some(Instant::now());
            }
        }
        if actions.dismiss_launch_tips {
            self.launch_tips_dismissed = true;
            self.chrome.launch_tips_alpha = 0.0;
        }
        if actions.cycle_metrics {
            // F2: metrics overlay only — never open Image/Son/Effets toolbar.
            self.overlay.metrics_mode = self.overlay.metrics_mode.next();
        }
        if actions.lag_test_start {
            self.lag_test_started = Some(Instant::now());
            self.lag_flash_until = Some(Instant::now() + Duration::from_millis(80));
            self.overlay.lag_test_running = true;
            self.overlay.lag_test_ms = 0.0;
        }
        if actions.lag_test_stop {
            if let Some(start) = self.lag_test_started.take() {
                self.overlay.lag_test_ms = start.elapsed().as_secs_f32() * 1000.0;
            }
            self.overlay.lag_test_running = false;
            self.lag_flash_until = None;
        }
        if let Some(mode) = actions.upscale_mode {
            self.effects_ui.mode = mode.to_string();
            self.ui_prefs.upscale_mode = mode.to_string();
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.upscale.set_mode(UpscaleMode::from_str_lossy(mode));
            }
            self.persist_ui_prefs();
        }
        if let Some(s) = actions.sharpness_changed {
            self.effects_ui.sharpness = s;
            self.ui_prefs.sharpness = s;
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.upscale.set_sharpness(s);
            }
            self.persist_ui_prefs();
        }
        if let Some(boost) = actions.dlss_boost_changed {
            self.effects_ui.dlss_boost = boost;
            self.ui_prefs.dlss_boost = boost;
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.upscale.dlss.set_boost(boost);
            }
            self.persist_ui_prefs();
        }
        if let Some(m) = actions.dlssg_multiplier {
            self.effects_ui.dlssg_multiplier = m.to_string();
            self.ui_prefs.dlssg_multiplier = m.to_string();
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.upscale
                    .dlss
                    .set_fg_multiplier(DlssGMultiplier::from_str_lossy(m));
            }
            self.persist_ui_prefs();
        }
        if let Some(mode) = actions.interp_mode {
            self.effects_ui.interp_mode = mode.to_string();
            self.ui_prefs.interp_mode = mode.to_string();
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.interpolate
                    .set_mode(InterpMode::from_str_lossy(mode));
            }
            // Framegen needs 2× presents + NoVsync (Auto) — resync phase immediately.
            self.next_present_at = Instant::now();
            self.apply_present_mode_for_profile();
            if InterpMode::from_str_lossy(mode).wants_effect() {
                self.show_toast(self.i18n.t("msg_interp_needs_120"));
            }
            self.persist_ui_prefs();
        }
        if let Some(blur) = actions.interp_motion_blur_changed {
            let blur = blur.clamp(0.0, 1.0);
            self.effects_ui.interp_motion_blur = blur;
            self.ui_prefs.interp_motion_blur = blur;
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.interpolate.set_motion_blur(blur);
            }
            self.persist_ui_prefs();
        }
        if let Some(limit) = actions.display_fps_limit {
            self.ui_prefs.display_fps_limit = limit;
            self.overlay.display_fps_limit = limit;
            self.persist_ui_prefs();
        }
        if let Some(mode) = actions.vsync_mode {
            let mode = normalize_vsync_mode(mode);
            self.ui_prefs.vsync_mode = mode.clone();
            self.effects_ui.vsync_mode = mode.clone();
            self.overlay.vsync_mode = mode;
            self.apply_present_mode_for_profile();
            self.sync_present_overlay();
            self.persist_ui_prefs();
        }
        if let Some(on) = actions.hdr_output_changed {
            self.ui_prefs.hdr_output = on;
            self.effects_ui.hdr_output = on;
            self.refresh_os_display_caps(true);
            self.apply_hdr_output();
            self.persist_ui_prefs();
        }
        if let Some(on) = actions.discord_presence_changed {
            self.ui_prefs.discord_presence = on;
            self.overlay.discord_presence = on;
            self.restart_discord_presence();
            self.push_discord_activity(true);
            self.persist_ui_prefs();
        }
        if let Some(id) = actions.discord_app_id_changed {
            self.ui_prefs.discord_app_id = id.trim().to_string();
            self.overlay.discord_app_id = self.ui_prefs.discord_app_id.clone();
            self.restart_discord_presence();
            self.push_discord_activity(true);
            self.persist_ui_prefs();
        }
        if let Some(level) = actions.log_level_changed {
            let level = crate::logging::normalize_level(level).to_string();
            self.ui_prefs.log_level = level.clone();
            self.overlay.log_level = level.clone();
            let _ = crate::logging::set_level(&level);
            self.persist_ui_prefs();
        }
        if let Some(mon) = actions.monitor_prefs_changed {
            self.ui_prefs.monitor = mon.clone();
            self.overlay.monitor = mon;
            self.persist_ui_prefs();
        }
        if let Some(lang) = actions.language_changed {
            let lang = Lang::from_pref(lang);
            self.ui_prefs.ui_language = lang.as_str().into();
            self.i18n = I18n::with_lang(lang);
            if let Some(gpu) = self.gpu.as_ref() {
                gpu.window.set_title(&self.i18n.t("app_title"));
            }
            self.push_discord_activity(true);
            self.persist_ui_prefs();
        }
        if let Some(mode) = actions.deinterlace_mode {
            self.effects_ui.deinterlace_mode = mode.to_string();
            self.ui_prefs.deinterlace_mode = mode.to_string();
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.deinterlace
                    .set_mode(DeinterlaceMode::from_str_lossy(mode));
            }
            self.persist_ui_prefs();
        }
        if let Some(range) = actions.color_range_changed {
            self.color_ui.range_mode = range.to_string();
            let id = self.devices[self.selection.device_idx].id.as_str().to_string();
            let matrix = self.color_ui.matrix_mode.clone();
            self.ui_prefs
                .set_color_for_device(&id, &self.color_ui.range_mode, &matrix);
            self.apply_color_convert();
            self.persist_ui_prefs();
        }
        if actions.color_range_toggle {
            let next = ColorRangeMode::from_str_lossy(&self.color_ui.range_mode)
                .toggle_concrete(self.color_ui.resolved_limited);
            self.color_ui.range_mode = next.as_str().into();
            let id = self.devices[self.selection.device_idx].id.as_str().to_string();
            let matrix = self.color_ui.matrix_mode.clone();
            self.ui_prefs
                .set_color_for_device(&id, &self.color_ui.range_mode, &matrix);
            self.apply_color_convert();
            self.persist_ui_prefs();
            self.show_toast(self.i18n.t("msg_color_toggled"));
        }
        if let Some(matrix) = actions.color_matrix_changed {
            self.color_ui.matrix_mode = matrix.to_string();
            let id = self.devices[self.selection.device_idx].id.as_str().to_string();
            let range = self.color_ui.range_mode.clone();
            self.ui_prefs
                .set_color_for_device(&id, &range, &self.color_ui.matrix_mode);
            self.apply_color_convert();
            self.persist_ui_prefs();
        }

        if let Some(id) = actions.apply_profile {
            self.apply_named_profile(&id);
        }
        if let Some(id) = actions.save_profile {
            self.save_into_named_profile(&id);
        }
        if actions.toggle_hud {
            self.ui_prefs.hud_enabled = !self.ui_prefs.hud_enabled;
            self.persist_ui_prefs();
        }
        if let Some(on) = actions.hud_enabled_changed {
            self.ui_prefs.hud_enabled = on;
            self.persist_ui_prefs();
        }
        if let Some(on) = actions.hud_always_on_top_changed {
            self.ui_prefs.hud_always_on_top = on;
            self.apply_window_level();
            self.persist_ui_prefs();
        }
        if let Some(on) = actions.replay_enabled_changed {
            self.ui_prefs.replay_enabled = on;
            self.replay.set_enabled(on);
            self.persist_ui_prefs();
        }
        if let Some(secs) = actions.replay_seconds_changed {
            self.ui_prefs.replay_seconds = secs.clamp(3, 30);
            self.replay.set_seconds(self.ui_prefs.replay_seconds);
            self.persist_ui_prefs();
        }
        if actions.save_replay_clip {
            self.save_replay_clip();
        }
        if let Some((w, h, num, den)) = actions.resolution_preset {
            self.apply_resolution_preset(w, h, Rational::new(num, den));
        }
        if let Some(g) = actions.grade_changed {
            self.ui_prefs.grade = g.clamp();
            self.apply_grade_to_gpu();
            self.ui_prefs.profiles.active = "custom".into();
            self.persist_ui_prefs();
        }
        if actions.grade_reset {
            self.ui_prefs.grade = GradePrefs::default();
            self.apply_grade_to_gpu();
            self.persist_ui_prefs();
        }
        if let Some(path) = actions.cube_path_changed {
            self.load_cube_lut(path);
        }
        if actions.cube_clear {
            self.cube_lut = None;
            self.ui_prefs.cube_lut_path.clear();
            self.ui_prefs.grade.lut_mix = 0.0;
            self.apply_grade_to_gpu();
            self.persist_ui_prefs();
        }
        if let Some(on) = actions.spout_enabled_changed {
            self.export.prefs.spout_enabled = on;
            self.ui_prefs.export.spout_enabled = on;
            self.export.sync_active_flags();
            if on {
                self.export.ensure_spout();
            } else {
                #[cfg(windows)]
                {
                    // Drop sender when disabled (ensure_spout no-ops when off).
                    self.export.ensure_spout();
                }
            }
            self.persist_ui_prefs();
        }
        if let Some(on) = actions.ndi_enabled_changed {
            self.export.prefs.ndi_enabled = on;
            self.ui_prefs.export.ndi_enabled = on;
            self.export.sync_active_flags();
            self.persist_ui_prefs();
        }
        if let Some(name) = actions.spout_name_changed {
            self.export.prefs.spout_name = name.clone();
            self.ui_prefs.export.spout_name = name;
            self.persist_ui_prefs();
        }
        if let Some(name) = actions.ndi_name_changed {
            self.export.prefs.ndi_name = name.clone();
            self.ui_prefs.export.ndi_name = name;
            self.persist_ui_prefs();
        }
        if actions.export_refresh {
            self.export.refresh_probe();
        }
        if actions.ndi_open_download {
            crate::export::open_url(crate::export::NDI_DOWNLOAD_URL);
        }
        if actions.ndi_open_folder {
            let folder = if !self.export.status.ndi_folder.is_empty() {
                std::path::PathBuf::from(&self.export.status.ndi_folder)
            } else {
                crate::export::find_ndi_folder().unwrap_or_default()
            };
            if !folder.as_os_str().is_empty() {
                crate::export::open_folder(&folder);
            }
        }
        if actions.dlss_open_docs {
            crate::export::open_url(crate::export::DLSS_DOCS_URL);
        }
        if actions.dlss_open_folder {
            if let Some(dir) = crate::export::dlss_byo_dir() {
                let _ = std::fs::create_dir_all(&dir);
                crate::export::open_folder(&dir);
            }
        }
        if let Some(on) = actions.health_auto_reinit_changed {
            self.health.prefs.auto_reinit = on;
            self.ui_prefs.health.auto_reinit = on;
            self.persist_ui_prefs();
        }
        if let Some(secs) = actions.health_stall_secs_changed {
            self.health.prefs.stall_secs = secs.clamp(2, 15);
            self.ui_prefs.health.stall_secs = self.health.prefs.stall_secs;
            self.persist_ui_prefs();
        }
        if actions.update_check {
            self.update.request_check(true);
        }
        if actions.update_open {
            self.update.open_latest();
        }
        if actions.update_skip_this_version {
            let tag = self.update.status.latest_tag.clone();
            if !tag.is_empty() {
                self.ui_prefs.update_skip_tag = tag;
                self.persist_ui_prefs();
            }
        }
        if actions.dismiss_update_popup {
            self.chrome.update_popup_open = false;
            self.update_popup_dismissed = true;
        }
    }

    fn persist_ui_prefs(&mut self) {
        self.ui_prefs.wizard_step = self.chrome.wizard_step;
        self.ui_prefs.upscale_mode = self.effects_ui.mode.clone();
        self.ui_prefs.sharpness = self.effects_ui.sharpness;
        self.ui_prefs.dlss_boost = self.effects_ui.dlss_boost;
        self.ui_prefs.dlssg_multiplier = self.effects_ui.dlssg_multiplier.clone();
        self.ui_prefs.interp_mode = self.effects_ui.interp_mode.clone();
        self.ui_prefs.interp_motion_blur = self.effects_ui.interp_motion_blur;
        self.ui_prefs.deinterlace_mode = self.effects_ui.deinterlace_mode.clone();
        self.ui_prefs.vsync_mode = normalize_vsync_mode(&self.effects_ui.vsync_mode);
        self.ui_prefs.hdr_output = self.effects_ui.hdr_output;
        self.ui_prefs.export = self.export.prefs.clone();
        self.ui_prefs.health = self.health.prefs.clone();
        // Debounce disk writes — sync save on every slider tick froze the picture.
        self.prefs_dirty = true;
        self.prefs_dirty_at = Instant::now();
    }

    fn apply_grade_to_gpu(&mut self) {
        let grade = self.ui_prefs.grade.as_gpu();
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.pipeline.set_grade(grade);
            gpu.upscale.set_grade(grade);
            gpu.interpolate.set_grade(grade);
        }
    }

    fn apply_window_level(&mut self) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let level = if self.ui_prefs.hud_always_on_top {
            WindowLevel::AlwaysOnTop
        } else {
            WindowLevel::Normal
        };
        gpu.window.set_window_level(level);
    }

    fn apply_named_profile(&mut self, id: &str) {
        self.ui_prefs.profiles.ensure_builtins();
        let Some(profile) = self.ui_prefs.profiles.get(id).cloned() else {
            return;
        };
        self.effects_ui.mode = profile.upscale_mode.clone();
        self.effects_ui.sharpness = profile.sharpness;
        self.effects_ui.interp_mode = profile.interp_mode.clone();
        self.effects_ui.interp_motion_blur = profile.interp_motion_blur;
        self.effects_ui.dlssg_multiplier = profile.dlssg_multiplier.clone();
        self.effects_ui.vsync_mode = profile.vsync_mode.clone();
        self.effects_ui.hdr_output = profile.hdr_output;
        self.ui_prefs.upscale_mode = profile.upscale_mode.clone();
        self.ui_prefs.sharpness = profile.sharpness;
        self.ui_prefs.interp_mode = profile.interp_mode.clone();
        self.ui_prefs.interp_motion_blur = profile.interp_motion_blur;
        self.ui_prefs.dlssg_multiplier = profile.dlssg_multiplier.clone();
        self.ui_prefs.display_fps_limit = profile.display_fps_limit;
        self.ui_prefs.vsync_mode = profile.vsync_mode.clone();
        self.ui_prefs.hdr_output = profile.hdr_output;
        self.ui_prefs.monitor = profile.monitor.clone();
        self.ui_prefs.profiles.active = profile.id.clone();
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.upscale
                .set_mode(UpscaleMode::from_str_lossy(&profile.upscale_mode));
            gpu.upscale.set_sharpness(profile.sharpness);
            gpu.interpolate
                .set_mode(InterpMode::from_str_lossy(&profile.interp_mode));
            gpu.interpolate
                .set_motion_blur(profile.interp_motion_blur);
            gpu.upscale
                .dlss
                .set_fg_multiplier(DlssGMultiplier::from_str_lossy(&profile.dlssg_multiplier));
        }
        self.apply_present_mode_for_profile();
        self.refresh_os_display_caps(true);
        self.apply_hdr_output();
        self.persist_ui_prefs();
        self.show_toast(self.i18n.t("msg_profile_applied"));
    }

    fn save_into_named_profile(&mut self, id: &str) {
        self.ui_prefs.profiles.ensure_builtins();
        let snap = NamedProfile {
            id: id.to_string(),
            label_key: self
                .ui_prefs
                .profiles
                .get(id)
                .map(|p| p.label_key.clone())
                .unwrap_or_else(|| id.to_string()),
            upscale_mode: self.effects_ui.mode.clone(),
            sharpness: self.effects_ui.sharpness,
            interp_mode: self.effects_ui.interp_mode.clone(),
            interp_motion_blur: self.effects_ui.interp_motion_blur,
            dlssg_multiplier: self.effects_ui.dlssg_multiplier.clone(),
            display_fps_limit: self.ui_prefs.display_fps_limit,
            vsync_mode: normalize_vsync_mode(&self.effects_ui.vsync_mode),
            hdr_output: self.effects_ui.hdr_output,
            monitor: self.ui_prefs.monitor.clone(),
        };
        if let Some(slot) = self.ui_prefs.profiles.get_mut(id) {
            *slot = snap;
        } else {
            self.ui_prefs.profiles.profiles.push(snap);
        }
        self.ui_prefs.profiles.active = id.to_string();
        self.persist_ui_prefs();
        self.show_toast(self.i18n.t("msg_profile_saved"));
    }

    fn apply_resolution_preset(&mut self, w: u32, h: u32, rate: Rational) {
        let caps = &self.devices[self.selection.device_idx].formats;
        let fmt = modes::pick_format_at(caps, w, h, rate).or_else(|| {
            modes::rates_for_resolution(caps, w, h)
                .first()
                .and_then(|r| modes::pick_format_at(caps, w, h, *r))
        });
        if let Some(fmt) = fmt {
            let msg = self.i18n.t("err_resolution");
            if self.restart_with_fallback(fmt, &msg) {
                self.request_window_for_capture();
                self.show_toast(self.i18n.t("msg_hotswap_ok"));
            }
        }
        self.selection.width = self.current_format.width;
        self.selection.height = self.current_format.height;
        self.selection.rate = self
            .current_format
            .frame_rates
            .first()
            .copied()
            .unwrap_or(self.selection.rate);
    }

    fn load_cube_lut(&mut self, path: String) {
        let path = path.trim().to_string();
        if path.is_empty() {
            self.cube_lut = None;
            self.ui_prefs.cube_lut_path.clear();
            self.ui_prefs.grade.lut_mix = 0.0;
            self.apply_grade_to_gpu();
            self.persist_ui_prefs();
            return;
        }
        match CubeLut1d::load_path(std::path::Path::new(&path)) {
            Ok(lut) => {
                self.cube_lut = Some(lut);
                self.ui_prefs.cube_lut_path = path;
                if self.ui_prefs.grade.lut_mix < 0.01 {
                    self.ui_prefs.grade.lut_mix = 1.0;
                }
                self.apply_grade_to_gpu();
                self.persist_ui_prefs();
                self.show_toast(self.i18n.t("msg_cube_loaded"));
            }
            Err(e) => {
                tracing::warn!(error = %e, "cube lut load failed");
                self.show_toast(self.i18n.t("msg_cube_err"));
            }
        }
    }

    fn save_replay_clip(&mut self) {
        match self.replay.save_clip() {
            Ok(dir) => {
                let frames = self.replay.frame_count();
                let msg = format!(
                    "{} ({frames}) — {}",
                    self.i18n.t("msg_replay_saved"),
                    dir.display()
                );
                // Longer dwell so the folder path is readable.
                self.show_toast_kind(msg, ToastKind::Success, Duration::from_secs(6));
            }
            Err(e) if e.contains("empty") => {
                self.show_toast_kind(
                    self.i18n.t("msg_replay_empty"),
                    ToastKind::Error,
                    Duration::from_secs(5),
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "replay save failed");
                self.show_toast_kind(
                    self.i18n.t("msg_replay_err"),
                    ToastKind::Error,
                    Duration::from_secs(5),
                );
            }
        }
    }

    fn sync_feature_overlay(&mut self) {
        self.update.poll();
        self.maybe_open_update_popup();
        self.overlay.hud_enabled = self.ui_prefs.hud_enabled;
        self.overlay.hud_always_on_top = self.ui_prefs.hud_always_on_top;
        self.overlay.replay_enabled = self.ui_prefs.replay_enabled;
        self.overlay.replay_seconds = self.ui_prefs.replay_seconds;
        self.overlay.replay_frames = self.replay.frame_count() as u32;
        self.overlay.grade = self.ui_prefs.grade;
        self.overlay.cube_lut_path = self.ui_prefs.cube_lut_path.clone();
        self.overlay.health = self.health.ui.clone();
        self.overlay.health_auto_reinit = self.health.prefs.auto_reinit;
        self.overlay.health_stall_secs = self.health.prefs.stall_secs;
        self.overlay.export = self.export.status.clone();
        self.overlay.spout_enabled = self.export.prefs.spout_enabled;
        self.overlay.ndi_enabled = self.export.prefs.ndi_enabled;
        self.overlay.spout_name = self.export.prefs.spout_name.clone();
        self.overlay.ndi_name = self.export.prefs.ndi_name.clone();
        self.overlay.update = self.update.status.clone();
        self.overlay.current_version = env!("CARGO_PKG_VERSION").into();
        self.overlay.profiles = self.ui_prefs.profiles.clone();
    }

    /// Open the startup update modal once when a newer release arrives (non-blocking).
    fn maybe_open_update_popup(&mut self) {
        if self.update_popup_dismissed || self.chrome.update_popup_open {
            return;
        }
        if self.chrome.wizard_open {
            return;
        }
        // Avoid stacking on top of the brief launch shortcuts card.
        if !self.launch_tips_dismissed && self.chrome.launch_tips_alpha > 0.01 {
            return;
        }
        if !self.ui_prefs.update_check {
            return;
        }
        let st = &self.update.status;
        if st.checking || !st.newer || st.latest_tag.is_empty() {
            return;
        }
        if tags_match(&st.latest_tag, &self.ui_prefs.update_skip_tag) {
            return;
        }
        self.chrome.update_popup_open = true;
        // Wake the UI loop so the modal appears even if capture is idle.
        if let Some(proxy) = &self.proxy {
            let _ = proxy.send_event(UserEvent::FrameReady);
        }
    }

    fn flush_prefs_if_due(&mut self) {
        if !self.prefs_dirty {
            return;
        }
        if self.prefs_dirty_at.elapsed() < Duration::from_millis(400) {
            return;
        }
        save_ui_prefs(&self.ui_prefs);
        self.prefs_dirty = false;
    }

    fn flush_prefs_now(&mut self) {
        if !self.prefs_dirty {
            self.ui_prefs.wizard_step = self.chrome.wizard_step;
            self.ui_prefs.upscale_mode = self.effects_ui.mode.clone();
            self.ui_prefs.sharpness = self.effects_ui.sharpness;
            self.ui_prefs.dlss_boost = self.effects_ui.dlss_boost;
            self.ui_prefs.dlssg_multiplier = self.effects_ui.dlssg_multiplier.clone();
            self.ui_prefs.interp_mode = self.effects_ui.interp_mode.clone();
            self.ui_prefs.interp_motion_blur = self.effects_ui.interp_motion_blur;
            self.ui_prefs.deinterlace_mode = self.effects_ui.deinterlace_mode.clone();
            self.ui_prefs.vsync_mode = normalize_vsync_mode(&self.effects_ui.vsync_mode);
            self.ui_prefs.hdr_output = self.effects_ui.hdr_output;
        }
        save_ui_prefs(&self.ui_prefs);
        self.prefs_dirty = false;
    }

    fn sync_effects_ui(&mut self) {
        let (cw, ch) = self
            .gpu
            .as_ref()
            .map(|g| g.pipeline.content_size)
            .unwrap_or((0, 0));
        // Prefer live frame size; fall back to negotiated capture format before first frame.
        let (cw, ch) = if cw > 0 && ch > 0 {
            (cw, ch)
        } else {
            (self.current_format.width, self.current_format.height)
        };
        let (vw, vh) = self
            .gpu
            .as_ref()
            .map(|g| (g.config.width, g.config.height))
            .unwrap_or((1, 1));
        // Grey FSR/NIS only when capture already fills the letterboxed window (not 1080→1440).
        let native = is_native_or_downscale(cw, ch, vw, vh);
        if let Some(gpu) = self.gpu.as_ref() {
            let st = gpu.upscale.status();
            self.effects_ui.mode = st.mode.as_str().into();
            self.effects_ui.sharpness = st.sharpness;
            self.effects_ui.compute_ok = st.compute_ok;
            self.effects_ui.native_res = native;
            self.effects_ui.auto_disabled = st.block == UpscaleBlockReason::AutoDisabled;
            self.effects_ui.last_gpu_ms = st.last_gpu_ms.max(gpu.interpolate.last_gpu_ms);
            self.effects_ui.vendor = st.vendor;
            self.effects_ui.backend = st.backend;
            self.effects_ui.dlss_ok = st.dlss.availability.can_evaluate();
            self.effects_ui.dlss_reason_key = st.dlss.availability.as_reason_key().into();
            self.effects_ui.dlss_fallback_nis = !st.dlss_nr_active && st.mode.as_str() == "dlss";
            self.effects_ui.dlss_boost = st.dlss.boost;
            self.effects_ui.dlssg_ok = st.dlss.fg.availability.can_evaluate();
            self.effects_ui.dlssg_reason_key = st.dlss.fg.availability.as_reason_key().into();
            self.effects_ui.dlssg_multiplier = st.dlss.fg.multiplier.as_str().into();
            let ist = gpu.interpolate.status();
            self.effects_ui.interp_mode = ist.mode.as_str().into();
            self.effects_ui.interp_compute_ok = ist.compute_ok;
            self.effects_ui.interp_auto_disabled =
                ist.block == InterpBlockReason::AutoDisabled;
            self.effects_ui.interp_delay_ms = ist.delay_ms;
            self.effects_ui.deinterlace_mode = gpu.deinterlace.mode.as_str().into();
            self.effects_ui.source_interlaced = gpu.deinterlace.source_interlaced;
            self.effects_ui.deinterlace_active = gpu.deinterlace.should_run();
            self.effects_ui.vsync_mode = normalize_vsync_mode(&self.ui_prefs.vsync_mode);
            self.effects_ui.hdr_output = self.ui_prefs.hdr_output;
        } else {
            self.effects_ui.native_res = native;
            self.effects_ui.vsync_mode = normalize_vsync_mode(&self.ui_prefs.vsync_mode);
            self.effects_ui.hdr_output = self.ui_prefs.hdr_output;
        }
    }

    fn push_support(&mut self, line: String) {
        if self.support_log.len() > 200 {
            self.support_log.remove(0);
        }
        self.support_log.push(line);
    }

    fn copy_support_report(&mut self) {
        let log_path = crate::logging::log_file_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".into());
        let log_tail = crate::logging::log_tail();
        let backend = self
            .gpu
            .as_ref()
            .map(|g| g.upscale.backend.as_str())
            .unwrap_or("?");
        let report = format!(
            "CaptureView support report\n\
             build={} capture_wake={}\n\
             locale={:?}\n\
             device={}\n\
             format={}x{} {:?} @ {:?}\n\
             audio_in={} audio_out={}\n\
             fps={:.1} capture_fps={:.1} frame_gen_fps={:.1} display_limit={} vsync={} present={} hdr_out={} hdr_active={} vrr={} pipeline_delay_ms={:.1}\n\
             dropped={} 1%low={:.1} gpu={} backend={}\n\
             log_level={} log_file={}\n\
             -- support events --\n{}\n\
             -- log tail --\n{}\n",
            env!("CAPTUREVIEW_GIT_HASH"),
            env!("CAPTUREVIEW_CAPTURE_WAKE"),
            self.i18n.lang(),
            self.overlay.label,
            self.current_format.width,
            self.current_format.height,
            self.current_format.pixel_format,
            self.current_format.frame_rates,
            self.audio_ui.input_id,
            self.audio_ui.output_id,
            self.overlay.fps,
            self.overlay.capture_fps,
            self.overlay.frame_gen_fps,
            self.ui_prefs.display_fps_limit,
            self.ui_prefs.vsync_mode,
            self.overlay.present_mode,
            self.ui_prefs.hdr_output,
            self.overlay.hdr_active,
            self.overlay.vrr_capable,
            self.overlay.pipeline_delay_ms,
            self.overlay.dropped_frames,
            self.overlay.one_percent_low_fps,
            self.overlay.gpu_name,
            backend,
            self.ui_prefs.log_level,
            log_path,
            self.support_log.join("\n"),
            log_tail
        );
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(report)) {
            Ok(()) => self.show_toast(self.i18n.t("msg_copied")),
            Err(e) => {
                self.push_support(format!("clipboard failed: {e}"));
                self.show_toast(self.i18n.t("err_audio"));
            }
        }
    }

    fn show_toast(&mut self, msg: String) {
        self.show_toast_kind(msg, ToastKind::Neutral, Duration::from_secs(3));
    }

    fn show_toast_kind(&mut self, msg: String, kind: ToastKind, dwell: Duration) {
        self.overlay.toast = msg;
        self.overlay.toast_kind = kind;
        self.toast_until = Some(Instant::now() + dwell);
    }

    /// Resize the windowed client area to the active capture size (physical pixels).
    /// Keeps windowed 1080 at a true 1:1 present instead of a soft fractional fit.
    fn request_window_for_capture(&mut self) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        if self.chrome.fullscreen {
            return;
        }
        let w = self.current_format.width;
        let h = self.current_format.height;
        if w == 0 || h == 0 {
            return;
        }
        let _ = gpu.window.request_inner_size(winit::dpi::PhysicalSize::new(w, h));
        tracing::info!(w, h, "requested windowed client size for capture 1:1");
    }

    fn toggle_fullscreen(&mut self) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        self.chrome.fullscreen = !self.chrome.fullscreen;
        if self.chrome.fullscreen {
            // Prefer the monitor the window is on so FSR/NIS see the real pixel size
            // on the first fullscreen frame (don't wait for a late Resized event).
            let monitor = gpu
                .window
                .current_monitor()
                .or_else(|| gpu.window.primary_monitor());
            if let Some(monitor) = monitor.clone() {
                let size = monitor.size();
                gpu.window
                    .set_fullscreen(Some(winit::window::Fullscreen::Borderless(Some(
                        monitor,
                    ))));
                if size.width > 0 && size.height > 0 {
                    gpu.config.width = size.width;
                    gpu.config.height = size.height;
                    gpu.surface.configure(&gpu.device, &gpu.config);
                    tracing::info!(
                        width = size.width,
                        height = size.height,
                        "fullscreen surface configured for upscale"
                    );
                }
            } else {
                gpu.window
                    .set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
            }
        } else {
            gpu.window.set_fullscreen(None);
        }
    }

    fn reset_to_auto(&mut self) {
        self.selection.mode = CaptureMode::Auto;
        let caps = self.devices[self.selection.device_idx].formats.clone();
        if let Some(fmt) = modes::select_format(CaptureMode::Auto, &caps) {
            let msg = self.i18n.t("err_mode");
            let _ = self.restart_with_fallback(fmt, &msg);
        }
        self.audio_ui.delay_ms = 0.0;
        self.audio_ui.buffer_ms = 30.0;
        self.audio_ui.volume = 1.0;
        self.audio_ui.muted = false;
        self.suggest_audio_for_current_video();
        self.restart_audio();
        self.show_toast(self.i18n.t("msg_reset_auto"));
    }

    fn redraw(&mut self) {
        let period = Duration::from_secs_f64(1.0 / self.target_display_hz());
        let now = Instant::now();
        let framegen = self.framegen_wants_2x();
        let pending = self
            .source
            .as_ref()
            .map(|s| !s.receiver().is_empty())
            .unwrap_or(false);
        let since_present = self
            .last_present_at
            .map(|t| now.duration_since(t))
            .unwrap_or(Duration::from_secs(1));
        let min_gap = Duration::from_millis(1);
        let capture_due = !framegen && pending && since_present >= min_gap;
        // Hard drop surplus RedrawRequested (egui may queue many under NoVsync).
        if !capture_due && now + Duration::from_micros(500) < self.next_present_at {
            return;
        }
        // Capture-driven: snap phase to the capture clock so latency stays flat.
        // Framegen: advance absolute 2× grid (catch-up capped).
        if capture_due || (!framegen && pending) {
            self.next_present_at = now + period;
        } else {
            self.next_present_at += period;
            if self.next_present_at + period < now {
                self.next_present_at = now + period;
            }
        }
        self.last_redraw_at = now;
        // Drain frames before borrowing GPU exclusively for UI state updates.
        let mut latest: Option<VideoFrame> = None;
        let mut drained = 0u64;
        if let Some(source) = self.source.as_ref() {
            let rx = source.receiver();
            while let Ok(frame) = rx.try_recv() {
                self.metrics.record_capture();
                drained += 1;
                latest = Some(frame);
            }
            let drops = rx.dropped_count();
            if drops > self.last_channel_drops {
                self.metrics
                    .record_drop(drops - self.last_channel_drops);
                self.last_channel_drops = drops;
            }
        }
        if drained > 1 {
            self.metrics.record_coalesced(drained - 1);
        }
        self.captures_in_window += drained as u32;

        if let Some(ref frame) = latest {
            self.last_frame_at = Instant::now();
            self.health.note_frame();
            self.replay.push_frame(frame);
            self.export.on_frame(
                frame.width,
                frame.height,
                frame.pixel_format,
                frame.data.as_slice(),
            );
            if approx_black(frame) {
                self.black_since.get_or_insert_with(Instant::now);
            } else {
                self.black_since = None;
            }
        }

        let stream_health = self
            .source
            .as_ref()
            .map(|s| s.stream_status().get())
            .unwrap_or(StreamHealth::Ok);
        let device_busy = matches!(
            stream_health,
            StreamHealth::DeviceBusy | StreamHealth::StreamError
        );
        let drops_now = self
            .source
            .as_ref()
            .map(|s| s.receiver().dropped_count())
            .unwrap_or(self.last_channel_drops);
        if self.health.tick(
            stream_health,
            drops_now,
            self.capture_restarting,
            device_busy,
        ) {
            let fmt = self.current_format.clone();
            let idx = self.selection.device_idx;
            if let Err(e) = self.open_and_start(idx, fmt) {
                self.apply_capture_err(&e);
            } else {
                self.show_toast(self.i18n.t("msg_health_reinit"));
            }
        }
        // True HDMI black / no input — never during an intentional capture restart.
        let no_signal = !device_busy
            && !self.capture_restarting
            && self.last_frame_at.elapsed() > Duration::from_secs(1);
        let soft_black = self
            .black_since
            .map(|t| t.elapsed() > Duration::from_secs(3))
            .unwrap_or(false)
            && !no_signal
            && !device_busy;
        if device_busy {
            self.overlay.status = self.i18n.t("err_device_busy");
        } else if self.overlay.status == self.i18n.t("err_device_busy") {
            self.overlay.status.clear();
        }

        self.frames_in_window += 1;
        let elapsed = self.fps_window_start.elapsed().as_secs_f32();
        if elapsed >= 0.5 {
            self.overlay.fps = self.frames_in_window as f32 / elapsed;
            self.overlay.capture_fps = self.captures_in_window as f32 / elapsed;
            self.overlay.frame_gen_fps = self.gen_frames_in_window as f32 / elapsed;
            self.frames_in_window = 0;
            self.captures_in_window = 0;
            self.gen_frames_in_window = 0;
            self.fps_window_start = Instant::now();
        }
        self.overlay.display_fps_limit = self.ui_prefs.display_fps_limit;
        self.overlay.vsync_mode = normalize_vsync_mode(&self.ui_prefs.vsync_mode);
        self.sync_present_overlay();
        // If OS HDR flipped while we prefer HDR, rebuild the swapchain format once.
        if let Some(gpu) = self.gpu.as_ref() {
            let prefer = self.want_hdr_swapchain();
            let active = gpu.present_caps.hdr_active;
            let can_hdr = gpu.present_caps.hdr_format_supported;
            if (prefer && can_hdr && !active) || (!prefer && active) {
                self.apply_hdr_output();
            }
        }
        self.overlay.discord_presence = self.ui_prefs.discord_presence;
        self.overlay.discord_app_id = self.ui_prefs.discord_app_id.clone();
        self.overlay.discord_active = self.discord.is_active();
        self.overlay.log_level = crate::logging::normalize_level(&self.ui_prefs.log_level).to_string();
        self.overlay.log_path = crate::logging::log_file_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();

        // Keep monitor toggles live even if prefs were edited elsewhere.
        self.overlay.monitor = self.ui_prefs.monitor.clone();
        self.sync_feature_overlay();

        // Refresh process / system samples ~2 Hz (UI thread only).
        if self.process_sample_at.elapsed() >= Duration::from_millis(500) {
            let sample = sample_process();
            const HISTORY: usize = 60;
            let mb = 1024.0_f64 * 1024.0;
            self.overlay.ram_mb = (sample.rss_bytes as f64 / mb) as f32;
            self.overlay.cpu_one_core = sample.cpu_one_core;
            self.overlay.system_cpu_frac = sample.system_cpu_frac;
            self.overlay.per_core_frac = sample.per_core_frac;
            self.overlay.affinity_mask = sample.affinity_mask;
            self.overlay.logical_cores = sample.logical_cores;
            self.overlay.system_ram_used_mb = (sample.system_ram_used_bytes as f64 / mb) as f32;
            self.overlay.system_ram_total_mb = (sample.system_ram_total_bytes as f64 / mb) as f32;
            self.overlay.vram_used_mb = (sample.vram_used_bytes as f64 / mb) as f32;
            self.overlay.vram_budget_mb = (sample.vram_budget_bytes as f64 / mb) as f32;
            self.overlay.io_read_mbps = (sample.io_read_bps / mb) as f32;
            self.overlay.io_write_mbps = (sample.io_write_bps / mb) as f32;
            push_history(
                &mut self.overlay.cpu_history,
                sample.cpu_one_core * 100.0,
                HISTORY,
            );
            push_history(&mut self.overlay.ram_history, self.overlay.ram_mb, HISTORY);
            self.process_sample_at = Instant::now();
        }

        if let Some(start) = self.lag_test_started {
            self.overlay.lag_test_ms = start.elapsed().as_secs_f32() * 1000.0;
        }
        self.overlay.lag_flash = self
            .lag_flash_until
            .map(|t| Instant::now() < t)
            .unwrap_or(false);
        if self.overlay.lag_flash == false {
            if let Some(until) = self.lag_flash_until {
                if Instant::now() >= until {
                    self.lag_flash_until = None;
                }
            }
        }

        // Cheap counters every frame; percentile + graph copy at ~4 Hz (avoids sort/alloc hitch).
        let snap_light = self.metrics.snapshot_light();
        self.overlay.pipeline_delay_ms = snap_light.last_latency_ms;
        self.overlay.dropped_frames = snap_light.frames_dropped;
        if self.last_metrics_ui_at.elapsed() >= Duration::from_millis(250) {
            self.last_metrics_ui_at = Instant::now();
            let snap = self.metrics.snapshot();
            self.cached_one_percent_low = snap.one_percent_low_fps;
            self.cached_frame_times_ms = snap
                .frame_times_ns
                .iter()
                .map(|ns| *ns as f32 / 1_000_000.0)
                .collect();
            self.overlay.one_percent_low_fps = self.cached_one_percent_low;
            self.overlay.frame_times_ms.clone_from(&self.cached_frame_times_ms);
        } else {
            self.overlay.one_percent_low_fps = self.cached_one_percent_low;
        }

        self.overlay.frame_w = self.current_format.width;
        self.overlay.frame_h = self.current_format.height;
        self.overlay.label = self
            .source
            .as_ref()
            .map(|s| s.info().display_name.clone())
            .unwrap_or_else(|| "—".into());
        self.overlay.mode_label = match self.selection.mode {
            CaptureMode::Auto => self.i18n.t("label_mode_auto"),
            CaptureMode::Game => self.i18n.t("label_mode_game"),
            CaptureMode::Quality => self.i18n.t("label_mode_quality"),
            CaptureMode::Economy => self.i18n.t("label_mode_economy"),
        };
        let rate = self
            .current_format
            .frame_rates
            .first()
            .copied()
            .unwrap_or(self.selection.rate);
        // Dropdown must mirror the live MF format (avoids 720p preset vs 1080 capture).
        self.selection.width = self.current_format.width;
        self.selection.height = self.current_format.height;
        self.selection.rate = rate;
        self.overlay.format_label = format!(
            "{}×{} {:?} @ {}",
            self.current_format.width,
            self.current_format.height,
            self.current_format.pixel_format,
            hz_label(rate)
        );
        self.overlay.no_signal = no_signal;
        self.overlay.device_busy = device_busy;
        self.overlay.soft_black = soft_black;
        self.overlay.level = self.audio.meter().take_peak();
        self.overlay.audio_notice = self.audio.status().notice.clone();
        if let Some(until) = self.toast_until {
            if Instant::now() > until {
                self.overlay.toast.clear();
                self.overlay.toast_kind = ToastKind::Neutral;
                self.toast_until = None;
            }
        }
        if self.overlay.gpu_name.is_empty() {
            if let Some(gpu) = self.gpu.as_ref() {
                self.overlay.gpu_name = gpu.adapter_name.clone();
            }
        }
        // Hide floating toolbar after 3s without pointer motion.
        // Metrics (F2) alone must not keep Image/Son/Effets visible.
        if !self.chrome.settings_open
            && !self.chrome.wizard_open
            && self.last_pointer_at.elapsed() > Duration::from_secs(3)
        {
            self.chrome.toolbar_visible = false;
        }

        // Idle cursor: hide over the preview; keep visible for settings / wizard / UI use.
        let ui_keeps_cursor = self.chrome.settings_open
            || self.chrome.wizard_open
            || self.chrome.update_popup_open
            || self.chrome.launch_tips_alpha > 0.01;
        let want_hide = !ui_keeps_cursor && self.last_pointer_at.elapsed() > CURSOR_IDLE_HIDE;
        self.chrome.hide_idle_cursor = want_hide;
        if want_hide {
            self.hide_os_cursor();
        } else {
            self.show_os_cursor();
        }

        // Launch shortcuts tip: wait until the wizard is gone, then fade in once.
        if !self.launch_tips_dismissed && !self.chrome.wizard_open {
            if self.launch_tips_started.is_none() {
                self.launch_tips_started = Some(Instant::now());
            }
            if let Some(start) = self.launch_tips_started {
                let alpha = launch_tips_alpha(start.elapsed());
                self.chrome.launch_tips_alpha = alpha;
                if alpha <= 0.0 && start.elapsed() > Duration::from_secs(1) {
                    self.launch_tips_dismissed = true;
                }
            }
        } else if self.chrome.wizard_open {
            self.chrome.launch_tips_alpha = 0.0;
        }

        let devices = self.devices.clone();
        let selection = self.selection.clone();
        let current = self.current_format.clone();
        let overlay = self.overlay.clone();
        let inputs = self.audio_inputs.clone();
        let outputs = self.audio_outputs.clone();
        let audio_ui = self.audio_ui.clone();
        let chrome = self.chrome.clone();
        let i18n = self.i18n.clone();
        self.sync_effects_ui();
        let effects = self.effects_ui.clone();
        let color = self.color_ui.clone();
        // Video presents every tick; egui widgets only draw when chrome/overlay is active.

        let mut actions = ui::UiActions::default();
        let display_period =
            Duration::from_secs_f64(1.0 / self.target_display_hz());
        {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };

        let new_capture = latest.is_some();
        // Pipeline delay = capture Instant → this present, only on a fresh upload.
        // Present-only / Mid→Curr ticks keep the last reading (no sawtooth from aging).
        let mut latency_ns = self.metrics.last_latency_ns.load(Ordering::Relaxed);
        if let Some(ref frame) = latest {
            // With capture-driven wake, this stays ~GPU path (few ms), not 2→16 phase walk.
            latency_ns = frame.captured_at.elapsed().as_nanos() as u64;
            if upload_frame(&gpu.device, &gpu.queue, &mut gpu.pipeline, &frame) {
                let rate = self
                    .current_format
                    .frame_rates
                    .first()
                    .map(|r| r.as_f64())
                    .unwrap_or(60.0);
                let src_period_ms = if rate > 0.0 { 1000.0 / rate as f32 } else { 16.7 };
                gpu.interpolate.set_source_delay_ms(src_period_ms);
            }
        }

        gpu.pipeline
            .update_letterbox(&gpu.queue, gpu.config.width, gpu.config.height);

        let frame = match gpu.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                return;
            }
            Err(wgpu::SurfaceError::Timeout) => return,
            Err(e) => {
                tracing::error!("surface error: {e}");
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let raw_input = gpu.egui_state.take_egui_input(&gpu.window);
        let logo = crate::brand::ensure_logo_texture(&gpu.egui_ctx, &mut self.logo_tex);
        let full_output = gpu.egui_ctx.run(raw_input, |ctx| {
            actions = ui::draw_ui(
                ctx,
                ui::DrawInput {
                    i18n: &i18n,
                    devices: &devices,
                    selection: &selection,
                    current: &current,
                    overlay: &overlay,
                    inputs: &inputs,
                    outputs: &outputs,
                    audio: &audio_ui,
                    chrome: &chrome,
                    effects: &effects,
                    color: &color,
                    logo: logo.clone(),
                },
            );
        });
        gpu.egui_state
            .handle_platform_output(&gpu.window, full_output.platform_output);
        // egui may restore a cursor icon; re-assert idle hide after platform output.
        if self.chrome.hide_idle_cursor {
            gpu.window.set_cursor_visible(false);
            self.cursor_hidden = true;
        }
        // Steer egui away from immediate repaint storms under NoVsync; about_to_wait
        // is the source of truth for the next present.
        gpu.egui_ctx.request_repaint_after(display_period);

        let tris = gpu
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen = ScreenDescriptor {
            size_in_pixels: [gpu.config.width, gpu.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };
        for (id, delta) in &full_output.textures_delta.set {
            gpu.egui_renderer
                .update_texture(&gpu.device, &gpu.queue, *id, delta);
        }

        let clear = if overlay.lag_flash {
            wgpu::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            }
        } else if no_signal || soft_black {
            wgpu::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            }
        } else {
            wgpu::Color {
                r: 0.02,
                g: 0.02,
                b: 0.02,
                a: 1.0,
            }
        };

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame-encoder"),
            });

        // Pipeline: upload → color → deinterlace → interpolate → upscale → present.
        // DLSS-G: between captures, drain EvaluateFeature frames from the FG present queue.
        let mut presented = false;
        if !no_signal && !new_capture {
            if let Some(fg_frame) = gpu.upscale.pop_fg_present_frame() {
                presented = gpu.upscale.present_fg_frame(
                    &gpu.device,
                    &gpu.queue,
                    &mut encoder,
                    &fg_frame,
                    &view,
                    gpu.config.width,
                    gpu.config.height,
                    clear,
                );
                if presented {
                    self.gen_frames_in_window = self.gen_frames_in_window.saturating_add(1);
                }
            }
        }
        if !no_signal && !presented {
            let (cw, ch) = gpu.pipeline.content_size;
            let source_interlaced = self.current_format.interlaced;
            gpu.deinterlace.set_source_interlaced(source_interlaced);
            let ran_deint = gpu.deinterlace.should_run()
                && gpu
                    .deinterlace
                    .prepare(&gpu.device, &gpu.queue, &mut encoder, &gpu.pipeline)
                    .is_some();

            // Only feed interpolate when it is active — otherwise Bob alone must present.
            let interp_frame = if gpu.interpolate.active() {
                if ran_deint {
                    if let Some(tex) = gpu.deinterlace.output_texture() {
                        gpu.interpolate.prepare_from_texture(
                            &gpu.device,
                            &gpu.queue,
                            &mut encoder,
                            tex,
                            cw,
                            ch,
                            new_capture,
                        )
                    } else {
                        None
                    }
                } else {
                    gpu.interpolate.prepare(
                        &gpu.device,
                        &gpu.queue,
                        &mut encoder,
                        &gpu.pipeline,
                        new_capture,
                    )
                }
            } else {
                None
            };

            if let Some(frame_kind) = interp_frame {
                // Count every frame leaving interpolate (real Curr + synthetic Mid).
                // Old Mid-only counter read ~capture fps and looked "broken" vs FPS écran ~2×.
                self.gen_frames_in_window = self.gen_frames_in_window.saturating_add(1);
                if let Some(rgba) = gpu.interpolate.frame_view(frame_kind) {
                    presented = gpu.upscale.encode_from_rgba(
                        &gpu.device,
                        &gpu.queue,
                        &mut encoder,
                        rgba,
                        cw,
                        ch,
                        &view,
                        gpu.config.width,
                        gpu.config.height,
                        clear,
                    );
                    if !presented {
                        gpu.interpolate.present_view(
                            &gpu.device,
                            &gpu.queue,
                            &mut encoder,
                            rgba,
                            (cw, ch),
                            &view,
                            gpu.config.width,
                            gpu.config.height,
                            clear,
                        );
                        presented = true;
                    }
                }
            } else if ran_deint {
                // Bob without interp: present the sampleable deinterlace copy only when
                // this frame actually ran Bob (never a stale buffer after turning Bob off).
                if let Some(rgba) = gpu.deinterlace.output_view() {
                    presented = gpu.upscale.encode_from_rgba(
                        &gpu.device,
                        &gpu.queue,
                        &mut encoder,
                        rgba,
                        cw,
                        ch,
                        &view,
                        gpu.config.width,
                        gpu.config.height,
                        clear,
                    );
                    if !presented {
                        gpu.interpolate.present_view(
                            &gpu.device,
                            &gpu.queue,
                            &mut encoder,
                            rgba,
                            (cw, ch),
                            &view,
                            gpu.config.width,
                            gpu.config.height,
                            clear,
                        );
                        presented = true;
                    }
                }
            } else {
                presented = gpu.upscale.encode(
                    &gpu.device,
                    &gpu.queue,
                    &mut encoder,
                    &gpu.pipeline,
                    &view,
                    gpu.config.width,
                    gpu.config.height,
                    clear,
                );
            }
        }
        if !presented {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("video-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if !no_signal {
                gpu.pipeline.draw(&mut pass);
            }
        }

        let cmd_buffers =
            gpu.egui_renderer
                .update_buffers(&gpu.device, &gpu.queue, &mut encoder, &tris, &screen);

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let mut pass = pass.forget_lifetime();
            gpu.egui_renderer.render(&mut pass, &tris, &screen);
        }

        for id in &full_output.textures_delta.free {
            gpu.egui_renderer.free_texture(id);
        }

        gpu.queue.submit(
            cmd_buffers
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        frame.present();

        let now = Instant::now();
        let frame_time_ns = self
            .last_present_at
            .map(|t| now.duration_since(t).as_nanos() as u64)
            .unwrap_or(0);
        self.last_present_at = Some(now);
        self.metrics.record_present(latency_ns, frame_time_ns);
        let frame_ms = frame_time_ns as f32 / 1_000_000.0;
        let _ = gpu.upscale.observe_frame_ms(frame_ms);
        let _ = gpu.interpolate.observe_frame_ms(frame_ms);
        }

        // GPU borrow ended with the block above; safe to restart capture from UI.
        let (up_auto, interp_auto) = self
            .gpu
            .as_ref()
            .map(|g| {
                (
                    g.upscale.status().block == UpscaleBlockReason::AutoDisabled,
                    g.interpolate.status().block == InterpBlockReason::AutoDisabled,
                )
            })
            .unwrap_or((false, false));
        if up_auto && self.effects_ui.mode != "off" {
            self.effects_ui.mode = "off".into();
            self.show_toast(self.i18n.t("msg_upscale_off_budget"));
            self.persist_ui_prefs();
        }
        if interp_auto && self.effects_ui.interp_mode != "off" {
            self.effects_ui.interp_mode = "off".into();
            self.show_toast(self.i18n.t("msg_interp_off_budget"));
            self.persist_ui_prefs();
        }
        self.apply_ui_actions(actions);
    }
}

fn normalize_vsync_mode(mode: &str) -> String {
    match mode {
        "on" | "On" | "ON" => "on".into(),
        "off" | "Off" | "OFF" => "off".into(),
        _ => "auto".into(),
    }
}

impl App {
    fn restart_discord_presence(&mut self) {
        self.discord.shutdown();
        let id = resolve_app_id(&self.ui_prefs.discord_app_id);
        self.discord = DiscordPresence::start(self.ui_prefs.discord_presence, &id);
        self.overlay.discord_active = self.discord.is_active();
    }

    fn push_discord_activity(&mut self, force: bool) {
        if !self.discord.is_active() {
            return;
        }
        if !force && self.last_discord_push.elapsed() < Duration::from_secs(12) {
            return;
        }
        self.last_discord_push = Instant::now();
        let details = self.overlay.format_label.clone();
        let state = if self.overlay.no_signal {
            self.i18n.t("label_no_signal")
        } else if !self.overlay.label.is_empty() {
            self.overlay.label.clone()
        } else {
            "Capture".into()
        };
        self.discord.update(details, state);
    }
}

fn approx_black(frame: &VideoFrame) -> bool {
    let data = frame.data.as_ref();
    if data.is_empty() {
        return true;
    }
    let step = match frame.pixel_format {
        PixelFormat::Bgra8 => 4,
        PixelFormat::Yuy2 => 2,
        PixelFormat::Nv12 => 1,
        _ => 4,
    };
    let mut sum = 0u64;
    let mut count = 0u64;
    let stride = (step * 64).max(step);
    let limit = data.len().min(frame.width as usize * frame.height as usize * step);
    let mut i = 0;
    while i < limit {
        sum += data[i] as u64;
        count += 1;
        i += stride;
    }
    if count == 0 {
        return true;
    }
    (sum / count) < 8
}

fn adapter_score(info: &wgpu::AdapterInfo) -> i32 {
    let backend = match info.backend {
        // DX12 first on Windows — avoids TikTok/OBS Vulkan layer JSON spam + hitches.
        wgpu::Backend::Dx12 => 400,
        wgpu::Backend::Metal => 400,
        wgpu::Backend::Vulkan => 150,
        wgpu::Backend::Gl => 20,
        _ => 0,
    };
    let power = match info.device_type {
        wgpu::DeviceType::DiscreteGpu => 30,
        wgpu::DeviceType::IntegratedGpu => 10,
        wgpu::DeviceType::VirtualGpu => 5,
        _ => 0,
    };
    backend + power
}

fn pick_adapter(
    instance: &wgpu::Instance,
    surface: &wgpu::Surface<'_>,
) -> Option<wgpu::Adapter> {
    let mut best: Option<(i32, wgpu::Adapter)> = None;
    for adapter in instance.enumerate_adapters(wgpu::Backends::PRIMARY) {
        if !adapter.is_surface_supported(surface) {
            continue;
        }
        let score = adapter_score(&adapter.get_info());
        if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
            best = Some((score, adapter));
        }
    }
    if let Some((_, adapter)) = best {
        return Some(adapter);
    }
    // Fallback: default high-performance request (async path used by callers that await).
    None
}

async fn init_gpu(window: Arc<Window>, prefer_hdr: bool) -> anyhow::Result<GpuState> {
    // With --features dlss prefer DX12 (NGX NR/FG host); else PRIMARY (DX12/VK/GL).
    // DX12 is also preferred for reliable VRR (ALLOW_TEARING) and scRGB HDR.
    let backends = render::preferred_backends();
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends,
        ..Default::default()
    });
    let surface = instance.create_surface(window.clone())?;
    // Prefer DX12 on Windows: Vulkan + overlay layers (TikTok JSON spam) hitch UI
    // and tank 1% lows on Arthur's RX 7800 XT.
    let adapter = match pick_adapter(&instance, &surface) {
        Some(a) => a,
        None => instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| anyhow::anyhow!("no suitable GPU adapter"))?,
    };
    let adapter_info = adapter.get_info();
    let adapter_name = adapter_info.name.clone();
    let backend = adapter_info.backend;
    tracing::info!(
        gpu = %adapter_name,
        backend = ?backend,
        vendor = adapter_info.vendor,
        ?backends,
        build = env!("CAPTUREVIEW_GIT_HASH"),
        capture_wake = env!("CAPTUREVIEW_CAPTURE_WAKE"),
        "GPU adapter selected (DX12 preferred; DLSS builds keep RTX path; AMD uses FSR/NIS + optical FG)"
    );
    if matches!(backend, wgpu::Backend::Gl) {
        tracing::warn!(
            "running on OpenGL — FSR/NIS compute may be limited; DLSS NR/FG need DX12"
        );
    }
    if matches!(backend, wgpu::Backend::Vulkan) {
        tracing::warn!(
            "Vulkan selected — overlay layers may spam logs; DX12 is preferred on Windows for VRR/HDR"
        );
    }

    let (device, queue) = adapter
        .request_device(
            &wgpu::DeviceDescriptor {
                label: Some("captureview"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        )
        .await?;

    let caps = surface.get_capabilities(&adapter);
    // HDR: RGBA16F → scRGB on DX12 when OS Advanced Color is on. Else non-sRGB 8-bit
    // for video (YUV→RGB is display-referred; sRGB targets wash out midtones).
    let format = pick_surface_format(&caps, prefer_hdr);
    tracing::info!(
        ?format,
        srgb = format.is_srgb(),
        prefer_hdr,
        tearing = caps
            .present_modes
            .iter()
            .any(|m| *m == wgpu::PresentMode::Immediate),
        "surface format selected"
    );

    let size = window.inner_size();
    let present_mode = pick_present_mode(true, &caps);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &config);
    let present_caps = PresentCaps::from_surface(&caps, present_mode, format);

    let pipeline = VideoPipeline::new(&device, format);
    let upscale = UpscaleEngine::new(&device, &queue, format, &adapter, backend);
    let interpolate = InterpolateEngine::new(&device, format, &adapter);
    let deinterlace = DeinterlaceEngine::new(&device, &adapter);

    let egui_ctx = egui::Context::default();
    let egui_state = EguiWinitState::new(
        egui_ctx.clone(),
        egui_ctx.viewport_id(),
        window.as_ref(),
        Some(window.scale_factor() as f32),
        None,
        None,
    );
    let egui_renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);

    Ok(GpuState {
        surface,
        device,
        queue,
        adapter,
        backend,
        config,
        pipeline,
        upscale,
        interpolate,
        deinterlace,
        window,
        egui_renderer,
        egui_state,
        egui_ctx,
        adapter_name,
        present_caps,
    })
}
