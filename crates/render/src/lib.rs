//! wgpu present pipeline for CaptureView.

pub mod color;
pub mod deinterlace;
pub mod gpu;
pub mod interpolate;
pub mod pipeline;
pub mod present;
pub mod upload;
pub mod upscale;

pub use deinterlace::{DeinterlaceEngine, DeinterlaceMode};
pub use gpu::GpuContext;
pub use pipeline::VideoPipeline;
pub use interpolate::{
    InterpBlockReason, InterpFrame, InterpMode, InterpolateEngine, InterpStatus,
};
pub use upscale::{
    target_size, UpscaleBlockReason, UpscaleEngine, UpscaleMode, UpscaleStatus,
};
