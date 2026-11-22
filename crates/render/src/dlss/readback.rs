//! wgpu RGBA8 texture ↔ CPU helpers for the DLSS NR CPU-staging path.

use super::DlssEngine;

/// Read an RGBA8 texture into a tightly packed CPU buffer (row pitch = width*4).
pub fn read_texture_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let unpadded = (width as usize) * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let padded = (unpadded + align - 1) / align * align;
    let buffer_size = (padded as u64) * (height as u64);

    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dlss-nr-readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("dlss-nr-readback-enc"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded as u32),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv()
        .map_err(|_| "readback map channel closed".to_string())?
        .map_err(|e| format!("readback map failed: {e}"))?;

    let data = slice.get_mapped_range();
    let mut out = vec![0u8; unpadded * height as usize];
    for y in 0..height as usize {
        let src = &data[y * padded..y * padded + unpadded];
        out[y * unpadded..(y + 1) * unpadded].copy_from_slice(src);
    }
    drop(data);
    staging.unmap();
    Ok(out)
}

/// Upload tightly packed RGBA8 into an existing texture (must match size).
pub fn write_rgba8_texture(queue: &wgpu::Queue, texture: &wgpu::Texture, width: u32, height: u32, rgba: &[u8]) {
    let unpadded = (width as usize) * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let padded = (unpadded + align - 1) / align * align;
    let mut staging = vec![0u8; padded * height as usize];
    for y in 0..height as usize {
        let src = &rgba[y * unpadded..(y + 1) * unpadded];
        staging[y * padded..y * padded + unpadded].copy_from_slice(src);
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &staging,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(padded as u32),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// Read `src` texture, run NR, write into `dst` texture. Returns true if NR ran.
pub fn enhance_rgba8_with_nr(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    engine: &mut DlssEngine,
    src: &wgpu::Texture,
    dst: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<bool, String> {
    if !engine.can_evaluate() {
        return Ok(false);
    }
    let mut rgba = read_texture_rgba8(device, queue, src, width, height)?;
    let ran = engine.enhance_rgba8(&mut rgba, width, height)?;
    if ran {
        write_rgba8_texture(queue, dst, width, height, &rgba);
    }
    Ok(ran)
}
