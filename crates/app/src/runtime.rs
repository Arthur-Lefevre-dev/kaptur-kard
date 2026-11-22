//! winit + wgpu + egui runtime with Mock and Media Foundation sources.

use std::sync::Arc;
use std::time::{Duration, Instant};

use audio::{
    list_inputs, list_outputs, suggest_input_for_video, AudioDeviceInfo, AudioEngine,
    AudioEngineConfig, NONE_INPUT_ID,
};
use capture::{
    hz_label, modes, CaptureMode, DeviceInfo, FormatDesc, FrameReceiver, MockVideoSource,
    PixelFormat, Rational, VideoFrame, VideoSource,
};
use egui_wgpu::ScreenDescriptor;
use egui_winit::State as EguiWinitState;
use metrics::{sample_process, SharedMetrics};
use render::pipeline::VideoPipeline;
use render::upload::upload_frame;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::i18n::I18n;
use crate::prefs::{
    load_audio_prefs, load_ui_prefs, save_audio_prefs, save_ui_prefs, AudioPrefs, UiPrefs,
};
use crate::ui::{self, AudioUiState, MetricsMode, OverlayState, UiChrome, UiSelection};

enum ActiveSource {
    Mock(MockVideoSource),
    #[cfg(windows)]
    Mf(capture::MfVideoSource),
}

impl ActiveSource {
    fn info(&self) -> &DeviceInfo {
        match self {
            Self::Mock(s) => s.info(),
            #[cfg(windows)]
            Self::Mf(s) => s.info(),
        }
    }

    fn start(&mut self, selected: &FormatDesc) -> Result<(), capture::CaptureError> {
        match self {
            Self::Mock(s) => s.start(selected),
            #[cfg(windows)]
            Self::Mf(s) => s.start(selected),
        }
    }

    fn stop(&mut self) -> Result<(), capture::CaptureError> {
        match self {
            Self::Mock(s) => s.stop(),
            #[cfg(windows)]
            Self::Mf(s) => s.stop(),
        }
    }

    fn receiver(&self) -> FrameReceiver {
        match self {
            Self::Mock(s) => s.receiver(),
            #[cfg(windows)]
            Self::Mf(s) => s.receiver(),
        }
    }
}

struct GpuState {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: VideoPipeline,
    window: Arc<Window>,
    egui_renderer: egui_wgpu::Renderer,
    egui_state: EguiWinitState,
    egui_ctx: egui::Context,
    adapter_name: String,
}

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
    last_frame_at: Instant,
    last_present_at: Option<Instant>,
    last_channel_drops: u64,
    black_since: Option<Instant>,
    running: bool,
    audio: AudioEngine,
    audio_inputs: Vec<AudioDeviceInfo>,
    audio_outputs: Vec<AudioDeviceInfo>,
    audio_ui: AudioUiState,
    i18n: I18n,
    chrome: UiChrome,
    ui_prefs: UiPrefs,
    last_pointer_at: Instant,
    toast_until: Option<Instant>,
    support_log: Vec<String>,
    process_sample_at: Instant,
    lag_test_started: Option<Instant>,
    lag_flash_until: Option<Instant>,
}

