//! CPU pixel format helpers shared by Spout export and the replay buffer.

use capture::{PixelFormat, VideoFrame};

/// Convert a live frame to tightly-packed BGRA8 at source resolution.
#[allow(dead_code)]
pub fn frame_to_bgra(frame: &VideoFrame) -> Option<Vec<u8>> {
    frame_to_bgra_scaled(frame, frame.width)
}

/// Convert a live frame to BGRA8, point-downsampling so width ≤ `max_w`.
pub fn frame_to_bgra_scaled(frame: &VideoFrame, max_w: u32) -> Option<Vec<u8>> {
    let (dw, dh) = stored_size(frame.width, frame.height, max_w);
    let src = frame.data.as_slice();
    match frame.pixel_format {
        PixelFormat::Bgra8 => downsample_bgra(src, frame.width, frame.height, dw, dh),
        PixelFormat::Yuy2 => {
            let full = yuy2_to_bgra(src, frame.width, frame.height)?;
            if dw == frame.width && dh == frame.height {
                Some(full)
            } else {
                downsample_bgra(&full, frame.width, frame.height, dw, dh)
            }
        }
        PixelFormat::Nv12 => nv12_to_bgra_scaled(src, frame.width, frame.height, dw, dh),
        _ => None,
    }
}

/// Point-downsample tightly-packed NV12 into `dw×dh` (even dimensions preferred).
pub fn downsample_nv12(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Option<Vec<u8>> {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || sw < 2 || sh < 2 {
        return None;
    }
    // Keep chroma grid valid.
    let dw = dw & !1;
    let dh = dh & !1;
    if dw < 2 || dh < 2 {
        return None;
    }
    let y_size = (sw * sh) as usize;
    let uv_size = (sw * sh / 2) as usize;
    if src.len() < y_size + uv_size {
        return None;
    }
    if sw == dw && sh == dh {
        return Some(src[..y_size + uv_size].to_vec());
    }
    let y_plane = &src[..y_size];
    let uv_plane = &src[y_size..y_size + uv_size];
    let mut out = vec![0u8; (dw * dh + dw * dh / 2) as usize];
    let out_y = (dw * dh) as usize;
    for dy in 0..dh {
        let sy = (dy as u64 * sh as u64 / dh as u64) as u32;
        for dx in 0..dw {
            let sx = (dx as u64 * sw as u64 / dw as u64) as u32;
            out[(dy * dw + dx) as usize] = y_plane[(sy * sw + sx) as usize];
        }
    }
    for dy in 0..(dh / 2) {
        let sy = (dy as u64 * (sh / 2) as u64 / (dh / 2) as u64) as u32;
        for dx in 0..(dw / 2) {
            let sx = (dx as u64 * (sw / 2) as u64 / (dw / 2) as u64) as u32;
            let si = (sy * sw + sx * 2) as usize;
            let di = out_y + (dy * dw + dx * 2) as usize;
            out[di] = uv_plane[si];
            out[di + 1] = uv_plane[si + 1];
        }
    }
    Some(out)
}

pub fn stored_size(w: u32, h: u32, max_w: u32) -> (u32, u32) {
    if w <= max_w || w == 0 {
        return (w.max(1), h.max(1));
    }
    let scale = max_w as f32 / w as f32;
    let nw = max_w;
    let nh = ((h as f32 * scale).round() as u32).max(1);
    (nw, nh)
}

pub fn downsample_bgra(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Option<Vec<u8>> {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return None;
    }
    let need = (sw * sh * 4) as usize;
    if src.len() < need {
        return None;
    }
    if sw == dw && sh == dh {
        return Some(src[..need].to_vec());
    }
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for y in 0..dh {
        let sy = (y as u64 * sh as u64 / dh as u64) as u32;
        for x in 0..dw {
            let sx = (x as u64 * sw as u64 / dw as u64) as u32;
            let si = ((sy * sw + sx) * 4) as usize;
            let di = ((y * dw + x) * 4) as usize;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    Some(out)
}

pub fn yuy2_to_bgra(src: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let need = (w * h * 2) as usize;
    if src.len() < need || w < 2 {
        return None;
    }
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in (0..w).step_by(2) {
            let i = ((y * w + x) * 2) as usize;
            let y0 = src[i] as f32;
            let u = src[i + 1] as f32;
            let y1 = src[i + 2] as f32;
            let v = src[i + 3] as f32;
            let (b0, g0, r0) = yuv_to_bgra(y0, u, v);
            let (b1, g1, r1) = yuv_to_bgra(y1, u, v);
            let o0 = ((y * w + x) * 4) as usize;
            out[o0] = b0;
            out[o0 + 1] = g0;
            out[o0 + 2] = r0;
            out[o0 + 3] = 255;
            if x + 1 < w {
                let o1 = o0 + 4;
                out[o1] = b1;
                out[o1 + 1] = g1;
                out[o1 + 2] = r1;
                out[o1 + 3] = 255;
            }
        }
    }
    Some(out)
}

/// Convert tightly-packed NV12 to BGRA, sampling into `dw×dh` (point filter).
pub fn nv12_to_bgra_scaled(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Option<Vec<u8>> {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || sw < 2 || sh < 2 {
        return None;
    }
    let y_size = (sw * sh) as usize;
    let uv_size = (sw * sh / 2) as usize;
    if src.len() < y_size + uv_size {
        return None;
    }
    let y_plane = &src[..y_size];
    let uv_plane = &src[y_size..y_size + uv_size];
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for dy in 0..dh {
        let sy = (dy as u64 * sh as u64 / dh as u64) as u32;
        for dx in 0..dw {
            let sx = (dx as u64 * sw as u64 / dw as u64) as u32;
            let y_val = y_plane[(sy * sw + sx) as usize] as f32;
            let uv_i = ((sy / 2) * sw + (sx & !1)) as usize;
            let u = uv_plane[uv_i] as f32;
            let v = uv_plane[uv_i + 1] as f32;
            let (b, g, r) = yuv_to_bgra(y_val, u, v);
            let o = ((dy * dw + dx) * 4) as usize;
            out[o] = b;
            out[o + 1] = g;
            out[o + 2] = r;
            out[o + 3] = 255;
        }
    }
    Some(out)
}

fn yuv_to_bgra(y: f32, u: f32, v: f32) -> (u8, u8, u8) {
    let c = y - 16.0;
    let d = u - 128.0;
    let e = v - 128.0;
    let r = (298.0 * c + 409.0 * e + 128.0) / 256.0;
    let g = (298.0 * c - 100.0 * d - 208.0 * e + 128.0) / 256.0;
    let b = (298.0 * c + 516.0 * d + 128.0) / 256.0;
    (
        b.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        r.clamp(0.0, 255.0) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_noop() {
        let src = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let out = downsample_bgra(&src, 2, 1, 2, 1).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn nv12_roundtrip_size() {
        let sw = 4u32;
        let sh = 2u32;
        let mut src = vec![16u8; (sw * sh) as usize];
        src.extend_from_slice(&[128, 128, 128, 128]);
        let out = downsample_nv12(&src, sw, sh, 4, 2).unwrap();
        assert_eq!(out.len(), src.len());
        let bgra = nv12_to_bgra_scaled(&out, 4, 2, 4, 2).unwrap();
        assert_eq!(bgra.len(), 4 * 2 * 4);
    }
}
