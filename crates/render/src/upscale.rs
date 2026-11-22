//! FSR 1 (EASU+RCAS) and NIS upscaling via WGSL compute.
//!
//! Runs after color conversion, before present. Textures are reused across frames.
//! Shader ports: see `licenses/AMD-FSR1-MIT.txt` and `licenses/NVIDIA-NIS-MIT.txt`.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::pipeline::VideoPipeline;
use crate::present::{compute_letterbox, LetterboxUniforms};

/// Frames over budget before auto-disable.
const OVER_BUDGET_LIMIT: u32 = 45;
/// Display present interval is not GPU cost — only trip on real stalls (>1 frame @ 30 Hz).
const FRAME_BUDGET_MS: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpscaleMode {
    Off,
    Fsr1,
    Nis,
}

impl UpscaleMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Fsr1 => "fsr1",
            Self::Nis => "nis",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "fsr1" | "Fsr1" | "FSR1" => Self::Fsr1,
            "nis" | "Nis" | "NIS" => Self::Nis,
            _ => Self::Off,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpscaleBlockReason {
    None,
    /// Capture already matches (or exceeds) the screen content size.
    NativeResolution,
    /// Adapter downlevel flags report no compute shaders.
    NoCompute,
    /// Repeatedly exceeded frame budget.
    AutoDisabled,
}

#[derive(Clone, Debug)]
pub struct UpscaleStatus {
    pub mode: UpscaleMode,
    pub sharpness: f32,
    pub compute_ok: bool,
    pub block: UpscaleBlockReason,
    pub vendor: String,
    pub backend: String,
    pub last_gpu_ms: f32,
    pub over_budget_streak: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct EasuGpu {
    con0: [f32; 4],
    con1: [f32; 4],
    con2: [f32; 4],
    con3: [f32; 4],
    out_size: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RcasGpu {
    con: [f32; 4],
    size: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct NisGpu {
    scale: [f32; 4],
    src_norm: [f32; 4],
    dst_size: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PresentGpu {
    scale_offset: [f32; 4],
    pad: [u32; 4],
}

struct TexSlot {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

pub struct UpscaleEngine {
    pub mode: UpscaleMode,
    /// 0.0 = soft … 1.0 = sharp (mapped to FSR stops / NIS strength).
    pub sharpness: f32,
    pub compute_ok: bool,
    pub block: UpscaleBlockReason,
    pub vendor: String,
    pub backend: String,
    pub last_gpu_ms: f32,
    over_budget_streak: u32,
    auto_disabled: bool,

    sampler: wgpu::Sampler,
    easu_pipeline: Option<wgpu::ComputePipeline>,
    rcas_pipeline: Option<wgpu::ComputePipeline>,
    nis_pipeline: Option<wgpu::ComputePipeline>,
    easu_bgl: wgpu::BindGroupLayout,
    rcas_bgl: wgpu::BindGroupLayout,
    nis_bgl: wgpu::BindGroupLayout,
    easu_buf: wgpu::Buffer,
    rcas_buf: wgpu::Buffer,
    nis_buf: wgpu::Buffer,

    convert_pipeline: wgpu::RenderPipeline,
    convert_bgl: wgpu::BindGroupLayout,
    present_pipeline: wgpu::RenderPipeline,
    present_bgl: wgpu::BindGroupLayout,
    present_buf: wgpu::Buffer,
    present_sampler: wgpu::Sampler,

    rgba_src: Option<TexSlot>,
    mid: Option<TexSlot>,
    rgba_dst: Option<TexSlot>,
}

impl UpscaleEngine {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        adapter: &wgpu::Adapter,
        backend: wgpu::Backend,
    ) -> Self {
        let info = adapter.get_info();
        let vendor = info.name.clone();
        let backend_name = format!("{backend:?}");
        let down = adapter.get_downlevel_capabilities();
        let compute_ok = down.flags.contains(wgpu::DownlevelFlags::COMPUTE_SHADERS);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("upscale-samp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let present_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("upscale-present-samp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let easu_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("easu-bgl"),
            entries: &[
                tex_sample_entry(0),
                sampler_entry(1),
                storage_tex_entry(2),
                uniform_entry(3, wgpu::ShaderStages::COMPUTE),
            ],
        });
        let rcas_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rcas-bgl"),
            entries: &[
                tex_sample_entry(0),
                storage_tex_entry(1),
                uniform_entry(2, wgpu::ShaderStages::COMPUTE),
            ],
        });
        let nis_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nis-bgl"),
            entries: &[
                tex_sample_entry(0),
                sampler_entry(1),
                storage_tex_entry(2),
                uniform_entry(3, wgpu::ShaderStages::COMPUTE),
            ],
        });

