//! NVIDIA RTX path: DLSS 5 Neural Rendering + optional DLSS-G Frame Generation.
//!
//! Architecture (public NeuralScreen TECHNICAL.md ideas, **not** their PolyForm
//! source): capture → (optional Boost) NR feature 18 → optional DLSS-G FG.
//! AMD hosts keep FSR1/NIS + optical interpolate; Mode C / FG refuse cleanly.
//!
//! Proprietary `nvngx_dlssnr.dll` / `nvngx_dlssg.dll` are **BYO** — never vendored.
//! See `docs/DLSS.md`.

mod fg;
mod probe;
mod readback;

#[cfg(feature = "dlss")]
mod fg_ffi;

#[cfg(feature = "dlss")]
mod nr_ffi;

#[cfg(not(feature = "dlss"))]
mod nr_ffi {
    use super::DlssAvailability;

    pub struct NrRuntime;

    impl NrRuntime {
        pub fn try_open(
            _vendor_id: u32,
            _name: &str,
            _backend: wgpu::Backend,
        ) -> (DlssAvailability, Option<Self>) {
            (DlssAvailability::FeatureDisabled, None)
        }

        pub fn process_rgb(
            &mut self,
            _rgb_in: &[f32],
            _rgb_out: &mut [f32],
            _width: u32,
            _height: u32,
            _reset: bool,
        ) -> Result<(), String> {
            Err("dlss feature disabled".into())
        }

        pub fn gpu_name(&self) -> &str {
            ""
        }
    }
}

pub use fg::{
    DlssGAvailability, DlssGEngine, DlssGMultiplier, DlssGStatus, FgPresentFrame,
};
pub use probe::{nvidia_vendor, DlssAvailability, DlssEngine, DlssStatus};
pub use readback::{enhance_rgba8_with_nr, read_texture_rgba8, write_rgba8_texture};

/// Preferred wgpu backends when the `dlss` feature is enabled (DX12 first).
pub fn preferred_backends() -> wgpu::Backends {
    #[cfg(feature = "dlss")]
    {
        // NeuralScreen / obs-dlss5-nr host NGX on D3D12; prefer DX12 for CaptureView too.
        wgpu::Backends::DX12 | wgpu::Backends::VULKAN
    }
    #[cfg(not(feature = "dlss"))]
    {
        wgpu::Backends::PRIMARY
    }
}

pub const STREAMLINE_DOC_VERSION: &str = "2.14.16";

pub const DLSS_MARKETING_NOTE: &str =
    "DLSS 5 NR = NGX feature 18 (nvngx_dlssnr.dll); optional FG = nvngx_dlssg.dll; BYO DLLs; RTX path kept for future users.";
