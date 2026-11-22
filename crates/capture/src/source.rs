use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError as CrossbeamTryRecv};
use thiserror::Error;

use crate::device::{DeviceInfo, FormatDesc};
use crate::pool::PooledBytes;

/// Bounded frame bus depth (drop-oldest). Locked decision: 2.
pub const FRAME_CHANNEL_DEPTH: usize = 2;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("capture already running")]
    AlreadyRunning,
    #[error("capture is not running")]
    NotRunning,
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error("platform backend unavailable: {0}")]
    Unsupported(String),
    #[error("device open failed: {0}")]
    DeviceOpen(String),
    /// Device is held by another process or HW MFT cannot start streaming.
    #[error("device busy or hardware streaming failed: {0}")]
    DeviceBusy(String),
    #[error("media type negotiation failed: {0}")]
    Negotiate(String),
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// Live capture health reported by the platform worker (polled by the UI).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum StreamHealth {
    /// Receiving frames (or not yet started).
    Ok = 0,
    /// Opened but no samples yet (true HDMI black / no input).
    WaitingForSignal = 1,
    /// Hardware MFT / exclusive access failure — another app may hold the card.
    DeviceBusy = 2,
    /// Other streaming error.
    StreamError = 3,
}

impl StreamHealth {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::WaitingForSignal,
            2 => Self::DeviceBusy,
            3 => Self::StreamError,
            _ => Self::Ok,
        }
    }
}

/// Shared status updated by the capture thread.
#[derive(Clone, Debug, Default)]
pub struct StreamStatus {
    health: Arc<AtomicU64>,
}

impl StreamStatus {
    pub fn new() -> Self {
        Self {
            health: Arc::new(AtomicU64::new(u64::from(StreamHealth::Ok as u8))),
        }
    }

    pub fn set(&self, health: StreamHealth) {
        self.health.store(u64::from(health as u8), Ordering::SeqCst);
    }

    pub fn get(&self) -> StreamHealth {
        StreamHealth::from_u8(self.health.load(Ordering::Relaxed) as u8)
    }
}

#[derive(Clone, Debug)]
pub struct VideoFrame {
    /// Capture-side Instant used for display-pipeline delay (→ present).
    pub captured_at: Instant,
    pub width: u32,
    pub height: u32,
    pub pixel_format: crate::format::PixelFormat,
    /// Pooled CPU bytes (BGRA8 / YUY2 / NV12). Drop returns capacity to the pool.
    pub data: Arc<PooledBytes>,
}

/// Thin handle; actual queue lives with the capture producer.
#[derive(Clone)]
pub struct FrameReceiver {
    inner: Receiver<VideoFrame>,
    drops: Arc<AtomicU64>,
}

impl FrameReceiver {
    pub fn try_recv(&self) -> Result<VideoFrame, TryRecvError> {
        match self.inner.try_recv() {
            Ok(frame) => Ok(frame),
            Err(CrossbeamTryRecv::Empty) => Err(TryRecvError::Empty),
            Err(CrossbeamTryRecv::Disconnected) => Err(TryRecvError::Disconnected),
        }
    }

    /// Frames discarded by drop-oldest on the producer side.
    pub fn dropped_count(&self) -> u64 {
        self.drops.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryRecvError {
    Empty,
    Disconnected,
}

/// Producer side that drops the oldest frame when the bus is full.
#[derive(Clone)]
pub struct FrameSender {
    tx: Sender<VideoFrame>,
    /// Extra receiver used only to drain the oldest slot under pressure.
    drain: Receiver<VideoFrame>,
    drops: Arc<AtomicU64>,
}

impl FrameSender {
    /// Returns true if the frame was accepted (possibly after dropping oldest).
    pub fn send_drop_oldest(&self, frame: VideoFrame) -> bool {
        match self.tx.try_send(frame) {
            Ok(()) => true,
            Err(crossbeam_channel::TrySendError::Full(frame)) => {
                // Drop oldest, then push the newest frame.
                if self.drain.try_recv().is_ok() {
                    self.drops.fetch_add(1, Ordering::Relaxed);
                }
                self.tx.try_send(frame).is_ok()
            }
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => false,
        }
    }

    pub fn dropped_count(&self) -> u64 {
        self.drops.load(Ordering::Relaxed)
    }
}

/// Create a bounded drop-oldest frame channel of [`FRAME_CHANNEL_DEPTH`].
pub fn frame_channel() -> (FrameSender, FrameReceiver) {
    let (tx, rx) = bounded(FRAME_CHANNEL_DEPTH);
    let drain = rx.clone();
    let drops = Arc::new(AtomicU64::new(0));
    (
        FrameSender {
            tx,
            drain,
            drops: drops.clone(),
        },
        FrameReceiver { inner: rx, drops },
    )
}

pub trait VideoSource: Send {
    fn info(&self) -> &DeviceInfo;
    fn start(&mut self, selected: &FormatDesc) -> Result<(), CaptureError>;
    fn stop(&mut self) -> Result<(), CaptureError>;
    /// Non-blocking poll handle into the capture thread queue.
    fn receiver(&self) -> FrameReceiver;
    /// Live health (busy / waiting / ok). Default is always Ok.
    fn stream_status(&self) -> StreamStatus {
        StreamStatus::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::PixelFormat;
    use crate::pool::BufferPool;

    #[test]
    fn stream_health_roundtrip_and_busy_distinct() {
        let status = StreamStatus::new();
        assert_eq!(status.get(), StreamHealth::Ok);
        status.set(StreamHealth::WaitingForSignal);
        assert_eq!(status.get(), StreamHealth::WaitingForSignal);
        status.set(StreamHealth::DeviceBusy);
        assert_eq!(status.get(), StreamHealth::DeviceBusy);
        status.set(StreamHealth::StreamError);
        assert_eq!(status.get(), StreamHealth::StreamError);
        assert_ne!(StreamHealth::DeviceBusy, StreamHealth::WaitingForSignal);
    }

    #[test]
    fn drop_oldest_increments_counter() {
        let (tx, rx) = frame_channel();
        let pool = BufferPool::new();
        for _ in 0..(FRAME_CHANNEL_DEPTH + 3) {
            let data = pool.take(16).into_arc();
            let frame = VideoFrame {
                captured_at: Instant::now(),
                width: 2,
                height: 2,
                pixel_format: PixelFormat::Bgra8,
                data,
            };
            assert!(tx.send_drop_oldest(frame));
        }
        assert!(rx.dropped_count() >= 3);
        // Receiver can still drain remaining slots.
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, FRAME_CHANNEL_DEPTH);
    }
}
