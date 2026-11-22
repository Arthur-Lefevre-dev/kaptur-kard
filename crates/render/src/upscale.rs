//! Post-v1 upscale hooks (FSR1 / NIS). Disabled in v1.

#[cfg(feature = "upscale")]
pub fn enabled() -> bool {
    true
}

#[cfg(not(feature = "upscale"))]
pub fn enabled() -> bool {
    false
}