        let easu_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("easu-ub"),
            size: std::mem::size_of::<EasuGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let rcas_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rcas-ub"),
            size: std::mem::size_of::<RcasGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let nis_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nis-ub"),
            size: std::mem::size_of::<NisGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let present_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("upscale-present-ub"),
            contents: bytemuck::bytes_of(&PresentGpu {
                scale_offset: [1.0, 1.0, 0.0, 0.0],
                pad: [0; 4],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let (easu_pipeline, rcas_pipeline, nis_pipeline) = if compute_ok {
            let easu_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("fsr1-easu"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../shaders/fsr1_easu.wgsl").into(),
                ),
            });
            let rcas_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("fsr1-rcas"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../shaders/fsr1_rcas.wgsl").into(),
                ),
            });
            let nis_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("nis-scaler"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../shaders/nis_scaler.wgsl").into(),
                ),
            });
            let easu_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("easu-pl"),
                bind_group_layouts: &[&easu_bgl],
                push_constant_ranges: &[],
            });
            let rcas_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("rcas-pl"),
                bind_group_layouts: &[&rcas_bgl],
                push_constant_ranges: &[],
            });
            let nis_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("nis-pl"),
                bind_group_layouts: &[&nis_bgl],
                push_constant_ranges: &[],
            });
            (
                Some(device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("easu-pipe"),
                    layout: Some(&easu_pl),
                    module: &easu_mod,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    cache: None,
                })),
                Some(device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("rcas-pipe"),
                    layout: Some(&rcas_pl),
                    module: &rcas_mod,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    cache: None,
                })),
                Some(device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("nis-pipe"),
                    layout: Some(&nis_pl),
                    module: &nis_mod,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    cache: None,
                })),
            )
        } else {
            tracing::warn!(
                vendor = %vendor,
                backend = %backend_name,
                "compute shaders unavailable; FSR/NIS disabled"
            );
            (None, None, None)
        };

        // Convert pass: reuse fullscreen.wgsl into RGBA8 (identity letterbox via params).
        let convert_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("convert-bgl"),
            entries: &[
                frag_tex(0),
                frag_tex(1),
                frag_samp(2),
                uniform_entry(3, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let convert_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("convert-fullscreen"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../shaders/fullscreen.wgsl").into(),
            ),
        });
        let convert_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("convert-pl"),
            bind_group_layouts: &[&convert_bgl],
            push_constant_ranges: &[],
        });
        let convert_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("convert-rgba"),
            layout: Some(&convert_pl),
            vertex: wgpu::VertexState {
                module: &convert_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &convert_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let present_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("upscale-present-bgl"),
            entries: &[
                frag_tex(0),
                frag_samp(1),
                uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let present_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("present-rgba"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../shaders/present_rgba.wgsl").into(),
            ),
        });
        let present_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("upscale-present-pl"),
            bind_group_layouts: &[&present_bgl],
            push_constant_ranges: &[],
        });
        let present_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("upscale-present"),
            layout: Some(&present_pl),
            vertex: wgpu::VertexState {
                module: &present_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &present_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let _ = queue; // reserved for future coef uploads
        let block = if compute_ok {
            UpscaleBlockReason::None
        } else {
            UpscaleBlockReason::NoCompute
        };

        Self {
            mode: UpscaleMode::Off,
            sharpness: 0.4,
            compute_ok,
            block,
            vendor,
            backend: backend_name,
            last_gpu_ms: 0.0,
            over_budget_streak: 0,
            auto_disabled: false,
            sampler,
            easu_pipeline,
            rcas_pipeline,
            nis_pipeline,
            easu_bgl,
            rcas_bgl,
            nis_bgl,
            easu_buf,
            rcas_buf,
            nis_buf,
            convert_pipeline,
            convert_bgl,
            present_pipeline,
            present_bgl,
            present_buf,
            present_sampler,
            rgba_src: None,
            mid: None,
            rgba_dst: None,
        }
    }

    pub fn status(&self) -> UpscaleStatus {
        UpscaleStatus {
            mode: self.mode,
            sharpness: self.sharpness,
            compute_ok: self.compute_ok,
            block: self.block,
            vendor: self.vendor.clone(),
            backend: self.backend.clone(),
            last_gpu_ms: self.last_gpu_ms,
            over_budget_streak: self.over_budget_streak,
        }
    }

    pub fn set_mode(&mut self, mode: UpscaleMode) {
        if mode != UpscaleMode::Off {
            self.auto_disabled = false;
            if self.compute_ok {
                self.block = UpscaleBlockReason::None;
            }
        }
        self.mode = mode;
        self.over_budget_streak = 0;
    }

    pub fn set_sharpness(&mut self, s: f32) {
        self.sharpness = s.clamp(0.0, 1.0);
    }

    /// True when content already fills the window — FSR/NIS still run for sharpening only.
    pub fn sharpen_only(&self, content: (u32, u32), view: (u32, u32)) -> bool {
        is_native_or_downscale(content.0, content.1, view.0, view.1)
    }

    /// Returns true when upscale/sharpen should run for this frame.
    pub fn should_run(&mut self, content: (u32, u32), view: (u32, u32)) -> bool {
        if self.mode == UpscaleMode::Off || self.auto_disabled {
            if self.auto_disabled {
                self.block = UpscaleBlockReason::AutoDisabled;
            }
            return false;
        }
        if !self.compute_ok {
            self.block = UpscaleBlockReason::NoCompute;
            return false;
        }
        // Native resolution: still run RCAS / NIS sharpen (no greying of Mode A/B).
        self.block = UpscaleBlockReason::None;
        let _ = view;
        let _ = content;
        true
    }

    /// Color-convert → upscale → present into `surface_view`. Returns true if handled.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        video: &VideoPipeline,
        surface_view: &wgpu::TextureView,
        view_w: u32,
        view_h: u32,
        clear: wgpu::Color,
    ) -> bool {
        let (cw, ch) = video.content_size;
        if cw == 0 || ch == 0 || !self.should_run((cw, ch), (view_w, view_h)) {
            return false;
        }
        let (out_w, out_h) = target_size(cw, ch, view_w, view_h);
        let Some(src_bg) = video.bind_group_ref() else {
            return false;
        };

        ensure_tex(device, &mut self.rgba_src, cw, ch, "upscale-src", false);
        ensure_tex(device, &mut self.mid, out_w, out_h, "upscale-mid", true);
        ensure_tex(device, &mut self.rgba_dst, out_w, out_h, "upscale-dst", true);

        let src = self.rgba_src.as_ref().unwrap();
        let mid = self.mid.as_ref().unwrap();
        let dst = self.rgba_dst.as_ref().unwrap();

        // Identity letterbox into rgba_src (full UV content).
        video.write_identity_params(queue);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("upscale-convert"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &src.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // Bind groups layouts match between convert and video for the shared fullscreen shader,
            // but convert has its own BGL — rebuild a bind group from video textures each time would
            // allocate. Instead we render using the video pipeline's draw into convert target:
            // VideoPipeline.draw uses its own pipeline (surface format). So we must use convert_pipeline
            // with a bind group that mirrors video's.
            let convert_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("convert-bg"),
                layout: &self.convert_bgl,
                entries: &video.convert_bind_entries(),
            });
            pass.set_pipeline(&self.convert_pipeline);
            pass.set_bind_group(0, &convert_bg, &[]);
            pass.draw(0..3, 0..1);
            let _ = src_bg;
        }

        match self.mode {
            UpscaleMode::Fsr1 => {
                self.dispatch_fsr(
                    device,
                    queue,
                    encoder,
                    &src.view,
                    mid,
                    dst,
                    cw,
                    ch,
                    out_w,
                    out_h,
                );
            }
            UpscaleMode::Nis => {
                self.dispatch_nis(device, queue, encoder, &src.view, dst, cw, ch, out_w, out_h);
            }
            UpscaleMode::Off => return false,
        }

        self.present_dst(device, queue, encoder, dst, out_w, out_h, surface_view, view_w, view_h, clear);
        true
    }

    /// Upscale an already color-converted RGBA view (e.g. after interpolation).
    pub fn encode_from_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        src_view: &wgpu::TextureView,
        src_w: u32,
        src_h: u32,
        surface_view: &wgpu::TextureView,
        view_w: u32,
        view_h: u32,
        clear: wgpu::Color,
    ) -> bool {
        if src_w == 0 || src_h == 0 || !self.should_run((src_w, src_h), (view_w, view_h)) {
            return false;
        }
        let (out_w, out_h) = target_size(src_w, src_h, view_w, view_h);
        ensure_tex(device, &mut self.mid, out_w, out_h, "upscale-mid", true);
        ensure_tex(device, &mut self.rgba_dst, out_w, out_h, "upscale-dst", true);
        let mid = self.mid.as_ref().unwrap();
        let dst = self.rgba_dst.as_ref().unwrap();

        match self.mode {
            UpscaleMode::Fsr1 => {
                self.dispatch_fsr(
                    device, queue, encoder, src_view, mid, dst, src_w, src_h, out_w, out_h,
                );
            }
            UpscaleMode::Nis => {
                self.dispatch_nis(
                    device, queue, encoder, src_view, dst, src_w, src_h, out_w, out_h,
                );
            }
            UpscaleMode::Off => return false,
        }
        self.present_dst(device, queue, encoder, dst, out_w, out_h, surface_view, view_w, view_h, clear);
        true
    }

    fn present_dst(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        dst: &TexSlot,
        out_w: u32,
        out_h: u32,
        surface_view: &wgpu::TextureView,
        view_w: u32,
        view_h: u32,
        clear: wgpu::Color,
    ) {
        let LetterboxUniforms { scale_offset } = compute_letterbox(out_w, out_h, view_w, view_h);
        queue.write_buffer(
            &self.present_buf,
            0,
            bytemuck::bytes_of(&PresentGpu {
                scale_offset,
                pad: [0; 4],
            }),
        );
        let present_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("upscale-present-bg"),
            layout: &self.present_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&dst.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.present_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.present_buf.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("upscale-present"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: surface_view,
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
        pass.set_pipeline(&self.present_pipeline);
        pass.set_bind_group(0, &present_bg, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Observe end-to-end frame time while upscale is active (proxy for GPU cost).
    /// Auto-disables after repeated budget overruns.
    pub fn observe_frame_ms(&mut self, frame_ms: f32) -> bool {
        if self.mode == UpscaleMode::Off || self.auto_disabled {
            return false;
        }
        self.last_gpu_ms = frame_ms;
        if frame_ms > FRAME_BUDGET_MS {
            self.over_budget_streak = self.over_budget_streak.saturating_add(1);
            if self.over_budget_streak >= OVER_BUDGET_LIMIT {
                self.auto_disabled = true;
                self.mode = UpscaleMode::Off;
                self.block = UpscaleBlockReason::AutoDisabled;
                tracing::warn!(
                    frame_ms,
                    "upscale auto-disabled after repeated budget overruns"
                );
                return true;
            }
        } else {
            self.over_budget_streak = 0;
        }
        false
    }

    fn dispatch_fsr(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        src_view: &wgpu::TextureView,
        mid: &TexSlot,
        dst: &TexSlot,
        in_w: u32,
        in_h: u32,
        out_w: u32,
        out_h: u32,
    ) {
        let Some(rcas) = self.rcas_pipeline.as_ref() else {
            return;
        };
        let sharpen_only = in_w == out_w && in_h == out_h;
        let rcas_src = if sharpen_only {
            // 1:1: skip EASU, RCAS sharpens the source directly.
            src_view
        } else {
            let Some(easu) = self.easu_pipeline.as_ref() else {
                return;
            };
            let con = easu_constants(in_w, in_h, out_w, out_h);
            queue.write_buffer(&self.easu_buf, 0, bytemuck::bytes_of(&con));
            let easu_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("easu-bg"),
                layout: &self.easu_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(src_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&mid.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.easu_buf.as_entire_binding(),
                    },
                ],
            });
            {
                let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("easu"),
                    timestamp_writes: None,
                });
                cpass.set_pipeline(easu);
                cpass.set_bind_group(0, &easu_bg, &[]);
                cpass.dispatch_workgroups((out_w + 7) / 8, (out_h + 7) / 8, 1);
            }
            &mid.view
        };

        // UI 0=soft … 1=sharp → AMD RCAS stops (0=sharpest). Map into ~0..2 stops.
        // con = exp2(-stops); lobe *= con (lobe ≤ 0) so larger con = stronger sharpen.
        let stops = (1.0 - self.sharpness) * 2.0;
        let rcas_con = RcasGpu {
            con: [(-stops).exp2(), 0.0, 0.0, 0.0],
            size: [out_w as f32, out_h as f32, 0.0, 0.0],
        };
        queue.write_buffer(&self.rcas_buf, 0, bytemuck::bytes_of(&rcas_con));
        let rcas_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rcas-bg"),
            layout: &self.rcas_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(rcas_src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&dst.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.rcas_buf.as_entire_binding(),
                },
            ],
        });
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("rcas"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(rcas);
            cpass.set_bind_group(0, &rcas_bg, &[]);
            cpass.dispatch_workgroups((out_w + 7) / 8, (out_h + 7) / 8, 1);
        }
    }

    fn dispatch_nis(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        src_view: &wgpu::TextureView,
        dst: &TexSlot,
        in_w: u32,
        in_h: u32,
        out_w: u32,
        out_h: u32,
    ) {
        let Some(nis) = self.nis_pipeline.as_ref() else {
            return;
        };
        let params = NisGpu {
            scale: [
                in_w as f32 / out_w as f32,
                in_h as f32 / out_h as f32,
                self.sharpness,
                1.0,
            ],
            src_norm: [1.0 / in_w as f32, 1.0 / in_h as f32, 0.0, 0.0],
            dst_size: [out_w as f32, out_h as f32, 0.0, 0.0],
        };
        queue.write_buffer(&self.nis_buf, 0, bytemuck::bytes_of(&params));
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nis-bg"),
            layout: &self.nis_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&dst.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.nis_buf.as_entire_binding(),
                },
            ],
        });
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("nis"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(nis);
            cpass.set_bind_group(0, &bg, &[]);
            cpass.dispatch_workgroups((out_w + 7) / 8, (out_h + 7) / 8, 1);
        }
    }

}

