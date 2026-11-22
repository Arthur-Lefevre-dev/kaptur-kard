use std::sync::Arc;

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
    #[error("media type negotiation failed: {0}")]
    Negotiate(String),
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

#[derive(Clone, Debug)]
pub struct VideoFrame {
    pub timestamp_ns: u64,
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
}

impl FrameReceiver {
    pub fn try_recv(&self) -> Result<VideoFrame, TryRecvError> {
        match self.inner.try_recv() {
            Ok(frame) => Ok(frame),
            Err(CrossbeamTryRecv::Empty) => Err(TryRecvError::Empty),
            Err(CrossbeamTryRecv::Disconnected) => Err(TryRecvError::Disconnected),
        }
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
}

impl FrameSender {
    pub fn send_drop_oldest(&self, frame: VideoFrame) -> bool {
        match self.tx.try_send(frame) {
            Ok(()) => true,
            Err(crossbeam_channel::TrySendError::Full(frame)) => {
                // Drop oldest, then push the newest frame.
                let _ = self.drain.try_recv();
                self.tx.try_send(frame).is_ok()
            }
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => false,
        }
    }
}

/// Create a bounded drop-oldest frame channel of [`FRAME_CHANNEL_DEPTH`].
pub fn frame_channel() -> (FrameSender, FrameReceiver) {
    let (tx, rx) = bounded(FRAME_CHANNEL_DEPTH);
    let drain = rx.clone();
    (
        FrameSender { tx, drain },
        FrameReceiver { inner: rx },
    )
}

pub trait VideoSource: Send {
    fn info(&self) -> &DeviceInfo;
    fn start(&mut self, selected: &FormatDesc) -> Result<(), CaptureError>;
    fn stop(&mut self) -> Result<(), CaptureError>;
    /// Non-blocking poll handle into the capture thread queue.
    fn receiver(&self) -> FrameReceiver;
}
