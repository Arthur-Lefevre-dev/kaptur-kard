//! winit + wgpu + egui runtime with Mock and platform video sources.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use audio::{
    list_inputs, list_outputs, suggest_input_for_video, AudioDeviceInfo, AudioEngine,
    AudioEngineConfig, NONE_INPUT_ID,
};
use capture::{
    hz_label, modes, CaptureError, CaptureMode, DeviceInfo, FormatDesc, FrameReceiver,
    MockVideoSource, PixelFormat, Rational, StreamHealth, StreamStatus, VideoFrame, VideoSource,
};
use egui_wgpu::ScreenDescriptor;
use egui_winit::State as EguiWinitState;
use metrics::{sample_process, SharedMetrics};
use render::pipeline::VideoPipeline;
use render::upload::upload_frame;
use render::{
    is_native_or_downscale, resolve_color_convert, ColorMatrixMode, ColorRangeMode,
    DeinterlaceEngine, DeinterlaceMode, InterpBlockReason, InterpMode,
    InterpolateEngine, UpscaleBlockReason, UpscaleEngine, UpscaleMode,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::{Window, WindowId};

use crate::discord_presence::{resolve_app_id, DiscordPresence};
use crate::i18n::I18n;
use crate::prefs::{
    load_audio_prefs, load_ui_prefs, save_audio_prefs, save_ui_prefs, AudioPrefs, UiPrefs,
};
use crate::ui::{
    self, AudioUiState, ColorUiState, EffectsUiState, OverlayState, UiChrome, UiSelection,
};

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
}

/// Presence-only check interval. Full MF probe runs only when the id set changes.
const HOTPLUG_INTERVAL: Duration = Duration::from_secs(5);

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
    toast_until: Option<Instant>,
    support_log: Vec<String>,
    process_sample_at: Instant,
    lag_test_started: Option<Instant>,
    lag_flash_until: Option<Instant>,
    last_hotplug_at: Instant,
    /// True while stop/start runs — keep showing the last GPU frame (audio stays up).
    capture_restarting: bool,
    prefs_dirty: bool,
    prefs_dirty_at: Instant,
    /// Per-device last FormatDesc for reconnect-after-unplug.
    device_settings: HashMap<String, FormatDesc>,
    discord: DiscordPresence,
    last_discord_push: Instant,
    logo_tex: Option<egui::TextureHandle>,
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
        let i18n = I18n::load();
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
            overlay: OverlayState::default(),
            fps_window_start: Instant::now(),
            frames_in_window: 0,
            captures_in_window: 0,
            gen_frames_in_window: 0,
            last_frame_at: Instant::now(),
            last_present_at: None,
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
                deinterlace_mode: ui_prefs.deinterlace_mode.clone(),
                source_interlaced: format.interlaced,
                vsync_mode: normalize_vsync_mode(&ui_prefs.vsync_mode),
                ..EffectsUiState::default()
            },
            color_ui,
            i18n,
            chrome,
            ui_prefs,
            last_pointer_at: Instant::now(),
            toast_until: None,
            support_log: Vec::new(),
            process_sample_at: Instant::now(),
            lag_test_started: None,
            lag_flash_until: None,
            last_hotplug_at: Instant::now(),
            capture_restarting: false,
            prefs_dirty: false,
            prefs_dirty_at: Instant::now(),
            device_settings: HashMap::new(),
            discord: DiscordPresence::start(false, ""),
            last_discord_push: Instant::now()
                .checked_sub(Duration::from_secs(60))
                .unwrap_or_else(Instant::now),
            logo_tex: None,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }

        let mut attrs = Window::default_attributes()
            .with_title(self.i18n.t("app_title"))
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        if let Some(icon) = crate::brand::window_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("create window"),
        );

        let mut gpu = pollster::block_on(init_gpu(window.clone())).expect("init gpu");
        // Respect OS DPI / egui style scaling.
        let scale = window.scale_factor() as f32;
        gpu.egui_ctx.set_pixels_per_point(scale);
        gpu.upscale
            .set_mode(UpscaleMode::from_str_lossy(&self.effects_ui.mode));
        gpu.upscale.set_sharpness(self.effects_ui.sharpness);
        gpu.interpolate
            .set_mode(InterpMode::from_str_lossy(&self.effects_ui.interp_mode));
        gpu.deinterlace
            .set_mode(DeinterlaceMode::from_str_lossy(&self.effects_ui.deinterlace_mode));
        gpu.deinterlace
            .set_source_interlaced(self.current_format.interlaced);
        self.effects_ui.vendor = gpu.upscale.vendor.clone();
        self.effects_ui.backend = gpu.upscale.backend.clone();
        self.effects_ui.compute_ok = gpu.upscale.compute_ok;
        self.effects_ui.interp_compute_ok = gpu.interpolate.compute_ok;
        self.gpu = Some(gpu);
        self.apply_color_convert();
        self.apply_present_mode_for_profile();

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
        let period = Duration::from_secs_f64(1.0 / self.target_display_hz());
        let redraw_due = self.last_redraw_at.elapsed() >= period;

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
        self.poll_hotplug();
        self.flush_prefs_if_due();
        self.push_discord_activity(false);

        // Always honor the display FPS cap — never let has_pending alone bypass it
        // (Arthur saw 240 FPS écran with limit 120 when pending woke every tick).
        let period = Duration::from_secs_f64(1.0 / self.target_display_hz());
        let elapsed = self.last_redraw_at.elapsed();
        let due = elapsed >= period;
        if due {
            if let Some(gpu) = self.gpu.as_ref() {
                gpu.window.request_redraw();
            }
            // Park until RedrawRequested runs. A 1ms WaitUntil here busy-looped
            // under NoVsync (Arthur: FPS écran ~101, CPU ~82%, 1% low ~31).
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        // Wake when the next present is legal under the FPS cap.
        let wait = period
            .saturating_sub(elapsed)
            .max(Duration::from_micros(200))
            .min(Duration::from_millis(33));
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + wait));
    }
}

