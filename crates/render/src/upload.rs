//! CPU → GPU texture upload helpers.

use capture::{PixelFormat, VideoFrame};

use crate::pipeline::VideoPipeline;

/// Upload a video frame into the pipeline's frame texture when format/size match.
pub fn upload_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut VideoPipeline,
    frame: &VideoFrame,
) -> bool {
    if frame.pixel_format != PixelFormat::Bgra8 {
        tracing::warn!(
            ?frame.pixel_format,
            "step (a) only uploads Bgra8; skipping frame"
        );
        return false;
    }
    pipeline.ensure_texture(device, frame.width, frame.height);
    pipeline.write_bgra(queue, &frame.data);
    true
}
