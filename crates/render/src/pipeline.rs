//! Upload → present pipeline (BGRA passthrough + letterboxing).

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::present::{compute_letterbox, LetterboxUniforms};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LetterboxGpu {
    scale_offset: [f32; 4],
}

pub struct VideoPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    pub letterbox_buf: wgpu::Buffer,
    pub texture: Option<wgpu::Texture>,
    pub texture_view: Option<wgpu::TextureView>,
    pub bind_group: Option<wgpu::BindGroup>,
    pub content_size: (u32, u32),
}

impl VideoPipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fullscreen"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../../shaders/fullscreen.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("video-pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("video-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
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

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("video-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let letterbox_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("letterbox-uniform"),
            contents: bytemuck::bytes_of(&LetterboxGpu {
                scale_offset: [1.0, 1.0, 0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            letterbox_buf,
            texture: None,
            texture_view: None,
            bind_group: None,
            content_size: (0, 0),
        }
    }

    pub fn ensure_texture(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.content_size == (width, height) && self.texture.is_some() {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("video-frame"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("video-bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.letterbox_buf.as_entire_binding(),
                },
            ],
        });
        self.texture = Some(texture);
        self.texture_view = Some(view);
        self.bind_group = Some(bind_group);
        self.content_size = (width, height);
    }

    pub fn write_bgra(&self, queue: &wgpu::Queue, data: &[u8]) {
        let Some(texture) = self.texture.as_ref() else {
            return;
        };
        let (width, height) = self.content_size;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }

    pub fn update_letterbox(&self, queue: &wgpu::Queue, view_w: u32, view_h: u32) {
        let (cw, ch) = self.content_size;
        if cw == 0 || ch == 0 {
            return;
        }
        let LetterboxUniforms { scale_offset } = compute_letterbox(cw, ch, view_w, view_h);
        queue.write_buffer(
            &self.letterbox_buf,
            0,
            bytemuck::bytes_of(&LetterboxGpu { scale_offset }),
        );
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if let Some(bg) = self.bind_group.as_ref() {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
