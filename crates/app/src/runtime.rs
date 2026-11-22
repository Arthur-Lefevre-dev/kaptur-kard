//! winit + wgpu + egui runtime wired to MockVideoSource.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use capture::{MockVideoSource, VideoSource};
use egui_wgpu::ScreenDescriptor;
use egui_winit::State as EguiWinitState;
use metrics::SharedMetrics;
use render::pipeline::VideoPipeline;
use render::upload::upload_frame;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::ui::{self, OverlayState};

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
}

pub struct App {
    gpu: Option<GpuState>,
    source: Option<MockVideoSource>,
    metrics: Arc<SharedMetrics>,
    overlay: OverlayState,
    fps_window_start: Instant,
    frames_in_window: u32,
    last_frame_size: (u32, u32),
}

impl Default for App {
    fn default() -> Self {
        Self {
            gpu: None,
            source: None,
            metrics: SharedMetrics::new(),
            overlay: OverlayState::default(),
            fps_window_start: Instant::now(),
            frames_in_window: 0,
            last_frame_size: (0, 0),
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
                        .with_title("CaptureView — Mock")
                        .with_inner_size(LogicalSize::new(1280.0, 720.0)),
                )
                .expect("create window"),
        );

        let gpu = pollster::block_on(init_gpu(window.clone())).expect("init gpu");

        let mut source = MockVideoSource::new();
        let format = source.info().formats[0].clone();
        source.start(&format).expect("start mock source");

        self.source = Some(source);
        self.gpu = Some(gpu);
        tracing::info!("CaptureView window ready (MockVideoSource 1080p60)");
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
                if let Some(mut source) = self.source.take() {
                    let _ = source.stop();
                }
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
    fn redraw(&mut self) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };

        // Drain drop-oldest bus; keep newest frame only.
        let mut latest = None;
        if let Some(source) = self.source.as_ref() {
            let rx = source.receiver();
            while let Ok(frame) = rx.try_recv() {
                self.metrics
                    .frames_captured
                    .fetch_add(1, Ordering::Relaxed);
                latest = Some(frame);
            }
        }

        if let Some(frame) = latest {
            self.last_frame_size = (frame.width, frame.height);
            upload_frame(&gpu.device, &gpu.queue, &mut gpu.pipeline, &frame);
            self.metrics
                .frames_presented
                .fetch_add(1, Ordering::Relaxed);
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

        // egui overlay metrics (copied so the GPU borrow stays exclusive).
        self.frames_in_window += 1;
        let elapsed = self.fps_window_start.elapsed().as_secs_f32();
        if elapsed >= 0.5 {
            self.overlay.fps = self.frames_in_window as f32 / elapsed;
            self.frames_in_window = 0;
            self.fps_window_start = Instant::now();
        }
        self.overlay.frame_w = self.last_frame_size.0;
        self.overlay.frame_h = self.last_frame_size.1;
        self.overlay.label = "Mock".into();
        let overlay = OverlayState {
            fps: self.overlay.fps,
            label: self.overlay.label.clone(),
            frame_w: self.overlay.frame_w,
            frame_h: self.overlay.frame_h,
        };

        let raw_input = gpu.egui_state.take_egui_input(&gpu.window);
        let full_output = gpu.egui_ctx.run(raw_input, |ctx| {
            ui::draw_overlay(ctx, &overlay);
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
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.02,
                            g: 0.02,
                            b: 0.02,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            gpu.pipeline.draw(&mut pass);
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
            // egui-wgpu 0.31 expects RenderPass<'static>; wgpu 24 provides forget_lifetime.
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
    })
}
