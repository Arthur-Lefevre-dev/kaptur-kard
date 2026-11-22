//! wgpu present pipeline for CaptureView.

pub mod color;
pub mod deinterlace;
pub mod gpu;
pub mod interpolate;
pub mod pipeline;
pub mod present;
pub mod upload;
pub mod upscale;

pub use color::{
    prefer_limited_range, resolve_color_convert, ColorConvertParams, ColorMatrixMode,
    ColorRangeMode,
};
pub use deinterlace::{DeinterlaceEngine, DeinterlaceMode};
pub use gpu::GpuContext;
pub use pipeline::VideoPipeline;
pub use interpolate::{
    InterpBlockReason, InterpFrame, InterpMode, InterpolateEngine, InterpStatus,
};
pub use upscale::{
    is_native_or_downscale, target_size, UpscaleBlockReason, UpscaleEngine, UpscaleMode,
    UpscaleStatus,
};
