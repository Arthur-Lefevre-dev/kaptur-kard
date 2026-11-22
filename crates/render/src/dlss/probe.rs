//! DLSS availability probing shared by stub and Streamline-backed builds.

use wgpu::Backend;

#[cfg(feature = "dlss")]
use super::streamline::probe_streamline;
use super::streamline::sdk_version_hint;

/// PCI vendor id for NVIDIA.
pub const NVIDIA_VENDOR_ID: u32 = 0x10DE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DlssAvailability {
    /// Feature not compiled in (default builds).
    FeatureDisabled,
    /// Non-NVIDIA GPU (e.g. AMD / Intel).
    NotNvidia,
    /// wgpu backend is not Dx12/Vulkan (Streamline needs a native device).
    WrongBackend,
    /// STREAMLINE_SDK_PATH / bundled DLLs not found.
    SdkMissing,
    /// DLL present but LoadLibrary / slInit probe failed.
    InitFailed,
    /// SDK DLLs found; native evaluate hook not yet wired — NIS fallback at runtime.
    SdkPresentPendingHook,
    /// Reserved for a future full evaluate path.
    Ready,
}

impl DlssAvailability {
    pub fn is_selectable(self) -> bool {
        // Allow UI selection even when unavailable — encode falls back to NIS.
        true
    }

    /// True when NGX evaluate could run (not yet used until DXGI/VK hook lands).
    pub fn can_evaluate(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// True when Mode C should fall back to the NIS compute path.
    pub fn uses_nis_fallback(self) -> bool {
        !self.can_evaluate()
    }

    pub fn as_reason_key(self) -> &'static str {
        match self {
            Self::FeatureDisabled => "effect_reason_dlss_feature",
            Self::NotNvidia => "effect_reason_dlss_gpu",
            Self::WrongBackend => "effect_reason_dlss_backend",
            Self::SdkMissing => "effect_reason_dlss_sdk",
            Self::InitFailed => "effect_reason_dlss_init",
            Self::SdkPresentPendingHook => "effect_reason_dlss_pending",
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
}

pub struct DlssEngine {
    availability: DlssAvailability,
    vendor_id: u32,
    backend: Backend,
}

impl DlssEngine {
    pub fn new(adapter: &wgpu::Adapter, backend: Backend) -> Self {
        let info = adapter.get_info();
        let vendor_id = info.vendor;
        let availability = classify(vendor_id, &info.name, backend);
        if availability != DlssAvailability::Ready {
            tracing::info!(
                ?availability,
                vendor = %info.name,
                vendor_id,
                ?backend,
                hint = sdk_version_hint(),
                "DLSS / Streamline status"
            );
        }
        Self {
            availability,
            vendor_id,
            backend,
        }
    }

    pub fn availability(&self) -> DlssAvailability {
        self.availability
    }

    pub fn status(&self) -> DlssStatus {
        DlssStatus {
            availability: self.availability,
            sdk_hint: sdk_version_hint().into(),
            vendor_id: self.vendor_id,
            backend: format!("{:?}", self.backend),
        }
    }

    pub fn can_evaluate(&self) -> bool {
        self.availability.can_evaluate()
    }

    pub fn uses_nis_fallback(&self) -> bool {
        self.availability.uses_nis_fallback()
    }
}

pub fn nvidia_vendor(vendor_id: u32, name: &str) -> bool {
    vendor_id == NVIDIA_VENDOR_ID || name.to_ascii_lowercase().contains("nvidia")
}

fn classify(vendor_id: u32, name: &str, backend: Backend) -> DlssAvailability {
    #[cfg(not(feature = "dlss"))]
    {
        let _ = (vendor_id, name, backend);
        return DlssAvailability::FeatureDisabled;
    }
    #[cfg(feature = "dlss")]
    {
        if !nvidia_vendor(vendor_id, name) {
            return DlssAvailability::NotNvidia;
        }
        match backend {
            Backend::Dx12 | Backend::Vulkan => probe_streamline(vendor_id, backend),
            _ => DlssAvailability::WrongBackend,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amd_is_not_nvidia() {
        assert!(!nvidia_vendor(0x1002, "AMD Radeon RX 7800 XT"));
    }

    #[test]
    fn nvidia_by_id_or_name() {
        assert!(nvidia_vendor(NVIDIA_VENDOR_ID, "Unknown"));
        assert!(nvidia_vendor(0, "NVIDIA GeForce RTX 4070"));
    }

    #[test]
    fn fallback_when_not_ready() {
        assert!(DlssAvailability::NotNvidia.uses_nis_fallback());
        assert!(DlssAvailability::SdkMissing.uses_nis_fallback());
        assert!(DlssAvailability::SdkPresentPendingHook.uses_nis_fallback());
        assert!(!DlssAvailability::Ready.uses_nis_fallback());
    }

    #[test]
    fn reason_keys_nonempty_except_ready() {
        assert!(!DlssAvailability::SdkMissing.as_reason_key().is_empty());
        assert!(DlssAvailability::Ready.as_reason_key().is_empty());
    }
}
