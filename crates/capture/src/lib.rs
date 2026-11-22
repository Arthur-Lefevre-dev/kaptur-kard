//! CaptureView video source abstraction.
//!
//! Real device backends (Media Foundation, etc.) land in later steps.
//! Step (a) ships [`MockVideoSource`] behind the shared [`VideoSource`] trait.

pub mod device;
pub mod format;
pub mod modes;
pub mod source;

#[cfg(feature = "mock")]
pub mod mock;

pub mod platform;

pub use device::{DeviceId, DeviceInfo, FormatDesc};
pub use format::{CaptureMode, PixelFormat, Rational};
pub use source::{
    CaptureError, FrameReceiver, FrameSender, TryRecvError, VideoFrame, VideoSource,
    FRAME_CHANNEL_DEPTH,
};

#[cfg(feature = "mock")]
pub use mock::MockVideoSource;
