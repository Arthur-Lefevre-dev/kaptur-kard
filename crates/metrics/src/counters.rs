use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct SharedMetrics {
    pub frames_presented: AtomicU64,
    pub frames_dropped: AtomicU64,
    pub frames_captured: AtomicU64,
    pub last_latency_ns: AtomicU64,
}

impl SharedMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            frames_presented: self.frames_presented.load(Ordering::Relaxed),
            frames_dropped: self.frames_dropped.load(Ordering::Relaxed),
            frames_captured: self.frames_captured.load(Ordering::Relaxed),
            last_latency_ns: self.last_latency_ns.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MetricsSnapshot {
    pub frames_presented: u64,
    pub frames_dropped: u64,
    pub frames_captured: u64,
    pub last_latency_ns: u64,
}
