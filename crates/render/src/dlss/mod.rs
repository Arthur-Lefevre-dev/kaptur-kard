//! NVIDIA Streamline / DLSS Super Resolution scaffolding.
//!
//! Default builds compile a stub that never evaluates NGX. Enable `--features dlss`
//! after installing the Streamline SDK (see `docs/DLSS.md`) to probe for
//! `sl.interposer.dll` / NGX DLLs. Full `slEvaluateFeature` still needs a native
//! D3D12/Vulkan device hook from wgpu — until that lands, Mode C falls back to NIS.

mod probe;

#[cfg(feature = "dlss")]
mod streamline;

#[cfg(not(feature = "dlss"))]
mod streamline {
    pub fn sdk_version_hint() -> &'static str {
        "Streamline 2.14.x / NGX (DLSS SR) — enable `--features dlss` after SDK install"
    }
}

pub use probe::{nvidia_vendor, DlssAvailability, DlssEngine, DlssStatus};

/// Target Streamline release documented for Windows installs.
pub const STREAMLINE_DOC_VERSION: &str = "2.14.16";

/// Public note: NVIDIA markets DLSS 4.x / transformer presets via current NGX;
/// there is no separate public “DLSS 5” SDK package as of this integration.
pub const DLSS_MARKETING_NOTE: &str =
    "Latest Streamline/NGX DLSS Super Resolution (DLSS 4.x transformer presets); not a separate DLSS 5 SDK.";