fn ensure_tex(
    device: &wgpu::Device,
    slot: &mut Option<TexSlot>,
    w: u32,
    h: u32,
    label: &str,
    storage: bool,
) {
    if let Some(existing) = slot.as_ref() {
        if existing.size == (w, h) {
            return;
        }
    }
    let mut usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC;
    if storage {
        usage |= wgpu::TextureUsages::STORAGE_BINDING;
    } else {
        usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
    }
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    *slot = Some(TexSlot {
        texture,
        view,
        size: (w, h),
    });
}

pub fn target_size(content_w: u32, content_h: u32, view_w: u32, view_h: u32) -> (u32, u32) {
    let u = compute_letterbox(content_w, content_h, view_w, view_h);
    let out_w = ((view_w as f32) * u.scale_offset[0]).round().max(1.0) as u32;
    let out_h = ((view_h as f32) * u.scale_offset[1]).round().max(1.0) as u32;
    (out_w, out_h)
}

/// True when capture already fills (or exceeds) the letterboxed window target —
/// FSR/NIS would not enlarge the image (1080p on 1080p window, or downscale cases).
/// False for 1080p on a 1440p/4K window (upscale useful).
pub fn is_native_or_downscale(content_w: u32, content_h: u32, view_w: u32, view_h: u32) -> bool {
    if content_w == 0 || content_h == 0 {
        return false;
    }
    let (tw, th) = target_size(content_w, content_h, view_w, view_h);
    content_w >= tw && content_h >= th
}

