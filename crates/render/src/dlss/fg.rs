//! DLSS-G (Frame Generation) probe for NVIDIA desktop FG.
//!
//! Architecture adapted from publicly documented NeuralScreen behaviour
//! (TECHNICAL.md): after NR, optional DLSS-G with flat depth + estimated MVs,
//! multiplier ×2–×6, refuse politely when the runtime/GPU rejects.
//!
//! We do **not** copy NeuralScreen PolyForm source or vendor `nvngx_dlssg.dll`.
//! Full EvaluateFeature + dedicated present queue needs DXGI swapchain ownership
//! beyond wgpu — until that lands, Mode FG reports PendingPresentHook when the
//! BYO DLL is present on an NVIDIA GPU.

#[cfg(feature = "dlss")]
use std::path::{Path, PathBuf};

#[cfg(feature = "dlss")]
use super::probe::nvidia_vendor;
use wgpu::Backend;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DlssGAvailability {
    FeatureDisabled,
    NotNvidia,
    WrongBackend,
    SdkMissing,
    /// DLL found; EvaluateFeature / present-queue hook not wired yet.
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
}

pub struct DlssGEngine {
    availability: DlssGAvailability,
    multiplier: DlssGMultiplier,
}

impl DlssGEngine {
    pub fn new(vendor_id: u32, name: &str, backend: Backend) -> Self {
        let availability = classify(vendor_id, name, backend);
        tracing::info!(?availability, "DLSS-G (nvngx_dlssg.dll) status");
        Self {
            availability,
            multiplier: DlssGMultiplier::Off,
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
            // Keep requested value for UI, but evaluate path stays off.
            tracing::debug!(
                ?m,
                ?self.availability,
                "DLSS-G multiplier set while not Ready — will refuse at evaluate"
            );
        }
        self.multiplier = m;
    }

    pub fn status(&self) -> DlssGStatus {
        DlssGStatus {
            availability: self.availability,
            multiplier: self.multiplier,
            dll_hint: "Place nvngx_dlssg.dll beside nvngx_dlssnr.dll (BYO, docs/DLSS.md)".into(),
        }
    }

    /// Returns false when FG cannot run (AMD / missing DLL / pending hook).
    pub fn try_begin_frame(&self) -> bool {
        if self.multiplier == DlssGMultiplier::Off {
            return false;
        }
        if !self.availability.can_evaluate() {
            return false;
        }
        false // present-queue hook not wired — never claim FG frames yet
    }
}

fn classify(vendor_id: u32, name: &str, backend: Backend) -> DlssGAvailability {
    #[cfg(not(feature = "dlss"))]
    {
        let _ = (vendor_id, name, backend);
        return DlssGAvailability::FeatureDisabled;
    }
    #[cfg(feature = "dlss")]
    {
        if !nvidia_vendor(vendor_id, name) {
            return DlssGAvailability::NotNvidia;
        }
        match backend {
            Backend::Dx12 | Backend::Vulkan => {}
            _ => return DlssGAvailability::WrongBackend,
        }
        if find_dlssg_dll().is_none() {
            return DlssGAvailability::SdkMissing;
        }
        // DLL present on NVIDIA + DX12/VK — EvaluateFeature needs DXGI present ownership.
        DlssGAvailability::PendingPresentHook
    }
}

#[cfg(feature = "dlss")]
fn find_dlssg_dll() -> Option<PathBuf> {
    let dirs = runtime_dirs();
    for dir in dirs {
        let p = dir.join("nvngx_dlssg.dll");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[cfg(feature = "dlss")]
fn runtime_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for key in ["CAPTUREVIEW_DLSSNR_DIR", "CAPTUREVIEW_DLSSG_DIR"] {
        if let Ok(v) = std::env::var(key) {
            let p = PathBuf::from(v.trim());
            if p.is_dir() {
                out.push(p);
            }
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        out.push(PathBuf::from(appdata).join("CaptureView").join("dlss5-nr"));
    }
    let native = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("native")
        .join("dlss5-nr")
        .join("runtime");
    if native.is_dir() {
        out.push(native);
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
    }
}
