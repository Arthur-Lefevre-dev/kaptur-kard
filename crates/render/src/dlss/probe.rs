//! Availability + Boost for DLSS 5 Neural Rendering (feature 18).

use wgpu::Backend;

use super::fg::{DlssGEngine, DlssGMultiplier, DlssGStatus};
use super::nr_ffi::NrRuntime;

/// PCI vendor id for NVIDIA.
pub const NVIDIA_VENDOR_ID: u32 = 0x10DE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DlssAvailability {
    FeatureDisabled,
    NotNvidia,
    WrongBackend,
    SdkMissing,
    InitFailed,
    Ready,
}

impl DlssAvailability {
    pub fn can_evaluate(self) -> bool {
        matches!(self, Self::Ready)
    }

    pub fn as_reason_key(self) -> &'static str {
        match self {
            Self::FeatureDisabled => "effect_reason_dlss_feature",
            Self::NotNvidia => "effect_reason_dlss_gpu",
            Self::WrongBackend => "effect_reason_dlss_backend",
            Self::SdkMissing => "effect_reason_dlss_sdk",
            Self::InitFailed => "effect_reason_dlss_init",
            Self::Ready => "",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DlssStatus {
    pub availability: DlssAvailability,
    pub sdk_hint: String,
    pub vendor_id: u32,
    pub backend: String,
    pub nr_active: bool,
    pub gpu_name: String,
    pub boost: bool,
    pub fg: DlssGStatus,
}

pub struct DlssEngine {
    availability: DlssAvailability,
    vendor_id: u32,
    backend: Backend,
    runtime: Option<NrRuntime>,
    last_error: String,
    frames_since_reset: u32,
    /// Boost: NR at half resolution, composite onto the native frame (faster).
    boost: bool,
    fg: DlssGEngine,
}

impl DlssEngine {
    pub fn new(adapter: &wgpu::Adapter, backend: Backend) -> Self {
        let info = adapter.get_info();
        let vendor_id = info.vendor;
        let (availability, runtime) = NrRuntime::try_open(vendor_id, &info.name, backend);
        let fg = DlssGEngine::new(vendor_id, &info.name, backend);
        tracing::info!(
            ?availability,
            vendor = %info.name,
            vendor_id,
            ?backend,
            fg = ?fg.availability(),
            "DLSS 5 NR + DLSS-G status (RTX path retained for future users)"
        );
        Self {
            availability,
            vendor_id,
            backend,
            runtime,
            last_error: String::new(),
            frames_since_reset: 0,
            boost: true, // NeuralScreen default: Boost on
            fg,
        }
    }

    pub fn availability(&self) -> DlssAvailability {
        self.availability
    }

    pub fn can_evaluate(&self) -> bool {
        self.availability.can_evaluate() && self.runtime.is_some()
    }

    pub fn set_boost(&mut self, on: bool) {
        if on != self.boost {
            self.boost = on;
            self.frames_since_reset = 0;
        }
    }

    pub fn boost(&self) -> bool {
        self.boost
    }

    pub fn fg_mut(&mut self) -> &mut DlssGEngine {
        &mut self.fg
    }

    pub fn fg(&self) -> &DlssGEngine {
        &self.fg
    }

    pub fn set_fg_multiplier(&mut self, m: DlssGMultiplier) {
        self.fg.set_multiplier(m);
    }

    pub fn status(&self) -> DlssStatus {
        DlssStatus {
            availability: self.availability,
            sdk_hint: "BYO nvngx_dlssnr.dll (+ optional nvngx_dlssg.dll) — docs/DLSS.md".into(),
            vendor_id: self.vendor_id,
            backend: format!("{:?}", self.backend),
            nr_active: self.can_evaluate(),
            gpu_name: self
                .runtime
                .as_ref()
                .map(|r| r.gpu_name().to_string())
                .unwrap_or_default(),
            boost: self.boost,
            fg: self.fg.status(),
        }
    }

    pub fn last_error(&self) -> &str {
        &self.last_error
    }

    /// Enhance RGBA8 via DLSS 5 NR. With Boost: half-res NR + composite onto original.
    pub fn enhance_rgba8(
        &mut self,
        rgba: &mut [u8],
        width: u32,
        height: u32,
    ) -> Result<bool, String> {
        let Some(rt) = self.runtime.as_mut() else {
            return Ok(false);
        };
        if !self.availability.can_evaluate() {
            return Ok(false);
        }
        let px = (width as usize).saturating_mul(height as usize);
        if rgba.len() < px.saturating_mul(4) {
            return Err("RGBA buffer too small".into());
        }

        let reset = self.frames_since_reset == 0;
        let use_boost = self.boost && width >= 960 && height >= 540;

        let (work_w, work_h, rgb_in) = if use_boost {
            let ww = (width / 2).max(1);
            let wh = (height / 2).max(1);
            let mut small = vec![0.0f32; (ww * wh * 3) as usize];
            downsample_rgba8_to_rgb(rgba, width, height, &mut small, ww, wh);
            (ww, wh, small)
        } else {
            let mut rgb = vec![0.0f32; px * 3];
            for i in 0..px {
                rgb[i * 3] = rgba[i * 4] as f32 / 255.0;
                rgb[i * 3 + 1] = rgba[i * 4 + 1] as f32 / 255.0;
                rgb[i * 3 + 2] = rgba[i * 4 + 2] as f32 / 255.0;
            }
            (width, height, rgb)
        };

        let mut rgb_out = vec![0.0f32; (work_w * work_h * 3) as usize];
        match rt.process_rgb(&rgb_in, &mut rgb_out, work_w, work_h, reset) {
            Ok(()) => {
                let use_bgr = prefer_bgr_slots(&rgb_in, &rgb_out);
                if use_boost {
                    composite_boost(rgba, width, height, &rgb_out, work_w, work_h, use_bgr);
                } else {
                    for i in 0..px {
                        let (r, g, b) = if use_bgr {
                            (rgb_out[i * 3 + 2], rgb_out[i * 3 + 1], rgb_out[i * 3])
                        } else {
                            (rgb_out[i * 3], rgb_out[i * 3 + 1], rgb_out[i * 3 + 2])
                        };
                        rgba[i * 4] = (r.clamp(0.0, 1.0) * 255.0) as u8;
                        rgba[i * 4 + 1] = (g.clamp(0.0, 1.0) * 255.0) as u8;
                        rgba[i * 4 + 2] = (b.clamp(0.0, 1.0) * 255.0) as u8;
                    }
                }
                self.frames_since_reset = self.frames_since_reset.saturating_add(1);
                self.last_error.clear();
                // NeuralScreen order: NR first, then DLSS-G EvaluateFeature → present queue.
                if self.fg.try_begin_frame() {
                    self.fg.ingest_real_rgba8(rgba, width, height);
                }
                Ok(true)
            }
            Err(e) => {
                self.last_error = e.clone();
                Err(e)
            }
        }
    }

    pub fn request_history_reset(&mut self) {
        self.frames_since_reset = 0;
    }
}

fn downsample_rgba8_to_rgb(
    src: &[u8],
    sw: u32,
    sh: u32,
    dst: &mut [f32],
    dw: u32,
    dh: u32,
) {
    for y in 0..dh {
        let sy = (y * sh / dh).min(sh - 1);
        for x in 0..dw {
            let sx = (x * sw / dw).min(sw - 1);
            let si = ((sy * sw + sx) * 4) as usize;
            let di = ((y * dw + x) * 3) as usize;
            dst[di] = src[si] as f32 / 255.0;
            dst[di + 1] = src[si + 1] as f32 / 255.0;
            dst[di + 2] = src[si + 2] as f32 / 255.0;
        }
    }
}

/// Upsample NR work buffer and lerp onto the native frame (Boost composite).
fn composite_boost(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    nr: &[f32],
    ww: u32,
    wh: u32,
    use_bgr: bool,
) {
    let blend = 0.85f32;
    for y in 0..height {
        let fy = (y as f32 + 0.5) * (wh as f32) / (height as f32) - 0.5;
        let y0 = fy.floor().max(0.0) as u32;
        let y1 = (y0 + 1).min(wh - 1);
        let ty = fy - y0 as f32;
        for x in 0..width {
            let fx = (x as f32 + 0.5) * (ww as f32) / (width as f32) - 0.5;
            let x0 = fx.floor().max(0.0) as u32;
            let x1 = (x0 + 1).min(ww - 1);
            let tx = fx - x0 as f32;
            let sample = |xx: u32, yy: u32| {
                let i = ((yy * ww + xx) * 3) as usize;
                if use_bgr {
                    (nr[i + 2], nr[i + 1], nr[i])
                } else {
                    (nr[i], nr[i + 1], nr[i + 2])
                }
            };
            let (r00, g00, b00) = sample(x0, y0);
            let (r10, g10, b10) = sample(x1, y0);
            let (r01, g01, b01) = sample(x0, y1);
            let (r11, g11, b11) = sample(x1, y1);
            let r0 = r00 + (r10 - r00) * tx;
            let g0 = g00 + (g10 - g00) * tx;
            let b0 = b00 + (b10 - b00) * tx;
            let r1 = r01 + (r11 - r01) * tx;
            let g1 = g01 + (g11 - g01) * tx;
            let b1 = b01 + (b11 - b01) * tx;
            let nr_r = r0 + (r1 - r0) * ty;
            let nr_g = g0 + (g1 - g0) * ty;
            let nr_b = b0 + (b1 - b0) * ty;
            let oi = ((y * width + x) * 4) as usize;
            let or = rgba[oi] as f32 / 255.0;
            let og = rgba[oi + 1] as f32 / 255.0;
            let ob = rgba[oi + 2] as f32 / 255.0;
            rgba[oi] = ((or + (nr_r - or) * blend).clamp(0.0, 1.0) * 255.0) as u8;
            rgba[oi + 1] = ((og + (nr_g - og) * blend).clamp(0.0, 1.0) * 255.0) as u8;
            rgba[oi + 2] = ((ob + (nr_b - ob) * blend).clamp(0.0, 1.0) * 255.0) as u8;
        }
    }
}

fn prefer_bgr_slots(src: &[f32], out: &[f32]) -> bool {
    if src.len() != out.len() || src.is_empty() {
        return false;
    }
    let n = src.len() / 3;
    let mut raw_mae = 0.0f64;
    let mut swp_mae = 0.0f64;
    let step = (n / 4096).max(1);
    let mut count = 0usize;
    for i in (0..n).step_by(step) {
        let sr = src[i * 3] as f64;
        let sg = src[i * 3 + 1] as f64;
        let sb = src[i * 3 + 2] as f64;
        let o0 = out[i * 3] as f64;
        let o1 = out[i * 3 + 1] as f64;
        let o2 = out[i * 3 + 2] as f64;
        raw_mae += (o0 - sr).abs() + (o1 - sg).abs() + (o2 - sb).abs();
        swp_mae += (o2 - sr).abs() + (o1 - sg).abs() + (o0 - sb).abs();
        count += 1;
    }
    count > 0 && swp_mae < raw_mae
}

pub fn nvidia_vendor(vendor_id: u32, name: &str) -> bool {
    vendor_id == NVIDIA_VENDOR_ID || name.to_ascii_lowercase().contains("nvidia")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amd_is_not_nvidia() {
        assert!(!nvidia_vendor(0x1002, "AMD Radeon RX 7800 XT"));
    }

    #[test]
    fn ready_can_evaluate() {
        assert!(DlssAvailability::Ready.can_evaluate());
        assert!(!DlssAvailability::SdkMissing.can_evaluate());
    }
}
