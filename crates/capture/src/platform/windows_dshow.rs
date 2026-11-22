//! DirectShow fallback behind [`crate::source::VideoSource`] (Windows).
//!
//! ## Status (step h) — **partial / feature-gated**
//!
//! Enable with:
//!
//! ```text
//! cargo build -p capture --features directshow
//! cargo run -p captureview --features capture/directshow
//! ```
//!
//! Default builds leave this module **out** so the Media Foundation path stays
//! untouched. When the feature is on, enumeration returns an empty list and
//! `start` returns [`CaptureError::Unsupported`]: the module documents the
//! intended last-resort surface for cards missing from MF without shipping a
//! half-broken COM graph yet.
//!
//! Full DirectShow filter-graph capture (sample grabber → NV12/YUY2) remains
//! future work. Prefer MF whenever the device appears there.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tracing::{info, warn};

use crate::device::{DeviceId, DeviceInfo, FormatDesc};
use crate::pool::BufferPool;
use crate::source::{
    frame_channel, CaptureError, FrameReceiver, FrameSender, VideoSource,
};

/// Intended to enumerate DirectShow video capture filters.
///
/// Currently returns an empty list and logs once — keeps MF as the primary
/// Windows backend until a real DS graph is implemented.
pub fn enumerate_devices() -> Result<Vec<DeviceInfo>, CaptureError> {
    info!(
        "DirectShow fallback enabled but not fully implemented; returning no DS devices \
         (use Media Foundation for capture)"
    );
    Ok(Vec::new())
}

/// Scaffolding source for a future DirectShow sample-grabber path.
pub struct DshowVideoSource {
    info: DeviceInfo,
    #[allow(dead_code)]
    sender: FrameSender,
    receiver: FrameReceiver,
    running: AtomicBool,
    #[allow(dead_code)]
    pool: Arc<BufferPool>,
}

impl DshowVideoSource {
    pub fn open(info: DeviceInfo) -> Result<Self, CaptureError> {
        if info.backend != "dshow" {
            warn!(
                backend = info.backend,
                "opening DshowVideoSource for non-dshow DeviceInfo"
            );
        }
        let (sender, receiver) = frame_channel();
        Ok(Self {
            info,
            sender,
            receiver,
            running: AtomicBool::new(false),
            pool: BufferPool::new(),
        })
    }

    #[allow(dead_code)]
    pub fn placeholder_info() -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new("dshow:pending"),
            display_name: "DirectShow (not yet implemented)".into(),
            backend: "dshow",
            formats: Vec::new(),
        }
    }
}

impl VideoSource for DshowVideoSource {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn start(&mut self, _selected: &FormatDesc) -> Result<(), CaptureError> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(CaptureError::AlreadyRunning);
        }
        self.running.store(false, Ordering::SeqCst);
        Err(CaptureError::Unsupported(
            "DirectShow capture is feature-gated scaffolding; use Media Foundation".into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerate_is_empty_stub() {
        let list = enumerate_devices().expect("enum");
        assert!(list.is_empty());
    }

    #[test]
    fn start_is_unsupported() {
        let mut src = DshowVideoSource::open(DshowVideoSource::placeholder_info()).unwrap();
        let err = src
            .start(&FormatDesc {
                pixel_format: crate::format::PixelFormat::Yuy2,
                width: 640,
                height: 480,
                frame_rates: vec![crate::format::Rational::new(30, 1)],
                interlaced: false,
            })
            .unwrap_err();
        assert!(matches!(err, CaptureError::Unsupported(_)));
    }
}
