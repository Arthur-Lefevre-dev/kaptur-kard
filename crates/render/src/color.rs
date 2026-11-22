//! Color conversion helpers for the present shader.

use capture::PixelFormat;

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuPixelMode {
    Bgra = 0,
    Yuy2 = 1,
    Nv12 = 2,
}

pub fn gpu_mode(format: PixelFormat) -> Option<GpuPixelMode> {
    match format {
        PixelFormat::Bgra8 | PixelFormat::Rgb24 => Some(GpuPixelMode::Bgra),
        PixelFormat::Yuy2 => Some(GpuPixelMode::Yuy2),
        PixelFormat::Nv12 => Some(GpuPixelMode::Nv12),
        PixelFormat::Mjpeg | PixelFormat::Unknown(_) => None,
    }
}

pub fn needs_yuv_convert(format: PixelFormat) -> bool {
    matches!(format, PixelFormat::Nv12 | PixelFormat::Yuy2)
}
