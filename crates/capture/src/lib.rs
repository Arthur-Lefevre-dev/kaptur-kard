//! CaptureView video source abstraction.
//!
//! - Windows: Media Foundation ([`platform::windows_mf`]); optional DirectShow
//!   scaffolding behind `--features directshow`.
//! - Linux: V4L2 ([`platform::linux_v4l2`]).
//! - macOS: AVFoundation scaffolding ([`platform::macos_avf`]).
//! - [`MockVideoSource`] remains available for CI and card-free development.

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
pub use format::{looks_interlaced, CaptureMode, PixelFormat, Rational};
pub use labels::{
    device_capture_score, device_capture_score_for, device_list_label, device_sort_key,
    display_hz, hz_label, is_virtual_camera, pick_preferred_device_index, resolution_label,
};
pub use pool::{BufferPool, PooledBytes};
pub use source::{
    CaptureError, FrameReceiver, FrameSender, StreamHealth, StreamStatus, TryRecvError,
    VideoFrame, VideoSource, FRAME_CHANNEL_DEPTH,
};

#[cfg(feature = "mock")]
pub use mock::MockVideoSource;

/// Platform device list (MF / V4L2 / AVF depending on OS).
pub use platform::{list_device_presence, list_devices};

#[cfg(windows)]
pub use platform::windows_mf::{enumerate_devices, MfVideoSource};

#[cfg(all(windows, feature = "directshow"))]
pub use platform::windows_dshow::DshowVideoSource;

#[cfg(target_os = "linux")]
pub use platform::linux_v4l2::V4l2VideoSource;

#[cfg(target_os = "macos")]
pub use platform::macos_avf::AvfVideoSource;
