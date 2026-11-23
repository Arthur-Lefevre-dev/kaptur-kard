//! Rolling stereo PCM tap for replay-clip mux (pre-volume, post fold/resample).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Shared tap the audio engine pushes into when the app enables the replay buffer.
pub type SharedReplayTap = Arc<Mutex<AudioReplayTap>>;

#[derive(Clone, Debug)]
pub struct AudioClipSnapshot {
    /// Interleaved stereo f32 (pre-monitoring volume / mute).
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// Wall-clock-aligned stereo ring for clip encode.
pub struct AudioReplayTap {
    enabled: bool,
    seconds: u32,
    sample_rate: u32,
    /// Interleaved L/R samples.
    samples: VecDeque<f32>,
    /// Approximate start time of the oldest sample (for trim against video Instant).
    oldest_at: Option<Instant>,
}

impl Default for AudioReplayTap {
    fn default() -> Self {
        Self {
            enabled: false,
            seconds: 8,
            sample_rate: 48_000,
            samples: VecDeque::new(),
            oldest_at: None,
        }
    }
}

impl AudioReplayTap {
    pub fn new_shared() -> SharedReplayTap {
        Arc::new(Mutex::new(Self::default()))
    }

    pub fn configure(&mut self, enabled: bool, seconds: u32, sample_rate: u32) {
        let seconds = seconds.clamp(3, 30);
        let sample_rate = sample_rate.max(1);
        let reset = !enabled
            || self.seconds != seconds
            || self.sample_rate != sample_rate
            || self.enabled != enabled;
        self.enabled = enabled;
        self.seconds = seconds;
        self.sample_rate = sample_rate;
        if reset {
            self.samples.clear();
            self.oldest_at = None;
        }
        self.trim();
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Push interleaved stereo (length even). No-op when disabled.
    pub fn push_stereo(&mut self, interleaved: &[f32]) {
        if !self.enabled || interleaved.is_empty() {
            return;
        }
        let n = interleaved.len() - (interleaved.len() % 2);
        if n == 0 {
            return;
        }
        if self.oldest_at.is_none() {
            self.oldest_at = Some(Instant::now());
        }
        self.samples.extend(interleaved[..n].iter().copied());
        self.trim();
    }

    /// Snapshot PCM covering roughly `[from, to]` (video frame Instant range).
    pub fn snapshot_range(&self, from: Instant, to: Instant) -> AudioClipSnapshot {
        let rate = self.sample_rate.max(1);
        if !self.enabled || self.samples.is_empty() {
            return AudioClipSnapshot {
                samples: Vec::new(),
                sample_rate: rate,
            };
        }
        let Some(oldest) = self.oldest_at else {
            return AudioClipSnapshot {
                samples: Vec::new(),
                sample_rate: rate,
            };
        };
        let total_frames = self.samples.len() / 2;
        let span = to.saturating_duration_since(from);
        // Map Instant range onto the ring using oldest_at + sample count.
        let start_off = from.saturating_duration_since(oldest);
        let start_frame = ((start_off.as_secs_f64() * rate as f64).round() as isize).max(0) as usize;
        let need_frames = ((span.as_secs_f64() * rate as f64).round() as usize).max(1);
        let start = start_frame.min(total_frames.saturating_sub(1));
        let end = (start + need_frames).min(total_frames);
        let mut samples = Vec::with_capacity((end - start) * 2);
        for i in start..end {
            samples.push(self.samples[i * 2]);
            samples.push(self.samples[i * 2 + 1]);
        }
        // If range mapping failed (clock skew), fall back to the whole ring.
        if samples.len() < rate as usize {
            samples = self.samples.iter().copied().collect();
        }
        AudioClipSnapshot {
            samples,
            sample_rate: rate,
        }
    }

    /// Entire ring (used when video Instant mapping is unavailable).
    pub fn snapshot_all(&self) -> AudioClipSnapshot {
        AudioClipSnapshot {
            samples: self.samples.iter().copied().collect(),
            sample_rate: self.sample_rate.max(1),
        }
    }

    fn trim(&mut self) {
        let max_frames = (self.sample_rate as usize).saturating_mul(self.seconds as usize);
        let max_samples = max_frames.saturating_mul(2);
        while self.samples.len() > max_samples {
            self.samples.pop_front();
            self.samples.pop_front();
            if let Some(oldest) = self.oldest_at.as_mut() {
                *oldest += Duration::from_secs_f64(1.0 / self.sample_rate.max(1) as f64);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_to_seconds() {
        let mut tap = AudioReplayTap::default();
        tap.configure(true, 3, 1_000);
        // 4 seconds of stereo silence at 1 kHz → keep 3s.
        let chunk = vec![0.1f32; 2_000]; // 1s
        for _ in 0..4 {
            tap.push_stereo(&chunk);
        }
        assert_eq!(tap.samples.len() / 2, 3_000);
    }
}
