//! macOS AVFoundation capture scaffolding behind [`crate::source::VideoSource`].
//!
//! ## Status (step h)
//!
//! **Stub / scaffolding.** Compiles under `cfg(target_os = "macos")` and exposes
//! the same `enumerate_devices` / `AvfVideoSource` surface as other backends.
//! Enumeration returns an empty list; `start` returns
//! [`CaptureError::Unsupported`] until AVFoundation device I/O is wired
//! (e.g. via `cidre` / `objc2-av-foundation` in a later change).
//!
//! ## Build on macOS
//!
//! ```bash
//! cargo build -p capture --target aarch64-apple-darwin
//! # or on an Apple host:
//! cargo run -p captureview
//! ```
//!
//! This module is never compiled on Windows/Linux hosts.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tracing::info;

use crate::device::{DeviceId, DeviceInfo, FormatDesc};
use crate::pool::BufferPool;
use crate::source::{
    frame_channel, CaptureError, FrameReceiver, FrameSender, VideoSource,
};

/// Placeholder enumeration — returns no devices until AVFoundation is linked.
pub fn enumerate_devices() -> Result<Vec<DeviceInfo>, CaptureError> {
    info!("AVFoundation backend is scaffolding-only; no devices enumerated yet");
    Ok(Vec::new())
}

/// Scaffolding source that refuses to start until a real AVF path lands.
pub struct AvfVideoSource {
    info: DeviceInfo,
    sender: FrameSender,
    receiver: FrameReceiver,
    running: AtomicBool,
    #[allow(dead_code)]
    pool: Arc<BufferPool>,
}

impl AvfVideoSource {
    pub fn open(info: DeviceInfo) -> Result<Self, CaptureError> {
        let (sender, receiver) = frame_channel();
        Ok(Self {
            info,
            sender,
            receiver,
            running: AtomicBool::new(false),
            pool: BufferPool::new(),
        })
    }

    /// Reserved for a future discovery helper that builds a synthetic DeviceInfo.
    #[allow(dead_code)]
    pub fn placeholder_info() -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new("avf:pending"),
            display_name: "AVFoundation (not yet implemented)".into(),
            backend: "avf",
            formats: Vec::new(),
            enum_note: Some("scaffolding".into()),
        }
    }
}

impl VideoSource for AvfVideoSource {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn start(&mut self, _selected: &FormatDesc) -> Result<(), CaptureError> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(CaptureError::AlreadyRunning);
        }
        self.running.store(false, Ordering::SeqCst);
        Err(CaptureError::Unsupported(
            "macOS AVFoundation capture is scaffolding-only in this build".into(),
        ))
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        if !self.running.swap(false, Ordering::SeqCst) {
            return Err(CaptureError::NotRunning);
        }
        Ok(())
    }

    fn receiver(&self) -> FrameReceiver {
        self.receiver.clone()
    }
}
