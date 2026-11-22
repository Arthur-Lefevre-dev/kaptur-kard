//! CPU → GPU texture upload helpers.

use capture::VideoFrame;

use crate::pipeline::VideoPipeline;

/// Upload a video frame into the pipeline (BGRA / YUY2 / NV12).
pub fn upload_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut VideoPipeline,
    frame: &VideoFrame,
) -> bool {
    if !pipeline.ensure_textures(device, frame.width, frame.height, frame.pixel_format) {
        tracing::warn!(
            ?frame.pixel_format,
            "unsupported pixel format for GPU upload"
        );
        return false;
    }
    pipeline.write_frame(queue, frame.pixel_format, frame.data.as_ref());
    true
}
