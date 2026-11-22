//! Bob deinterlace compute pass (auto when format is interlaced).

use bytemuck::{Pod, Zeroable};

use crate::pipeline::VideoPipeline;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeinterlaceMode {
    /// Off unless auto-triggered by an interlaced format.
    #[default]
    Auto,
    Off,
    Bob,
}

impl DeinterlaceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Bob => "bob",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "off" | "Off" => Self::Off,
            "bob" | "Bob" => Self::Bob,
            _ => Self::Auto,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ParamsGpu {
    size: [f32; 4],
}

struct TexSlot {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

pub struct DeinterlaceEngine {
    pub mode: DeinterlaceMode,
    pub source_interlaced: bool,
    pipeline: Option<wgpu::ComputePipeline>,
    bgl: wgpu::BindGroupLayout,
    buf: wgpu::Buffer,
    convert_pipeline: wgpu::RenderPipeline,
    convert_bgl: wgpu::BindGroupLayout,
    src: Option<TexSlot>,
    dst: Option<TexSlot>,
}

impl DeinterlaceEngine {
    pub fn new(device: &wgpu::Device, adapter: &wgpu::Adapter) -> Self {
        let compute_ok = adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS);

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("deint-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("deint-ub"),
            size: std::mem::size_of::<ParamsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let pipeline = if compute_ok {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("deinterlace-bob"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../shaders/deinterlace_bob.wgsl").into(),
                ),
            });
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("deint-pl"),
                bind_group_layouts: &[&bgl],
                push_constant_ranges: &[],
            });
            Some(device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("deint-pipe"),
                layout: Some(&pl),
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            }))
        } else {
            None
        };

        let convert_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("deint-convert-bgl"),
            entries: &[
                frag_tex(0),
                frag_tex(1),
                frag_samp(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let convert_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("deint-convert"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../shaders/fullscreen.wgsl").into(),
            ),
        });
        let convert_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("deint-convert-pl"),
            bind_group_layouts: &[&convert_bgl],
            push_constant_ranges: &[],
        });
        let convert_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("deint-convert"),
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

        Self {
            mode: DeinterlaceMode::Auto,
            source_interlaced: false,
            pipeline,
            bgl,
            buf,
            convert_pipeline,
            convert_bgl,
            src: None,
            dst: None,
        }
    }

    pub fn set_mode(&mut self, mode: DeinterlaceMode) {
        self.mode = mode;
    }

    pub fn set_source_interlaced(&mut self, interlaced: bool) {
        self.source_interlaced = interlaced;
    }

    pub fn should_run(&self) -> bool {
        match self.mode {
            DeinterlaceMode::Off => false,
            DeinterlaceMode::Bob => self.pipeline.is_some(),
            DeinterlaceMode::Auto => self.source_interlaced && self.pipeline.is_some(),
        }
    }

    /// Color-convert then Bob-deinterlace into `dst`. Returns dst view when run.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        video: &VideoPipeline,
    ) -> Option<&wgpu::TextureView> {
        if !self.should_run() || video.bind_group_ref().is_none() {
            return None;
        }
        let (w, h) = video.content_size;
        if w == 0 || h == 0 {
            return None;
        }
        ensure(device, &mut self.src, w, h, "deint-src", false);
        ensure(device, &mut self.dst, w, h, "deint-dst", true);

        video.write_identity_params(queue);
        {
            let src_view = &self.src.as_ref().unwrap().view;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("deint-convert"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: src_view,
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
                label: Some("deint-convert-bg"),
                layout: &self.convert_bgl,
                entries: &video.convert_bind_entries(),
            });
            pass.set_pipeline(&self.convert_pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }

        let Some(pipe) = self.pipeline.as_ref() else {
            return None;
        };
        queue.write_buffer(
            &self.buf,
            0,
            bytemuck::bytes_of(&ParamsGpu {
                size: [w as f32, h as f32, 0.0, 0.0],
            }),
        );
        let src = self.src.as_ref().unwrap();
        let dst = self.dst.as_ref().unwrap();
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("deint-bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&src.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&dst.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.buf.as_entire_binding(),
                },
            ],
        });
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("deinterlace-bob"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(pipe);
            cpass.set_bind_group(0, &bg, &[]);
            cpass.dispatch_workgroups((w + 7) / 8, (h + 7) / 8, 1);
        }
        Some(&self.dst.as_ref().unwrap().view)
    }

    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        self.dst.as_ref().map(|t| &t.texture)
    }

    pub fn output_view(&self) -> Option<&wgpu::TextureView> {
        self.dst.as_ref().map(|t| &t.view)
    }
}

fn ensure(
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
        assert_eq!(DeinterlaceMode::from_str_lossy("bob"), DeinterlaceMode::Bob);
        assert_eq!(DeinterlaceMode::from_str_lossy("off"), DeinterlaceMode::Off);
        assert_eq!(DeinterlaceMode::from_str_lossy("auto"), DeinterlaceMode::Auto);
    }
}
