//! Availability classification for DLSS 5 Neural Rendering (feature 18).

use wgpu::Backend;

use super::nr_ffi::NrRuntime;

/// PCI vendor id for NVIDIA.
pub const NVIDIA_VENDOR_ID: u32 = 0x10DE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DlssAvailability {
    FeatureDisabled,
    NotNvidia,
    WrongBackend,
    /// `nvngx_dlssnr.dll` / bridge not found.
    SdkMissing,
    /// Bridge loaded but CreateFeature(18) / Init failed (e.g. non-RTX-50).
    InitFailed,
    /// Native bridge ready — Mode C evaluates real NR.
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
}

pub struct DlssEngine {
    availability: DlssAvailability,
    vendor_id: u32,
    backend: Backend,
    runtime: Option<NrRuntime>,
    last_error: String,
    /// Frame counter for temporal reset (first enhanced frame resets history).
    frames_since_reset: u32,
}

impl DlssEngine {
    pub fn new(adapter: &wgpu::Adapter, backend: Backend) -> Self {
        let info = adapter.get_info();
        let vendor_id = info.vendor;
        let (availability, runtime) = NrRuntime::try_open(vendor_id, &info.name, backend);
        tracing::info!(
            ?availability,
            vendor = %info.name,
            vendor_id,
            ?backend,
            "DLSS 5 Neural Rendering (NGX feature 18) status"
        );
        Self {
            availability,
            vendor_id,
            backend,
            runtime,
            last_error: String::new(),
            frames_since_reset: 0,
        }
    }

    pub fn availability(&self) -> DlssAvailability {
        self.availability
    }

    pub fn can_evaluate(&self) -> bool {
        self.availability.can_evaluate() && self.runtime.is_some()
    }

    pub fn status(&self) -> DlssStatus {
        DlssStatus {
            availability: self.availability,
            sdk_hint: "Place nvngx_dlssnr.dll in CAPTUREVIEW_DLSSNR_DIR or %APPDATA%\\CaptureView\\dlss5-nr — see docs/DLSS.md".into(),
            vendor_id: self.vendor_id,
            backend: format!("{:?}", self.backend),
            nr_active: self.can_evaluate(),
            gpu_name: self
                .runtime
                .as_ref()
                .map(|r| r.gpu_name().to_string())
                .unwrap_or_default(),
        }
    }

    pub fn last_error(&self) -> &str {
        &self.last_error
    }

    /// Enhance an RGBA8 frame in-place via DLSS 5 NR (CPU staging through D3D12).
    /// Returns Ok(true) when NR ran; Ok(false) when unavailable; Err on process failure.
    pub fn enhance_rgba8(&mut self, rgba: &mut [u8], width: u32, height: u32) -> Result<bool, String> {
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
        let mut rgb_in = vec![0.0f32; px * 3];
        let mut rgb_out = vec![0.0f32; px * 3];
        for i in 0..px {
            rgb_in[i * 3] = rgba[i * 4] as f32 / 255.0;
            rgb_in[i * 3 + 1] = rgba[i * 4 + 1] as f32 / 255.0;
            rgb_in[i * 3 + 2] = rgba[i * 4 + 2] as f32 / 255.0;
        }
        let reset = self.frames_since_reset == 0;
        match rt.process_rgb(&rgb_in, &mut rgb_out, width, height, reset) {
            Ok(()) => {
                // Auto channel-order: compare RGB vs BGR against source mean.
                let use_bgr = prefer_bgr_slots(&rgb_in, &rgb_out);
                for i in 0..px {
                    let (r, g, b) = if use_bgr {
                        (rgb_out[i * 3 + 2], rgb_out[i * 3 + 1], rgb_out[i * 3])
                    } else {
                        (rgb_out[i * 3], rgb_out[i * 3 + 1], rgb_out[i * 3 + 2])
                    };
                    rgba[i * 4] = (r.clamp(0.0, 1.0) * 255.0) as u8;
                    rgba[i * 4 + 1] = (g.clamp(0.0, 1.0) * 255.0) as u8;
                    rgba[i * 4 + 2] = (b.clamp(0.0, 1.0) * 255.0) as u8;
                    // Keep alpha.
                }
                self.frames_since_reset = self.frames_since_reset.saturating_add(1);
                self.last_error.clear();
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
    if count == 0 {
        return false;
    }
    swp_mae < raw_mae
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