impl Default for App {
    fn default() -> Self {
        let mock_info = MockVideoSource::device_info();
        let mut devices = vec![mock_info.clone()];
        #[cfg(windows)]
        match capture::enumerate_devices() {
            Ok(mut real) => {
                tracing::info!(count = real.len(), "MF devices enumerated");
                devices.append(&mut real);
            }
            Err(e) => tracing::warn!(error = %e, "MF enumeration failed; mock only"),
        }

        let device_idx = devices
            .iter()
            .enumerate()
            .filter(|(_, d)| d.backend != "mock" && !d.formats.is_empty())
            .max_by_key(|(_, d)| capture::device_capture_score(&d.display_name))
            .map(|(i, _)| i)
            .unwrap_or(0);

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
        let ui_prefs = load_ui_prefs();
        let i18n = I18n::load();
        let mut chrome = UiChrome::default();
        chrome.wizard_open = !ui_prefs.wizard_done;
        chrome.wizard_step = ui_prefs.wizard_step.min(2);
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
            current_format: format,
            metrics: SharedMetrics::new(),
            overlay: OverlayState::default(),
            fps_window_start: Instant::now(),
            frames_in_window: 0,
            captures_in_window: 0,
            last_frame_at: Instant::now(),
            last_present_at: None,
            last_channel_drops: 0,
            black_since: None,
            running: false,
            audio: AudioEngine::new(),
            audio_inputs,
            audio_outputs,
            audio_ui,
            i18n,
            chrome,
            ui_prefs,
            last_pointer_at: Instant::now(),
            toast_until: None,
            support_log: Vec::new(),
            process_sample_at: Instant::now(),
            lag_test_started: None,
            lag_flash_until: None,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }

        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title(self.i18n.t("app_title"))
                        .with_inner_size(LogicalSize::new(1280.0, 720.0)),
                )
                .expect("create window"),
        );

        let gpu = pollster::block_on(init_gpu(window.clone())).expect("init gpu");
        // Respect OS DPI / egui style scaling.
        let scale = window.scale_factor() as f32;
        gpu.egui_ctx.set_pixels_per_point(scale);
        self.gpu = Some(gpu);

        if let Err(e) = self.open_and_start(self.selection.device_idx, self.current_format.clone()) {
            tracing::error!(error = %e, "failed to start capture; falling back to mock");
            self.push_support(format!("start failed: {e}"));
            self.selection.device_idx = 0;
            let fmt = self.devices[0].formats[0].clone();
            self.current_format = fmt.clone();
            self.selection.width = fmt.width;
            self.selection.height = fmt.height;
            self.selection.rate = fmt.frame_rates[0];
            let _ = self.open_and_start(0, fmt);
            self.overlay.status = self.i18n.t("err_device_start");
        }

        self.restart_audio();
        if self.devices.iter().all(|d| d.backend == "mock") {
            self.overlay.status = self.i18n.t("status_no_card");
        }
        tracing::info!("CaptureView ready");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };

        let egui_response = gpu.egui_state.on_window_event(&gpu.window, &event);
        if egui_response.repaint {
            gpu.window.request_redraw();
        }

        match event {
            WindowEvent::CloseRequested => {
                self.persist_audio_prefs();
                self.persist_ui_prefs();
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

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(gpu) = self.gpu.as_ref() {
            gpu.window.request_redraw();
        }
    }
}

impl App {
    fn stop_source(&mut self) {
        if let Some(mut source) = self.source.take() {
            let _ = source.stop();
        }
        self.running = false;
    }

