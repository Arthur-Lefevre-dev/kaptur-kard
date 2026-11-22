//! DLSS-G (Frame Generation) — EvaluateFeature + dedicated DXGI present queue.
//!
//! Architecture adapted from publicly documented NeuralScreen behaviour
//! (TECHNICAL.md): after NR, optional DLSS-G with flat depth + estimated MVs,
//! multiplier ×2–×6, refuse politely when the runtime/GPU rejects.
//!
//! We do **not** copy NeuralScreen PolyForm source or vendor `nvngx_dlssg.dll`.
//! Native bridge: `dlss5g_bridge.dll` (MIT) owns a HIGH-priority DXGI present
//! queue + NGX Feature_FrameGeneration EvaluateFeature path.

use std::collections::VecDeque;

use wgpu::Backend;

#[cfg(feature = "dlss")]
use super::fg_ffi::{FgOpenStatus, FgRuntime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DlssGAvailability {
    FeatureDisabled,
    NotNvidia,
    WrongBackend,
    SdkMissing,
    InitFailed,
    /// Kept for locale compatibility; no longer returned by the live probe.
    PendingPresentHook,
    Ready,
}

impl DlssGAvailability {
    pub fn can_evaluate(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// UI may offer the control (NVIDIA path kept for future RTX users).
    pub fn selectable(self) -> bool {
        !matches!(self, Self::FeatureDisabled)
    }

    pub fn as_reason_key(self) -> &'static str {
        match self {
            Self::FeatureDisabled => "effect_reason_dlssg_feature",
            Self::NotNvidia => "effect_reason_dlssg_gpu",
            Self::WrongBackend => "effect_reason_dlssg_backend",
            Self::SdkMissing => "effect_reason_dlssg_sdk",
            Self::InitFailed => "effect_reason_dlssg_init",
            Self::PendingPresentHook => "effect_reason_dlssg_pending",
            Self::Ready => "",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DlssGMultiplier {
    Off,
    X2,
    X3,
    X4,
    X5,
    X6,
}

impl DlssGMultiplier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::X2 => "x2",
            Self::X3 => "x3",
            Self::X4 => "x4",
            Self::X5 => "x5",
            Self::X6 => "x6",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "x2" | "2" => Self::X2,
            "x3" | "3" => Self::X3,
            "x4" | "4" => Self::X4,
            "x5" | "5" => Self::X5,
            "x6" | "6" => Self::X6,
            _ => Self::Off,
        }
    }

    pub fn generated_frames(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::X2 => 1,
            Self::X3 => 2,
            Self::X4 => 3,
            Self::X5 => 4,
            Self::X6 => 5,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DlssGStatus {
    pub availability: DlssGAvailability,
    pub multiplier: DlssGMultiplier,
    pub dll_hint: String,
    pub present_queue: bool,
}

/// One RGBA8 frame waiting to be shown (generated or real).
#[derive(Clone, Debug)]
pub struct FgPresentFrame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub generated: bool,
}

pub struct DlssGEngine {
    availability: DlssGAvailability,
    multiplier: DlssGMultiplier,
    #[cfg(feature = "dlss")]
    runtime: Option<FgRuntime>,
    prev_rgb: Option<(u32, u32, Vec<f32>)>,
    pending: VecDeque<FgPresentFrame>,
    present_queue_ok: bool,
    frames_since_reset: u32,
    last_error: String,
}

impl DlssGEngine {
    pub fn new(vendor_id: u32, name: &str, backend: Backend) -> Self {
        #[cfg(not(feature = "dlss"))]
        {
            let _ = (vendor_id, name, backend);
            tracing::info!(availability = ?DlssGAvailability::FeatureDisabled, "DLSS-G disabled");
            return Self {
                availability: DlssGAvailability::FeatureDisabled,
                multiplier: DlssGMultiplier::Off,
                prev_rgb: None,
                pending: VecDeque::new(),
                present_queue_ok: false,
                frames_since_reset: 0,
                last_error: String::new(),
            };
        }
        #[cfg(feature = "dlss")]
        {
            let (open, runtime) = FgRuntime::try_open(vendor_id, name, backend);
            let availability = match open {
                FgOpenStatus::NotNvidia => DlssGAvailability::NotNvidia,
                FgOpenStatus::WrongBackend => DlssGAvailability::WrongBackend,
                FgOpenStatus::SdkMissing => DlssGAvailability::SdkMissing,
                FgOpenStatus::InitFailed => DlssGAvailability::InitFailed,
                FgOpenStatus::Ready => DlssGAvailability::Ready,
            };
            let present_queue_ok = runtime
                .as_ref()
                .map(|r| r.present_queue_ready())
                .unwrap_or(false);
            let gpu = runtime.as_ref().map(|r| r.gpu_name()).unwrap_or("");
            tracing::info!(
                ?availability,
                present_queue_ok,
                gpu,
                "DLSS-G (nvngx_dlssg.dll) status"
            );
            Self {
                availability,
                multiplier: DlssGMultiplier::Off,
                runtime,
                prev_rgb: None,
                pending: VecDeque::new(),
                present_queue_ok,
                frames_since_reset: 0,
                last_error: String::new(),
            }
        }
    }

    pub fn availability(&self) -> DlssGAvailability {
        self.availability
    }

    pub fn multiplier(&self) -> DlssGMultiplier {
        self.multiplier
    }

    pub fn set_multiplier(&mut self, m: DlssGMultiplier) {
        if m != DlssGMultiplier::Off && !self.availability.can_evaluate() {
            tracing::debug!(
                ?m,
                ?self.availability,
                "DLSS-G multiplier set while not Ready — will refuse at evaluate"
            );
        }
        if m == DlssGMultiplier::Off {
            self.pending.clear();
            self.prev_rgb = None;
            self.frames_since_reset = 0;
        }
        self.multiplier = m;
    }

    pub fn status(&self) -> DlssGStatus {
        DlssGStatus {
            availability: self.availability,
            multiplier: self.multiplier,
            dll_hint: "Place nvngx_dlssg.dll beside nvngx_dlssnr.dll (BYO, docs/DLSS.md)".into(),
            present_queue: self.present_queue_ok,
        }
    }

    pub fn last_error(&self) -> &str {
        &self.last_error
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn pop_present_frame(&mut self) -> Option<FgPresentFrame> {
        self.pending.pop_front()
    }

    /// Returns true when FG should drive extra presents (queue non-empty or evaluate ready).
    pub fn try_begin_frame(&self) -> bool {
        if self.multiplier == DlssGMultiplier::Off {
            return false;
        }
        if !self.availability.can_evaluate() {
            return false;
        }
        true
    }

    /// After NR (or pass-through), ingest the real RGBA8 frame: generate intermediates
    /// into the present queue, then enqueue the real frame last.
    pub fn ingest_real_rgba8(&mut self, rgba: &[u8], width: u32, height: u32) {
        if self.multiplier == DlssGMultiplier::Off || !self.availability.can_evaluate() {
            return;
        }
        let gen_n = self.multiplier.generated_frames();
        if gen_n == 0 {
            return;
        }

        // Drop stale generated frames when a new real frame arrives.
        self.pending.clear();

        let px = (width as usize).saturating_mul(height as usize);
        if rgba.len() < px.saturating_mul(4) {
            return;
        }

        let mut curr_rgb = vec![0.0f32; px * 3];
        for i in 0..px {
            curr_rgb[i * 3] = rgba[i * 4] as f32 / 255.0;
            curr_rgb[i * 3 + 1] = rgba[i * 4 + 1] as f32 / 255.0;
            curr_rgb[i * 3 + 2] = rgba[i * 4 + 2] as f32 / 255.0;
        }

        #[cfg(feature = "dlss")]
        {
            let reset = self.frames_since_reset == 0
                || self
                    .prev_rgb
                    .as_ref()
                    .map(|(w, h, _)| *w != width || *h != height)
                    .unwrap_or(true);

            if let Some(rt) = self.runtime.as_mut() {
                if let Some((_, _, prev)) = self.prev_rgb.as_ref() {
                    let mut out = vec![0.0f32; px * 3 * gen_n as usize];
                    match rt.process_pair(
                        prev,
                        &curr_rgb,
                        &mut out,
                        width,
                        height,
                        gen_n,
                        reset,
                    ) {
                        Ok(n) => {
                            self.present_queue_ok = rt.present_queue_ready();
                            for g in 0..n as usize {
                                let slice = &out[g * px * 3..(g + 1) * px * 3];
                                self.pending.push_back(FgPresentFrame {
                                    rgba: rgb_f32_to_rgba8(slice, width, height),
                                    width,
                                    height,
                                    generated: true,
                                });
                            }
                            self.last_error.clear();
                        }
                        Err(e) => {
                            self.last_error = e;
                            tracing::warn!(error = %self.last_error, "DLSS-G EvaluateFeature failed");
                        }
                    }
                }
            }
        }

        // Real frame is presented by the normal NR/upscale path on this capture tick.
        // Only generated frames sit in the queue for between-capture presents.
        self.prev_rgb = Some((width, height, curr_rgb));
        self.frames_since_reset = self.frames_since_reset.saturating_add(1);
    }

    pub fn request_history_reset(&mut self) {
        self.frames_since_reset = 0;
        self.prev_rgb = None;
        self.pending.clear();
    }
}

#[cfg(feature = "dlss")]
fn rgb_f32_to_rgba8(rgb: &[f32], width: u32, height: u32) -> Vec<u8> {
    let px = (width as usize).saturating_mul(height as usize);
    let mut out = vec![0u8; px * 4];
    for i in 0..px {
        out[i * 4] = (rgb[i * 3].clamp(0.0, 1.0) * 255.0) as u8;
        out[i * 4 + 1] = (rgb[i * 3 + 1].clamp(0.0, 1.0) * 255.0) as u8;
        out[i * 4 + 2] = (rgb[i * 3 + 2].clamp(0.0, 1.0) * 255.0) as u8;
        out[i * 4 + 3] = 255;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplier_roundtrip() {
        assert_eq!(DlssGMultiplier::from_str_lossy("x4"), DlssGMultiplier::X4);
        assert_eq!(DlssGMultiplier::X2.generated_frames(), 1);
        assert_eq!(DlssGMultiplier::Off.as_str(), "off");
    }

    #[test]
    fn amd_not_ready() {
        let e = DlssGEngine::new(0x1002, "AMD Radeon", Backend::Dx12);
        assert_eq!(
            e.availability(),
            if cfg!(feature = "dlss") {
                DlssGAvailability::NotNvidia
            } else {
                DlssGAvailability::FeatureDisabled
            }
        );
        assert!(!e.try_begin_frame());
        e_refuses_ingest(&e);
    }

    fn e_refuses_ingest(e: &DlssGEngine) {
        // ingest is &mut — use a fresh AMD engine
        let mut e2 = DlssGEngine::new(0x1002, "AMD Radeon", Backend::Dx12);
        e2.set_multiplier(DlssGMultiplier::X2);
        let rgba = vec![0u8; 16];
        e2.ingest_real_rgba8(&rgba, 2, 2);
        assert_eq!(e2.pending_count(), 0);
        let _ = e;
    }
}
