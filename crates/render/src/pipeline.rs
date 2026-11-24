//! Upload → color convert → present pipeline (BGRA / YUY2 / NV12).

use bytemuck::{Pod, Zeroable};
use capture::PixelFormat;
use wgpu::util::DeviceExt;

use crate::color::{gpu_mode, ColorConvertParams, GpuPixelMode};
use crate::present::{compute_present_fit_ex, PresentFilter};

const COPY_ALIGN: u32 = 256;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ParamsGpu {
    scale_offset: [f32; 4],
    format_pad: [u32; 4],
    /// brightness, contrast, gamma, pad
    grade: [f32; 4],
}

fn align_up(v: u32, align: u32) -> u32 {
    (v + align - 1) / align * align
}

pub struct VideoPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    pub params_buf: wgpu::Buffer,
    primary: Option<wgpu::Texture>,
    primary_view: Option<wgpu::TextureView>,
    uv: Option<wgpu::Texture>,
    uv_view: Option<wgpu::TextureView>,
    _dummy_uv: wgpu::Texture,
    dummy_uv_view: wgpu::TextureView,
    bind_group: Option<wgpu::BindGroup>,
    pub content_size: (u32, u32),
    pub mode: GpuPixelMode,
    /// YUV → RGB range/matrix flags (ignored for BGRA).
    pub color: ColorConvertParams,
    /// Present-time grade (identity during offscreen convert).
    pub grade: [f32; 4],
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
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
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

        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("video-params"),
            contents: bytemuck::bytes_of(&ParamsGpu {
                scale_offset: [1.0, 1.0, 0.0, 0.0],
                // Default: limited + BT.709 (HD capture-card friendly).
                format_pad: [0, 1, 1, 0],
                grade: [0.0, 1.0, 1.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let dummy_uv = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("dummy-uv"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let dummy_uv_view = dummy_uv.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            params_buf,
            primary: None,
            primary_view: None,
            uv: None,
            uv_view: None,
            _dummy_uv: dummy_uv,
            dummy_uv_view,
            bind_group: None,
            content_size: (0, 0),
            mode: GpuPixelMode::Bgra,
            color: ColorConvertParams {
                limited: true,
                bt709: true,
            },
            grade: [0.0, 1.0, 1.0, 0.0],
        }
    }

    pub fn set_color_convert(&mut self, color: ColorConvertParams) {
        self.color = color;
    }

    pub fn set_grade(&mut self, grade: [f32; 4]) {
        self.grade = grade;
    }

    fn format_pad(&self, nearest: bool) -> [u32; 4] {
        [
            self.mode as u32,
            u32::from(self.color.limited),
            u32::from(self.color.bt709),
            u32::from(nearest),
        ]
    }

    pub fn ensure_textures(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> bool {
        let Some(mode) = gpu_mode(format) else {
            return false;
        };
        if self.content_size == (width, height) && self.mode == mode && self.primary.is_some() {
            return true;
        }

        let (primary_fmt, primary_w, primary_h) = match mode {
            // Unorm (not sRGB): bytes are already display-referred when sampled.
            GpuPixelMode::Bgra => (wgpu::TextureFormat::Bgra8Unorm, width, height),
            GpuPixelMode::Yuy2 => (wgpu::TextureFormat::Rgba8Unorm, width / 2, height),
            GpuPixelMode::Nv12 => (wgpu::TextureFormat::R8Unorm, width, height),
        };

        let primary = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("video-primary"),
            size: wgpu::Extent3d {
                width: primary_w.max(1),
                height: primary_h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: primary_fmt,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let primary_view = primary.create_view(&wgpu::TextureViewDescriptor::default());

        let (uv, uv_view) = if mode == GpuPixelMode::Nv12 {
            let uv = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("video-uv"),
                size: wgpu::Extent3d {
                    width: (width / 2).max(1),
                    height: (height / 2).max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rg8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = uv.create_view(&wgpu::TextureViewDescriptor::default());
            (Some(uv), Some(view))
        } else {
            (None, None)
        };

        let uv_binding = uv_view.as_ref().unwrap_or(&self.dummy_uv_view);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("video-bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&primary_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(uv_binding),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.params_buf.as_entire_binding(),
                },
            ],
        });

        self.primary = Some(primary);
        self.primary_view = Some(primary_view);
        self.uv = uv;
        self.uv_view = uv_view;
        self.bind_group = Some(bind_group);
        self.content_size = (width, height);
        self.mode = mode;
        true
    }

    pub fn write_frame(&self, queue: &wgpu::Queue, format: PixelFormat, data: &[u8]) {
        let Some(mode) = gpu_mode(format) else {
            return;
        };
        let Some(primary) = self.primary.as_ref() else {
            return;
        };
        let (width, height) = self.content_size;

        match mode {
            GpuPixelMode::Bgra => {
                write_texture_aligned(queue, primary, width, height, 4, data);
            }
            GpuPixelMode::Yuy2 => {
                write_texture_aligned(queue, primary, width / 2, height, 4, data);
            }
            GpuPixelMode::Nv12 => {
                let y_size = (width * height) as usize;
                let y_plane = &data[..y_size.min(data.len())];
                write_texture_aligned(queue, primary, width, height, 1, y_plane);
                if let Some(uv_tex) = self.uv.as_ref() {
                    let uv_plane = if data.len() > y_size {
                        &data[y_size..]
                    } else {
                        &[]
                    };
                    write_texture_aligned(queue, uv_tex, width / 2, height / 2, 2, uv_plane);
                }
            }
        }
    }

    pub fn update_letterbox(
        &self,
        queue: &wgpu::Queue,
        view_w: u32,
        view_h: u32,
        fill_viewport: bool,
    ) {
        let (cw, ch) = self.content_size;
        if cw == 0 || ch == 0 {
            return;
        }
        let fit = compute_present_fit_ex(cw, ch, view_w, view_h, fill_viewport);
        let nearest = matches!(fit.filter, PresentFilter::Nearest);
        queue.write_buffer(
            &self.params_buf,
            0,
            bytemuck::bytes_of(&ParamsGpu {
                scale_offset: fit.scale_offset,
                format_pad: self.format_pad(nearest),
                grade: self.grade,
            }),
        );
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if let Some(bg) = self.bind_group.as_ref() {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    pub fn bind_group_ref(&self) -> Option<&wgpu::BindGroup> {
        self.bind_group.as_ref()
    }

    /// Full-bleed UV mapping for offscreen color convert (no letterbox).
    pub fn write_identity_params(&self, queue: &wgpu::Queue) {
        queue.write_buffer(
            &self.params_buf,
            0,
            bytemuck::bytes_of(&ParamsGpu {
                scale_offset: [1.0, 1.0, 0.0, 0.0],
                // Convert is always 1:1 into rgba_src — sample filter unused for identity UV.
                format_pad: self.format_pad(false),
                // Identity grade so convert does not bake calibration twice.
                grade: [0.0, 1.0, 1.0, 0.0],
            }),
        );
    }

    /// Bind entries mirroring the video group (for a convert pipeline with the same layout).
    pub fn convert_bind_entries(&self) -> [wgpu::BindGroupEntry<'_>; 4] {
        let primary = self
            .primary_view
            .as_ref()
            .expect("primary view for convert");
        let uv = self.uv_view.as_ref().unwrap_or(&self.dummy_uv_view);
        [
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(primary),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(uv),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: self.params_buf.as_entire_binding(),
            },
        ]
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
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

fn write_texture_aligned(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    bytes_per_pixel: u32,
    data: &[u8],
) {
    let row_bytes = width * bytes_per_pixel;
    let aligned = align_up(row_bytes, COPY_ALIGN);
    if aligned == row_bytes {
        let needed = (row_bytes * height) as usize;
        let slice = &data[..needed.min(data.len())];
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            slice,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        return;
    }

    // Pad rows to 256-byte alignment required by write_texture on some backends.
    let mut padded = vec![0u8; (aligned * height) as usize];
    for y in 0..height as usize {
        let src_off = y * row_bytes as usize;
        let dst_off = y * aligned as usize;
        let end = (src_off + row_bytes as usize).min(data.len());
        if src_off >= data.len() {
            break;
        }
        let n = end - src_off;
        padded[dst_off..dst_off + n].copy_from_slice(&data[src_off..end]);
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &padded,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(aligned),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}
