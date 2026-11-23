//! Rolling replay buffer of recent capture frames + PNG-sequence clip save.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use capture::{PixelFormat, PooledBytes, VideoFrame};

use crate::prefs::app_data_dir;

/// Hard RAM ceiling for buffered frame bytes (~192 MiB).
const MAX_BUFFER_BYTES: usize = 192 * 1024 * 1024;
/// Cap width for stored frames (downscale via stride skip when larger).
const MAX_STORE_WIDTH: u32 = 1280;

#[derive(Clone)]
struct StoredFrame {
    at: Instant,
    width: u32,
    height: u32,
    /// BGRA8 tightly packed.
    bgra: Arc<Vec<u8>>,
}

pub struct ReplayBuffer {
    enabled: bool,
    seconds: u32,
    frames: VecDeque<StoredFrame>,
    bytes: usize,
    last_push: Instant,
    /// Min gap between stored frames (saves RAM at high capture rates).
    min_gap: Duration,
}

impl Default for ReplayBuffer {
    fn default() -> Self {
        Self {
            enabled: false,
            seconds: 8,
            frames: VecDeque::new(),
            bytes: 0,
            last_push: Instant::now() - Duration::from_secs(1),
            min_gap: Duration::from_millis(33),
        }
    }
}

impl ReplayBuffer {
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.clear();
        }
    }

    pub fn set_seconds(&mut self, seconds: u32) {
        self.seconds = seconds.clamp(3, 30);
        self.trim();
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn seconds(&self) -> u32 {
        self.seconds
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn clear(&mut self) {
        self.frames.clear();
        self.bytes = 0;
    }

    pub fn push_frame(&mut self, frame: &VideoFrame) {
        if !self.enabled {
            return;
        }
        if self.last_push.elapsed() < self.min_gap {
            return;
        }
        let Some(bgra) = frame_to_bgra_downscaled(frame, MAX_STORE_WIDTH) else {
            return;
        };
        let bytes = bgra.len();
        if bytes == 0 || bytes > MAX_BUFFER_BYTES / 2 {
            return;
        }
        let (w, h) = stored_size(frame.width, frame.height, MAX_STORE_WIDTH);
        self.frames.push_back(StoredFrame {
            at: Instant::now(),
            width: w,
            height: h,
            bgra: Arc::new(bgra),
        });
        self.bytes += bytes;
        self.last_push = Instant::now();
        self.trim();
    }

    fn trim(&mut self) {
        let keep = Duration::from_secs(self.seconds as u64);
        let now = Instant::now();
        while let Some(front) = self.frames.front() {
            let too_old = now.duration_since(front.at) > keep;
            let too_big = self.bytes > MAX_BUFFER_BYTES;
            if !too_old && !too_big {
                break;
            }
            if let Some(f) = self.frames.pop_front() {
                self.bytes = self.bytes.saturating_sub(f.bgra.len());
            }
        }
    }

    /// Write buffered frames as PNG sequence under `%APPDATA%/CaptureView/clips/…`.
    pub fn save_clip(&self) -> Result<PathBuf, String> {
        if self.frames.is_empty() {
            return Err("replay buffer empty".into());
        }
        let root = clips_dir().ok_or_else(|| "no app data dir".to_string())?;
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dir = root.join(format!("clip-{stamp}"));
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

        for (i, frame) in self.frames.iter().enumerate() {
            let path = dir.join(format!("frame_{i:05}.png"));
            write_bgra_png(&path, frame.width, frame.height, &frame.bgra)?;
        }
        let readme = dir.join("README.txt");
        let _ = fs::write(
            readme,
            format!(
                "KapturKard replay clip — {} frames (~{} s)\n\
                 Encode with ffmpeg:\n\
                 ffmpeg -framerate 30 -i frame_%05d.png -c:v libx264 -pix_fmt yuv420p clip.mp4\n",
                self.frames.len(),
                self.seconds
            ),
        );
        Ok(dir)
    }
}

pub fn clips_dir() -> Option<PathBuf> {
    let mut p = app_data_dir()?;
    p.push("clips");
    Some(p)
}

fn stored_size(w: u32, h: u32, max_w: u32) -> (u32, u32) {
    if w <= max_w || w == 0 {
        return (w.max(1), h.max(1));
    }
    let scale = max_w as f32 / w as f32;
    let nw = max_w;
    let nh = ((h as f32 * scale).round() as u32).max(1);
    (nw, nh)
}

fn frame_to_bgra_downscaled(frame: &VideoFrame, max_w: u32) -> Option<Vec<u8>> {
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
        // NV12 / others: skip to keep the buffer light (BGRA/YUY2 cover MF cards).
        _ => None,
    }
}

fn downsample_bgra(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Option<Vec<u8>> {
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

fn yuy2_to_bgra(src: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
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

fn write_bgra_png(path: &Path, w: u32, h: u32, bgra: &[u8]) -> Result<(), String> {
    let need = (w * h * 4) as usize;
    if bgra.len() < need {
        return Err("bgra too short".into());
    }
    // Convert BGRA → RGBA for the image crate.
    let mut rgba = Vec::with_capacity(need);
    for chunk in bgra[..need].chunks_exact(4) {
        rgba.push(chunk[2]);
        rgba.push(chunk[1]);
        rgba.push(chunk[0]);
        rgba.push(chunk[3]);
    }
    image::save_buffer(
        path,
        &rgba,
        w,
        h,
        image::ColorType::Rgba8,
    )
    .map_err(|e| e.to_string())
}

/// Helper so callers can stash Arc without importing pool details.
#[allow(dead_code)]
pub fn pooled_len(data: &Arc<PooledBytes>) -> usize {
    data.as_slice().len()
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
}
