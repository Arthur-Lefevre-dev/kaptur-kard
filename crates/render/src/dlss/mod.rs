//! DLSS 5 Neural Rendering (NGX feature 18 / `nvngx_dlssnr.dll`).
//!
//! Mode C enhances video frames via a MIT-licensed D3D12 bridge adapted from
//! ComfyUI-DLSS5-NR / obs-dlss5-nr (CPU staging path). The proprietary
//! `nvngx_dlssnr.dll` is **not** vendored — see `docs/DLSS.md`.
//!
//! Default builds compile a stub. Enable `--features dlss` to load the native
//! bridge and evaluate NR when runtime + RTX GPU allow it.

mod probe;
mod readback;

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

pub use probe::{nvidia_vendor, DlssAvailability, DlssEngine, DlssStatus};
pub use readback::{enhance_rgba8_with_nr, read_texture_rgba8, write_rgba8_texture};

/// Documented Streamline release for classic SR docs (Mode C NR uses NGX feature 18).
pub const STREAMLINE_DOC_VERSION: &str = "2.14.16";

/// Public note for UI / docs.
pub const DLSS_MARKETING_NOTE: &str =
    "DLSS 5 Neural Rendering = NGX feature 18 (nvngx_dlssnr.dll); officially RTX 50-series.";
