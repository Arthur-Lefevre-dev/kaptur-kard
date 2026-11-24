//! wgpu present pipeline for CaptureView.

pub mod color;
pub mod deinterlace;
pub mod dlss;
pub mod gpu;
pub mod interpolate;
pub mod pipeline;
pub mod present;
pub mod swapchain;
pub mod upload;
pub mod upscale;

pub use color::{
    prefer_limited_range, resolve_color_convert, ColorConvertParams, ColorMatrixMode,
    ColorRangeMode,
};
pub use deinterlace::{DeinterlaceEngine, DeinterlaceMode};
pub use dlss::{
    preferred_backends, DlssAvailability, DlssEngine, DlssGAvailability, DlssGMultiplier,
    DlssGStatus, DlssStatus, FgPresentFrame, DLSS_MARKETING_NOTE, STREAMLINE_DOC_VERSION,
};
pub use gpu::GpuContext;
pub use pipeline::VideoPipeline;
pub use interpolate::{
    InterpBlockReason, InterpFrame, InterpMode, InterpolateEngine, InterpStatus,
};
pub use present::{
    compute_present_fit, compute_present_fit_ex, PresentFilter, PresentFit,
};
pub use swapchain::{
    format_label, pick_present_mode, pick_surface_format, present_mode_label, PresentCaps,
};
pub use upscale::{
    is_native_or_downscale, target_size, UpscaleBlockReason, UpscaleEngine, UpscaleMode,
    UpscaleStatus,
};
