//! Resampler wrapper stub (`rubato` lands with real audio).

/// No-op placeholder so the crate graph stays stable.
pub struct ResamplerStub;

impl ResamplerStub {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ResamplerStub {
    fn default() -> Self {
        Self::new()
    }
}
