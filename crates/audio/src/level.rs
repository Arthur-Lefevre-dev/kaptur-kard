//! Peak level meter shared with the UI.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct LevelMeter {
    /// Peak amplitude × 1000 (0..1000+).
    peak_milli: AtomicU32,
}

impl LevelMeter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn observe_interleaved_stereo(&self, samples: &[f32]) {
        let mut peak = 0.0f32;
        for s in samples {
            peak = peak.max(s.abs());
        }
        let milli = (peak * 1000.0).round() as u32;
        // Decay-friendly store of max seen since last UI read.
        let _ = self.peak_milli.fetch_max(milli, Ordering::Relaxed);
    }

    /// Read and decay the peak for UI (returns 0.0..1.0+).
    pub fn take_peak(&self) -> f32 {
        let v = self.peak_milli.swap(0, Ordering::Relaxed);
        // Soft decay leftover if UI polls faster than audio.
        (v as f32 / 1000.0).min(2.0)
    }

    pub fn peek_peak(&self) -> f32 {
        self.peak_milli.load(Ordering::Relaxed) as f32 / 1000.0
    }
}
