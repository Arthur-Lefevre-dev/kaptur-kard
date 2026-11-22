//! CaptureView video source abstraction.
//!
//! Windows Media Foundation implements [`platform::windows_mf`].
//! [`MockVideoSource`] remains available for CI and card-free development.

pub mod device;
pub mod format;
pub mod labels;
pub mod modes;
pub mod pool;
pub mod source;

#[cfg(feature = "mock")]
pub mod mock;

pub mod platform;

pub use device::{DeviceId, DeviceInfo, FormatDesc};
pub use format::{CaptureMode, PixelFormat, Rational};
pub use labels::{device_capture_score, display_hz, hz_label, resolution_label};
pub use pool::{BufferPool, PooledBytes};
pub use source::{
    CaptureError, FrameReceiver, FrameSender, TryRecvError, VideoFrame, VideoSource,
    FRAME_CHANNEL_DEPTH,
};

#[cfg(feature = "mock")]
pub use mock::MockVideoSource;

#[cfg(windows)]
pub use platform::windows_mf::{enumerate_devices, MfVideoSource};
