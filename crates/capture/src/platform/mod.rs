//! Platform capture backends.

#[cfg(windows)]
pub mod windows_mf;

#[cfg(target_os = "linux")]
pub mod linux_v4l2;

#[cfg(target_os = "macos")]
pub mod macos_avf;

/// Cross-platform device enumeration entry point.
pub fn list_devices() -> Result<Vec<crate::device::DeviceInfo>, crate::source::CaptureError> {
    #[cfg(windows)]
    {
        windows_mf::enumerate_devices()
    }
    #[cfg(not(windows))]
    {
        Err(crate::source::CaptureError::Unsupported(
            "video capture not implemented on this OS in v1".into(),
        ))
    }
}