impl App {
    fn stop_source(&mut self) {
        if let Some(mut source) = self.source.take() {
            let _ = source.stop();
        }
        self.running = false;
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
        let auto = if self.effects_ui.interp_mode != "off" {
            (capture_hz * 2.0).min(240.0)
        } else {
            capture_hz
        };
        let hz = match limit {
            0 => auto,
            9999 => 360.0,
            n => n as f64,
        };
        hz.clamp(24.0, 360.0)
    }

    /// Apply swapchain present mode from the VSync pref (auto/on/off).
    /// Auto: Auto/Game → NoVsync (avoids Fifo phase-beat with software pacing);
    /// Quality/Economy → VSync. Explicit on/off overrides the profile.
    fn apply_present_mode_for_profile(&mut self) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let mode = normalize_vsync_mode(&self.ui_prefs.vsync_mode);
        let profile_low_latency = matches!(
            self.selection.mode,
            CaptureMode::Game | CaptureMode::Auto
        );
        let use_vsync = match mode.as_str() {
            "on" => true,
            "off" => false,
            _ => !profile_low_latency,
        };
        let want = if use_vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        let latency = if use_vsync { 2 } else { 1 };
        if gpu.config.present_mode == want && gpu.config.desired_maximum_frame_latency == latency {
            return;
        }
        gpu.config.present_mode = want;
        gpu.config.desired_maximum_frame_latency = latency;
        gpu.surface.configure(&gpu.device, &gpu.config);
        tracing::info!(?want, latency, vsync = %mode, "present mode updated");
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

    /// Presence poll: cheap id scan; full MF format probe only when devices appear/disappear.
    /// Re-opening the active NZXT every 2s was Arthur's periodic frametime spike.
    fn poll_hotplug(&mut self) {
        if self.last_hotplug_at.elapsed() < HOTPLUG_INTERVAL {
            return;
        }
        self.last_hotplug_at = Instant::now();

        let Ok(presence) = capture::list_device_presence() else {
            return;
        };
        let ids_after: std::collections::HashSet<String> =
            presence.iter().map(|(id, _)| id.clone()).collect();
        let ids_before: std::collections::HashSet<String> = self
            .devices
            .iter()
            .filter(|d| d.backend != "mock")
            .map(|d| d.id.as_str().to_string())
            .collect();
        if ids_after == ids_before {
            return;
        }

        tracing::info!(
            before = ids_before.len(),
            after = ids_after.len(),
            "device set changed — full MF re-probe"
        );
        let Ok(real) = capture::list_devices() else {
            return;
        };
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
            self.selection.width = w;
            self.selection.height = h;
            let caps = &self.devices[self.selection.device_idx].formats;
            let rates = modes::rates_for_resolution(caps, w, h);
            if let Some(r) = rates.first() {
                self.selection.rate = *r;
            }
            if let Some(fmt) = modes::pick_format_at(caps, w, h, self.selection.rate) {
                let msg = self.i18n.t("err_resolution");
                let _ = self.restart_with_fallback(fmt, &msg);
            }
        }

