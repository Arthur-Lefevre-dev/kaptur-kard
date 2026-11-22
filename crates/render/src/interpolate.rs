//! Optical-flow frame interpolation (x2) via WGSL compute.
//!
//! Pipeline slot: after color convert, before upscale/present.
//! Adds ~one source frame of display delay when active.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::pipeline::VideoPipeline;
use crate::present::{compute_letterbox, LetterboxUniforms};

const OVER_BUDGET_LIMIT: u32 = 45;
/// Display present dt is not GPU cost — only trip on real stalls (Arthur: CPU spin).
const FRAME_BUDGET_MS: f32 = 40.0;
/// Block-match radius; 2 is cheaper than 3 at 1080p with little quality loss.
const SEARCH_RADIUS: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterpMode {
    Off,
    X2,
    /// Run x2 while the GPU keeps the frame budget; auto-disable on repeated misses.
    Auto,
}

impl InterpMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::X2 => "x2",
            Self::Auto => "auto",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "x2" | "X2" => Self::X2,
            "auto" | "Auto" => Self::Auto,
            _ => Self::Off,
        }
    }

    pub fn wants_effect(self) -> bool {
        matches!(self, Self::X2 | Self::Auto)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterpBlockReason {
    None,
    NoCompute,
    AutoDisabled,
    NeedPrevFrame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterpFrame {
    Mid,
    Curr,
}

#[derive(Clone, Debug)]
pub struct InterpStatus {
    pub mode: InterpMode,
    pub compute_ok: bool,
    pub block: InterpBlockReason,
    pub delay_ms: f32,
    pub last_gpu_ms: f32,
    pub showing_mid: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FlowGpu {
    size: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WarpGpu {
    size: [f32; 4],
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

pub struct InterpolateEngine {
    pub mode: InterpMode,
    pub compute_ok: bool,
    pub block: InterpBlockReason,
    pub delay_ms: f32,
    pub last_gpu_ms: f32,
    over_budget_streak: u32,
    auto_disabled: bool,
    /// After a new capture pair, present mid once before curr.
    pending_mid: bool,
    has_prev: bool,

    sampler: wgpu::Sampler,
    flow_pipeline: Option<wgpu::ComputePipeline>,
    warp_pipeline: Option<wgpu::ComputePipeline>,
    flow_bgl: wgpu::BindGroupLayout,
    warp_bgl: wgpu::BindGroupLayout,
    flow_buf: wgpu::Buffer,
    warp_buf: wgpu::Buffer,

    convert_pipeline: wgpu::RenderPipeline,
    convert_bgl: wgpu::BindGroupLayout,
    present_pipeline: wgpu::RenderPipeline,
    present_bgl: wgpu::BindGroupLayout,
    present_buf: wgpu::Buffer,
    present_sampler: wgpu::Sampler,

    prev: Option<TexSlot>,
    curr: Option<TexSlot>,
    mid: Option<TexSlot>,
    flow: Option<TexSlot>,
}

impl InterpolateEngine {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        adapter: &wgpu::Adapter,
    ) -> Self {
        let compute_ok = adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("interp-samp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let present_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("interp-present-samp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let flow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flow-bgl"),
            entries: &[
                tex_entry(0, wgpu::ShaderStages::COMPUTE),
                tex_entry(1, wgpu::ShaderStages::COMPUTE),
                storage_rg_entry(2),
                uniform_entry(3, wgpu::ShaderStages::COMPUTE),
            ],
        });
        let warp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("warp-bgl"),
            entries: &[
                tex_entry(0, wgpu::ShaderStages::COMPUTE),
                tex_entry(1, wgpu::ShaderStages::COMPUTE),
                // Flow is rg32float — non-filterable, loaded via textureLoad.
                tex_entry_unfilterable(2, wgpu::ShaderStages::COMPUTE),
                sampler_entry(3),
                storage_rgba_entry(4),
                uniform_entry(5, wgpu::ShaderStages::COMPUTE),
            ],
        });

        let flow_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flow-ub"),
            size: std::mem::size_of::<FlowGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let warp_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("warp-ub"),
            size: std::mem::size_of::<WarpGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let present_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("interp-present-ub"),
            contents: bytemuck::bytes_of(&PresentGpu {
                scale_offset: [1.0, 1.0, 0.0, 0.0],
                pad: [0; 4],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let (flow_pipeline, warp_pipeline) = if compute_ok {
            let flow_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("interp-flow"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../shaders/interp_flow.wgsl").into(),
                ),
            });
            let warp_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("interp-warp"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../shaders/interp_warp.wgsl").into(),
                ),
            });
            let flow_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("flow-pl"),
                bind_group_layouts: &[&flow_bgl],
                push_constant_ranges: &[],
            });
            let warp_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("warp-pl"),
                bind_group_layouts: &[&warp_bgl],
                push_constant_ranges: &[],
            });
            (
                Some(device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("flow-pipe"),
                    layout: Some(&flow_pl),
                    module: &flow_mod,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    cache: None,
                })),
                Some(device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("warp-pipe"),
                    layout: Some(&warp_pl),
                    module: &warp_mod,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    cache: None,
                })),
            )
        } else {
            tracing::warn!("compute shaders unavailable; interpolation disabled");
            (None, None)
        };

        let convert_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("interp-convert-bgl"),
            entries: &[
                frag_tex(0),
                frag_tex(1),
                frag_samp(2),
                uniform_entry(3, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let convert_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("interp-convert"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../shaders/fullscreen.wgsl").into(),
            ),
        });
        let convert_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("interp-convert-pl"),
            bind_group_layouts: &[&convert_bgl],
            push_constant_ranges: &[],
        });
        let convert_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("interp-convert"),
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
            label: Some("interp-present-bgl"),
            entries: &[
                frag_tex(0),
                frag_samp(1),
                uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let present_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("interp-present"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../shaders/present_rgba.wgsl").into(),
            ),
        });
        let present_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("interp-present-pl"),
            bind_group_layouts: &[&present_bgl],
            push_constant_ranges: &[],
        });
        let present_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("interp-present"),
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

        let block = if compute_ok {
            InterpBlockReason::None
        } else {
            InterpBlockReason::NoCompute
        };

        Self {
            mode: InterpMode::Off,
            compute_ok,
            block,
            delay_ms: 0.0,
            last_gpu_ms: 0.0,
            over_budget_streak: 0,
            auto_disabled: false,
            pending_mid: false,
            has_prev: false,
            sampler,
            flow_pipeline,
            warp_pipeline,
            flow_bgl,
            warp_bgl,
            flow_buf,
            warp_buf,
            convert_pipeline,
            convert_bgl,
            present_pipeline,
            present_bgl,
            present_buf,
            present_sampler,
            prev: None,
            curr: None,
            mid: None,
            flow: None,
        }
    }

    pub fn status(&self) -> InterpStatus {
        InterpStatus {
            mode: self.mode,
            compute_ok: self.compute_ok,
            block: self.block,
            delay_ms: self.delay_ms,
            last_gpu_ms: self.last_gpu_ms,
            showing_mid: self.pending_mid,
        }
    }

    pub fn set_mode(&mut self, mode: InterpMode) {
        if mode.wants_effect() {
            self.auto_disabled = false;
            if self.compute_ok {
                self.block = InterpBlockReason::None;
            }
        }
        self.mode = mode;
        self.over_budget_streak = 0;
        if !mode.wants_effect() {
            self.pending_mid = false;
            self.delay_ms = 0.0;
            self.release_scratch();
        }
    }

    /// Drop prev/curr/mid/flow textures while interpolation is off.
    pub fn release_scratch(&mut self) {
        self.prev = None;
        self.curr = None;
        self.mid = None;
        self.flow = None;
        self.has_prev = false;
    }

    pub fn set_source_delay_ms(&mut self, ms: f32) {
        if self.mode.wants_effect() && !self.auto_disabled {
            self.delay_ms = ms.max(0.0);
        } else {
            self.delay_ms = 0.0;
        }
    }

    pub fn active(&self) -> bool {
        self.mode.wants_effect() && self.compute_ok && !self.auto_disabled
    }

    pub fn observe_frame_ms(&mut self, frame_ms: f32) -> bool {
        if !self.mode.wants_effect() || self.auto_disabled {
            return false;
        }
        self.last_gpu_ms = frame_ms;
        if frame_ms > FRAME_BUDGET_MS {
            self.over_budget_streak = self.over_budget_streak.saturating_add(1);
            if self.over_budget_streak >= OVER_BUDGET_LIMIT {
                self.auto_disabled = true;
                self.mode = InterpMode::Off;
                self.block = InterpBlockReason::AutoDisabled;
                self.pending_mid = false;
                self.delay_ms = 0.0;
                tracing::warn!(frame_ms, "interpolation auto-disabled after budget overruns");
                return true;
            }
        } else {
            self.over_budget_streak = 0;
        }
        false
    }

    /// Convert current video frame; on new capture synthesize mid and queue it.
    /// Returns which buffer should feed upscale/present this tick.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        video: &VideoPipeline,
        new_capture: bool,
    ) -> Option<InterpFrame> {
        if !self.active() {
            self.block = if !self.compute_ok {
                InterpBlockReason::NoCompute
            } else if self.auto_disabled {
                InterpBlockReason::AutoDisabled
            } else {
                InterpBlockReason::None
            };
            return None;
        }
        // Present-only tick (no new capture): reuse last curr/mid — do not re-convert
        // or re-run flow (was ~2× CPU work at 120 Hz display; Arthur saw ~95% CPU).
        if !new_capture {
            self.block = InterpBlockReason::None;
            return if self.curr.is_some() {
                Some(InterpFrame::Curr)
            } else {
                None
            };
        }
        let (cw, ch) = video.content_size;
        if cw == 0 || ch == 0 || video.bind_group_ref().is_none() {
            return None;
        }

        ensure_rgba(device, &mut self.curr, cw, ch, "interp-curr", false);
        ensure_rgba(device, &mut self.prev, cw, ch, "interp-prev", false);
        ensure_rgba(device, &mut self.mid, cw, ch, "interp-mid", true);
        ensure_flow(device, &mut self.flow, cw, ch);

        // Color-convert into curr.
        video.write_identity_params(queue);
        {
            let curr_view = &self.curr.as_ref().unwrap().view;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("interp-convert"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: curr_view,
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
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("interp-convert-bg"),
                layout: &self.convert_bgl,
                entries: &video.convert_bind_entries(),
            });
            pass.set_pipeline(&self.convert_pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }

        if new_capture && self.has_prev {
            self.dispatch_flow_warp(device, queue, encoder, cw, ch);
            self.pending_mid = true;
            // Copy curr → prev for next pair (after warp used the old prev).
            if let (Some(curr), Some(prev)) = (self.curr.as_ref(), self.prev.as_ref()) {
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &curr.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &prev.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: cw,
                        height: ch,
                        depth_or_array_layers: 1,
                    },
                );
            }
        } else if new_capture && !self.has_prev {
            if let (Some(curr), Some(prev)) = (self.curr.as_ref(), self.prev.as_ref()) {
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &curr.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &prev.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: cw,
                        height: ch,
                        depth_or_array_layers: 1,
                    },
                );
            }
            self.has_prev = true;
            self.block = InterpBlockReason::NeedPrevFrame;
        }

        if self.pending_mid {
            self.pending_mid = false;
            self.block = InterpBlockReason::None;
            Some(InterpFrame::Mid)
        } else {
            self.block = InterpBlockReason::None;
            Some(InterpFrame::Curr)
        }
    }

    pub fn frame_view(&self, frame: InterpFrame) -> Option<&wgpu::TextureView> {
        match frame {
            InterpFrame::Mid => self.mid.as_ref().map(|t| &t.view),
            InterpFrame::Curr => self.curr.as_ref().map(|t| &t.view),
        }
    }

    /// Like [`prepare`] but ingests an already color-converted (and optionally deinterlaced) RGBA texture.
    pub fn prepare_from_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::Texture,
        width: u32,
        height: u32,
        new_capture: bool,
    ) -> Option<InterpFrame> {
        if !self.active() || width == 0 || height == 0 {
            return None;
        }
        // Present-only tick: reuse buffers (see prepare).
        if !new_capture {
            self.block = InterpBlockReason::None;
            return if self.curr.is_some() {
                Some(InterpFrame::Curr)
            } else {
                None
            };
        }
        let _ = queue;
        ensure_rgba(device, &mut self.curr, width, height, "interp-curr", false);
        ensure_rgba(device, &mut self.prev, width, height, "interp-prev", false);
        ensure_rgba(device, &mut self.mid, width, height, "interp-mid", true);
        ensure_flow(device, &mut self.flow, width, height);

        if let Some(curr) = self.curr.as_ref() {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: src,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &curr.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }

        if new_capture && self.has_prev {
            self.dispatch_flow_warp(device, queue, encoder, width, height);
            self.pending_mid = true;
            if let (Some(curr), Some(prev)) = (self.curr.as_ref(), self.prev.as_ref()) {
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &curr.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &prev.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
            }
        } else if new_capture && !self.has_prev {
            if let (Some(curr), Some(prev)) = (self.curr.as_ref(), self.prev.as_ref()) {
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &curr.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &prev.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
            }
            self.has_prev = true;
            self.block = InterpBlockReason::NeedPrevFrame;
        }

        if self.pending_mid {
            self.pending_mid = false;
            self.block = InterpBlockReason::None;
            Some(InterpFrame::Mid)
        } else {
            self.block = InterpBlockReason::None;
            Some(InterpFrame::Curr)
        }
    }

    /// Letterbox-present an RGBA view to the swapchain.
    pub fn present_view(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        rgba: &wgpu::TextureView,
        content: (u32, u32),
        surface_view: &wgpu::TextureView,
        view_w: u32,
        view_h: u32,
        clear: wgpu::Color,
    ) {
        let LetterboxUniforms { scale_offset } =
            compute_letterbox(content.0, content.1, view_w, view_h);
        queue.write_buffer(
            &self.present_buf,
            0,
            bytemuck::bytes_of(&PresentGpu {
                scale_offset,
                pad: [0; 4],
            }),
        );
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("interp-present-bg"),
            layout: &self.present_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(rgba),
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
            label: Some("interp-present"),
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
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }

    fn dispatch_flow_warp(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        w: u32,
        h: u32,
    ) {
        let Some(flow_pipe) = self.flow_pipeline.as_ref() else {
            return;
        };
        let Some(warp_pipe) = self.warp_pipeline.as_ref() else {
            return;
        };
        let prev = self.prev.as_ref().unwrap();
        let curr = self.curr.as_ref().unwrap();
        let mid = self.mid.as_ref().unwrap();
        let flow = self.flow.as_ref().unwrap();

        queue.write_buffer(
            &self.flow_buf,
            0,
            bytemuck::bytes_of(&FlowGpu {
                size: [w as f32, h as f32, SEARCH_RADIUS, 0.0],
            }),
        );
        let flow_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flow-bg"),
            layout: &self.flow_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&prev.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&curr.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&flow.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.flow_buf.as_entire_binding(),
                },
            ],
        });
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("interp-flow"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(flow_pipe);
            cpass.set_bind_group(0, &flow_bg, &[]);
            cpass.dispatch_workgroups((w + 7) / 8, (h + 7) / 8, 1);
        }

        queue.write_buffer(
            &self.warp_buf,
            0,
            bytemuck::bytes_of(&WarpGpu {
                size: [w as f32, h as f32, 0.5, 0.0],
            }),
        );
        let warp_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("warp-bg"),
            layout: &self.warp_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&prev.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&curr.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&flow.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&mid.view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.warp_buf.as_entire_binding(),
                },
            ],
        });
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("interp-warp"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(warp_pipe);
            cpass.set_bind_group(0, &warp_bg, &[]);
            cpass.dispatch_workgroups((w + 7) / 8, (h + 7) / 8, 1);
        }
    }
}

fn ensure_rgba(
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
        usage |= wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST;
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

fn ensure_flow(device: &wgpu::Device, slot: &mut Option<TexSlot>, w: u32, h: u32) {
    if let Some(existing) = slot.as_ref() {
        if existing.size == (w, h) {
            return;
        }
    }
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("interp-flow"),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rg32Float,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    *slot = Some(TexSlot {
        texture,
        view,
        size: (w, h),
    });
}

fn tex_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            multisampled: false,
            view_dimension: wgpu::TextureViewDimension::D2,
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
        },
        count: None,
    }
}

fn tex_entry_unfilterable(
    binding: u32,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            multisampled: false,
            view_dimension: wgpu::TextureViewDimension::D2,
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
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

fn storage_rgba_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
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

fn storage_rg_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: wgpu::TextureFormat::Rg32Float,
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
    fn mode_roundtrip() {
        assert_eq!(InterpMode::from_str_lossy("x2"), InterpMode::X2);
        assert_eq!(InterpMode::from_str_lossy("auto"), InterpMode::Auto);
        assert_eq!(InterpMode::from_str_lossy("nope"), InterpMode::Off);
        assert!(InterpMode::Auto.wants_effect());
        assert!(!InterpMode::Off.wants_effect());
    }
}
