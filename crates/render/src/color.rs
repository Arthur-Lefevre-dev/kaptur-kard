//! Color conversion stage (YUV → RGB). Passthrough for BGRA in step (a).

use capture::PixelFormat;

/// Whether the present path needs a YUV convert pass for this format.
pub fn needs_yuv_convert(format: PixelFormat) -> bool {
    matches!(format, PixelFormat::Nv12 | PixelFormat::Yuy2)
}