fn easu_constants(in_w: u32, in_h: u32, out_w: u32, out_h: u32) -> EasuGpu {
    let iw = in_w as f32;
    let ih = in_h as f32;
    let ow = out_w.max(1) as f32;
    let oh = out_h.max(1) as f32;
    EasuGpu {
        con0: [
            iw / ow,
            ih / oh,
            0.5 * iw / ow - 0.5,
            0.5 * ih / oh - 0.5,
        ],
        con1: [1.0 / iw, 1.0 / ih, 1.0 / iw, -1.0 / ih],
        con2: [-1.0 / iw, 2.0 / ih, 1.0 / iw, 2.0 / ih],
        con3: [0.0, 4.0 / ih, 0.0, 0.0],
        out_size: [ow, oh, 0.0, 0.0],
    }
}

fn tex_sample_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            multisampled: false,
            view_dimension: wgpu::TextureViewDimension::D2,
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn storage_tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: wgpu::TextureFormat::Rgba8Unorm,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn frag_tex(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            multisampled: false,
            view_dimension: wgpu::TextureViewDimension::D2,
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
        },
        count: None,
    }
}

fn frag_samp(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_size_scales_up_for_small_content() {
        let (w, h) = target_size(640, 360, 1920, 1080);
        assert!(w > 640 && h > 360);
    }

    #[test]
    fn native_content_not_larger_target() {
        let (w, h) = target_size(1920, 1080, 1280, 720);
        assert!(w <= 1920 && h <= 1080);
    }

    #[test]
    fn fsr_not_greyed_for_1080_on_1440() {
        // Arthur case: 1080p capture on a 1440p window must keep FSR/NIS available.
        assert!(!is_native_or_downscale(1920, 1080, 2560, 1440));
        assert!(!is_native_or_downscale(1920, 1080, 3840, 2160));
    }

    #[test]
    fn native_is_sharpen_only_not_blocked() {
        // 1:1 still reports native for UI tip, but Mode A/B must remain usable.
        assert!(is_native_or_downscale(1920, 1080, 1920, 1080));
        assert!(is_native_or_downscale(2560, 1440, 1920, 1080));
    }

    #[test]
    fn mode_roundtrip() {
        assert_eq!(UpscaleMode::from_str_lossy("fsr1"), UpscaleMode::Fsr1);
        assert_eq!(UpscaleMode::from_str_lossy("nis"), UpscaleMode::Nis);
        assert_eq!(UpscaleMode::from_str_lossy("nope"), UpscaleMode::Off);
    }

    #[test]
    fn rcas_con_stronger_when_ui_sharpness_higher() {
        // UI 1 = sharpest → stops 0 → con 1; UI 0 = soft → smaller con.
        let con = |s: f32| {
            let stops = (1.0 - s) * 2.0;
            (-stops).exp2()
        };
        assert!((con(1.0) - 1.0).abs() < 1e-5);
        assert!(con(1.0) > con(0.5));
        assert!(con(0.5) > con(0.0));
    }
}
