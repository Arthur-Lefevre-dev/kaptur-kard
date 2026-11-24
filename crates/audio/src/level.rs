//! Peak level meters shared with the UI (stereo + optional 5.1 channels).

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
    peak_ls_milli: AtomicU32,
    peak_rs_milli: AtomicU32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ChannelPeaks {
    pub peak: f32,
    pub left: f32,
    pub right: f32,
    pub center: f32,
    pub lfe: f32,
    /// Surround / back left (WAVE 5.1 ch 4).
    pub ls: f32,
    /// Surround / back right (WAVE 5.1 ch 5).
    pub rs: f32,
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

    /// Observe pre-fold multi-channel frames (FL FR FC LFE BL BR …) for per-channel VU.
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
        let mut ls = 0.0f32;
        let mut rs = 0.0f32;
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
            // WAVEFORMATEXTENSIBLE 5.1: ch4 = BL/SL, ch5 = BR/SR.
            if channels > 4 {
                let s = samples[base + 4].abs();
                ls = ls.max(s);
                peak = peak.max(s);
            }
            if channels > 5 {
                let s = samples[base + 5].abs();
                rs = rs.max(s);
                peak = peak.max(s);
            }
        }
        Self::store_max(&self.peak_milli, (peak * 1000.0).round() as u32);
        Self::store_max(&self.peak_l_milli, (left * 1000.0).round() as u32);
        Self::store_max(&self.peak_r_milli, (right * 1000.0).round() as u32);
        Self::store_max(&self.peak_c_milli, (center * 1000.0).round() as u32);
        Self::store_max(&self.peak_lfe_milli, (lfe * 1000.0).round() as u32);
        Self::store_max(&self.peak_ls_milli, (ls * 1000.0).round() as u32);
        Self::store_max(&self.peak_rs_milli, (rs * 1000.0).round() as u32);
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
            ls: Self::take_slot(&self.peak_ls_milli),
            rs: Self::take_slot(&self.peak_rs_milli),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_peaks() {
        let m = LevelMeter::new();
        m.observe_interleaved_stereo(&[0.5, -0.25, 0.1, 0.8]);
        let p = m.take_peaks();
        assert!((p.left - 0.5).abs() < 0.001);
        assert!((p.right - 0.8).abs() < 0.001);
        assert!((p.peak - 0.8).abs() < 0.001);
    }

    #[test]
    fn surround_peaks_include_ls_rs() {
        let m = LevelMeter::new();
        // One 5.1 frame: FL FR FC LFE LS RS
        let frame = [0.1, 0.2, 0.3, 0.4, 0.55, 0.66];
        m.observe_interleaved_channels(&frame, 6);
        let p = m.take_peaks();
        assert!((p.left - 0.1).abs() < 0.001);
        assert!((p.right - 0.2).abs() < 0.001);
        assert!((p.center - 0.3).abs() < 0.001);
        assert!((p.lfe - 0.4).abs() < 0.001);
        assert!((p.ls - 0.55).abs() < 0.001);
        assert!((p.rs - 0.66).abs() < 0.001);
        assert!((p.peak - 0.66).abs() < 0.001);
    }
}
