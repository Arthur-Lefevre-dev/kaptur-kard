//! Optional frame interpolation (feature-gated; off in v1).

#[cfg(feature = "interpolate")]
pub fn enabled() -> bool {
    true
}

#[cfg(not(feature = "interpolate"))]
pub fn enabled() -> bool {
    false
}