    fn open_and_start(
        &mut self,
        device_idx: usize,
        format: FormatDesc,
    ) -> Result<(), capture::CaptureError> {
        self.stop_source();
        let info = self.devices[device_idx].clone();
        let mut source = if info.backend == "mock" {
            ActiveSource::Mock(MockVideoSource::new())
        } else {
            #[cfg(windows)]
            {
                ActiveSource::Mf(capture::MfVideoSource::open(info)?)
            }
            #[cfg(not(windows))]
            {
                return Err(capture::CaptureError::Unsupported(
                    "MF only on Windows".into(),
                ));
            }
        };
        source.start(&format)?;
        self.source = Some(source);
        self.current_format = format;
        self.running = true;
        self.last_frame_at = Instant::now();
        self.black_since = None;
        Ok(())
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
                self.overlay.status = on_fail_msg.to_string();
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
            let caps = &self.devices[self.selection.device_idx].formats;
            if let Some(fmt) = modes::select_format(mode, caps) {
                let msg = self.i18n.t("err_mode");
                if !self.restart_with_fallback(fmt, &msg) {
                    self.selection.mode = previous_mode;
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
            self.chrome.toolbar_visible = true;
            self.last_pointer_at = Instant::now();
        }
        if actions.close_settings {
            self.chrome.settings_open = false;
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
        }
        if actions.retry {
            let fmt = self.current_format.clone();
            let idx = self.selection.device_idx;
            let msg = self.i18n.t("err_device_start");
            let _ = self.open_and_start(idx, fmt).map_err(|e| {
                self.overlay.status = msg;
                self.push_support(format!("retry failed: {e}"));
            });
            self.restart_audio();
        }
        if actions.copy_support {
            self.copy_support_report();
        }
        if actions.wizard_next {
            self.chrome.wizard_step = (self.chrome.wizard_step + 1).min(2);
            self.persist_ui_prefs();
        }
        if actions.wizard_back {
            self.chrome.wizard_step = self.chrome.wizard_step.saturating_sub(1);
            self.persist_ui_prefs();
        }
        if actions.wizard_skip || actions.wizard_finish {
            self.chrome.wizard_open = false;
            self.ui_prefs.wizard_done = true;
            self.persist_ui_prefs();
        }
        if actions.cycle_metrics {
            self.overlay.metrics_mode = self.overlay.metrics_mode.next();
            // Keep toolbar visible briefly so the user sees the mode change.
            self.chrome.toolbar_visible = true;
            self.last_pointer_at = Instant::now();
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
    }

    fn persist_ui_prefs(&mut self) {
        self.ui_prefs.wizard_step = self.chrome.wizard_step;
        save_ui_prefs(&self.ui_prefs);
    }

    fn push_support(&mut self, line: String) {
        if self.support_log.len() > 200 {
            self.support_log.remove(0);
        }
        self.support_log.push(line);
    }

    fn copy_support_report(&mut self) {
        let report = format!(
            "CaptureView support report\n\
             locale={:?}\n\
             device={}\n\
             format={}x{} {:?} @ {:?}\n\
             audio_in={} audio_out={}\n\
             fps={:.1} capture_fps={:.1} pipeline_delay_ms={:.1}\n\
             dropped={} 1%low={:.1} gpu={}\n\
             -- log --\n{}\n",
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
            self.overlay.pipeline_delay_ms,
            self.overlay.dropped_frames,
            self.overlay.one_percent_low_fps,
            self.overlay.gpu_name,
            self.support_log.join("\n")
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
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        self.chrome.fullscreen = !self.chrome.fullscreen;
        gpu.window.set_fullscreen(if self.chrome.fullscreen {
            Some(winit::window::Fullscreen::Borderless(None))
        } else {
            None
        });
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

        let no_signal = self.last_frame_at.elapsed() > Duration::from_secs(1);
        let soft_black = self
            .black_since
            .map(|t| t.elapsed() > Duration::from_secs(3))
            .unwrap_or(false)
            && !no_signal;

        self.frames_in_window += 1;
        let elapsed = self.fps_window_start.elapsed().as_secs_f32();
        if elapsed >= 0.5 {
            self.overlay.fps = self.frames_in_window as f32 / elapsed;
            self.overlay.capture_fps = self.captures_in_window as f32 / elapsed;
            self.frames_in_window = 0;
            self.captures_in_window = 0;
            self.fps_window_start = Instant::now();
        }

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
        // Hide floating toolbar after 3s without pointer motion (unless settings/metrics open).
        if !self.chrome.settings_open
            && !self.chrome.wizard_open
            && self.overlay.metrics_mode == MetricsMode::Off
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
        // Video presents every tick; egui widgets only draw when chrome/overlay is active.

        let mut actions = ui::UiActions::default();
        {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };

        if let Some(frame) = latest {
            let latency_ns = frame.captured_at.elapsed().as_nanos() as u64;
            if upload_frame(&gpu.device, &gpu.queue, &mut gpu.pipeline, &frame) {
                let now = Instant::now();
                let frame_time_ns = self
                    .last_present_at
                    .map(|t| now.duration_since(t).as_nanos() as u64)
                    .unwrap_or(0);
                self.last_present_at = Some(now);
                self.metrics.record_present(latency_ns, frame_time_ns);
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
                },
            );
        });
        gpu.egui_state
            .handle_platform_output(&gpu.window, full_output.platform_output);

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

        {
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
        }

        // GPU borrow ended with the block above; safe to restart capture from UI.
        self.apply_ui_actions(actions);
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
    let adapter_name = adapter.get_info().name;

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
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or(caps.formats[0]);

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
        window,
        egui_renderer,
        egui_state,
        egui_ctx,
        adapter_name,
    })
}