        if let Some(rate) = actions.rate_changed {
            self.selection.rate = rate;
            let caps = &self.devices[self.selection.device_idx].formats;
            if let Some(fmt) =
                modes::pick_format_at(caps, self.selection.width, self.selection.height, rate)
            {
                let msg = self.i18n.t("err_rate");
                let _ = self.restart_with_fallback(fmt, &msg);
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
            self.last_pointer_at = Instant::now();
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
            self.last_pointer_at = Instant::now();
        }
        if actions.reset_auto {
            self.reset_to_auto();
            self.effects_ui.mode = "off".into();
            self.effects_ui.sharpness = 0.4;
            self.effects_ui.interp_mode = "off".into();
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
        if let Some(mode) = actions.interp_mode {
            self.effects_ui.interp_mode = mode.to_string();
            self.ui_prefs.interp_mode = mode.to_string();
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.interpolate
                    .set_mode(InterpMode::from_str_lossy(mode));
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
    }

    fn persist_ui_prefs(&mut self) {
        self.ui_prefs.wizard_step = self.chrome.wizard_step;
        self.ui_prefs.upscale_mode = self.effects_ui.mode.clone();
        self.ui_prefs.sharpness = self.effects_ui.sharpness;
        self.ui_prefs.interp_mode = self.effects_ui.interp_mode.clone();
        self.ui_prefs.deinterlace_mode = self.effects_ui.deinterlace_mode.clone();
        self.ui_prefs.vsync_mode = normalize_vsync_mode(&self.effects_ui.vsync_mode);
        // Debounce disk writes — sync save on every slider tick froze the picture.
        self.prefs_dirty = true;
        self.prefs_dirty_at = Instant::now();
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
            self.ui_prefs.interp_mode = self.effects_ui.interp_mode.clone();
            self.ui_prefs.deinterlace_mode = self.effects_ui.deinterlace_mode.clone();
            self.ui_prefs.vsync_mode = normalize_vsync_mode(&self.effects_ui.vsync_mode);
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
        } else {
            self.effects_ui.native_res = native;
            self.effects_ui.vsync_mode = normalize_vsync_mode(&self.ui_prefs.vsync_mode);
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
        let report = format!(
            "CaptureView support report\n\
             locale={:?}\n\
             device={}\n\
             format={}x{} {:?} @ {:?}\n\
             audio_in={} audio_out={}\n\
             fps={:.1} capture_fps={:.1} frame_gen_fps={:.1} display_limit={} vsync={} pipeline_delay_ms={:.1}\n\
             dropped={} 1%low={:.1} gpu={}\n\
             log_level={} log_file={}\n\
             -- support events --\n{}\n\
             -- log tail --\n{}\n",
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
            self.overlay.pipeline_delay_ms,
            self.overlay.dropped_frames,
            self.overlay.one_percent_low_fps,
            self.overlay.gpu_name,
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
        self.overlay.toast = msg;
        self.toast_until = Some(Instant::now() + Duration::from_secs(3));
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
        // Hard drop surplus RedrawRequested (egui may queue many under NoVsync).
        if self.last_redraw_at.elapsed() + Duration::from_micros(500) < period {
            return;
        }
        self.last_redraw_at = Instant::now();
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
        self.overlay.discord_presence = self.ui_prefs.discord_presence;
        self.overlay.discord_app_id = self.ui_prefs.discord_app_id.clone();
        self.overlay.discord_active = self.discord.is_active();
        self.overlay.log_level = crate::logging::normalize_level(&self.ui_prefs.log_level).to_string();
        self.overlay.log_path = crate::logging::log_file_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();

        // Refresh process CPU/RAM ~2 Hz (UI thread only).
        if self.process_sample_at.elapsed() >= Duration::from_millis(500) {
            let sample = sample_process();
            self.overlay.ram_mb = sample.rss_bytes as f32 / (1024.0 * 1024.0);
            self.overlay.cpu_one_core = sample.cpu_one_core;
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

        let snap = self.metrics.snapshot();
        self.overlay.pipeline_delay_ms = snap.last_latency_ms;
        self.overlay.one_percent_low_fps = snap.one_percent_low_fps;
        self.overlay.dropped_frames = snap.frames_dropped;
        self.overlay.frame_times_ms = snap
            .frame_times_ns
            .iter()
            .map(|ns| *ns as f32 / 1_000_000.0)
            .collect();

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
        // Pipeline delay only updates on a fresh capture; frame-time / 1% low are
        // recorded after swapchain present so they match FPS écran (not coalesce gaps).
        let mut latency_ns = self.metrics.last_latency_ns.load(Ordering::Relaxed);
        if let Some(frame) = latest {
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
        let mut presented = false;
        if !no_signal {
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

async fn init_gpu(window: Arc<Window>) -> anyhow::Result<GpuState> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..Default::default()
    });
    let surface = instance.create_surface(window.clone())?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })
        .await
        .ok_or_else(|| anyhow::anyhow!("no suitable GPU adapter"))?;
    let adapter_info = adapter.get_info();
    let adapter_name = adapter_info.name.clone();
    let backend = adapter_info.backend;
    tracing::info!(
        gpu = %adapter_name,
        backend = ?backend,
        vendor = adapter_info.vendor,
        "GPU adapter selected (DX12/Vulkan preferred; GL is a fallback path)"
    );
    if matches!(backend, wgpu::Backend::Gl) {
        tracing::warn!(
            "running on OpenGL — FSR/NIS compute may be limited; prefer DX12 or Vulkan"
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
    // Prefer non-sRGB for video: YUV→RGB yields display-referred R'G'B'. Writing that
    // into an sRGB target (GPU assumes linear) lifts midtones and looks washed out.
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| !f.is_srgb())
        .or_else(|| caps.formats.iter().copied().find(|f| f.is_srgb()))
        .unwrap_or(caps.formats[0]);
    tracing::info!(?format, srgb = format.is_srgb(), "surface format selected");

    let size = window.inner_size();
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::AutoVsync,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &config);

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
    })
}
