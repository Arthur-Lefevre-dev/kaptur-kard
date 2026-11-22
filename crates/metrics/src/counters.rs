//! Atomic counters shared across capture / render / UI. No mutex on the hot path.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::percentile::{fps_from_frame_ns, one_percent_low_fps, percentile_u64};

/// Number of recent frame times kept for the graph / 1% low.
pub const FRAME_TIME_RING: usize = 120;

#[derive(Debug)]
pub struct SharedMetrics {
    pub frames_presented: AtomicU64,
    pub frames_dropped: AtomicU64,
    pub frames_captured: AtomicU64,
    pub frames_coalesced: AtomicU64,
    /// Last display-pipeline delay: capture Instant → present (nanoseconds).
    pub last_latency_ns: AtomicU64,
    pub last_frame_time_ns: AtomicU64,
    /// Write index into the frame-time ring (monotonic).
    frame_time_idx: AtomicU64,
    frame_times_ns: [AtomicU64; FRAME_TIME_RING],
}

impl Default for SharedMetrics {
    fn default() -> Self {
        Self {
            frames_presented: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
            frames_captured: AtomicU64::new(0),
            frames_coalesced: AtomicU64::new(0),
            last_latency_ns: AtomicU64::new(0),
            last_frame_time_ns: AtomicU64::new(0),
            frame_time_idx: AtomicU64::new(0),
            frame_times_ns: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl SharedMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    #[inline]
    pub fn record_capture(&self) {
        self.frames_captured.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_drop(&self, n: u64) {
        if n > 0 {
            self.frames_dropped.fetch_add(n, Ordering::Relaxed);
        }
    }

    #[inline]
    pub fn record_coalesced(&self, n: u64) {
        if n > 0 {
            self.frames_coalesced.fetch_add(n, Ordering::Relaxed);
        }
    }

    /// Record a presented frame: pipeline delay + inter-present frame time.
    #[inline]
    pub fn record_present(&self, latency_ns: u64, frame_time_ns: u64) {
        self.frames_presented.fetch_add(1, Ordering::Relaxed);
        self.last_latency_ns.store(latency_ns, Ordering::Relaxed);
        self.last_frame_time_ns
            .store(frame_time_ns, Ordering::Relaxed);
        let idx = self.frame_time_idx.fetch_add(1, Ordering::Relaxed) as usize % FRAME_TIME_RING;
        self.frame_times_ns[idx].store(frame_time_ns, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let mut frame_times = [0u64; FRAME_TIME_RING];
        let written = self.frame_time_idx.load(Ordering::Relaxed);
        let valid = written.min(FRAME_TIME_RING as u64) as usize;
        if valid > 0 {
            let start = if written >= FRAME_TIME_RING as u64 {
                (written as usize) % FRAME_TIME_RING
            } else {
                0
            };
            for i in 0..valid {
                let idx = if written >= FRAME_TIME_RING as u64 {
                    (start + i) % FRAME_TIME_RING
                } else {
                    i
                };
                frame_times[i] = self.frame_times_ns[idx].load(Ordering::Relaxed);
            }
        }
        let times_slice = &frame_times[..valid];
        let last_latency_ns = self.last_latency_ns.load(Ordering::Relaxed);
        let last_frame_time_ns = self.last_frame_time_ns.load(Ordering::Relaxed);
        MetricsSnapshot {
            frames_presented: self.frames_presented.load(Ordering::Relaxed),
            frames_dropped: self.frames_dropped.load(Ordering::Relaxed),
            frames_captured: self.frames_captured.load(Ordering::Relaxed),
            frames_coalesced: self.frames_coalesced.load(Ordering::Relaxed),
            last_latency_ns,
            last_latency_ms: last_latency_ns as f32 / 1_000_000.0,
            last_frame_time_ns,
            display_fps_instant: fps_from_frame_ns(last_frame_time_ns),
            one_percent_low_fps: one_percent_low_fps(times_slice),
            p50_frame_ns: percentile_u64(times_slice, 50),
            frame_times_ns: times_slice.to_vec(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MetricsSnapshot {
    pub frames_presented: u64,
    pub frames_dropped: u64,
    pub frames_captured: u64,
    pub frames_coalesced: u64,
    pub last_latency_ns: u64,
    pub last_latency_ms: f32,
    pub last_frame_time_ns: u64,
    pub display_fps_instant: f32,
    pub one_percent_low_fps: f32,
    pub p50_frame_ns: u64,
    pub frame_times_ns: Vec<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_counter_accumulates() {
        let m = SharedMetrics::new();
        m.record_drop(1);
        m.record_drop(2);
        assert_eq!(m.snapshot().frames_dropped, 3);
    }

    #[test]
    fn present_fills_ring_and_latency() {
        let m = SharedMetrics::new();
        for i in 0..10 {
            m.record_present(1_000_000 * (i + 1), 16_666_667);
        }
        let s = m.snapshot();
        assert_eq!(s.frames_presented, 10);
        assert!((s.last_latency_ms - 10.0).abs() < 0.01);
        assert_eq!(s.frame_times_ns.len(), 10);
        assert!(s.one_percent_low_fps > 50.0);
    }
}
