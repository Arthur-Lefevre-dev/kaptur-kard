//! Peak level meters shared with the UI (stereo + optional center/LFE).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct LevelMeter {
    /// Peak amplitude × 1000 (0..1000+), combined.
    peak_milli: AtomicU32,
    peak_l_milli: AtomicU32,
    peak_r_milli: AtomicU32,
    peak_c_milli: AtomicU32,
    peak_lfe_milli: AtomicU32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ChannelPeaks {
    pub peak: f32,
    pub left: f32,
    pub right: f32,
    pub center: f32,
    pub lfe: f32,
}

impl LevelMeter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn store_max(slot: &AtomicU32, milli: u32) {
        let _ = slot.fetch_max(milli, Ordering::Relaxed);
    }

    pub fn observe_interleaved_stereo(&self, samples: &[f32]) {
        let mut peak = 0.0f32;
        let mut left = 0.0f32;
        let mut right = 0.0f32;
        for chunk in samples.chunks_exact(2) {
            let l = chunk[0].abs();
            let r = chunk[1].abs();
            left = left.max(l);
            right = right.max(r);
            peak = peak.max(l).max(r);
        }
        Self::store_max(&self.peak_milli, (peak * 1000.0).round() as u32);
        Self::store_max(&self.peak_l_milli, (left * 1000.0).round() as u32);
        Self::store_max(&self.peak_r_milli, (right * 1000.0).round() as u32);
    }

    /// Observe pre-fold multi-channel frames (FL FR FC LFE …) for per-channel VU.
    pub fn observe_interleaved_channels(&self, samples: &[f32], channels: usize) {
        if channels == 0 || samples.is_empty() {
            return;
        }
        if channels == 2 {
            self.observe_interleaved_stereo(samples);
            return;
        }
        let frames = samples.len() / channels;
        let mut peak = 0.0f32;
        let mut left = 0.0f32;
        let mut right = 0.0f32;
        let mut center = 0.0f32;
        let mut lfe = 0.0f32;
        for f in 0..frames {
            let base = f * channels;
            let fl = samples[base].abs();
            let fr = if channels > 1 {
                samples[base + 1].abs()
            } else {
                fl
            };
            left = left.max(fl);
            right = right.max(fr);
            peak = peak.max(fl).max(fr);
            if channels > 2 {
                let fc = samples[base + 2].abs();
                center = center.max(fc);
                peak = peak.max(fc);
            }
            if channels > 3 {
                let lf = samples[base + 3].abs();
                lfe = lfe.max(lf);
                peak = peak.max(lf);
            }
        }
        Self::store_max(&self.peak_milli, (peak * 1000.0).round() as u32);
        Self::store_max(&self.peak_l_milli, (left * 1000.0).round() as u32);
        Self::store_max(&self.peak_r_milli, (right * 1000.0).round() as u32);
        Self::store_max(&self.peak_c_milli, (center * 1000.0).round() as u32);
        Self::store_max(&self.peak_lfe_milli, (lfe * 1000.0).round() as u32);
    }

    fn take_slot(slot: &AtomicU32) -> f32 {
        let v = slot.swap(0, Ordering::Relaxed);
        (v as f32 / 1000.0).min(2.0)
    }

    /// Read and clear peaks for UI (0.0..1.0+).
    pub fn take_peaks(&self) -> ChannelPeaks {
        ChannelPeaks {
            peak: Self::take_slot(&self.peak_milli),
            left: Self::take_slot(&self.peak_l_milli),
            right: Self::take_slot(&self.peak_r_milli),
            center: Self::take_slot(&self.peak_c_milli),
            lfe: Self::take_slot(&self.peak_lfe_milli),
        }
    }

    /// Read and decay the combined peak for UI (returns 0.0..1.0+).
    pub fn take_peak(&self) -> f32 {
        self.take_peaks().peak
    }

    pub fn peek_peak(&self) -> f32 {
        self.peak_milli.load(Ordering::Relaxed) as f32 / 1000.0
    }
}
